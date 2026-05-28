//! UsageAggregator tests — buffer behavior, flush correctness, recent() ordering.

use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use crate::store::Store;
use crate::usage::aggregator::UsageAggregator;
use crate::usage::types::{
    CacheOutcome, CallOutcome, TokenUsage, UsageEvent, UsageKind,
};

fn make_store() -> (TempDir, Arc<Mutex<Store>>) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("v.db")).unwrap();
    (dir, Arc::new(Mutex::new(store)))
}

fn mkevent(ts_ms: i64) -> UsageEvent {
    UsageEvent {
        ts_ms,
        kind: UsageKind::LlmChat,
        usage: TokenUsage::empty("ollama", "qwen2.5:3b"),
        cost_usd: Some(0.0),
        cache: CacheOutcome::Miss,
        outcome: CallOutcome::Ok,
        latency_ms: 100,
        agent_id: None,
        query_hash: None,
    }
}

#[tokio::test]
async fn flush_now_persists_buffered_events() {
    let (_dir, store) = make_store();
    let agg = UsageAggregator::new(store.clone(), 100, 50);
    agg.record(mkevent(100));
    agg.record(mkevent(200));
    assert_eq!(agg.buffer_len(), 2);

    agg.flush_now().await;
    assert_eq!(agg.buffer_len(), 0, "buffer drained after flush");

    let summary = store.lock().unwrap().usage_summary(0, 9_999_999_999_999).unwrap();
    assert_eq!(summary.events, 2);
}

#[tokio::test]
async fn flush_now_on_empty_buffer_is_noop() {
    let (_dir, store) = make_store();
    let agg = UsageAggregator::new(store.clone(), 100, 50);
    agg.flush_now().await;
    let summary = store.lock().unwrap().usage_summary(0, 9_999_999_999_999).unwrap();
    assert_eq!(summary.events, 0);
}

#[tokio::test]
async fn ring_buffer_full_drops_oldest() {
    let (_dir, store) = make_store();
    let agg = UsageAggregator::new(store.clone(), 100, 2); // cap=2

    agg.record(mkevent(100));
    agg.record(mkevent(200));
    agg.record(mkevent(300)); // drops 100
    assert_eq!(agg.buffer_len(), 2);

    agg.flush_now().await;
    let conn_store = store.lock().unwrap();
    let conn = conn_store.raw_connection_for_test();
    let ts: Vec<i64> = conn
        .prepare("SELECT ts_ms FROM usage_events ORDER BY ts_ms")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(ts, vec![200, 300], "oldest (ts=100) dropped");
}

#[tokio::test]
async fn recent_returns_newest_first() {
    let (_dir, store) = make_store();
    let agg = UsageAggregator::new(store, 100, 50);
    agg.record(mkevent(100));
    agg.record(mkevent(200));
    agg.record(mkevent(300));

    let r = agg.recent(2);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].ts_ms, 300, "newest first");
    assert_eq!(r[1].ts_ms, 200);
}

#[tokio::test]
async fn recent_only_sees_buffered_events() {
    let (_dir, store) = make_store();
    let agg = UsageAggregator::new(store.clone(), 100, 50);
    agg.record(mkevent(100));
    agg.flush_now().await;
    // After flush, the event is in SQL but no longer in the ring buffer.
    assert_eq!(agg.recent(10).len(), 0);
    // The store does still hold it.
    let summary = store.lock().unwrap().usage_summary(0, 9_999_999_999_999).unwrap();
    assert_eq!(summary.events, 1);
}

#[tokio::test]
async fn record_after_zero_cap_does_not_crash() {
    // cap is clamped to >= 1 inside new(); a zero-cap caller still gets a
    // working aggregator instead of UB.
    let (_dir, store) = make_store();
    let agg = UsageAggregator::new(store, 100, 0);
    agg.record(mkevent(100));
    assert_eq!(agg.buffer_len(), 1, "cap clamped to >= 1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_flusher_persists_after_interval() {
    // `tokio::time::advance` would be ideal but requires the `test-util`
    // feature on tokio, which the workspace doesn't enable. Fall back to a
    // real-time sleep with a short flush interval — the test budget is small.
    let (_dir, store) = make_store();
    let agg = Arc::new(UsageAggregator::new(store.clone(), 20, 50));
    let handle = agg.clone().spawn_flusher();

    agg.record(mkevent(100));

    // Sleep long enough for the flusher to wake at least twice (one throwaway
    // first tick + one real tick + safety margin).
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    handle.abort();
    let _ = handle.await;

    let summary = store
        .lock()
        .unwrap()
        .usage_summary(0, 9_999_999_999_999)
        .unwrap();
    assert_eq!(summary.events, 1, "background flusher persisted the event");
}
