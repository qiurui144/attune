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

/// Per CLAUDE.md「Agent 验证铁律」§2 (6 类测试覆盖下限) — structural gate.
///
/// Equivalent of attune-pro's `agent_golden_gate.rs::six_category_floor_check`
/// (Phase 2) and attune-server's `office_six_category_floor.rs` (D5.5), adapted
/// to a single deterministic agent (no multi-scene fan-out needed).
///
/// | Category       | Source                                                | Floor |
/// |----------------|-------------------------------------------------------|-------|
/// | Golden case    | tests/golden/memory_promotion/*.yaml (approved)       | ≥ 10 real + 1 sentinel |
/// | Error case     | tests/golden/memory_promotion/error/*.yaml (approved) | ≥ 3   |
/// | Proptest       | tests/memory_consolidation_agent_proptests.rs         | ≥ 3   |
/// | Boundary       | src/memory/consolidation_agent.rs `#[cfg(test)]`      | ≥ 5   |
/// | Integration    | tests/memory_consolidation_agent_integration.rs       | ≥ 1   |
/// | Regression     | 11-sentinel-*.yaml + case 09 tiebreak-fix             | ≥ 1   |
///
/// **ENFORCE mode**: `ATTUNE_ENFORCE_MEMORY_FLOOR=1` → panic on any miss.
/// Default off, but emits warnings + still asserts on the hard golden counts so
/// the gate cannot silently regress.
#[test]
fn memory_consolidation_six_class_floor() {
    let enforce = std::env::var("ATTUNE_ENFORCE_MEMORY_FLOOR").is_ok();
    let mut violations: Vec<String> = Vec::new();

    // 1 + 6: Golden + sentinel (sentinel is one of the 11 main YAMLs by convention,
    // file 11-sentinel-*.yaml).
    let main_cases = count_yaml(&golden_root());
    let has_sentinel = std::fs::read_dir(golden_root())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .contains("sentinel")
        });
    if main_cases < 11 {
        violations.push(format!(
            "Golden floor: ≥ 10 real + 1 sentinel = 11; got {}",
            main_cases
        ));
    }
    if !has_sentinel {
        violations.push("Sentinel YAML (file named *sentinel*) missing".into());
    }

    // 2: Error
    let error_cases = count_yaml(&golden_root().join("error"));
    if error_cases < 3 {
        violations.push(format!("Error floor: ≥ 3; got {}", error_cases));
    }

    // 3: Proptest — count `proptest! { ... fn prop_*(...) }` arms.
    let prop_count = count_proptest_arms(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/memory_consolidation_agent_proptests.rs"),
    );
    if prop_count < 3 {
        violations.push(format!("Proptest floor: ≥ 3; got {}", prop_count));
    }

    // 4: Boundary — `#[test] fn boundary_*` count in src/memory/consolidation_agent.rs.
    let boundary_count = count_test_arms(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/memory/consolidation_agent.rs"),
        "boundary_",
    );
    if boundary_count < 5 {
        violations.push(format!("Boundary floor: ≥ 5; got {}", boundary_count));
    }

    // 5: Integration
    let integ_count = count_test_arms(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/memory_consolidation_agent_integration.rs"),
        "",
    );
    if integ_count < 1 {
        violations.push(format!("Integration floor: ≥ 1; got {}", integ_count));
    }

    eprintln!(
        "memory_consolidation six-class floor:\n  main golden = {}\n  sentinel    = {}\n  error       = {}\n  proptest    = {}\n  boundary    = {}\n  integration = {}",
        main_cases, has_sentinel, error_cases, prop_count, boundary_count, integ_count
    );

    if !violations.is_empty() {
        let msg = format!(
            "Six-class floor violations ({} total):\n  - {}",
            violations.len(),
            violations.join("\n  - ")
        );
        if enforce {
            panic!("{msg}\n(set ATTUNE_ENFORCE_MEMORY_FLOOR=1 was on → CI block)");
        } else {
            // Even without ENFORCE we must hold the hard golden+error counts —
            // those are inviolable contract per "Agent 验证铁律". Tiebreak fix
            // path counts as ≥1 regression in main golden via case 09.
            assert!(
                main_cases >= 11 && error_cases >= 3,
                "Hard contract: ≥11 main + ≥3 error always required; {msg}"
            );
            eprintln!("WARN: {msg}");
        }
    } else {
        eprintln!("memory_consolidation six-class floor: 0 violations ✓");
    }
}

// ── Static counters (intentionally don't call agent.run()) ──────────────────

fn count_yaml(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x == "yaml")
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn count_proptest_arms(path: &Path) -> usize {
    let src = std::fs::read_to_string(path).unwrap_or_default();
    src.lines().filter(|l| l.trim_start().starts_with("fn prop_")).count()
}

fn count_test_arms(path: &Path, prefix: &str) -> usize {
    let src = std::fs::read_to_string(path).unwrap_or_default();
    let mut count = 0;
    let mut prev_was_test_attr = false;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed == "#[test]" {
            prev_was_test_attr = true;
            continue;
        }
        if prev_was_test_attr {
            if trimmed.starts_with("fn ") {
                let name_start = trimmed.trim_start_matches("fn ").trim_start();
                if prefix.is_empty() || name_start.starts_with(prefix) {
                    count += 1;
                }
            }
            prev_was_test_attr = false;
        }
    }
    count
}
