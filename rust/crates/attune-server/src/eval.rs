//! Eval-surface helpers — T2 (KB-bench integration, v1.0.6).
//!
//! Pure builders that translate internal types into the JSON shapes that
//! `bench/adapters/attune.py` and `routes/chat.rs` / `routes/search.rs`
//! depend on. Keeping them as pure functions lets us unit-test the full
//! response surface without spawning the HTTP server (Argon2id setup costs
//! ~30s per test — see `vault_lock_endpoint_test.rs` for the E2E pattern).
//!
//! ## Backward compatibility contract
//!
//! [`build_citation`] **must preserve every key already emitted today**:
//! `item_id`, `title`, `relevance`, `breadcrumb`, `chunk_offset_start`,
//! `chunk_offset_end`. New keys (`chunk_id`, `span`, `score`) are additive
//! — Chrome extension and Web UI keep working unmodified.
//!
//! ## Spec
//!
//! `docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md`
//! §9.4 P0 #2/#3 + §9.2 R3/G4 + §11 R3.

use attune_core::chat_reliability::ChatReliabilityReport;
use attune_core::cost;
use axum::http::HeaderMap;
use serde::Serialize;

// ============================================================================
// Citations
// ============================================================================

/// Build one citation JSON value from a `knowledge[]` entry (the inline
/// `serde_json::Value` shape that `routes/chat.rs` builds from `SearchResult`
/// or a web-search hit). Preserves all legacy keys and adds T2 keys
/// (`chunk_id`, `span`, `score`).
///
/// The caller (`routes/chat.rs::chat`) already collects citations from
/// `knowledge` for the legacy shape; this fn replaces that inline `json!{}`
/// so the new keys live alongside the old.
pub fn build_citation(k: &serde_json::Value) -> serde_json::Value {
    let item_id = k.get("item_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let title = k.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let score = k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let chunk_offset_start = k.get("chunk_offset_start").and_then(|v| v.as_u64());
    let chunk_offset_end = k.get("chunk_offset_end").and_then(|v| v.as_u64());
    let breadcrumb_arr = k
        .get("breadcrumb")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // legacy behavior: fall back to [title] when breadcrumb missing + title set
    let breadcrumb = if breadcrumb_arr.is_empty() && !title.is_empty() {
        vec![serde_json::Value::String(title.clone())]
    } else {
        breadcrumb_arr
    };

    // T2 NEW: deterministic chunk_id = "<item_id>:<offset_start>"
    // For chunks without offset (memory layer / web hits) → fall back to item_id alone.
    let chunk_id = match chunk_offset_start {
        Some(start) => format!("{item_id}:{start}"),
        None => item_id.clone(),
    };

    // T2 NEW: span as `[start, end]` 2-element array when both offsets known.
    // bench adapter prefers this shape (matches retrieval_eval.py schema).
    let span = match (chunk_offset_start, chunk_offset_end) {
        (Some(s), Some(e)) => serde_json::json!([s, e]),
        _ => serde_json::Value::Null,
    };

    serde_json::json!({
        // ── Legacy keys (Chrome extension + Web UI depend on these) ─────
        "item_id": item_id,
        "title": title,
        "relevance": score,
        "breadcrumb": breadcrumb,
        "chunk_offset_start": chunk_offset_start,
        "chunk_offset_end": chunk_offset_end,
        // ── T2 additions (bench R3 grounding eval consumes these) ───────
        "chunk_id": chunk_id,
        "span": span,
        "score": score,  // alias for relevance, matches bench schema
    })
}

// ============================================================================
// Grounding block
// ============================================================================

/// Bucketed grounding level — bench R3 wants a single label for filtering,
/// distinct from the raw `overall_confidence` float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GroundingLevel {
    High,
    Medium,
    Low,
}

impl GroundingLevel {
    /// Bucket the chat_reliability overall_confidence score:
    ///   - score ≥ 0.7 → High   (well-grounded, no contradictions, citations present)
    ///   - score ≥ 0.4 → Medium (some signal weakness)
    ///   - else        → Low    (likely fabricated or unsupported)
    ///
    /// Thresholds picked to align with `chat_reliability::confidence_from_signals`
    /// — neutral 0.5 falls into Medium (correct for "no chunks" path).
    pub fn from_score(score: f32) -> Self {
        if score >= 0.7 {
            Self::High
        } else if score >= 0.4 {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

/// Distill a [`ChatReliabilityReport`] into the JSON block bench consumes.
///
/// Bench R3 (groundedness aggregate) reads `grounding.score` per response
/// and `grounding.contradictions_count` for the failure rate. Surfacing
/// counts avoids re-parsing the full `citation_grounded[]` array.
pub fn build_grounding_block(report: &ChatReliabilityReport) -> serde_json::Value {
    let score = report.overall_confidence;
    let level = GroundingLevel::from_score(score);
    serde_json::json!({
        "score": score,
        "level": level,
        "citations_count": report.citation_grounded.len(),
        "contradictions_count": report.contradictions.len(),
        "hallucination_flags_count": report.hallucination_flags.len(),
    })
}

// ============================================================================
// Cost block
// ============================================================================

/// Map a model name to a provider id. Mirrors the dispatch in
/// `attune_core::cost::lookup_pricing` so the two stay in lock-step.
fn provider_for(model: &str) -> &'static str {
    let m = model.to_lowercase();
    if m.starts_with("gpt-") || m.starts_with("o1-") || m.starts_with("o3-") {
        "openai"
    } else if m.contains("claude") {
        "anthropic"
    } else if m.contains("gemini") {
        "google"
    } else if m.contains("deepseek") {
        "deepseek"
    } else if m.starts_with("qwen") || m.starts_with("llama") || m.starts_with("phi") || m.starts_with("mistral") {
        "ollama"
    } else if m.starts_with("doubao") || m.starts_with("ernie") || m.contains("baichuan") {
        "tencent"
    } else {
        "unknown"
    }
}

/// Build the `cost` block for a chat response.
///
/// `is_local` tells us whether the LLM provider is Ollama-on-laptop / K3 form
/// factor → estimated_usd forced to 0.0 (the user is paying CPU/GPU, not USD).
/// Cloud models that have no pricing entry also fall through to 0.0 (rather
/// than `null`), so bench can sum `cost.estimated_usd` across rows without
/// a null filter.
pub fn build_cost_block(
    tokens_in: usize,
    tokens_out: usize,
    model: &str,
    is_local: bool,
) -> serde_json::Value {
    let provider = if is_local {
        "ollama"
    } else {
        provider_for(model)
    };
    let estimated_usd = if is_local {
        0.0
    } else {
        cost::estimate_cost_usd(tokens_in, tokens_out, model).unwrap_or(0.0)
    };
    serde_json::json!({
        "tokens_in": tokens_in,
        "tokens_out": tokens_out,
        "estimated_usd": estimated_usd,
        "model": model,
        "provider": provider,
    })
}

// ============================================================================
// Eval headers + block
// ============================================================================

/// Parsed `X-Attune-Eval-*` request headers. All fields default to off so
/// production clients (Chrome extension / Web UI) see no behavior change.
#[derive(Debug, Default, Clone)]
pub struct ParsedEvalHeaders {
    /// `X-Attune-Eval-Mode: 1` — surface the `eval` response block.
    pub eval_mode: bool,
    /// `X-Attune-Eval-Seed: <u64>` — forwarded to LLM provider when supported
    /// (T1 work). Invalid values drop to None silently.
    pub seed: Option<u64>,
    /// `X-Attune-Eval-Trace: full` — surface per-stage latency breakdown.
    pub trace_full: bool,
    /// `X-Attune-Eval-Force-Temp-Zero: true` — pin temperature=0 + top_p=1
    /// on the LLM call (T1, v1.0.6 KB-bench). When any provider does not
    /// honor seed but does honor temperature (e.g. Anthropic), this still
    /// gives the bench a low-noise signal — surfaced via
    /// `eval.determinism = "temp0"`.
    pub force_temp_zero: bool,
    /// `X-Attune-Eval-Skip-Rewrite: true` — bypass query rewrite for search
    /// route (T1). Honored in `routes/search.rs`.
    pub skip_rewrite: bool,
    /// `X-Attune-Eval-Skip-Rerank: true` — bypass rerank for search
    /// route (T1). Honored in `routes/search.rs`.
    pub skip_rerank: bool,
}

impl ParsedEvalHeaders {
    /// True if **any** eval knob was set by the caller. T1: the chat / search
    /// handlers branch on this to (a) surface the eval response block and
    /// (b) take the `chat_with_options` codepath that forwards seed /
    /// temperature / top_p to the LLM provider.
    pub fn any_set(&self) -> bool {
        self.eval_mode
            || self.seed.is_some()
            || self.trace_full
            || self.force_temp_zero
            || self.skip_rewrite
            || self.skip_rerank
    }
}

/// Parse eval headers from a request. Never fails — invalid values drop to
/// default (per spec §7 graceful degradation; we don't want a malformed
/// bench harness to 422 the chat path).
pub fn parse_eval_headers(headers: &HeaderMap) -> ParsedEvalHeaders {
    let truthy = |name: &str| -> bool {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| matches!(s.trim(), "1" | "true" | "TRUE" | "True"))
            .unwrap_or(false)
    };
    let eval_mode = truthy("x-attune-eval-mode");
    let seed = headers
        .get("x-attune-eval-seed")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let trace_full = headers
        .get("x-attune-eval-trace")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().eq_ignore_ascii_case("full"))
        .unwrap_or(false);
    let force_temp_zero = truthy("x-attune-eval-force-temp-zero");
    let skip_rewrite = truthy("x-attune-eval-skip-rewrite");
    let skip_rerank = truthy("x-attune-eval-skip-rerank");
    ParsedEvalHeaders {
        eval_mode,
        seed,
        trace_full,
        force_temp_zero,
        skip_rewrite,
        skip_rerank,
    }
}

/// Build the `eval` response block. Returns `Value::Null` when no eval header
/// was set — caller can stash that into the response JSON and it serializes
/// as `"eval": null`, preserving backward compat for clients that don't
/// expect the key.
///
/// **T1 backward-compat note**: C-T2 callers in `routes/chat.rs` /
/// `routes/search.rs` currently invoke this from the legacy 2-argument site
/// — that path is preserved (`determinism = "best_effort"`, no
/// `abstained` field). T1 introduces [`build_eval_block_with_determinism`]
/// for the chat route which can surface the actual provider determinism
/// level (Exact / Temp0 / BestEffort).
pub fn build_eval_block(headers: &ParsedEvalHeaders, total_latency_ms: u64) -> serde_json::Value {
    build_eval_block_with_determinism(headers, total_latency_ms, None)
}

/// T1 (v1.0.6 KB-bench) — full eval block with provider-supplied determinism.
///
/// Returns `Value::Null` if no eval header was set (= every field on
/// `ParsedEvalHeaders` is default). When `determinism` is `Some`, surfaces
/// that label in the JSON; when `None`, falls back to `"best_effort"` for
/// legacy callers that have not been routed through `LlmProvider::determinism_level`
/// yet.
pub fn build_eval_block_with_determinism(
    headers: &ParsedEvalHeaders,
    total_latency_ms: u64,
    determinism: Option<&str>,
) -> serde_json::Value {
    if !headers.any_set() {
        return serde_json::Value::Null;
    }

    let det = determinism.unwrap_or("best_effort");

    let trace = if headers.trace_full {
        serde_json::json!({
            "latency_breakdown_ms": build_latency_breakdown(total_latency_ms),
        })
    } else {
        serde_json::Value::Null
    };

    serde_json::json!({
        "determinism": det,
        "seed_used": headers.seed,
        // T1: bench harness reads abstained=false / abstention_reason=null
        // as "answer was produced". v1.1 IDK-abstention work (per spec §11
        // Risk B + R3 / G4) will populate these fields when chat_reliability
        // signals low confidence.
        "abstained": false,
        "abstention_reason": serde_json::Value::Null,
        "trace": trace,
    })
}

/// Build the `latency_breakdown_ms` block.
///
/// `total_ms` is the only field measured today; per-stage placeholders
/// (`rewrite` / `bm25` / `vector` / `rrf` / `rerank`) are zero until v1.1
/// introduces `SearchTracer` (per plan §9.5 #6). Bench can already aggregate
/// `total` for end-to-end latency curves.
pub fn build_latency_breakdown(total_ms: u64) -> serde_json::Value {
    serde_json::json!({
        "rewrite": 0u64,
        "bm25": 0u64,
        "vector": 0u64,
        "rrf": 0u64,
        "rerank": 0u64,
        "total": total_ms,
    })
}

// ============================================================================
// Inline unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_for_known_models() {
        assert_eq!(provider_for("gpt-4o"), "openai");
        assert_eq!(provider_for("gpt-4o-mini"), "openai");
        assert_eq!(provider_for("claude-3-5-sonnet"), "anthropic");
        assert_eq!(provider_for("gemini-1.5-pro"), "google");
        assert_eq!(provider_for("deepseek-chat"), "deepseek");
        assert_eq!(provider_for("qwen2.5:3b"), "ollama");
        assert_eq!(provider_for("totally-mystery"), "unknown");
    }

    #[test]
    fn provider_for_is_case_insensitive() {
        assert_eq!(provider_for("CLAUDE-3-OPUS"), "anthropic");
        assert_eq!(provider_for("GeMiNi-1.5-pro"), "google");
    }

    #[test]
    fn parse_eval_headers_default_off() {
        let h = HeaderMap::new();
        let p = parse_eval_headers(&h);
        assert!(!p.eval_mode);
        assert!(p.seed.is_none());
        assert!(!p.trace_full);
        assert!(!p.force_temp_zero);
        assert!(!p.skip_rewrite);
        assert!(!p.skip_rerank);
        assert!(!p.any_set());
    }

    #[test]
    fn parse_eval_headers_t1_knobs() {
        let mut h = HeaderMap::new();
        h.insert("x-attune-eval-seed", "42".parse().unwrap());
        h.insert("x-attune-eval-force-temp-zero", "true".parse().unwrap());
        h.insert("x-attune-eval-skip-rewrite", "true".parse().unwrap());
        h.insert("x-attune-eval-skip-rerank", "1".parse().unwrap());
        let p = parse_eval_headers(&h);
        assert_eq!(p.seed, Some(42));
        assert!(p.force_temp_zero);
        assert!(p.skip_rewrite);
        assert!(p.skip_rerank);
        assert!(p.any_set());
    }

    #[test]
    fn build_eval_block_null_when_no_headers() {
        let p = ParsedEvalHeaders::default();
        assert!(build_eval_block(&p, 12).is_null());
    }

    #[test]
    fn build_eval_block_populated_when_seed_only() {
        // T1: seed alone (no eval_mode) still surfaces the eval block —
        // this is the structural contract `eval_determinism_test::seed_header_propagates_to_llm_options`
        // depends on.
        let p = ParsedEvalHeaders {
            seed: Some(42),
            ..Default::default()
        };
        let v = build_eval_block_with_determinism(&p, 7, Some("exact"));
        assert_eq!(v["determinism"], "exact");
        assert_eq!(v["seed_used"], 42);
        assert_eq!(v["abstained"], false);
        assert!(v["abstention_reason"].is_null());
    }
}
