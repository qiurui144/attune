//! memory_consolidation_agent — deterministic golden gate (CI-blocking).
//!
//! Walks every `tests/golden/memory_promotion/[error/]*.yaml` whose
//! `reviewer.approved == true`, seeds the scenario into a fresh `Store::open_memory`,
//! runs `run_promotion_cycle`, and asserts against the file's `expected.*` block.
//!
//! per `attune/CLAUDE.md` §"Agent 验证铁律":
//!   - deterministic agent → required pass rate **1.00**.
//!   - ground truth **independent** of agent.run() (every case's `expected.*`
//!     is hand-computed from the score formula, see file headers).
//!   - 6-class coverage floor:
//!       * Golden case ≥ 10 + 1 sentinel ............... 11 in this dir
//!       * Property tests ≥ 3 .......................... see `prop_tests` mod
//!       * Boundary `#[test]` ≥ 5 ...................... lib unit tests
//!       * Error case ≥ 3 .............................. `error/` subdir
//!       * Integration E2E ≥ 1 ......................... see separate
//!                                                       `memory_consolidation_agent_integration.rs`
//!       * Regression fixture .......................... 11-sentinel-*.yaml

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use attune_core::crypto::Key32;
use attune_core::memory::consolidation_agent::{run_promotion_cycle, PromotionConfig};
use attune_core::store::Store;
use serde::Deserialize;

// ── YAML schema (mirrors the golden file shape) ─────────────────────────────

#[derive(Debug, Deserialize)]
struct GoldenCase {
    id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    episodic_memories: Vec<GoldenEpisodic>,
    #[serde(default)]
    citation_hits: Vec<GoldenCitation>,
    config: GoldenConfig,
    expected: GoldenExpected,
    #[serde(default)]
    pre_promote: bool,
    #[serde(default)]
    reviewer: Reviewer,
}

#[derive(Debug, Deserialize)]
struct GoldenEpisodic {
    id: String,
    chunk_hashes: Vec<String>,
    summary: String,
    created_at_offset_days: i64,
}

#[derive(Debug, Deserialize)]
struct GoldenCitation {
    chunk: String,
    count: u32,
}

#[derive(Debug, Deserialize)]
struct GoldenConfig {
    promotion_window_days: u32,
    min_access_count: u32,
    min_score: f64,
    max_promotions_per_run: usize,
}

#[derive(Debug, Deserialize, Default)]
struct GoldenExpected {
    #[serde(default)]
    promoted_ids: Vec<String>,
    gated_by_access: usize,
    gated_by_score: usize,
    #[serde(default)]
    already_promoted: usize,
    #[serde(default)]
    considered: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct Reviewer {
    #[serde(default)]
    approved: bool,
}

// Fixed reference time for all golden cases. 2026-04-01T00:00:00Z = 1764547200.
// All `created_at_offset_days` are measured relative to this. Using a fixed
// reference (not chrono::Utc::now) keeps tests reproducible across CI runs.
const NOW_SECS: i64 = 1_764_547_200;

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/memory_promotion")
}

/// Collect every approved YAML in `root` recursively (1 level — `error/` subdir).
fn collect_approved(root: &Path) -> Vec<(PathBuf, GoldenCase)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).expect("golden dir exists") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            // recurse once (error/)
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
    // Sort by file name for stable harness output.
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

/// Apply one golden case to a fresh `Store::open_memory`, then run the agent
/// and compare its output to `expected.*`. Panics on any disagreement so a
/// single failing case fails the whole gate.
fn execute_case(case: &GoldenCase) -> Result<(), String> {
    let store = Store::open_memory().map_err(|e| format!("open_memory: {e}"))?;
    let dek = Key32::generate();

    // Seed episodic memories at (NOW_SECS - offset*86400).
    for ep in &case.episodic_memories {
        let created_at = NOW_SECS - ep.created_at_offset_days * 86_400;
        // We can't directly assign episodic_id, so use insert_memory and then look
        // up the row by chunk_hashes to bind ep.id ↔ inserted uuid.
        store
            .insert_memory(
                &dek,
                "episodic",
                created_at,
                created_at + 86_400,
                &ep.chunk_hashes,
                &ep.summary,
                "golden-seed",
                created_at,
            )
            .map_err(|e| format!("insert_memory({}): {e}", ep.id))?;
    }

    // Seed citation_hit signals.
    for cit in &case.citation_hits {
        for _ in 0..cit.count {
            store
                .record_signal_event("citation_hit", &cit.chunk, None)
                .map_err(|e| format!("record_signal_event: {e}"))?;
        }
    }

    let cfg = PromotionConfig {
        promotion_window_days: case.config.promotion_window_days,
        min_access_count: case.config.min_access_count,
        min_score: case.config.min_score,
        max_promotions_per_run: case.config.max_promotions_per_run,
    };

    // Pre-promote scenario: run the agent once first, then assert on the second.
    if case.pre_promote {
        let _ = run_promotion_cycle(&store, &dek, &cfg, NOW_SECS, "golden-pre")
            .map_err(|e| format!("pre_promote cycle: {e}"))?;
    }

    let result = run_promotion_cycle(&store, &dek, &cfg, NOW_SECS, "golden-run")
        .map_err(|e| format!("run_promotion_cycle: {e}"))?;

    // Map: the inserted memory uuid → the golden-case episodic id, via chunk-hash join.
    // We look up live episodic memories and match by sorted chunk_hashes.
    let live_episodic = store
        .list_live_memories(&dek, "episodic", false)
        .map_err(|e| format!("list_live_memories: {e}"))?;
    let id_to_golden: std::collections::HashMap<String, String> = live_episodic
        .iter()
        .filter_map(|m| {
            let mut my = m.source_chunk_hashes.clone();
            my.sort();
            case.episodic_memories
                .iter()
                .find(|g| {
                    let mut gh = g.chunk_hashes.clone();
                    gh.sort();
                    gh == my
                })
                .map(|g| (m.id.clone(), g.id.clone()))
        })
        .collect();

    // Newly-promoted are records with `semantic_id.is_some()`.
    let promoted_golden_ids: BTreeSet<String> = result
        .promoted
        .iter()
        .filter(|r| r.semantic_id.is_some())
        .filter_map(|r| id_to_golden.get(&r.episodic_id).cloned())
        .collect();
    let expected_golden_ids: BTreeSet<String> =
        case.expected.promoted_ids.iter().cloned().collect();

    if promoted_golden_ids != expected_golden_ids {
        return Err(format!(
            "case {}: promoted_ids mismatch: actual {:?} vs expected {:?}",
            case.id, promoted_golden_ids, expected_golden_ids
        ));
    }
    if result.gated_by_access != case.expected.gated_by_access {
        return Err(format!(
            "case {}: gated_by_access actual {} vs expected {}",
            case.id, result.gated_by_access, case.expected.gated_by_access
        ));
    }
    if result.gated_by_score != case.expected.gated_by_score {
        return Err(format!(
            "case {}: gated_by_score actual {} vs expected {}",
            case.id, result.gated_by_score, case.expected.gated_by_score
        ));
    }
    if result.already_promoted != case.expected.already_promoted {
        return Err(format!(
            "case {}: already_promoted actual {} vs expected {}",
            case.id, result.already_promoted, case.expected.already_promoted
        ));
    }
    if let Some(exp_considered) = case.expected.considered {
        if result.considered != exp_considered {
            return Err(format!(
                "case {}: considered actual {} vs expected {}",
                case.id, result.considered, exp_considered
            ));
        }
    }

    Ok(())
}

#[test]
fn memory_promotion_golden_gate_pass_rate_must_be_one() {
    let root = golden_root();
    let cases = collect_approved(&root);
    assert!(
        cases.len() >= 14,
        "expected at least 11 main + 3 error = 14 approved golden cases, got {}",
        cases.len()
    );

    let mut failures: Vec<String> = Vec::new();
    let mut report = Vec::new();
    for (path, case) in &cases {
        report.push(format!("[{}] {}", path.file_name().unwrap().to_string_lossy(), case.id));
        match execute_case(case) {
            Ok(()) => {}
            Err(e) => failures.push(format!("FAIL {}: {e}", case.id)),
        }
    }
    eprintln!(
        "memory_consolidation_agent_golden_gate: ran {} cases\n{}",
        cases.len(),
        report.join("\n")
    );
    assert!(
        failures.is_empty(),
        "deterministic golden gate requires 1.00 pass rate (per \"Agent 验证铁律\"); failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn golden_set_meets_six_class_floor() {
    // Structural assertion — never depend on agent.run() for this.
    let root = golden_root();
    let main_cases: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x == "yaml")
                .unwrap_or(false)
        })
        .collect();
    let error_cases: Vec<_> = std::fs::read_dir(root.join("error"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x == "yaml")
                .unwrap_or(false)
        })
        .collect();
    assert!(
        main_cases.len() >= 11,
        "Golden floor: ≥ 10 real + 1 sentinel; got {}",
        main_cases.len()
    );
    assert!(
        error_cases.len() >= 3,
        "Error case floor: ≥ 3; got {}",
        error_cases.len()
    );
}
