//! Document-comparison semantic-verdict — deterministic golden gate (CI-blocking).
//!
//! per `attune/CLAUDE.md` §「Agent 验证铁律」: the compare semantic verdict is an LLM-judgement
//! agent. Its CI-deterministic gate proves two things WITHOUT a network call:
//!
//!   1. **Parser robustness (§4.5.A)** — `DiffVerdict::parse_from_llm_response` extracts the
//!      correct 4-class verdict from EVERY realistic model-output shape (clean JSON, ```json
//!      fences, leading prose, reordered fields, free-text fallback). This is the fix that took
//!      real deepseek-chat verdict F1 0.91 → 1.00; here it is locked deterministically so a
//!      regression (e.g. reverting to "first line = label") fails CI.
//!   2. **Pipeline wiring** — `compare()` carries each labeled verdict through the full
//!      structural→textual→semantic path to the marked annotations, with a mock returning the
//!      schema-guided JSON the real model is steered to emit.
//!
//! Ground truth = the hand-labeled `golden/doc_compare_verdict/cases.yaml` (NOT agent-generated).
//! The same corpus is replayed against real DeepSeek by `doc_intel_real_llm_gate.rs` (#[ignore]).
//!
//! 6-class coverage floor for this agent:
//!   * Golden cases ≥ 10 + 1 sentinel .... 30 hand-labeled cases in cases.yaml
//!   * Boundary `#[test]` ≥ 5 ............. the `output_shape_*` + sentinel tests below
//!   * Error case ≥ 3 .................... empty / noise / unknown-enum shapes below
//!   * Property-ish ...................... shape matrix × every verdict (4×N) below
//!   * Integration E2E .................. `compare()` end-to-end per case below
//!   * Regression fixture ............... the whole file is the ratchet-locked regression set

use std::path::PathBuf;

use attune_core::document_intelligence::compare::{
    compare, CompareMode, DiffVerdict, OutputMode, StageLlms,
};
use attune_core::document_intelligence::model_routing::ModelRouter;
use attune_core::llm::RecordingMockLlm;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: Vec<VerdictCase>,
}

#[derive(Debug, Deserialize)]
struct VerdictCase {
    id: String,
    a: String,
    b: String,
    verdict: String,
}

fn load_corpus() -> Corpus {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "golden",
        "doc_compare_verdict",
        "cases.yaml",
    ]
    .iter()
    .collect();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read golden corpus {}: {e}", path.display()));
    serde_yaml::from_str(&text).expect("parse golden corpus yaml")
}

fn router() -> ModelRouter {
    ModelRouter::from_settings(&json!({
        "model_routing": { "cheap": "gpt-4o-mini", "reasoning": "gpt-4o", "vision": "gpt-4o-mini" }
    }))
}

/// Map a verdict label string to the canonical kebab form (the YAML uses kebab already).
fn expected_kebab(label: &str) -> &str {
    label.trim()
}

/// The realistic model-output SHAPES a DeepSeek-class model emits. Each must parse to the same
/// verdict. `{V}` / `{R}` are substituted with the case's verdict + a rationale.
fn output_shapes(verdict: &str, rationale: &str) -> Vec<String> {
    vec![
        // clean object
        format!(r#"{{"verdict":"{verdict}","rationale":"{rationale}"}}"#),
        // markdown-fenced (very common with DeepSeek)
        format!("```json\n{{\"verdict\": \"{verdict}\", \"rationale\": \"{rationale}\"}}\n```"),
        // leading prose then object
        format!("判定如下：\n{{\"verdict\":\"{verdict}\",\"rationale\":\"{rationale}\"}}"),
        // reordered fields
        format!(r#"{{"rationale":"{rationale}","verdict":"{verdict}"}}"#),
        // pretty-printed multi-line
        format!("{{\n  \"verdict\": \"{verdict}\",\n  \"rationale\": \"{rationale}\"\n}}"),
    ]
}

#[test]
fn golden_corpus_has_floor_and_all_classes() {
    let corpus = load_corpus();
    assert!(
        corpus.cases.len() >= 11,
        "golden corpus must have ≥10 + sentinel; got {}",
        corpus.cases.len()
    );
    // every 4-class verdict represented
    for want in ["rewrite", "substantive", "stance-reversal", "numeric-change"] {
        assert!(
            corpus.cases.iter().any(|c| c.verdict == want),
            "corpus must include at least one '{want}' case"
        );
    }
    // ids unique
    let mut ids: Vec<&str> = corpus.cases.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "case ids must be unique");
}

/// Parser robustness: EVERY shape of EVERY labeled case parses to the labeled verdict.
/// This is the deterministic stand-in for the real-LLM F1 — a 100% pass here means the parser
/// itself never mis-reads a well-formed verdict (the model still has to pick the right class,
/// which is the real-DeepSeek gate's job).
#[test]
fn parser_extracts_correct_verdict_from_every_output_shape() {
    let corpus = load_corpus();
    let mut total = 0usize;
    let mut correct = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for case in &corpus.cases {
        let want = expected_kebab(&case.verdict);
        for shape in output_shapes(want, "理由占位 rationale-placeholder") {
            total += 1;
            let got = DiffVerdict::parse_from_llm_response(&shape);
            if got.as_kebab() == want {
                correct += 1;
            } else {
                failures.push(format!(
                    "case {} shape {:?} → got {} want {want}",
                    case.id,
                    shape.chars().take(40).collect::<String>(),
                    got.as_kebab()
                ));
            }
        }
    }
    let rate = correct as f64 / total as f64;
    assert!(
        (rate - 1.0).abs() < 1e-9,
        "parser pass-rate must be 1.00 (deterministic floor); got {correct}/{total} = {rate:.3}\n{}",
        failures.join("\n")
    );
}

/// End-to-end: each labeled pair, with a mock returning the schema-guided JSON the real model is
/// steered to emit, carries the verdict through `compare()` to a marked annotation.
#[test]
fn compare_pipeline_carries_each_labeled_verdict() {
    let corpus = load_corpus();
    for case in &corpus.cases {
        let want = expected_kebab(&case.verdict);
        // Wrap each side in a heading so the structural+textual layers detect a modified section
        // and surface exactly one changed span (the new B line) → one verdict.
        let a = format!("# 段落\n\n{}\n", case.a);
        let b = format!("# 段落\n\n{}\n", case.b);
        let cheap = RecordingMockLlm::new("gpt-4o-mini")
            .with_response(&json!({"verdict": want, "rationale": "测试理由"}).to_string());
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("总体差异摘要");
        let llms = StageLlms { cheap: &cheap, reasoning: &reasoning };
        let r = compare(&a, &b, CompareMode::Semantic, OutputMode::Marked, true, &router(), &llms)
            .unwrap_or_else(|e| panic!("case {} compare failed: {e:?}", case.id));
        assert!(
            r.semantic_verdicts.iter().any(|v| v.verdict == want),
            "case {}: expected a '{want}' verdict in {:?}",
            case.id,
            r.semantic_verdicts.iter().map(|v| &v.verdict).collect::<Vec<_>>()
        );
        assert!(
            r.annotations.iter().any(|x| x.kind == want),
            "case {}: marked annotation must carry verdict kind '{want}'",
            case.id
        );
    }
}

// ── boundary / error shapes (≥3 error cases) ────────────────────────────────

#[test]
fn output_shape_empty_string_degrades_not_panics() {
    // Empty → conservative rewrite, no panic.
    assert_eq!(DiffVerdict::parse_from_llm_response(""), DiffVerdict::Rewrite);
}

#[test]
fn output_shape_pure_noise_degrades() {
    assert_eq!(DiffVerdict::parse_from_llm_response("？？？ %%% ###"), DiffVerdict::Rewrite);
}

#[test]
fn output_shape_unknown_enum_value_falls_to_keyword_heuristic() {
    // Valid JSON, out-of-enum verdict → keyword heuristic on the value ("modified" has no keyword
    // → conservative rewrite). Never panics.
    assert_eq!(
        DiffVerdict::parse_from_llm_response(r#"{"verdict":"modified","rationale":"x"}"#),
        DiffVerdict::Rewrite
    );
    // "numeric" keyword still recovered even from a near-miss enum spelling.
    assert_eq!(
        DiffVerdict::parse_from_llm_response(r#"{"verdict":"numeric","rationale":"x"}"#),
        DiffVerdict::NumericChange
    );
}

#[test]
fn output_shape_brace_inside_rationale_is_string_safe() {
    assert_eq!(
        DiffVerdict::parse_from_llm_response(r#"{"verdict":"substantive","rationale":"删除了 {a,b}"}"#),
        DiffVerdict::Substantive
    );
}

#[test]
fn output_shape_free_text_chinese_keyword_fallback() {
    // No JSON at all → legacy keyword heuristic still classifies the obvious cases.
    assert_eq!(DiffVerdict::parse_from_llm_response("立场反转"), DiffVerdict::StanceReversal);
    assert_eq!(DiffVerdict::parse_from_llm_response("数字变化"), DiffVerdict::NumericChange);
    assert_eq!(DiffVerdict::parse_from_llm_response("实质内容变化"), DiffVerdict::Substantive);
}
