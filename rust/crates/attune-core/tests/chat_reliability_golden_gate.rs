//! chat_reliability — deterministic golden gate (CI-blocking).
//!
//! Walks every `tests/corpora/chat_reliability_golden/[error/]*.yaml` whose
//! `reviewer.approved == true`, runs [`evaluate_response`], and asserts
//! against the file's `expected.*` block.
//!
//! Per `attune-pro/docs/agent-skill-training-methodology.md` §5:
//!   - deterministic agent → required pass rate **1.00**.
//!   - ground truth **independent** of `evaluate_response` output (every
//!     fixture's `expected.*` is hand-computed from the
//!     `confidence_from_signals` formula — see `# DERIVATION:` block in
//!     each YAML).
//!   - 6-class coverage floor:
//!       * Golden case ≥ 10 ........... `01..=10-*.yaml`
//!       * Error case  ≥  3 ........... `error/error-*.yaml`
//!       * Boundary `#[test]` ≥ 5 ..... lib unit tests (`chat_reliability::agent::tests`)
//!       * Property tests ≥ 3 ......... `chat_reliability_proptests.rs`
//!       * Integration E2E ≥ 1 ........ `chat_reliability_integration.rs`
//!       * ENFORCE 0 violations ....... `golden_gate_enforce_mode()` test below

use std::path::{Path, PathBuf};

use attune_core::chat_reliability::{
    evaluate_response, ChatReliabilityConfig, CitationStatus, HallucinationKind, RetrievedChunk,
};
use serde::Deserialize;

// ── YAML schema (mirrors the golden file shape) ─────────────────────────────

#[derive(Debug, Deserialize)]
struct GoldenCase {
    id: String,
    #[allow(dead_code)] // present in YAML for documentation; not asserted directly
    #[serde(default)]
    description: String,
    #[serde(default)]
    query: String,
    response: String,
    #[serde(default)]
    chunks: Vec<GoldenChunk>,
    expected: GoldenExpected,
    reviewer: Reviewer,
}

#[derive(Debug, Deserialize)]
struct GoldenChunk {
    item_id: String,
    chunk_text: String,
}

#[derive(Debug, Deserialize)]
struct GoldenExpected {
    #[serde(default)]
    citation_grounded: Vec<GoldenCitation>,
    #[serde(default)]
    contradictions_count: usize,
    #[serde(default)]
    hallucination_kinds: Vec<String>,
    confidence_range: [f32; 2],
}

#[derive(Debug, Deserialize)]
struct GoldenCitation {
    item_id: String,
    status: String,
}

#[derive(Debug, Deserialize, Default)]
struct Reviewer {
    #[serde(default)]
    approved: bool,
    #[serde(default)]
    #[allow(dead_code)] // recorded for provenance, not asserted
    name: String,
}

// ── Loader ──────────────────────────────────────────────────────────────────

fn corpus_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/corpora/chat_reliability_golden");
    p
}

fn load_yaml(path: &Path) -> Option<GoldenCase> {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read fixture {:?}: {e}", path));
    let case: GoldenCase = serde_yaml::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse fixture {:?}: {e}", path));
    if !case.reviewer.approved {
        return None;
    }
    Some(case)
}

fn collect_approved(root: &Path) -> Vec<(PathBuf, GoldenCase)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).expect("golden dir exists") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            // Recurse once (only `error/` subdir).
            for sub in std::fs::read_dir(&path).expect("error subdir readable") {
                let sub = sub.unwrap();
                let sp = sub.path();
                if sp.extension().and_then(|e| e.to_str()) == Some("yaml") {
                    if let Some(c) = load_yaml(&sp) {
                        out.push((sp, c));
                    }
                }
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            if let Some(c) = load_yaml(&path) {
                out.push((path, c));
            }
        }
    }
    // Stable harness order.
    out.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
    out
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn citation_status_from_str(s: &str) -> CitationStatus {
    match s {
        "grounded" => CitationStatus::Grounded,
        "weak_overlap" => CitationStatus::WeakOverlap,
        "fabricated" => CitationStatus::Fabricated,
        other => panic!("unknown citation status in fixture: {other}"),
    }
}

fn hallucination_kind_to_str(k: HallucinationKind) -> &'static str {
    match k {
        HallucinationKind::Date => "date",
        HallucinationKind::Number => "number",
        HallucinationKind::Organization => "organization",
        HallucinationKind::Person => "person",
    }
}

fn chunk_from_golden(g: &GoldenChunk) -> RetrievedChunk {
    RetrievedChunk::new(g.item_id.clone(), g.chunk_text.clone())
}

// ── Core gate ────────────────────────────────────────────────────────────────

/// Required class floor — golden ≥ 10 + error ≥ 3.
#[test]
fn golden_gate_minimum_fixture_counts() {
    let top = collect_approved(&corpus_dir())
        .into_iter()
        .filter(|(p, _)| p.parent().map(|d| d == corpus_dir()).unwrap_or(false))
        .count();
    let errors = collect_approved(&corpus_dir())
        .into_iter()
        .filter(|(p, _)| p.parent().map(|d| d != corpus_dir()).unwrap_or(true))
        .count();
    assert!(
        top >= 10,
        "must have ≥10 approved top-level golden cases (have {top})",
    );
    assert!(
        errors >= 3,
        "must have ≥3 approved error cases under error/ (have {errors})",
    );
}

/// Run every approved fixture and assert all four expected.* facets match.
/// ENFORCE mode — zero violations tolerated, any mismatch panics with full
/// context. This is the CI-blocking test.
#[test]
fn golden_gate_enforce_mode_zero_violations() {
    let config = ChatReliabilityConfig::default();
    let mut violations: Vec<String> = Vec::new();
    let fixtures = collect_approved(&corpus_dir());
    assert!(
        !fixtures.is_empty(),
        "no approved fixtures found — corpus_dir empty?",
    );

    for (path, case) in &fixtures {
        let chunks: Vec<RetrievedChunk> =
            case.chunks.iter().map(chunk_from_golden).collect();
        let report = evaluate_response(&case.response, &chunks, &case.query, &config);

        // ── Citation facet ─────────────────────────────────────────────────
        if report.citation_grounded.len() != case.expected.citation_grounded.len() {
            violations.push(format!(
                "[{}] citation count: agent emitted {}, expected {}",
                case.id,
                report.citation_grounded.len(),
                case.expected.citation_grounded.len()
            ));
        } else {
            for (i, exp) in case.expected.citation_grounded.iter().enumerate() {
                let actual = &report.citation_grounded[i];
                let exp_status = citation_status_from_str(&exp.status);
                if actual.item_id != exp.item_id {
                    violations.push(format!(
                        "[{}] cite[{}] item_id: agent={}, expected={}",
                        case.id, i, actual.item_id, exp.item_id
                    ));
                }
                if actual.status != exp_status {
                    violations.push(format!(
                        "[{}] cite[{}] status: agent={:?}, expected={:?}",
                        case.id, i, actual.status, exp_status
                    ));
                }
            }
        }

        // ── Contradictions facet ───────────────────────────────────────────
        if report.contradictions.len() != case.expected.contradictions_count {
            violations.push(format!(
                "[{}] contradictions count: agent={}, expected={}",
                case.id,
                report.contradictions.len(),
                case.expected.contradictions_count
            ));
        }

        // ── Hallucination facet ────────────────────────────────────────────
        let mut agent_kinds: Vec<&'static str> = report
            .hallucination_flags
            .iter()
            .map(|f| hallucination_kind_to_str(f.kind))
            .collect();
        agent_kinds.sort();
        let mut expected_kinds: Vec<&str> =
            case.expected.hallucination_kinds.iter().map(|s| s.as_str()).collect();
        expected_kinds.sort();
        if agent_kinds != expected_kinds {
            violations.push(format!(
                "[{}] hallucination kinds: agent={:?}, expected={:?}",
                case.id, agent_kinds, expected_kinds
            ));
        }

        // ── Confidence facet ───────────────────────────────────────────────
        let [lo, hi] = case.expected.confidence_range;
        if report.overall_confidence < lo - 1e-4
            || report.overall_confidence > hi + 1e-4
        {
            violations.push(format!(
                "[{}] confidence {} outside expected range [{}, {}] (path={:?})",
                case.id, report.overall_confidence, lo, hi, path
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "ENFORCE mode: {} violations:\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

/// Companion test: scan for any fixtures that are unapproved or missing
/// expected fields. Always passes but prints a friendly checklist so an
/// engineer adding a new fixture can spot uncommitted gaps. Mirrors the
/// methodology §5 "missing-ground-truth companion test" pattern.
#[test]
fn report_unapproved_or_partial_fixtures() {
    let mut unapproved: Vec<PathBuf> = Vec::new();
    let dir = corpus_dir();
    for entry in std::fs::read_dir(&dir).expect("dir") {
        let entry = entry.unwrap();
        let p = entry.path();
        let files: Vec<PathBuf> = if p.is_dir() {
            std::fs::read_dir(&p)
                .unwrap()
                .filter_map(|e| e.ok().map(|x| x.path()))
                .collect()
        } else {
            vec![p]
        };
        for f in files {
            if f.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let raw = std::fs::read_to_string(&f).unwrap();
            let parsed: Result<GoldenCase, _> = serde_yaml::from_str(&raw);
            match parsed {
                Ok(c) if !c.reviewer.approved => unapproved.push(f),
                Err(_) => unapproved.push(f),
                _ => {}
            }
        }
    }
    if !unapproved.is_empty() {
        // Print only, do not fail. Methodology §5: companion test never blocks.
        eprintln!(
            "[chat_reliability companion] {} unapproved / unparseable fixtures:",
            unapproved.len(),
        );
        for f in &unapproved {
            eprintln!("  - {:?}", f);
        }
    }
}
