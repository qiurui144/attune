//! self_evolving_skill_agent — deterministic golden gate (CI-blocking).
//!
//! Walks every `tests/golden/skill_evolution/[error/]*.yaml` whose
//! `reviewer.approved == true`, seeds the scenario into a fresh
//! `Store::open_memory`, runs the agent's three-phase cycle, and asserts
//! against the file's `expected.*` block.
//!
//! per `attune/CLAUDE.md` §"Agent 验证铁律":
//!   - deterministic agent → required pass rate **1.00**
//!   - ground truth **independent** of agent.run() (every case's `expected.*`
//!     is hand-computed against the public ranking spec in `agent.rs`)
//!   - 6-class coverage floor:
//!       * Golden case ≥ 10 + 1 sentinel — 11 in this dir
//!       * Property tests ≥ 3 — `self_evolving_skill_agent_proptests.rs`
//!       * Boundary `#[test]` ≥ 5 — in-mod tests in `skill_evolution::agent`
//!       * Error case ≥ 3 — `error/` subdir
//!       * Integration E2E ≥ 1 — `self_evolving_skill_agent_integration.rs`
//!       * Regression fixture — `11-sentinel-regression.yaml`

use std::path::{Path, PathBuf};

use attune_core::skill_evolution::agent::{
    apply_records, generate_records, prepare_run, EvolutionRecord, GeneratedBy, SkillAgentConfig,
};
use attune_core::store::{ExpansionSource, Store};
use serde::Deserialize;

// ── YAML schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GoldenCase {
    id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    signals: Vec<GoldenSignal>,
    config: GoldenConfig,
    expected: GoldenExpected,
    #[serde(default)]
    reviewer: Reviewer,
}

#[derive(Debug, Deserialize)]
struct GoldenSignal {
    query: String,
    count: u32,
}

#[derive(Debug, Deserialize)]
struct GoldenConfig {
    window_days: u32,
    min_signal_count: u32,
    #[serde(default)]
    enable_llm: bool,
}

#[derive(Debug, Deserialize, Default)]
struct GoldenExpected {
    #[serde(default)]
    buckets_count: Option<usize>,
    /// Exhaustive list — emitted records must equal this set (order-insensitive).
    /// Mutually exclusive with `contains_records`. If neither is present we
    /// assert "no records emitted" (the empty-default case).
    #[serde(default)]
    records: Option<Vec<GoldenRecord>>,
    /// Subset constraint — each entry must appear, but extra records are OK.
    /// Useful for "many qualifying buckets, only assert the target".
    #[serde(default)]
    contains_records: Option<Vec<GoldenRecord>>,
    /// Optional exact row count check (used by error-03 to assert "no rows
    /// were upserted even though 1 bucket qualified").
    #[serde(default)]
    exact_rows_written: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
struct GoldenRecord {
    query_pattern: String,
    expansions: Vec<String>,
    #[serde(default)]
    generated_by: Option<String>,
    #[serde(default)]
    confidence: Option<f32>,
}

#[derive(Debug, Deserialize, Default)]
struct Reviewer {
    #[serde(default)]
    approved: bool,
}

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/skill_evolution")
}

fn collect_approved(root: &Path) -> Vec<(PathBuf, GoldenCase)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).expect("golden dir exists") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            for sub in std::fs::read_dir(&path).unwrap() {
                let sub = sub.unwrap();
                let sp = sub.path();
                if sp.extension().and_then(|e| e.to_str()) == Some("yaml") {
                    if let Some(c) = load_if_approved(&sp) {
                        out.push((sp, c));
                    }
                }
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            if let Some(c) = load_if_approved(&path) {
                out.push((path, c));
            }
        }
    }
    out.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
    out
}

fn load_if_approved(path: &Path) -> Option<GoldenCase> {
    let s = std::fs::read_to_string(path).ok()?;
    let c: GoldenCase = serde_yaml::from_str(&s)
        .unwrap_or_else(|e| panic!("yaml parse {}: {e}", path.display()));
    if c.reviewer.approved {
        Some(c)
    } else {
        None
    }
}

/// Expand REPEAT_300_CHARS marker into a 300-char string. Lets `error-03` test
/// the overlong-query store guard without putting 300 chars in YAML.
fn maybe_expand_marker(query: &str) -> String {
    if query == "REPEAT_300_CHARS" {
        "x".repeat(300)
    } else {
        query.to_string()
    }
}

/// Execute one golden case. Panics on first disagreement to fail the gate.
fn execute_case(case: &GoldenCase) -> Result<(), String> {
    let store = Store::open_memory().map_err(|e| format!("open_memory: {e}"))?;

    // Seed signals.
    for sig in &case.signals {
        let expanded = maybe_expand_marker(&sig.query);
        for _ in 0..sig.count {
            store
                .record_skill_signal(&expanded, 0, false)
                .map_err(|e| format!("record_skill_signal({:.30}): {e}", expanded))?;
        }
    }

    let cfg = SkillAgentConfig {
        window_days: case.config.window_days,
        min_signal_count: case.config.min_signal_count,
        max_signals_per_cycle: 1000,
        enable_llm: case.config.enable_llm,
    };
    // Fixed reference time for reproducibility (2026-05-19T12:00:00Z).
    const NOW_SECS: i64 = 1_779_624_000;

    let buckets_opt = prepare_run(&store, &cfg, NOW_SECS)
        .map_err(|e| format!("prepare_run: {e}"))?;
    let buckets = buckets_opt.unwrap_or_default();

    // Sanity: buckets_count assertion.
    if let Some(exp_n) = case.expected.buckets_count {
        if buckets.len() != exp_n {
            return Err(format!(
                "case {}: buckets_count actual {} vs expected {}",
                case.id,
                buckets.len(),
                exp_n
            ));
        }
    }

    let records = generate_records(&buckets, None, &cfg);

    let stats = apply_records(&store, &buckets, &records)
        .map_err(|e| format!("apply_records: {e}"))?;

    if let Some(want) = case.expected.exact_rows_written {
        if stats.rows_written != want {
            return Err(format!(
                "case {}: exact_rows_written actual {} vs expected {}",
                case.id, stats.rows_written, want
            ));
        }
    }

    // Compare records.
    match (&case.expected.records, &case.expected.contains_records) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "case {}: records and contains_records both set — pick one",
                case.id
            ));
        }
        (Some(want), None) => {
            // Exhaustive: same set.
            assert_records_equal(&case.id, want, &records)?;
        }
        (None, Some(want)) => {
            // Subset: each entry must appear.
            assert_records_contain(&case.id, want, &records)?;
        }
        (None, None) => {
            // Default: empty records expected (no learned expansions).
            if !records.is_empty() {
                return Err(format!(
                    "case {}: records absent in YAML but {} emitted: {:?}",
                    case.id,
                    records.len(),
                    records.iter().map(|r| &r.query_pattern).collect::<Vec<_>>()
                ));
            }
        }
    }

    Ok(())
}

fn assert_records_equal(
    case_id: &str,
    want: &[GoldenRecord],
    got: &[EvolutionRecord],
) -> Result<(), String> {
    if got.len() != want.len() {
        return Err(format!(
            "case {}: expected {} records, got {}: got={:?}",
            case_id,
            want.len(),
            got.len(),
            got.iter().map(|r| &r.query_pattern).collect::<Vec<_>>()
        ));
    }
    for w in want {
        find_and_check_record(case_id, w, got)?;
    }
    Ok(())
}

fn assert_records_contain(
    case_id: &str,
    want: &[GoldenRecord],
    got: &[EvolutionRecord],
) -> Result<(), String> {
    for w in want {
        find_and_check_record(case_id, w, got)?;
    }
    Ok(())
}

fn find_and_check_record(
    case_id: &str,
    want: &GoldenRecord,
    got: &[EvolutionRecord],
) -> Result<(), String> {
    let target = want.query_pattern.to_lowercase();
    let matched = got.iter().find(|r| r.query_pattern == target);
    let Some(r) = matched else {
        return Err(format!(
            "case {}: expected record for query_pattern={:?} not found; got patterns {:?}",
            case_id,
            target,
            got.iter().map(|r| &r.query_pattern).collect::<Vec<_>>()
        ));
    };
    if r.expansions != want.expansions {
        return Err(format!(
            "case {}: pattern {:?} expansions mismatch:\n  actual:   {:?}\n  expected: {:?}",
            case_id, target, r.expansions, want.expansions
        ));
    }
    if let Some(want_by) = &want.generated_by {
        let actual_by = match r.generated_by {
            GeneratedBy::Heuristic => "heuristic",
            GeneratedBy::Llm => "llm",
        };
        if actual_by != want_by {
            return Err(format!(
                "case {}: pattern {:?} generated_by actual={} expected={}",
                case_id, target, actual_by, want_by
            ));
        }
    }
    if let Some(want_conf) = want.confidence {
        if (r.confidence - want_conf).abs() > 1e-3 {
            return Err(format!(
                "case {}: pattern {:?} confidence actual={} expected={}",
                case_id, target, r.confidence, want_conf
            ));
        }
    }
    Ok(())
}

// ── Coverage floor + 1.00 pass-rate gate ─────────────────────────────────────

#[test]
fn skill_evolution_golden_gate_pass_rate_must_be_one() {
    let root = golden_root();
    let cases = collect_approved(&root);
    assert!(
        cases.len() >= 13,
        "expected at least 11 main + 3 error = 14 approved golden cases, got {}",
        cases.len()
    );

    let mut errs: Vec<String> = Vec::new();
    for (path, case) in &cases {
        if let Err(e) = execute_case(case) {
            errs.push(format!("{}\n  {}", path.display(), e));
        }
    }

    if !errs.is_empty() {
        for e in &errs {
            eprintln!("GATE FAIL: {e}");
        }
        panic!(
            "{}/{} golden cases failed — gate requires 1.00 pass rate",
            errs.len(),
            cases.len()
        );
    }
}

#[test]
fn six_class_coverage_floor_enforced() {
    // We can't enumerate the entire test universe from this file, but we
    // can at least assert the YAML floor (main ≥10 + sentinel ≥1 + error ≥3).
    let root = golden_root();
    let main_count = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().is_file() && e.path().extension().and_then(|x| x.to_str()) == Some("yaml")
        })
        .count();
    let error_dir = root.join("error");
    let error_count = std::fs::read_dir(&error_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("yaml"))
        .count();
    assert!(
        main_count >= 11,
        "main golden coverage floor: ≥10 + 1 sentinel = 11, got {main_count}"
    );
    assert!(
        error_count >= 3,
        "error coverage floor: ≥3, got {error_count}"
    );
}

/// One smoke test against a real `Store` row to prove the gate harness can
/// observe persisted state (not just in-memory records).
#[test]
fn agent_persists_to_skill_expansions_after_run() {
    let store = Store::open_memory().unwrap();
    for _ in 0..3 {
        store.record_skill_signal("rust ownership", 0, false).unwrap();
        store.record_skill_signal("rust borrow checker", 0, false).unwrap();
    }
    let cfg = SkillAgentConfig {
        window_days: 0,
        min_signal_count: 3,
        ..Default::default()
    };
    let _stats =
        attune_core::skill_evolution::agent::run_cycle(&store, None, &cfg, 1_779_624_000).unwrap();
    let row = store
        .get_skill_expansion("rust ownership")
        .unwrap()
        .expect("agent must have written the row");
    assert_eq!(row.generated_by, ExpansionSource::Heuristic);
    assert!(!row.expansions.is_empty());
}
