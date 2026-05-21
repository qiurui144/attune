//! self_evolving_skill_agent — integration E2E (≥1 per "Agent 验证铁律").
//!
//! Exercises the full production path:
//!   * real tempfile-backed `Store` (not in-memory)
//!   * signals recorded via the public `record_skill_signal` API
//!   * 3-phase API mirroring what `state.rs::start_skill_evolver` would do
//!   * Both heuristic-only and LLM-augmented paths
//!   * Persisted `skill_expansions` rows + `expand_query_with_table` end-to-end
//!
//! Why tempfile vs `open_memory`? On-disk path validates schema migrations,
//! UNIQUE constraint behaviour, and SQLite WAL serialization that an
//! in-memory store can hide.

use attune_core::llm::{ChatMessage, LlmProvider, MockLlmProvider};
use attune_core::skill_evolution::agent::{
    apply_records, expand_query_with_table, generate_records, prepare_run, GeneratedBy,
    SkillAgentConfig,
};
use attune_core::store::{ExpansionSource, Store};

const NOW_SECS: i64 = 1_779_624_000;

fn open_tempfile_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("vault.sqlite")).unwrap();
    (store, dir)
}

/// End-to-end happy path: signals → heuristic cycle → persisted rows →
/// expand_query_with_table observes the row.
#[test]
fn heuristic_path_produces_persisted_expansions() {
    let (store, _dir) = open_tempfile_store();
    // Seed: 3 ML-related search misses crossing min_signal_count=3.
    for q in &[
        ("transformer attention head", 3),
        ("transformer feedforward layer", 3),
        ("transformer positional encoding", 3),
    ] {
        for _ in 0..q.1 {
            store.record_skill_signal(q.0, 0, false).unwrap();
        }
    }

    let cfg = SkillAgentConfig {
        window_days: 0,
        min_signal_count: 3,
        max_signals_per_cycle: 1000,
        enable_llm: false,
    };

    // 3-phase
    let buckets = prepare_run(&store, &cfg, NOW_SECS).unwrap().unwrap();
    assert_eq!(buckets.len(), 3);
    let records = generate_records(&buckets, None, &cfg);
    assert_eq!(records.len(), 3, "all 3 buckets should produce a record");
    let stats = apply_records(&store, &buckets, &records).unwrap();
    assert_eq!(stats.rows_written, 3);
    assert_eq!(stats.used_path, GeneratedBy::Heuristic);

    // Signals consumed.
    assert_eq!(store.count_unprocessed_signals().unwrap(), 0);

    // Persisted shape.
    let row = store
        .get_skill_expansion("transformer attention head")
        .unwrap()
        .unwrap();
    assert_eq!(row.generated_by, ExpansionSource::Heuristic);
    assert!(!row.expansions.is_empty());
    assert!((row.confidence - 0.4).abs() < 1e-3);

    // expand_query_with_table picks up the row.
    let legacy = serde_json::json!({});
    let expanded =
        expand_query_with_table(&store, "transformer attention head", &legacy);
    assert!(
        expanded != "transformer attention head",
        "expand_query_with_table must append expansions (got: {expanded:?})"
    );
}

/// LLM path replaces the heuristic row with an LLM row.
#[test]
fn llm_path_supersedes_heuristic_via_upsert() {
    let (store, _dir) = open_tempfile_store();
    for _ in 0..3 {
        store.record_skill_signal("kafka consumer group", 0, false).unwrap();
        store.record_skill_signal("kafka partition rebalance", 0, false).unwrap();
    }

    // First cycle: heuristic only.
    let cfg = SkillAgentConfig {
        window_days: 0,
        min_signal_count: 3,
        max_signals_per_cycle: 1000,
        enable_llm: false,
    };
    let buckets = prepare_run(&store, &cfg, NOW_SECS).unwrap().unwrap();
    let records = generate_records(&buckets, None, &cfg);
    apply_records(&store, &buckets, &records).unwrap();
    let row = store.get_skill_expansion("kafka consumer group").unwrap().unwrap();
    assert_eq!(row.generated_by, ExpansionSource::Heuristic);

    // Second cycle: enable LLM path. New signals seed re-trigger.
    for _ in 0..3 {
        store.record_skill_signal("kafka consumer group", 0, false).unwrap();
        store.record_skill_signal("kafka partition rebalance", 0, false).unwrap();
    }
    let llm = MockLlmProvider::new("test");
    llm.push_response(r#"{"terms": ["offset commit", "consumer lag", "rebalance protocol"]}"#);
    llm.push_response(r#"{"terms": ["assignment strategy", "leader election"]}"#);

    let cfg_llm = SkillAgentConfig {
        enable_llm: true,
        ..cfg
    };
    let buckets = prepare_run(&store, &cfg_llm, NOW_SECS).unwrap().unwrap();
    let llm_ref: &dyn LlmProvider = &llm;
    let records = generate_records(&buckets, Some(llm_ref), &cfg_llm);
    let stats = apply_records(&store, &buckets, &records).unwrap();
    assert_eq!(stats.used_path, GeneratedBy::Llm);
    assert!(stats.rows_written >= 1);

    let row = store.get_skill_expansion("kafka consumer group").unwrap().unwrap();
    assert_eq!(row.generated_by, ExpansionSource::Llm);
    assert!(row.expansions.iter().any(|t| t == "offset commit"
        || t == "consumer lag"
        || t == "rebalance protocol"));
    assert!((row.confidence - 0.7).abs() < 1e-3);
}

/// LLM failure falls back to heuristic without losing the bucket.
#[test]
fn llm_failure_falls_back_to_heuristic() {
    struct FailingLlm;
    impl LlmProvider for FailingLlm {
        fn chat(&self, _s: &str, _u: &str) -> attune_core::error::Result<String> {
            Err(attune_core::error::VaultError::LlmUnavailable(
                "induced failure".into(),
            ))
        }
        fn chat_with_history(&self, _m: &[ChatMessage]) -> attune_core::error::Result<String> {
            self.chat("", "")
        }
        fn is_available(&self) -> bool {
            true
        }
        fn model_name(&self) -> &str {
            "failing"
        }
    }

    let (store, _dir) = open_tempfile_store();
    for _ in 0..3 {
        store.record_skill_signal("graphql resolver", 0, false).unwrap();
        store.record_skill_signal("graphql subscription", 0, false).unwrap();
    }

    let cfg = SkillAgentConfig {
        window_days: 0,
        min_signal_count: 3,
        max_signals_per_cycle: 1000,
        enable_llm: true,
    };
    let buckets = prepare_run(&store, &cfg, NOW_SECS).unwrap().unwrap();
    let failing: &dyn LlmProvider = &FailingLlm;
    let records = generate_records(&buckets, Some(failing), &cfg);
    // LLM failed but heuristic still produced records.
    assert!(!records.is_empty(), "agent must fall back to heuristic on LLM error");
    for r in &records {
        assert_eq!(r.generated_by, GeneratedBy::Heuristic);
    }
    let stats = apply_records(&store, &buckets, &records).unwrap();
    assert_eq!(stats.used_path, GeneratedBy::Heuristic);
    assert!(stats.rows_written >= 1);
}

/// Empty signals: idle cycle, no panic, no spurious rows.
#[test]
fn empty_signals_idle_cycle() {
    let (store, _dir) = open_tempfile_store();
    let cfg = SkillAgentConfig::default();
    let buckets = prepare_run(&store, &cfg, NOW_SECS).unwrap();
    assert!(buckets.is_none());
    let stats = attune_core::skill_evolution::agent::run_cycle(&store, None, &cfg, NOW_SECS)
        .unwrap();
    assert_eq!(stats.rows_written, 0);
    assert_eq!(store.count_skill_expansions().unwrap(), 0);
}

/// Idempotency E2E — cycle on the same signal set twice writes the same row,
/// not a duplicate, and never blocks on UNIQUE constraint.
#[test]
fn idempotent_when_replayed_after_persisted() {
    let (store, _dir) = open_tempfile_store();
    for _ in 0..3 {
        store.record_skill_signal("postgres index btree", 0, false).unwrap();
        store.record_skill_signal("postgres index hash", 0, false).unwrap();
    }
    let cfg = SkillAgentConfig {
        window_days: 0,
        min_signal_count: 3,
        max_signals_per_cycle: 1000,
        enable_llm: false,
    };
    let stats1 = attune_core::skill_evolution::agent::run_cycle(&store, None, &cfg, NOW_SECS)
        .unwrap();
    assert!(stats1.rows_written >= 1);

    // Re-seed identical signals and run again. Because the new signals are
    // "fresh" (unprocessed), the cycle will run, but because the upsert
    // truncates/replaces rather than inserting duplicates, the rows count
    // stays bounded.
    for _ in 0..3 {
        store.record_skill_signal("postgres index btree", 0, false).unwrap();
        store.record_skill_signal("postgres index hash", 0, false).unwrap();
    }
    let _stats2 = attune_core::skill_evolution::agent::run_cycle(&store, None, &cfg, NOW_SECS)
        .unwrap();
    assert!(store.count_skill_expansions().unwrap() <= 2,
        "rows_count must stay bounded across multiple cycles");
}

/// SCHEMA_SQL migration creates the table on a fresh vault. (Guards against
/// regression where someone deletes the CREATE TABLE row.)
#[test]
fn schema_creates_skill_expansions_table() {
    let (store, _dir) = open_tempfile_store();
    // Initial count must be 0 — table exists.
    assert_eq!(store.count_skill_expansions().unwrap(), 0);
    // Round-trip a row through the public API.
    store
        .upsert_skill_expansion(
            "smoke",
            &["a".into(), "b".into()],
            ExpansionSource::Heuristic,
            0.4,
        )
        .unwrap();
    assert_eq!(store.count_skill_expansions().unwrap(), 1);
}
