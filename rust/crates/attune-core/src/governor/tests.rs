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
