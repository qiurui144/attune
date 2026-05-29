//! ACP-4 Cost Governor — integration test for the production wire.
//!
//! Verifies the exact component graph the chat route assembles
//! (`state.cache_backend()` + `state.usage()` → `governor::governed_chat`):
//! a cache hit avoids the upstream call AND a usage_events row is written.
//! Spec: docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md §9 (ACP-4 row).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use attune_core::cache::memory::MemoryLruCache;
use attune_core::cache::CacheBackend;
use attune_core::governor::governed_chat;
use attune_core::llm::{ChatMessage, LlmCallOptions, LlmProvider};
use attune_core::store::Store;
use attune_core::usage::{CacheOutcome, TokenUsage, UsageAggregator};

struct CountingProvider {
    calls: AtomicUsize,
    model: String,
}
impl LlmProvider for CountingProvider {
    fn chat(&self, _s: &str, _u: &str) -> attune_core::error::Result<(String, TokenUsage)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((
            "answer".to_string(),
            TokenUsage {
                tokens_in: 80,
                tokens_out: 12,
                cached_in: 0,
                model: self.model.clone(),
                provider: "ollama".into(),
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
        true
    }
}

#[test]
fn governed_chat_wire_caches_and_records_like_the_route() {
    // Exactly the component types the chat route hands to governed_chat.
    let provider = CountingProvider {
        calls: AtomicUsize::new(0),
        model: "qwen2.5:3b".into(),
    };
    let cache: Arc<dyn CacheBackend> = Arc::new(MemoryLruCache::new(512));
    let store = Arc::new(Mutex::new(Store::open_memory().expect("store")));
    let agg = UsageAggregator::new(store.clone(), 100, 1000);

    let messages = vec![ChatMessage::system("sys"), ChatMessage::user("hi")];
    let opts = LlmCallOptions::default();

    // First call: miss → upstream call + recorded.
    let r1 = governed_chat(
        &provider,
        &messages,
        &opts,
        Some(cache.as_ref()),
        Some(&agg),
        None,
        None,
    )
    .expect("first governed_chat");
    assert_eq!(r1.text, "answer");
    assert_eq!(r1.cache, CacheOutcome::Miss);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    // Second identical call: hit → no upstream call.
    let r2 = governed_chat(
        &provider,
        &messages,
        &opts,
        Some(cache.as_ref()),
        Some(&agg),
        None,
        None,
    )
    .expect("second governed_chat");
    assert_eq!(r2.cache, CacheOutcome::Hit, "identical prompt served from cache");
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "cache hit avoids upstream call (token saving)"
    );

    // Flush telemetry and assert usage_events really has rows (audit-C gap closed).
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(agg.flush_now());
    let summary = store
        .lock()
        .unwrap()
        .usage_summary(0, i64::MAX)
        .expect("usage summary");
    assert_eq!(summary.events, 2, "both miss and hit recorded to usage_events");
    assert!(summary.cache_hit_rate > 0.0, "hit registered in telemetry");
}

/// `AppState::install_usage_aggregator` opens a 2nd Store connection on the main
/// DB path and spawns the flusher (the minimal-surface fix for the A1
/// `Vault::store_arc` blocker). Smoke-test it boots without the big refactor.
#[tokio::test]
async fn install_usage_aggregator_boots_on_temp_home() {
    let tmp = tempfile::TempDir::new().unwrap();
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    assert!(state.usage().is_none(), "no aggregator before install");
    let handle = state.install_usage_aggregator();
    assert!(handle.is_some(), "flusher handle returned");
    assert!(state.usage().is_some(), "aggregator installed");
    // Second install is a no-op (already present).
    assert!(
        state.install_usage_aggregator().is_none(),
        "install is idempotent (no double flusher)"
    );
    if let Some(h) = handle {
        h.abort();
    }
}
