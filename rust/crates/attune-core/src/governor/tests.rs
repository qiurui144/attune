//! ACP-4 Cost Governor tests — cache hit/miss correctness, key stability under
//! R1 stale conditions, output cap propagation, and graceful degradation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::cache::memory::MemoryLruCache;
use crate::cache::CacheBackend;
use crate::governor::{governed_chat, llm_cache_key};
use crate::llm::{ChatMessage, LlmCallOptions, LlmProvider};
use crate::usage::{CacheOutcome, TokenUsage};

/// Counting LLM provider: returns a fixed reply and records how many upstream
/// calls it received, so a cache hit (= zero new upstream calls) is observable.
struct CountingProvider {
    calls: AtomicUsize,
    reply: String,
    model: String,
    local: bool,
}

impl CountingProvider {
    fn new(reply: &str) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            reply: reply.to_string(),
            model: "qwen2.5:3b".to_string(),
            local: true,
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl LlmProvider for CountingProvider {
    fn chat(&self, _system: &str, _user: &str) -> crate::error::Result<(String, TokenUsage)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((
            self.reply.clone(),
            TokenUsage {
                tokens_in: 100,
                tokens_out: 20,
                cached_in: 0,
                model: self.model.clone(),
                provider: "ollama".to_string(),
            },
        ))
    }
    fn is_available(&self) -> bool {
        true
    }
    fn model_name(&self) -> &str {
        &self.model
    }
    fn is_local(&self) -> bool {
        self.local
    }
}

fn msgs(user: &str) -> Vec<ChatMessage> {
    vec![ChatMessage::system("sys"), ChatMessage::user(user)]
}

#[test]
fn miss_then_hit_avoids_second_upstream_call() {
    let provider = CountingProvider::new("hello world");
    let cache: Arc<dyn CacheBackend> = Arc::new(MemoryLruCache::new(64));
    let opts = LlmCallOptions::default();
    let m = msgs("question A");

    let r1 = governed_chat(&provider, &m, &opts, Some(cache.as_ref()), None, None, None).unwrap();
    assert_eq!(r1.text, "hello world");
    assert_eq!(r1.cache, CacheOutcome::Miss, "first call is a miss");
    assert_eq!(provider.calls(), 1, "one upstream call on miss");

    let r2 = governed_chat(&provider, &m, &opts, Some(cache.as_ref()), None, None, None).unwrap();
    assert_eq!(r2.text, "hello world", "hit returns the cached text");
    assert_eq!(r2.cache, CacheOutcome::Hit, "second identical call is a hit");
    assert_eq!(
        provider.calls(),
        1,
        "cache hit must NOT issue a second upstream call (token saving)"
    );
    assert_eq!(
        r2.usage.cached_in, 100,
        "hit surfaces saved input tokens via cached_in"
    );
}

#[test]
fn different_message_is_a_miss() {
    let provider = CountingProvider::new("resp");
    let cache: Arc<dyn CacheBackend> = Arc::new(MemoryLruCache::new(64));
    let opts = LlmCallOptions::default();

    let _ = governed_chat(
        &provider,
        &msgs("question A"),
        &opts,
        Some(cache.as_ref()),
        None,
        None,
        None,
    )
    .unwrap();
    let r = governed_chat(
        &provider,
        &msgs("question B"),
        &opts,
        Some(cache.as_ref()),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(r.cache, CacheOutcome::Miss, "different prompt → miss");
    assert_eq!(provider.calls(), 2, "two distinct prompts → two calls");
}

#[test]
fn no_cache_always_calls_upstream() {
    let provider = CountingProvider::new("r");
    let opts = LlmCallOptions::default();
    let m = msgs("same");
    let _ = governed_chat(&provider, &m, &opts, None, None, None, None).unwrap();
    let r = governed_chat(&provider, &m, &opts, None, None, None, None).unwrap();
    assert_eq!(r.cache, CacheOutcome::Miss, "no cache → never a hit");
    assert_eq!(provider.calls(), 2, "no cache → every call hits upstream");
}

// ── R1 stale-cache key sensitivity ─────────────────────────────────────────

#[test]
fn key_changes_when_temperature_changes() {
    let m = msgs("q");
    let k_cold = llm_cache_key("model", &m, &LlmCallOptions::default());
    let k_hot = llm_cache_key(
        "model",
        &m,
        &LlmCallOptions {
            temperature: Some(0.9),
            ..Default::default()
        },
    );
    assert_ne!(k_cold, k_hot, "different temperature → different key (R1)");
}

#[test]
fn key_changes_when_seed_changes() {
    let m = msgs("q");
    let k1 = llm_cache_key(
        "model",
        &m,
        &LlmCallOptions {
            seed: Some(1),
            ..Default::default()
        },
    );
    let k2 = llm_cache_key(
        "model",
        &m,
        &LlmCallOptions {
            seed: Some(2),
            ..Default::default()
        },
    );
    assert_ne!(k1, k2, "different seed → different key (R1)");
}

#[test]
fn key_changes_when_message_content_changes() {
    // Injected RAG knowledge lives in message content; a changed source doc →
    // different content → different key → automatic invalidation.
    let opts = LlmCallOptions::default();
    let k1 = llm_cache_key("model", &msgs("doc v1 content"), &opts);
    let k2 = llm_cache_key("model", &msgs("doc v2 content"), &opts);
    assert_ne!(k1, k2, "changed injected context → different key (R1)");
}

#[test]
fn key_changes_when_model_changes() {
    let m = msgs("q");
    let opts = LlmCallOptions::default();
    assert_ne!(
        llm_cache_key("model-a", &m, &opts),
        llm_cache_key("model-b", &m, &opts),
        "different model → different key"
    );
}

#[test]
fn key_stable_for_identical_inputs() {
    let m = msgs("q");
    let opts = LlmCallOptions {
        seed: Some(7),
        temperature: Some(0.0),
        ..Default::default()
    };
    assert_eq!(
        llm_cache_key("m", &m, &opts),
        llm_cache_key("m", &m, &opts),
        "identical inputs → identical key (deterministic)"
    );
}

// ── Task 2: usage recorder wired (usage_events table really written) ────────

#[test]
fn governed_call_records_usage_event_to_store() {
    use crate::store::Store;
    use crate::usage::UsageAggregator;
    use std::sync::Mutex;

    let provider = CountingProvider::new("answer");
    let store = Arc::new(Mutex::new(Store::open_memory().expect("memory store")));
    let agg = UsageAggregator::new(store.clone(), 50, 1000);

    // miss → one usage event enqueued.
    let _ = governed_chat(
        &provider,
        &msgs("q1"),
        &LlmCallOptions::default(),
        None,
        Some(&agg),
        Some("chat"),
        None,
    )
    .unwrap();
    assert_eq!(agg.buffer_len(), 1, "miss enqueues exactly one usage event");

    // Flush to the usage_events table and confirm the row landed.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(agg.flush_now());

    let summary = {
        let s = store.lock().unwrap();
        s.usage_summary(0, i64::MAX).expect("usage summary")
    };
    assert_eq!(summary.events, 1, "usage_events table has one row after flush");
    assert!(summary.tokens_in >= 100, "recorded tokens_in from the miss");
}

#[test]
fn cache_hit_records_hit_outcome() {
    use crate::store::Store;
    use crate::usage::UsageAggregator;
    use std::sync::Mutex;

    let provider = CountingProvider::new("answer");
    let cache: Arc<dyn CacheBackend> = Arc::new(MemoryLruCache::new(64));
    let store = Arc::new(Mutex::new(Store::open_memory().expect("memory store")));
    let agg = UsageAggregator::new(store.clone(), 50, 1000);
    let m = msgs("q");

    // miss then hit → two events, one of which is a cache hit.
    let _ = governed_chat(
        &provider,
        &m,
        &LlmCallOptions::default(),
        Some(cache.as_ref()),
        Some(&agg),
        None,
        None,
    )
    .unwrap();
    let _ = governed_chat(
        &provider,
        &m,
        &LlmCallOptions::default(),
        Some(cache.as_ref()),
        Some(&agg),
        None,
        None,
    )
    .unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(agg.flush_now());

    let summary = {
        let s = store.lock().unwrap();
        s.usage_summary(0, i64::MAX).expect("usage summary")
    };
    assert_eq!(summary.events, 2, "miss + hit both recorded");
    assert!(
        summary.cache_hit_rate > 0.0,
        "cache hit must register in telemetry (hit_rate > 0)"
    );
}

// ── ACP-3 Task 3: failure outcome wired to telemetry ───────────────────────

/// A provider that always fails upstream — to prove the governor RECORDS the
/// failure (classified) before propagating the error, so the agent×model
/// failure-rate roll-up can see it.
struct FailingProvider {
    err: crate::error::VaultError,
    model: String,
}
impl FailingProvider {
    fn new(err: crate::error::VaultError) -> Self {
        Self { err: err_clone(&err), model: "qwen2.5:3b".to_string() }
    }
}
fn err_clone(e: &crate::error::VaultError) -> crate::error::VaultError {
    // VaultError is not Clone; reproduce the LlmUnavailable string we use.
    match e {
        crate::error::VaultError::LlmUnavailable(s) => {
            crate::error::VaultError::LlmUnavailable(s.clone())
        }
        _ => crate::error::VaultError::LlmUnavailable("upstream failure".into()),
    }
}
impl LlmProvider for FailingProvider {
    fn chat(&self, _system: &str, _user: &str) -> crate::error::Result<(String, TokenUsage)> {
        Err(err_clone(&self.err))
    }
    fn is_available(&self) -> bool {
        true
    }
    fn model_name(&self) -> &str {
        &self.model
    }
    fn is_local(&self) -> bool {
        true
    }
}

#[test]
fn upstream_failure_records_fail_outcome_to_telemetry() {
    use crate::agent_telemetry::AgentOutcome;
    use crate::store::Store;
    use crate::usage::UsageAggregator;
    use std::sync::Mutex;

    let provider = FailingProvider::new(crate::error::VaultError::LlmUnavailable(
        "rate limit exceeded".into(),
    ));
    let store = Arc::new(Mutex::new(Store::open_memory().expect("memory store")));
    let agg = UsageAggregator::new(store.clone(), 50, 1000);

    // The governed call MUST still return Err (we do not swallow the failure)...
    let res = governed_chat(
        &provider,
        &msgs("q-fail"),
        &LlmCallOptions::default(),
        None,
        Some(&agg),
        Some("defamation_extractor"),
        None,
    );
    assert!(res.is_err(), "a failing upstream must propagate Err");

    // ...AND it must have recorded a failure usage event for the agent×model.
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    rt.block_on(agg.flush_now());

    let health = {
        let s = store.lock().unwrap();
        s.agent_model_health(0, i64::MAX).expect("health roll-up")
    };
    assert_eq!(health.len(), 1, "exactly one (agent×model) row");
    let h = &health[0];
    assert_eq!(h.agent_id, "defamation_extractor");
    assert_eq!(h.model, "qwen2.5:3b");
    assert_eq!(h.total_calls, 1);
    assert_eq!(h.failures, 1, "the failed call must count as a failure");
    assert_eq!(h.failure_rate, 1.0);

    // The persisted outcome classifies as a rate-limit telemetry bucket.
    let s = store.lock().unwrap();
    let conn = s.raw_connection_for_test();
    let (outcome, error_kind): (String, Option<String>) = conn
        .query_row(
            "SELECT outcome, error_kind FROM usage_events WHERE agent_id = 'defamation_extractor'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(outcome, "fail");
    assert_eq!(error_kind.as_deref(), Some("quota"), "rate-limit → quota error_kind");
    // And reading it back as telemetry classifies as RateLimit.
    drop(s);
    assert_eq!(
        AgentOutcome::from_call_outcome(crate::usage::CallOutcome::Fail {
            error_kind: crate::usage::ErrorKind::Quota
        }),
        AgentOutcome::RateLimit
    );
}

#[test]
fn upstream_success_records_ok_not_failure() {
    // Regression guard: a successful call must NOT be counted as a failure.
    use crate::store::Store;
    use crate::usage::UsageAggregator;
    use std::sync::Mutex;

    let provider = CountingProvider::new("answer");
    let store = Arc::new(Mutex::new(Store::open_memory().expect("memory store")));
    let agg = UsageAggregator::new(store.clone(), 50, 1000);
    let _ = governed_chat(
        &provider,
        &msgs("q-ok"),
        &LlmCallOptions::default(),
        None,
        Some(&agg),
        Some("fact_extractor"),
        None,
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    rt.block_on(agg.flush_now());
    let health = {
        let s = store.lock().unwrap();
        s.agent_model_health(0, i64::MAX).expect("health")
    };
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].failures, 0, "success must not count as failure");
    assert_eq!(health[0].total_calls, 1);
}

#[test]
fn hit_serves_stored_text_not_provider_reply() {
    // After a miss caches "first", a provider whose reply later changes must
    // still get the cached "first" on an identical key (proves we read cache,
    // not re-call). Adversarial: guards against accidentally always-calling.
    let provider = CountingProvider::new("first");
    let cache: Arc<dyn CacheBackend> = Arc::new(MemoryLruCache::new(64));
    let opts = LlmCallOptions::default();
    let m = msgs("q");
    let _ = governed_chat(&provider, &m, &opts, Some(cache.as_ref()), None, None, None).unwrap();
    let r = governed_chat(&provider, &m, &opts, Some(cache.as_ref()), None, None, None).unwrap();
    assert_eq!(r.text, "first");
    assert_eq!(provider.calls(), 1);
}
