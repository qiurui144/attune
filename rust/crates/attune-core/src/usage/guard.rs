//! Drop guard for usage telemetry.
//!
//! Every LLM / Embed / Rerank / OCR / ASR / VLM call site must hold a
//! [`UsageRecorderGuard`] across its work and call [`UsageRecorderGuard::complete`]
//! with the final usage / cache / outcome before the guard goes out of scope.
//!
//! **Debug builds**: dropping the guard without calling `complete()` panics —
//! catching missing telemetry at test time.
//!
//! **Release builds**: dropping without `complete()` logs a warning via `log::warn!`
//! and continues. We choose graceful degradation over killing production callers,
//! per spec §11 risk 1 mitigation 2 + spec §7.3 graceful degradation.
//!
//! Latency is sampled from `Instant::now()` at construction; `complete()` reads
//! `started.elapsed()` and clamps to `u32::MAX` ms.

use std::time::Instant;

use crate::usage::types::{CacheOutcome, CallOutcome, TokenUsage, UsageEvent, UsageKind};

/// Closure type the guard invokes on `complete()` with the finalized event.
/// Boxed so multiple call sites can use different recorder implementations
/// (writing to SQLite directly, queueing into a ring buffer aggregator, etc.).
pub type RecordFn = Box<dyn FnOnce(UsageEvent) + Send>;

/// Holds work-in-progress telemetry until `complete()` is called.
///
/// Construction starts the latency clock. Optional fields (`agent_id`,
/// `query_hash`) can be attached via the builder methods.
pub struct UsageRecorderGuard {
    kind: UsageKind,
    provider: String,
    model: String,
    started: Instant,
    agent_id: Option<String>,
    query_hash: Option<String>,
    recorder: Option<RecordFn>,
    completed: bool,
}

impl UsageRecorderGuard {
    /// Begin a usage measurement. The latency timer starts here.
    pub fn new(
        kind: UsageKind,
        provider: &str,
        model: &str,
        recorder: RecordFn,
    ) -> Self {
        Self {
            kind,
            provider: provider.into(),
            model: model.into(),
            started: Instant::now(),
            agent_id: None,
            query_hash: None,
            recorder: Some(recorder),
            completed: false,
        }
    }

    /// Tag the recorded event with the calling agent id.
    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Tag the recorded event with a BLAKE3 query-hash prefix. Only called
    /// when `settings.usage.log_queries = true` (per spec §8.3 default off).
    pub fn with_query_hash(mut self, hash: impl Into<String>) -> Self {
        self.query_hash = Some(hash.into());
        self
    }

    /// Finalize the measurement, invoke the recorder, and mark the guard as
    /// completed. Idempotent — calling more than once is a no-op (the recorder
    /// is consumed on the first call).
    pub fn complete(
        &mut self,
        usage: TokenUsage,
        cache: CacheOutcome,
        outcome: CallOutcome,
    ) {
        if self.completed {
            return;
        }
        let latency_ms = self
            .started
            .elapsed()
            .as_millis()
            .min(u32::MAX as u128) as u32;
        // estimate_cost_usd returns Option<f64>; pricing-unknown is internal
        // telemetry only and must not block the main call path (spec §7.1).
        let cost_usd = crate::cost::estimate_cost_usd(
            usage.tokens_in as usize,
            usage.tokens_out as usize,
            &usage.model,
        );
        let event = UsageEvent {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            kind: self.kind,
            usage,
            cost_usd,
            cache,
            outcome,
            latency_ms,
            agent_id: self.agent_id.take(),
            query_hash: self.query_hash.take(),
        };
        if let Some(recorder) = self.recorder.take() {
            recorder(event);
        }
        self.completed = true;
    }
}

impl Drop for UsageRecorderGuard {
    fn drop(&mut self) {
        if !self.completed {
            // Debug builds: hard-fail the test so the missing-complete is fixed
            // before merge. Release builds: log + continue (graceful degradation).
            #[cfg(debug_assertions)]
            {
                // Skip the panic when the thread is already unwinding from a
                // different panic — double-panic aborts the process, which
                // makes the *original* failure invisible in test output.
                if !std::thread::panicking() {
                    panic!(
                        "UsageRecorderGuard dropped without complete() — \
                         provider={} model={} kind={:?}",
                        self.provider, self.model, self.kind
                    );
                }
            }
            #[cfg(not(debug_assertions))]
            {
                log::warn!(
                    "UsageRecorderGuard dropped without complete() — telemetry lost \
                     (provider={} model={} kind={:?})",
                    self.provider,
                    self.model,
                    self.kind
                );
            }
        }
    }
}
