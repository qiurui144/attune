//! Ring buffer + async batch flush for usage telemetry.
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md`
//! §3 (aggregator flow) + §11 risk 2 (write amplification mitigation).
//!
//! Design:
//! - Main path (every LLM/Embed/etc. call site) invokes [`UsageAggregator::record`]
//!   which is a sync `VecDeque::push_back` — microsecond latency, no blocking I/O.
//! - A background tokio task (spawned via [`UsageAggregator::spawn_flusher`])
//!   drains the buffer into SQLite every `flush_interval_ms`.
//! - When the ring buffer is full, the **oldest** event is dropped + a warning
//!   is logged. We prefer to lose old telemetry over blocking the LLM path
//!   (spec §7.3 graceful degradation).
//!
//! Store access is serialized through a `Mutex` because `rusqlite::Connection`
//! is `Send` but not `Sync` (it holds `RefCell<InnerConnection>`). The mutex
//! is never held across an `.await`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::store::Store;
use crate::usage::types::UsageEvent;

/// In-memory ring buffer of pending [`UsageEvent`]s + a background flusher
/// that periodically persists them to the `usage_events` SQLite table.
///
/// Cheap to clone (`Arc` internally — wrap in `Arc<UsageAggregator>` for
/// callers that need to share + spawn the flusher).
pub struct UsageAggregator {
    buffer: Arc<Mutex<VecDeque<UsageEvent>>>,
    store: Arc<Mutex<Store>>,
    cap: usize,
    flush_interval_ms: u64,
}

impl UsageAggregator {
    /// Construct a new aggregator.
    ///
    /// - `flush_interval_ms`: e.g. 100 (laptop) / 500 (K3) per spec §11 risk 6.
    /// - `cap`: max in-memory events before drop-oldest kicks in (typical 1000
    ///   per spec §11 risk 2 mitigation 5).
    pub fn new(store: Arc<Mutex<Store>>, flush_interval_ms: u64, cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(cap))),
            store,
            cap,
            flush_interval_ms,
        }
    }

    /// Enqueue one event. Called from every LLM/Embed/etc. recorder closure.
    ///
    /// Drop-oldest when buffer is full. Mutex poisoning treated as a no-op
    /// (cannot fail the main call path).
    pub fn record(&self, event: UsageEvent) {
        if let Ok(mut buf) = self.buffer.lock() {
            if buf.len() >= self.cap {
                buf.pop_front();
                log::warn!(
                    "UsageAggregator buffer full (cap={}); oldest event dropped",
                    self.cap
                );
            }
            buf.push_back(event);
        }
    }

    /// Drain the buffer to the store. Called from `tick()` and `flush_now()`.
    /// Visible for tests; production callers should use the spawned flusher.
    pub async fn tick(&self) {
        self.flush_now().await
    }

    /// Synchronously drain the buffer and persist every pending event. Errors
    /// during individual `record_usage` are logged and the event is skipped —
    /// one bad row must not block subsequent ones.
    pub async fn flush_now(&self) {
        let drained: Vec<UsageEvent> = {
            let Ok(mut buf) = self.buffer.lock() else {
                return;
            };
            buf.drain(..).collect()
        };
        if drained.is_empty() {
            return;
        }
        let Ok(store) = self.store.lock() else {
            log::warn!(
                "UsageAggregator: store mutex poisoned; {} events lost",
                drained.len()
            );
            return;
        };
        for e in &drained {
            if let Err(err) = store.record_usage(e) {
                log::warn!(
                    "record_usage failed: {err:?} — telemetry lost for kind={:?} provider={}",
                    e.kind,
                    e.usage.provider
                );
            }
        }
    }

    /// Snapshot of the most-recent `n` buffered events (newest first).
    /// Used by Plan A2's `CapabilityRouter` for routing-feedback decisions.
    /// Returns events still in the ring buffer; events already flushed to SQL
    /// are not visible here (those are queried via the store API).
    pub fn recent(&self, n: usize) -> Vec<UsageEvent> {
        self.buffer
            .lock()
            .map(|b| b.iter().rev().take(n).cloned().collect())
            .unwrap_or_default()
    }

    /// Current buffer occupancy — exposed for tests + observability.
    pub fn buffer_len(&self) -> usize {
        self.buffer.lock().map(|b| b.len()).unwrap_or(0)
    }

    /// Spawn a tokio task that ticks every `flush_interval_ms`. The handle is
    /// returned so the caller can `.abort()` on shutdown. Call this exactly
    /// once per `UsageAggregator` (per spec §11 risk 2 mitigation 1).
    pub fn spawn_flusher(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval = Duration::from_millis(self.flush_interval_ms);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Discard the immediate first tick so we wait a full interval
            // before the first flush.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                self.tick().await;
            }
        })
    }
}
