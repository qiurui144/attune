//! Usage events CRUD — backing the `/api/v1/usage/*` REST surface.
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md`
//! §3 (DB schema) + §5.2 (REST endpoints).
//!
//! All methods are inherent `impl Store` extensions (per the store module
//! convention — `items.rs` etc. follow the same pattern).

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::Store;
use crate::usage::types::{CacheOutcome, CallOutcome, ErrorKind, UsageEvent, UsageKind};

/// Aggregate summary returned by `GET /api/v1/usage/summary`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub events: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    /// attune's own full-response cache (governor L1/L2) hit fraction —
    /// `hit_events / total_events` over the `cache` column. NOT vendor cache.
    pub cache_hit_rate: f64,
    /// Vendor prompt-cache (prefix-cache) hit fraction —
    /// `sum(cached_in) / sum(tokens_in)`. This is the §4.5-G3 signal
    /// (target ≥ 0.50 on stable-prefix multi-turn flows). 0.0 when no input
    /// tokens are recorded. Distinct from [`Self::cache_hit_rate`].
    pub prompt_cache_hit_rate: f64,
}

/// Convert an enum to its SQLite TEXT column value (matches the serde
/// `rename_all = "snake_case"` wire format).
fn kind_to_sql(k: UsageKind) -> &'static str {
    match k {
        UsageKind::LlmChat => "llm_chat",
        UsageKind::LlmExtract => "llm_extract",
        UsageKind::Embed => "embed",
        UsageKind::Rerank => "rerank",
        UsageKind::Ocr => "ocr",
        UsageKind::Asr => "asr",
        UsageKind::Vlm => "vlm",
    }
}

fn cache_to_sql(c: CacheOutcome) -> &'static str {
    match c {
        CacheOutcome::Hit => "hit",
        CacheOutcome::Miss => "miss",
        CacheOutcome::Bypass => "bypass",
    }
}

fn err_kind_to_sql(e: ErrorKind) -> &'static str {
    match e {
        ErrorKind::Parse => "parse",
        ErrorKind::Grounding => "grounding",
        ErrorKind::Timeout => "timeout",
        ErrorKind::Quota => "quota",
        ErrorKind::Network => "network",
        ErrorKind::SchemaInvalid => "schema_invalid",
        ErrorKind::Other => "other",
    }
}

impl Store {
    /// Insert one [`UsageEvent`]. Never fails on duplicate (id is autoincrement).
    pub fn record_usage(&self, event: &UsageEvent) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO usage_events
             (ts_ms, kind, provider, model, agent_id, tokens_in, tokens_out, cached_in,
              cost_usd, cache, outcome, latency_ms, error_kind, query_hash)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        )?;
        let (outcome_str, err_kind_str) = match event.outcome {
            CallOutcome::Ok => ("ok", None),
            CallOutcome::Retry { .. } => ("retry", None),
            CallOutcome::Fail { error_kind } => ("fail", Some(err_kind_to_sql(error_kind))),
        };
        stmt.execute(params![
            event.ts_ms,
            kind_to_sql(event.kind),
            event.usage.provider,
            event.usage.model,
            event.agent_id,
            event.usage.tokens_in,
            event.usage.tokens_out,
            event.usage.cached_in,
            event.cost_usd,
            cache_to_sql(event.cache),
            outcome_str,
            event.latency_ms,
            err_kind_str,
            event.query_hash,
        ])?;
        Ok(())
    }

    /// Delete usage_events older than `cutoff_ms`. Returns row count removed.
    /// Spec §7.2: `usage_events` table > 100k rows triggers `purge_old` worker.
    pub fn purge_usage_older_than(&self, cutoff_ms: i64) -> Result<usize> {
        let n = self
            .conn
            .execute("DELETE FROM usage_events WHERE ts_ms < ?1", params![cutoff_ms])?;
        Ok(n)
    }

    /// Delete all usage_events. Used by `POST /api/v1/usage/reset` (user must
    /// confirm in Settings UI per spec §8.3).
    pub fn reset_usage(&self) -> Result<usize> {
        let n = self.conn.execute("DELETE FROM usage_events", [])?;
        Ok(n)
    }

    /// Aggregate summary over a time window. Backs `GET /api/v1/usage/summary`.
    /// `from_ms` and `to_ms` are inclusive bounds.
    pub fn usage_summary(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT count(*),
                    coalesce(sum(tokens_in),0),
                    coalesce(sum(tokens_out),0),
                    coalesce(sum(cost_usd),0.0),
                    coalesce(sum(CASE cache WHEN 'hit' THEN 1 ELSE 0 END),0),
                    coalesce(sum(cached_in),0)
             FROM usage_events WHERE ts_ms BETWEEN ?1 AND ?2",
        )?;
        let (events, ti, to_, cost, hits, cached): (i64, i64, i64, f64, i64, i64) = stmt
            .query_row(params![from_ms, to_ms], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })?;
        let cache_hit_rate = if events > 0 {
            hits as f64 / events as f64
        } else {
            0.0
        };
        // Vendor prompt-cache rate is token-weighted (a 768-token-cached call
        // and a 0-cached call are not "half cached"). §4.5-G3 target ≥ 0.50.
        let prompt_cache_hit_rate = if ti > 0 { cached as f64 / ti as f64 } else { 0.0 };
        Ok(UsageSummary {
            events: events as u64,
            tokens_in: ti as u64,
            tokens_out: to_ as u64,
            cost_usd: cost,
            cache_hit_rate,
            prompt_cache_hit_rate,
        })
    }
}

#[cfg(test)]
mod usage_test {
    use super::*;
    use crate::usage::types::TokenUsage;
    use tempfile::TempDir;

    fn make_event(ts_ms: i64, cache: CacheOutcome, cost: Option<f64>) -> UsageEvent {
        make_event_cached(ts_ms, cache, cost, 0)
    }

    fn make_event_cached(
        ts_ms: i64,
        cache: CacheOutcome,
        cost: Option<f64>,
        cached_in: u32,
    ) -> UsageEvent {
        UsageEvent {
            ts_ms,
            kind: UsageKind::LlmChat,
            usage: TokenUsage {
                tokens_in: 100,
                tokens_out: 50,
                cached_in,
                model: "qwen2.5:3b".into(),
                provider: "ollama".into(),
            },
            cost_usd: cost,
            cache,
            outcome: CallOutcome::Ok,
            latency_ms: 200,
            agent_id: None,
            query_hash: None,
        }
    }

    #[test]
    fn fresh_vault_has_usage_events_table() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("v.db")).unwrap();
        let conn = store.raw_connection_for_test();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='usage_events'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "usage_events table must exist after fresh open");
    }

    #[test]
    fn record_usage_inserts_one_row() {
        let store = Store::open_memory().unwrap();
        store.record_usage(&make_event(1000, CacheOutcome::Miss, Some(0.001))).unwrap();
        let summary = store.usage_summary(0, 2000).unwrap();
        assert_eq!(summary.events, 1);
        assert_eq!(summary.tokens_in, 100);
        assert_eq!(summary.tokens_out, 50);
        assert!((summary.cost_usd - 0.001).abs() < 1e-9);
        assert_eq!(summary.cache_hit_rate, 0.0);
    }

    #[test]
    fn usage_summary_prompt_cache_hit_rate_is_token_weighted() {
        let store = Store::open_memory().unwrap();
        // Each event: tokens_in=100. cached_in varies. prompt_cache_hit_rate
        // = sum(cached_in) / sum(tokens_in), NOT an event count.
        store.record_usage(&make_event_cached(100, CacheOutcome::Miss, None, 80)).unwrap();
        store.record_usage(&make_event_cached(200, CacheOutcome::Miss, None, 20)).unwrap();
        let s = store.usage_summary(0, 1000).unwrap();
        // (80 + 20) / (100 + 100) = 0.5
        assert!(
            (s.prompt_cache_hit_rate - 0.5).abs() < 1e-9,
            "vendor prompt-cache rate must be token-weighted, got {}",
            s.prompt_cache_hit_rate
        );
        // Response-cache rate (cache column) is independent and 0 here (all Miss).
        assert_eq!(s.cache_hit_rate, 0.0);
    }

    #[test]
    fn usage_summary_prompt_cache_hit_rate_zero_when_no_input_tokens() {
        let store = Store::open_memory().unwrap();
        let s = store.usage_summary(0, 999_999_999).unwrap();
        assert_eq!(
            s.prompt_cache_hit_rate, 0.0,
            "empty/no-input must not divide by zero"
        );
    }

    #[test]
    fn usage_summary_cache_hit_rate() {
        let store = Store::open_memory().unwrap();
        store.record_usage(&make_event(100, CacheOutcome::Miss, None)).unwrap();
        store.record_usage(&make_event(200, CacheOutcome::Hit, None)).unwrap();
        store.record_usage(&make_event(300, CacheOutcome::Hit, None)).unwrap();
        store.record_usage(&make_event(400, CacheOutcome::Bypass, None)).unwrap();
        let s = store.usage_summary(0, 1000).unwrap();
        assert_eq!(s.events, 4);
        // 2 hits out of 4 = 0.5
        assert!((s.cache_hit_rate - 0.5).abs() < 1e-9, "got {}", s.cache_hit_rate);
    }

    #[test]
    fn usage_summary_range_filter() {
        let store = Store::open_memory().unwrap();
        store.record_usage(&make_event(50, CacheOutcome::Miss, None)).unwrap();
        store.record_usage(&make_event(500, CacheOutcome::Miss, None)).unwrap();
        store.record_usage(&make_event(1500, CacheOutcome::Miss, None)).unwrap();
        let s = store.usage_summary(100, 1000).unwrap();
        assert_eq!(s.events, 1, "only the 500 ms event should be in [100, 1000]");
    }

    #[test]
    fn usage_summary_empty_table_returns_zero_hit_rate() {
        let store = Store::open_memory().unwrap();
        let s = store.usage_summary(0, 999_999_999).unwrap();
        assert_eq!(s.events, 0);
        assert_eq!(s.cache_hit_rate, 0.0, "empty must not divide by zero");
    }

    #[test]
    fn purge_usage_older_than_deletes_only_older_rows() {
        let store = Store::open_memory().unwrap();
        store.record_usage(&make_event(100, CacheOutcome::Miss, None)).unwrap();
        store.record_usage(&make_event(500, CacheOutcome::Miss, None)).unwrap();
        store.record_usage(&make_event(900, CacheOutcome::Miss, None)).unwrap();
        let removed = store.purge_usage_older_than(500).unwrap();
        assert_eq!(removed, 1, "only ts_ms=100 < 500");
        assert_eq!(store.usage_summary(0, 9999).unwrap().events, 2);
    }

    #[test]
    fn reset_usage_empties_table() {
        let store = Store::open_memory().unwrap();
        store.record_usage(&make_event(100, CacheOutcome::Miss, None)).unwrap();
        store.record_usage(&make_event(200, CacheOutcome::Miss, None)).unwrap();
        let removed = store.reset_usage().unwrap();
        assert_eq!(removed, 2);
        assert_eq!(store.usage_summary(0, 9999).unwrap().events, 0);
    }

    #[test]
    fn record_usage_persists_all_call_outcomes() {
        let store = Store::open_memory().unwrap();

        let mut ev = make_event(100, CacheOutcome::Miss, None);
        ev.outcome = CallOutcome::Retry { attempt: 2 };
        store.record_usage(&ev).unwrap();

        ev.ts_ms = 200;
        ev.outcome = CallOutcome::Fail {
            error_kind: ErrorKind::Timeout,
        };
        store.record_usage(&ev).unwrap();

        let conn = store.raw_connection_for_test();
        let outcomes: Vec<String> = conn
            .prepare("SELECT outcome FROM usage_events ORDER BY ts_ms")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(outcomes, vec!["retry".to_string(), "fail".to_string()]);

        let err_kinds: Vec<Option<String>> = conn
            .prepare("SELECT error_kind FROM usage_events ORDER BY ts_ms")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(err_kinds, vec![None, Some("timeout".to_string())]);
    }
}
