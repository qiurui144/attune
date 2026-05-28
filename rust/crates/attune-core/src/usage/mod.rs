//! Usage telemetry — standard API for LLM/Embed/Rerank/OCR/ASR/VLM call records.
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md` §5.1
//!
//! Public surface (frozen at Task M for Plan A2 routing consumers):
//! - [`TokenUsage`] — per-call vendor usage report (tokens_in / out / cached_in)
//! - [`UsageEvent`] — record persisted to `usage_events` table
//! - [`CacheOutcome`] / [`CallOutcome`] / [`UsageKind`] / [`ErrorKind`] — enums

pub mod aggregator;
pub mod guard;
pub mod types;

pub use aggregator::UsageAggregator;
pub use guard::{RecordFn, UsageRecorderGuard};
pub use types::{CacheOutcome, CallOutcome, ErrorKind, TokenUsage, UsageEvent, UsageKind};

#[cfg(test)]
mod tests;
