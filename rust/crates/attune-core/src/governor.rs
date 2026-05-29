//! ACP-4 Cost Governor — wires the (frozen) A1 cache + usage subsystems into
//! the live LLM call path, and threads the output-token cap + CoT budget.
//!
//! Spec: `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §3 (ACP-4 data flow) + §5.3 (governed_call contract).
//!
//! Pre-ACP-4 the `CacheBackend` / `UsageAggregator` public surfaces were frozen
//! but had **zero production callers** (audit C). [`governed_chat`] is the single
//! production entry point that closes that gap:
//!
//! ```text
//! ① cache.get(cache_key) ─hit─► record Hit + return (save tokens)
//! ② miss → chat_with_history_opts(cap + CoT budget)
//! ③ cache.put + usage.record(Miss)
//! ```
//!
//! Every step degrades gracefully (spec §7 / §11 R8): no cache → skip lookup;
//! no aggregator → skip telemetry; a cache/telemetry failure never blocks the
//! LLM response.

use crate::cache::{cache_key, CacheBackend, CacheScope, CachedValue};
use crate::error::Result;
use crate::llm::{ChatMessage, LlmCallOptions, LlmProvider};
use crate::usage::{
    CacheOutcome, CallOutcome, ErrorKind, TokenUsage, UsageAggregator, UsageEvent, UsageKind,
};

/// Build the ACP-4 LLM cache key. Per spec §11 R1 (stale-cache mitigation) the
/// key must fold in everything that can change the answer: model + sampling
/// knobs (temperature / top_p / seed) + output cap + the **full** message
/// content (system + history + user). Because injected RAG knowledge lives
/// inside the message content, a changed source document → different injected
/// context → different key → automatic invalidation (matching the product's
/// "annotation change invalidates the annotated-view summary" contract).
///
/// We feed a single composite "prompt" string into the frozen
/// [`cache_key`](crate::cache::cache_key)`(model, prompt)` so the 128-bit BLAKE3
/// surface stays unchanged.
pub fn llm_cache_key(model: &str, messages: &[ChatMessage], opts: &LlmCallOptions) -> String {
    let mut composite = String::with_capacity(64);
    // Sampling + cap knobs first (compact, fixed order).
    composite.push_str("t=");
    composite.push_str(&opts.temperature.map(|v| v.to_string()).unwrap_or_default());
    composite.push_str(";p=");
    composite.push_str(&opts.top_p.map(|v| v.to_string()).unwrap_or_default());
    composite.push_str(";s=");
    composite.push_str(&opts.seed.map(|v| v.to_string()).unwrap_or_default());
    composite.push_str(";cap=");
    composite.push_str(
        &opts
            .effective_output_cap()
            .map(|v| v.to_string())
            .unwrap_or_default(),
    );
    composite.push_str(";msgs=");
    for m in messages {
        // role + 0x1F unit separator + content + 0x1E record separator —
        // control bytes that cannot collide with normal prompt text.
        composite.push_str(&m.role);
        composite.push('\u{1f}');
        composite.push_str(&m.content);
        composite.push('\u{1e}');
    }
    cache_key(model, &composite)
}

/// Estimate cost for a usage record (telemetry only; never blocks).
fn record_usage_event(
    usage_agg: Option<&UsageAggregator>,
    kind: UsageKind,
    usage: &TokenUsage,
    cache: CacheOutcome,
    outcome: CallOutcome,
    latency_ms: u32,
    agent_id: Option<&str>,
) {
    let Some(agg) = usage_agg else { return };
    let cost_usd = crate::cost::estimate_cost_usd(
        usage.tokens_in as usize,
        usage.tokens_out as usize,
        &usage.model,
    );
    let event = UsageEvent {
        ts_ms: chrono::Utc::now().timestamp_millis(),
        kind,
        usage: usage.clone(),
        cost_usd,
        cache,
        outcome,
        latency_ms,
        agent_id: agent_id.map(|s| s.to_string()),
        query_hash: None,
    };
    agg.record(event);
}

/// Result of a governed call — the response text, the usage that should be
/// surfaced to the UI, and whether it was served from cache.
#[derive(Debug, Clone)]
pub struct GovernedResponse {
    /// The model response text.
    pub text: String,
    /// Vendor (or cache-reconstructed) token usage for this call.
    pub usage: TokenUsage,
    /// Cache disposition (`Hit` = served from cache, tokens saved).
    pub cache: CacheOutcome,
}

/// ACP-4 governed chat call. The single production entry point that wires the
/// A1 cache + usage subsystems and the output cap / CoT budget.
///
/// - `cache`: optional LLM-response cache. `None` → no cache lookup/store
///   (graceful: spec §7 "cache unavailable → degrade, don't block").
/// - `usage_agg`: optional usage aggregator. `None` → no telemetry.
/// - `agent_id`: tags the usage record (`None` for direct chat).
/// - `ttl_secs`: cache entry TTL (L2 honors it; L1 is recency-only).
///
/// On a cache **hit** the cost governor returns immediately without an upstream
/// call (the core token-saving behavior); the reconstructed [`TokenUsage`]
/// carries the originally-cached token counts so a hit is comparable to a miss
/// in telemetry, but is recorded with `cached_in = tokens_in` so downstream
/// cost rollups can credit the saving.
pub fn governed_chat(
    provider: &dyn LlmProvider,
    messages: &[ChatMessage],
    opts: &LlmCallOptions,
    cache: Option<&dyn CacheBackend>,
    usage_agg: Option<&UsageAggregator>,
    agent_id: Option<&str>,
    ttl_secs: Option<u32>,
) -> Result<GovernedResponse> {
    let model = provider.model_name().to_string();
    let key = llm_cache_key(&model, messages, opts);
    let started = std::time::Instant::now();

    // ① Cache lookup (graceful: any failure / absence → treat as miss).
    if let Some(c) = cache {
        if let Some(hit) = block_on_cache(c.get(CacheScope::Llm, &key)) {
            let text = String::from_utf8_lossy(&hit.bytes).to_string();
            // Reconstruct usage: a hit billed the user nothing upstream, so the
            // saved input tokens are surfaced via `cached_in`.
            let usage = TokenUsage {
                tokens_in: 0,
                tokens_out: 0,
                cached_in: hit.tokens_in,
                model: hit.model.clone(),
                provider: provider_tag(provider),
            };
            let latency = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
            record_usage_event(
                usage_agg,
                UsageKind::LlmChat,
                &usage,
                CacheOutcome::Hit,
                CallOutcome::Ok,
                latency,
                agent_id,
            );
            return Ok(GovernedResponse {
                text,
                usage,
                cache: CacheOutcome::Hit,
            });
        }
    }

    // ② Miss → upstream call with cap + CoT budget. On failure (ACP-3 / spec §3
    // "产出 outcome → ACP-3 telemetry") record a classified Fail event BEFORE
    // propagating the error, so the agent×model failure-rate roll-up sees it.
    let (text, usage) = match provider.chat_with_history_opts(messages, opts) {
        Ok(ok) => ok,
        Err(e) => {
            let latency = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
            let failed_usage = TokenUsage::empty(&provider_tag(provider), &model);
            record_usage_event(
                usage_agg,
                UsageKind::LlmChat,
                &failed_usage,
                CacheOutcome::Miss,
                CallOutcome::Fail {
                    error_kind: classify_llm_error(&e),
                },
                latency,
                agent_id,
            );
            return Err(e);
        }
    };
    let latency = started.elapsed().as_millis().min(u32::MAX as u128) as u32;

    // ③ Cache store + usage record (both graceful / non-blocking).
    if let Some(c) = cache {
        let value = CachedValue {
            bytes: text.as_bytes().to_vec(),
            tokens_in: usage.tokens_in,
            tokens_out: usage.tokens_out,
            model: usage.model.clone(),
        };
        block_on_cache(c.put(CacheScope::Llm, &key, value, ttl_secs));
    }
    record_usage_event(
        usage_agg,
        UsageKind::LlmChat,
        &usage,
        CacheOutcome::Miss,
        CallOutcome::Ok,
        latency,
        agent_id,
    );

    Ok(GovernedResponse {
        text,
        usage,
        cache: CacheOutcome::Miss,
    })
}

/// Classify an LLM-call error into a telemetry [`ErrorKind`] (ACP-3 §5.2). The
/// provider surface returns a single `VaultError`, so we inspect the variant +
/// message: timeout / rate-limit / quota / network keywords route to the precise
/// bucket; everything else falls back to `Other`. JSON/parse failures surface as
/// `VaultError::Json` (decode) → `Parse`.
fn classify_llm_error(err: &crate::error::VaultError) -> ErrorKind {
    use crate::error::VaultError;
    match err {
        VaultError::Json(_) => ErrorKind::Parse,
        VaultError::LlmUnavailable(msg) | VaultError::Classification(msg) => {
            let m = msg.to_ascii_lowercase();
            if m.contains("timeout") || m.contains("timed out") {
                ErrorKind::Timeout
            } else if m.contains("rate limit") || m.contains("rate-limit") || m.contains("quota") {
                ErrorKind::Quota
            } else if m.contains("parse") || m.contains("json") || m.contains("schema") {
                ErrorKind::Parse
            } else if m.contains("network") || m.contains("connect") || m.contains("dns") {
                ErrorKind::Network
            } else {
                ErrorKind::Other
            }
        }
        VaultError::Io(_) => ErrorKind::Network,
        _ => ErrorKind::Other,
    }
}

/// The provider's wire tag for telemetry. We do not have a `provider()` getter
/// on the trait, so infer from `is_local` + model only as a coarse label;
/// callers that need exact tags read `usage.provider` on a miss.
fn provider_tag(provider: &dyn LlmProvider) -> String {
    if provider.is_local() {
        "ollama".to_string()
    } else {
        "openai_compat".to_string()
    }
}

/// Run a cache future to completion from a sync context. The `CacheBackend`
/// trait is async for future Redis/disk backends, but the in-process L1/L2
/// backends complete synchronously (mutex + rusqlite), so a tiny current-thread
/// runtime is sufficient and avoids "runtime within runtime" panics.
fn block_on_cache<F: std::future::Future>(fut: F) -> F::Output {
    use std::cell::RefCell;
    thread_local! {
        static RT: RefCell<Option<tokio::runtime::Runtime>> = const { RefCell::new(None) };
    }
    RT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("cache mini-runtime"),
            );
        }
        slot.as_ref().unwrap().block_on(fut)
    })
}

#[cfg(test)]
mod tests;
