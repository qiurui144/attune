//! ACP-3 telemetry tests (spec §9 ACP-3 row): fail-rate calc correct / any
//! outcome sequence (proptest) / 0-call + all-fail boundary / write-never-blocks
//! / 1 real agent×model integration.

use super::*;
use crate::store::Store;
use crate::usage::types::{CacheOutcome, CallOutcome, ErrorKind, TokenUsage};

fn usage(model: &str) -> TokenUsage {
    TokenUsage {
        tokens_in: 100,
        tokens_out: 50,
        cached_in: 0,
        model: model.into(),
        provider: "ollama".into(),
    }
}

fn record(agent: &str, model: &str, outcome: AgentOutcome) -> AgentCallRecord {
    AgentCallRecord {
        agent_id: agent.into(),
        model: model.into(),
        outcome,
        retry_count: 0,
        latency_ms: 200,
        tokens: usage(model),
    }
}

// ── outcome mapping (golden) ────────────────────────────────────────────

#[test]
fn outcome_is_failure_classification() {
    assert!(!AgentOutcome::Ok.is_failure());
    assert!(AgentOutcome::ParseErr.is_failure());
    assert!(AgentOutcome::GroundingErr.is_failure());
    assert!(AgentOutcome::Timeout.is_failure());
    assert!(AgentOutcome::RateLimit.is_failure());
}

#[test]
fn outcome_round_trips_through_call_outcome() {
    // Each telemetry outcome maps to a persisted CallOutcome and back to the
    // same telemetry classification (the roll-up reads it back).
    for (oc, expected_back) in [
        (AgentOutcome::Ok, AgentOutcome::Ok),
        (AgentOutcome::ParseErr, AgentOutcome::ParseErr),
        (AgentOutcome::GroundingErr, AgentOutcome::GroundingErr),
        (AgentOutcome::Timeout, AgentOutcome::Timeout),
        (AgentOutcome::RateLimit, AgentOutcome::RateLimit),
    ] {
        let co = record("a", "m", oc).to_usage_event(0).outcome;
        assert_eq!(AgentOutcome::from_call_outcome(co), expected_back, "{oc:?}");
    }
}

#[test]
fn ok_with_retries_persists_as_retry_but_classifies_ok() {
    let mut r = record("a", "m", AgentOutcome::Ok);
    r.retry_count = 2;
    let ev = r.to_usage_event(0);
    assert!(matches!(ev.outcome, CallOutcome::Retry { attempt: 2 }));
    // Retry that ultimately succeeded is NOT a failure for the rate.
    assert_eq!(AgentOutcome::from_call_outcome(ev.outcome), AgentOutcome::Ok);
}

#[test]
fn schema_invalid_and_network_and_other_classify_into_telemetry_buckets() {
    assert_eq!(
        AgentOutcome::from_call_outcome(CallOutcome::Fail { error_kind: ErrorKind::SchemaInvalid }),
        AgentOutcome::ParseErr
    );
    assert_eq!(
        AgentOutcome::from_call_outcome(CallOutcome::Fail { error_kind: ErrorKind::Network }),
        AgentOutcome::Timeout
    );
    assert_eq!(
        AgentOutcome::from_call_outcome(CallOutcome::Fail { error_kind: ErrorKind::Other }),
        AgentOutcome::ParseErr
    );
}

// ── AgentModelHealth (golden + boundary) ────────────────────────────────

#[test]
fn health_failure_rate_computed_correctly() {
    let h = AgentModelHealth::new("a".into(), "m".into(), 10, 3);
    assert!((h.failure_rate - 0.30).abs() < 1e-9);
    assert!(!h.should_suggest_higher_tier(), "exactly 0.30 is NOT > threshold");
}

#[test]
fn health_above_threshold_suggests_higher_tier() {
    let h = AgentModelHealth::new("a".into(), "m".into(), 10, 4);
    assert!((h.failure_rate - 0.40).abs() < 1e-9);
    assert!(h.should_suggest_higher_tier());
}

#[test]
fn health_zero_calls_no_divide_by_zero() {
    let h = AgentModelHealth::new("a".into(), "m".into(), 0, 0);
    assert_eq!(h.failure_rate, 0.0);
    assert!(!h.should_suggest_higher_tier());
}

#[test]
fn health_all_fail_is_rate_one() {
    let h = AgentModelHealth::new("a".into(), "m".into(), 7, 7);
    assert_eq!(h.failure_rate, 1.0);
    assert!(h.should_suggest_higher_tier());
}

// ── store integration: record + roll-up ─────────────────────────────────

#[test]
fn store_records_agent_call_to_usage_events() {
    let store = Store::open_memory().unwrap();
    store
        .record_agent_call(&record("fact_extractor", "qwen2.5:3b", AgentOutcome::Ok), 1000)
        .unwrap();
    // The row landed in usage_events with the agent tag.
    let conn = store.raw_connection_for_test();
    let (agent, model, outcome): (Option<String>, String, String) = conn
        .query_row(
            "SELECT agent_id, model, outcome FROM usage_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(agent.as_deref(), Some("fact_extractor"));
    assert_eq!(model, "qwen2.5:3b");
    assert_eq!(outcome, "ok");
}

#[test]
fn store_rolls_up_per_agent_model_failure_rate() {
    let store = Store::open_memory().unwrap();
    // defamation_extractor on qwen: 1 ok + 3 parse-fails = 0.75 fail rate.
    store.record_agent_call(&record("defamation_extractor", "qwen2.5:3b", AgentOutcome::Ok), 100).unwrap();
    for ts in [200, 300, 400] {
        store
            .record_agent_call(
                &record("defamation_extractor", "qwen2.5:3b", AgentOutcome::ParseErr),
                ts,
            )
            .unwrap();
    }
    // fact_extractor on qwen: 2 ok = 0.0 fail rate.
    store.record_agent_call(&record("fact_extractor", "qwen2.5:3b", AgentOutcome::Ok), 500).unwrap();
    store.record_agent_call(&record("fact_extractor", "qwen2.5:3b", AgentOutcome::Ok), 600).unwrap();

    let health = store.agent_model_health(0, 10_000).unwrap();
    let def = health
        .iter()
        .find(|h| h.agent_id == "defamation_extractor")
        .expect("defamation row present");
    assert_eq!(def.total_calls, 4);
    assert_eq!(def.failures, 3);
    assert!((def.failure_rate - 0.75).abs() < 1e-9);
    assert!(def.should_suggest_higher_tier(), "0.75 > 0.30 threshold");

    let fact = health.iter().find(|h| h.agent_id == "fact_extractor").unwrap();
    assert_eq!(fact.total_calls, 2);
    assert_eq!(fact.failures, 0);
    assert!(!fact.should_suggest_higher_tier());
}

#[test]
fn store_rollup_splits_same_agent_across_models() {
    // §4.5-F is per (agent × model): the same agent on two models is two rows.
    let store = Store::open_memory().unwrap();
    store.record_agent_call(&record("defamation_extractor", "qwen2.5:3b", AgentOutcome::ParseErr), 100).unwrap();
    store.record_agent_call(&record("defamation_extractor", "qwen2.5:3b", AgentOutcome::ParseErr), 200).unwrap();
    store.record_agent_call(&record("defamation_extractor", "gpt-4o-mini", AgentOutcome::Ok), 300).unwrap();
    store.record_agent_call(&record("defamation_extractor", "gpt-4o-mini", AgentOutcome::Ok), 400).unwrap();

    let health = store.agent_model_health(0, 10_000).unwrap();
    let qwen = health.iter().find(|h| h.model == "qwen2.5:3b").unwrap();
    let gpt = health.iter().find(|h| h.model == "gpt-4o-mini").unwrap();
    assert_eq!(qwen.failure_rate, 1.0, "qwen all-fail");
    assert_eq!(gpt.failure_rate, 0.0, "gpt-4o-mini all-ok");
}

#[test]
fn store_rollup_empty_returns_no_rows() {
    let store = Store::open_memory().unwrap();
    let health = store.agent_model_health(0, 10_000).unwrap();
    assert!(health.is_empty(), "no calls → no health rows");
}

#[test]
fn store_rollup_ignores_non_agent_rows() {
    // Direct-chat usage rows (agent_id = NULL) must not pollute the roll-up.
    let store = Store::open_memory().unwrap();
    let direct = UsageEvent {
        ts_ms: 100,
        kind: UsageKind::LlmChat,
        usage: usage("qwen2.5:3b"),
        cost_usd: None,
        cache: CacheOutcome::Miss,
        outcome: CallOutcome::Fail { error_kind: ErrorKind::Parse },
        latency_ms: 10,
        agent_id: None,
        query_hash: None,
    };
    store.record_usage(&direct).unwrap();
    store.record_agent_call(&record("fact_extractor", "qwen2.5:3b", AgentOutcome::Ok), 200).unwrap();

    let health = store.agent_model_health(0, 10_000).unwrap();
    assert_eq!(health.len(), 1, "only the agent-tagged row counts");
    assert_eq!(health[0].agent_id, "fact_extractor");
}

// ── §7: telemetry write never blocks (best-effort) ──────────────────────

#[test]
fn record_agent_call_best_effort_swallows_error() {
    // record_agent_call_best_effort returns () and never panics even when the
    // underlying insert would fail. We can't easily corrupt an in-memory DB, so
    // we assert the API shape: it accepts a record and returns nothing.
    let store = Store::open_memory().unwrap();
    store.record_agent_call_best_effort(&record("a", "m", AgentOutcome::Ok), 100);
    // A subsequent normal read still works (no poisoning).
    let health = store.agent_model_health(0, 10_000).unwrap();
    assert_eq!(health.len(), 1);
}

// ── proptest: any outcome sequence yields a consistent roll-up ───────────

use proptest::prelude::*;

fn arb_outcome() -> impl Strategy<Value = AgentOutcome> {
    prop_oneof![
        Just(AgentOutcome::Ok),
        Just(AgentOutcome::ParseErr),
        Just(AgentOutcome::GroundingErr),
        Just(AgentOutcome::Timeout),
        Just(AgentOutcome::RateLimit),
    ]
}

proptest! {
    /// For ANY sequence of outcomes for a single (agent, model), the rolled-up
    /// failures == count of non-Ok and failure_rate == failures/total.
    #[test]
    fn rollup_matches_manual_count(outcomes in prop::collection::vec(arb_outcome(), 1..40)) {
        let store = Store::open_memory().unwrap();
        let mut expected_failures = 0u64;
        for (i, oc) in outcomes.iter().enumerate() {
            if oc.is_failure() { expected_failures += 1; }
            store
                .record_agent_call(&record("agent_x", "model_y", *oc), i as i64)
                .unwrap();
        }
        let health = store.agent_model_health(0, 10_000).unwrap();
        prop_assert_eq!(health.len(), 1);
        let h = &health[0];
        prop_assert_eq!(h.total_calls, outcomes.len() as u64);
        prop_assert_eq!(h.failures, expected_failures);
        let expected_rate = expected_failures as f64 / outcomes.len() as f64;
        prop_assert!((h.failure_rate - expected_rate).abs() < 1e-9);
        prop_assert_eq!(h.should_suggest_higher_tier(), expected_rate > FAILURE_RATE_ALERT_THRESHOLD);
    }
}
