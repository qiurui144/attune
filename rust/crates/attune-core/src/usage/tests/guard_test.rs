//! UsageRecorderGuard tests — happy path + debug panic on drop without complete.

use std::sync::{Arc, Mutex};

use crate::usage::guard::UsageRecorderGuard;
use crate::usage::types::{CacheOutcome, CallOutcome, TokenUsage, UsageEvent, UsageKind};

#[allow(clippy::type_complexity)]
fn shared_sink() -> (Arc<Mutex<Vec<UsageEvent>>>, Box<dyn FnOnce(UsageEvent) + Send>) {
    let sink: Arc<Mutex<Vec<UsageEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let s = sink.clone();
    let recorder = Box::new(move |e: UsageEvent| {
        s.lock().unwrap().push(e);
    });
    (sink, recorder)
}

#[test]
fn guard_records_on_complete() {
    let (sink, recorder) = shared_sink();
    {
        let mut g = UsageRecorderGuard::new(
            UsageKind::LlmChat,
            "ollama",
            "qwen2.5:3b",
            recorder,
        );
        g.complete(
            TokenUsage::empty("ollama", "qwen2.5:3b"),
            CacheOutcome::Miss,
            CallOutcome::Ok,
        );
    }
    let events = sink.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, UsageKind::LlmChat);
    assert_eq!(events[0].usage.provider, "ollama");
    assert_eq!(events[0].cache, CacheOutcome::Miss);
}

#[test]
fn guard_complete_is_idempotent() {
    let (sink, recorder) = shared_sink();
    let mut g = UsageRecorderGuard::new(
        UsageKind::LlmChat,
        "ollama",
        "qwen2.5:3b",
        recorder,
    );
    g.complete(
        TokenUsage::empty("ollama", "qwen2.5:3b"),
        CacheOutcome::Miss,
        CallOutcome::Ok,
    );
    // Second call should be a no-op (recorder already consumed).
    g.complete(
        TokenUsage::empty("ollama", "qwen2.5:3b"),
        CacheOutcome::Hit,
        CallOutcome::Ok,
    );
    drop(g);
    assert_eq!(sink.lock().unwrap().len(), 1, "only first complete records");
}

#[test]
fn guard_records_latency_and_cost() {
    let (sink, recorder) = shared_sink();
    let mut g = UsageRecorderGuard::new(
        UsageKind::LlmChat,
        "ollama",
        "qwen2.5:3b",
        recorder,
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut usage = TokenUsage::empty("ollama", "qwen2.5:3b");
    usage.tokens_in = 1000;
    usage.tokens_out = 500;
    g.complete(usage, CacheOutcome::Miss, CallOutcome::Ok);

    let events = sink.lock().unwrap();
    assert_eq!(events.len(), 1);
    // Local model qwen2.5:3b has no pricing entry → cost_usd should be None
    // OR Some(0.0). Either is acceptable per spec §7.1.
    assert!(
        events[0].cost_usd.is_none() || events[0].cost_usd == Some(0.0),
        "local model should produce None or 0.0 cost, got {:?}",
        events[0].cost_usd
    );
    assert!(events[0].latency_ms >= 5, "latency should be >= 5 ms");
}

#[test]
fn guard_builder_attaches_agent_and_query_hash() {
    let (sink, recorder) = shared_sink();
    {
        let mut g = UsageRecorderGuard::new(
            UsageKind::LlmExtract,
            "ollama",
            "qwen2.5:3b",
            recorder,
        )
        .with_agent("defamation_extractor")
        .with_query_hash("abcdef0123456789");
        g.complete(
            TokenUsage::empty("ollama", "qwen2.5:3b"),
            CacheOutcome::Miss,
            CallOutcome::Ok,
        );
    }
    let events = sink.lock().unwrap();
    assert_eq!(events[0].agent_id.as_deref(), Some("defamation_extractor"));
    assert_eq!(events[0].query_hash.as_deref(), Some("abcdef0123456789"));
}

#[test]
#[should_panic(expected = "UsageRecorderGuard dropped without complete()")]
#[cfg(debug_assertions)]
fn guard_panics_on_drop_without_complete_in_debug() {
    let (_sink, recorder) = shared_sink();
    let _g = UsageRecorderGuard::new(UsageKind::LlmChat, "ollama", "x", recorder);
    // Drop without complete() — must panic in debug builds.
}

#[test]
#[cfg(debug_assertions)]
fn guard_does_not_double_panic_during_unwind() {
    // If the guard's Drop fires while the thread is already panicking from
    // some other test failure, double-panic would abort the whole process.
    // The guard's Drop must skip the panic in that case.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (_sink, recorder) = shared_sink();
        let _g = UsageRecorderGuard::new(UsageKind::LlmChat, "ollama", "x", recorder);
        panic!("simulated upstream failure");
    }));
    assert!(result.is_err(), "panic propagated as expected");
}
