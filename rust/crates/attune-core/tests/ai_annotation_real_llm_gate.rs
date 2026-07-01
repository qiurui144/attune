//! AI-annotation (4 angles) — REAL-LLM verification gate (#[ignore], secret/Ollama-gated).
//!
//! ## Why this test exists
//!
//! Per Agent 验证铁律 (attune/CLAUDE.md) + docs/agent-capabilities.md §5.2 缺口 #1: the
//! ai_annotation plugin (risk / outdated / highlights / questions) was the WEAKEST real-LLM
//! gate in OSS attune. The only real-LLM assertion (`ai_annotator.rs::real_llm_*`) had three
//! holes:
//!   1. `#[ignore]` + **silent-pass on missing model** (Ollama unreachable → returned Ok, a
//!      green test that never exercised an LLM — a false sense of security);
//!   2. it covered **only the Highlights angle** — risk/outdated/questions had ZERO real-LLM
//!      coverage;
//!   3. it was a binary "non-empty located findings" check — **no F1, no floor, no N=3, no
//!      multi-tier matrix, no ratchet**.
//!
//! This gate closes all three. For EACH of the 4 angles it runs the production
//! `generate_annotations` path (schema-guided JSON + ≤3 retry-validate + few-shot + PII redact)
//! against a REAL model, N≥3 seeds, over a human-authored holdout corpus
//! (`tests/golden/ai_annotation/<angle>.yaml`, GT NEVER agent-generated), and computes a real
//! **micro-F1** with a hard floor. The silent-pass hole is fixed: `require_llm()` PANICS when
//! no model is reachable (a missing model is a fail-to-run, never a silent green); the CI
//! secret-gated lane decides run-vs-SKIP at the JOB level (skip-not-pass), not inside the test.
//!
//! ## F1 definition (real, human-GT)
//!
//! Each holdout case carries `targets` = the verbatim spans a correct annotation for that angle
//! must hit (or `expect_empty` precision-control notes with no target). Scoring a case:
//!   • TP — a located finding whose snippet overlaps a GT target span (each target matched once);
//!   • FP — a located finding overlapping no remaining GT target (penalises over-eager models,
//!          and EVERY finding on an `expect_empty` note is a FP);
//!   • FN — a GT target no finding matched.
//! Micro-F1 = 2·TP / (2·TP + FP + FN), aggregated over the whole corpus, per seed, then mean±std.
//! Overlap is character-span overlap on the located UTF-16 offsets vs the GT span's offsets in
//! the source — so this also exercises the §"backend locates offset" contract end-to-end.
//!
//! ## How to run — multi-tier matrix (§4.5-D)
//!
//! Provider-agnostic via env (identical contract to `doc_intel_real_llm_gate.rs`).
//!
//! **Tier 1 — weak local (Ollama qwen2.5:3b):**
//! ```bash
//! ollama pull qwen2.5:3b
//! export ATTUNE_LLM_PROVIDER=ollama ATTUNE_LLM_MODEL=qwen2.5:3b ATTUNE_REAL_LLM_SEEDS=3
//! cargo test -p attune-core --test ai_annotation_real_llm_gate -- --ignored --nocapture --test-threads=1
//! ```
//!
//! **Tier 2 — weak cloud (DeepSeek / gpt-4o-mini / gemini-flash):**
//! ```bash
//! set -a; source /tmp/secrets-deepseek/key.env; set +a    # DEEPSEEK_API_KEY
//! export ATTUNE_LLM_PROVIDER=openai_compat
//! export ATTUNE_LLM_ENDPOINT=https://api.deepseek.com/v1
//! export ATTUNE_LLM_API_KEY="$DEEPSEEK_API_KEY"           # NEVER echoed / committed
//! export ATTUNE_LLM_MODEL=deepseek-chat ATTUNE_REAL_LLM_SEEDS=3
//! cargo test -p attune-core --test ai_annotation_real_llm_gate -- --ignored --nocapture --test-threads=1
//! ```
//!
//! ## Floor (no goalpost-moving — Agent 验证铁律)
//!
//! micro-F1 ≥ 0.50 per angle. Annotation snippet-matching is intrinsically fuzzier than the
//! doc-intel 4-class verdict (a model may quote an adjacent valid span), so the floor is set
//! below the doc-intel 0.80; it is a real, measured floor over human GT and is ratcheted only
//! UP. If a real run falls below, RELEASE.md must label the angle Beta / raise the model tier —
//! NOT relax the floor here. Below-floor on a weak tier is recorded honestly (§6.3), not hidden.

#![allow(clippy::print_stdout)]

use std::collections::HashMap;
use std::path::PathBuf;

use attune_core::ai_annotator::{generate_annotations, AiAngle, LocatedFinding};
use attune_core::llm::{LlmProvider, OllamaLlmProvider, OpenAiLlmProvider};
use serde::Deserialize;

const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:3b";

/// micro-F1 floor per angle. Real, human-GT measured; ratcheted only up.
const F1_FLOOR: f64 = 0.50;

// ── provider (mirror doc_intel_real_llm_gate.rs; NEVER prints the api key) ───
fn require_llm() -> Box<dyn LlmProvider> {
    let kind = std::env::var("ATTUNE_LLM_PROVIDER").unwrap_or_else(|_| "ollama".into());
    match kind.as_str() {
        "openai_compat" | "openai" => {
            let endpoint = std::env::var("ATTUNE_LLM_ENDPOINT")
                .expect("ATTUNE_LLM_ENDPOINT required (e.g. https://api.deepseek.com/v1)");
            let api_key = std::env::var("ATTUNE_LLM_API_KEY").expect("ATTUNE_LLM_API_KEY required");
            let model = std::env::var("ATTUNE_LLM_MODEL").expect("ATTUNE_LLM_MODEL required");
            let host = endpoint.split("//").nth(1).unwrap_or(&endpoint);
            println!("[provider] openai_compat host={host} model={model}");
            Box::new(OpenAiLlmProvider::new(&endpoint, &api_key, &model))
        }
        _ => {
            let model =
                std::env::var("ATTUNE_LLM_MODEL").unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into());
            let p = OllamaLlmProvider::with_model(&model);
            // SILENT-PASS FIX: a missing/unreachable model is a fail-to-RUN, never a silent
            // green. (The CI secret-gated lane decides run-vs-SKIP at the JOB level.)
            assert!(
                p.is_available(),
                "Ollama not reachable on :11434 — start `ollama serve` + `ollama pull {model}`. \
                 A missing model must NOT silently pass this gate (Agent 验证铁律)."
            );
            println!("[provider] ollama model={model}");
            Box::new(p)
        }
    }
}

fn model_name() -> String {
    std::env::var("ATTUNE_LLM_MODEL").unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into())
}

fn seeds() -> usize {
    std::env::var("ATTUNE_REAL_LLM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(3)
}

fn mean_std(xs: &[f64]) -> (f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let std = (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    (mean, std)
}

// ── corpus ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Corpus {
    angle: String,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    text: String,
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    expect_empty: bool,
}

fn load_corpus(angle: &str) -> Corpus {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "golden",
        "ai_annotation",
        &format!("{angle}.yaml"),
    ]
    .iter()
    .collect();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read corpus {}: {e}", path.display()));
    let c: Corpus = serde_yaml::from_str(&text).expect("parse corpus");
    assert_eq!(c.angle, angle, "corpus angle mismatch");
    c
}

/// UTF-16 [start,end) of the first occurrence of `span` in `text` (for GT offsets), so GT and
/// located findings are compared in the SAME unit `LocatedFinding` reports (UTF-16 code units).
fn utf16_span(text: &str, span: &str) -> Option<(i64, i64)> {
    let byte = text.find(span)?;
    let start = text[..byte].encode_utf16().count() as i64;
    let len = span.encode_utf16().count() as i64;
    Some((start, start + len))
}

fn overlaps(a: (i64, i64), b: (i64, i64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Score one case → (tp, fp, fn_). Each GT target may be matched by at most one finding.
fn score_case(
    text: &str,
    targets: &[String],
    findings: &[LocatedFinding],
) -> (usize, usize, usize) {
    let gt: Vec<(i64, i64)> = targets.iter().filter_map(|t| utf16_span(text, t)).collect();
    let mut gt_hit = vec![false; gt.len()];
    let mut tp = 0usize;
    let mut fp = 0usize;
    for f in findings {
        let fspan = (f.offset_start, f.offset_end);
        // first un-hit GT target this finding overlaps
        let mut matched = false;
        for (i, g) in gt.iter().enumerate() {
            if !gt_hit[i] && overlaps(fspan, *g) {
                gt_hit[i] = true;
                tp += 1;
                matched = true;
                break;
            }
        }
        if !matched {
            fp += 1;
        }
    }
    let fn_ = gt_hit.iter().filter(|h| !**h).count();
    (tp, fp, fn_)
}

/// Run all four-angle gates share this body. Returns nothing; asserts the floor.
fn run_angle_gate(angle_tag: &str, angle: AiAngle) {
    let llm = require_llm();
    let corpus = load_corpus(angle_tag);
    let n_seeds = seeds();
    println!(
        "\n=== AI-ANNOTATION [{angle_tag}] — real LLM ({}), {n_seeds} seeds, {} cases ===",
        model_name(),
        corpus.cases.len()
    );

    let mut f1_per_seed: Vec<f64> = Vec::new();
    // accumulate per-case dropped/located stats for the run-log
    let mut total_located = 0usize;

    for seed in 0..n_seeds {
        let mut tp = 0usize;
        let mut fp = 0usize;
        let mut fn_ = 0usize;
        for case in &corpus.cases {
            let text = case.text.trim_end();
            let findings = generate_annotations(llm.as_ref(), text, text, 0, angle)
                .unwrap_or_else(|e| panic!("[{angle_tag}] case {} generate err: {e:?}", case.id));
            total_located += findings.len();

            // Grounding/schema contract: every located finding must be in-bounds (the
            // backend-locates-offset guarantee). A regression here is a hard fail.
            let units = text.encode_utf16().count() as i64;
            for f in &findings {
                assert!(
                    f.offset_start >= 0
                        && f.offset_end <= units
                        && f.offset_start < f.offset_end
                        && !f.snippet.trim().is_empty(),
                    "[{angle_tag}] case {} ungrounded finding {}..{} (len {units}) snippet={:?}",
                    case.id,
                    f.offset_start,
                    f.offset_end,
                    f.snippet
                );
            }

            let (ctp, cfp, cfn) = if case.expect_empty {
                // precision control: any finding is a FP, no targets to recall.
                (0, findings.len(), 0)
            } else {
                score_case(text, &case.targets, &findings)
            };
            tp += ctp;
            fp += cfp;
            fn_ += cfn;
            println!(
                "  [seed {seed}] {:<24} located={} tp={ctp} fp={cfp} fn={cfn}{}",
                case.id,
                findings.len(),
                if case.expect_empty {
                    " (precision-control)"
                } else {
                    ""
                }
            );
        }
        let denom = (2 * tp + fp + fn_) as f64;
        let f1 = if denom > 0.0 {
            2.0 * tp as f64 / denom
        } else {
            1.0
        };
        println!("  [seed {seed}] micro-F1 = {f1:.3}  (tp={tp} fp={fp} fn={fn_})");
        f1_per_seed.push(f1);
    }

    let (mean, std) = mean_std(&f1_per_seed);
    println!(
        "\n=== [{angle_tag}] RESULT: micro-F1 mean={mean:.3} std={std:.3} (N={n_seeds}); \
         floor={F1_FLOOR:.2}; total_located_findings={total_located} ===",
    );

    assert!(
        mean >= F1_FLOOR,
        "[{angle_tag}] micro-F1 {mean:.3} < {F1_FLOOR:.2} floor. Per Agent 验证铁律: label the \
         angle Beta / raise the model tier in RELEASE.md — do NOT relax the floor."
    );
}

// ── 4 angle gates (one binary; `real_llm` name filter runs all four) ─────────

#[test]
#[ignore = "requires real LLM — openai_compat (DeepSeek/DashScope) via env, or Ollama"]
fn real_llm_ai_annotation_risk() {
    run_angle_gate("risk", AiAngle::Risk);
}

#[test]
#[ignore = "requires real LLM — openai_compat (DeepSeek/DashScope) via env, or Ollama"]
fn real_llm_ai_annotation_outdated() {
    run_angle_gate("outdated", AiAngle::Outdated);
}

#[test]
#[ignore = "requires real LLM — openai_compat (DeepSeek/DashScope) via env, or Ollama"]
fn real_llm_ai_annotation_highlights() {
    run_angle_gate("highlights", AiAngle::Highlights);
}

#[test]
#[ignore = "requires real LLM — openai_compat (DeepSeek/DashScope) via env, or Ollama"]
fn real_llm_ai_annotation_questions() {
    run_angle_gate("questions", AiAngle::Questions);
}

// ── deterministic guards that DO run on every PR (no LLM) ────────────────────
// These keep the corpus + scoring honest without a model: corpus shape (≥10 cases,
// GT verbatim) and the scoring function's correctness. Naming avoids the `real_llm`
// filter so they run in the default `cargo test` lane.

#[test]
fn corpus_shape_all_angles() {
    // dead_code suppression: the no-LLM lane never calls these, but the corpus
    // loader + scoring must stay reachable/typed.
    let _ = (model_name as fn() -> String, seeds as fn() -> usize);
    for angle in ["risk", "outdated", "highlights", "questions"] {
        let c = load_corpus(angle);
        assert!(
            c.cases.len() >= 10,
            "angle {angle} has {} cases (< 10 required by Agent 验证铁律)",
            c.cases.len()
        );
        let mut ids = HashMap::new();
        for case in &c.cases {
            assert!(
                ids.insert(case.id.clone(), ()).is_none(),
                "dup id {}",
                case.id
            );
            // every GT target is a verbatim substring of the case text (fair to the model).
            for t in &case.targets {
                assert!(
                    case.text.contains(t),
                    "angle {angle} case {} GT target not verbatim in text: {t:?}",
                    case.id
                );
                assert!(
                    utf16_span(&case.text, t).is_some(),
                    "angle {angle} case {} GT target has no utf16 span: {t:?}",
                    case.id
                );
            }
            if case.expect_empty {
                assert!(
                    case.targets.is_empty(),
                    "expect_empty case {} must have no targets",
                    case.id
                );
            }
        }
    }
}

#[test]
fn scoring_overlap_logic() {
    // Two GT targets in "AAAA BBBB CCCC"; one finding overlaps target 0, one is spurious.
    let text = "数据库管理系统负责数据创建";
    let targets = vec!["数据库管理系统".to_string()];
    // finding overlapping the target → TP
    let f_tp = LocatedFinding {
        offset_start: 0,
        offset_end: 7,
        snippet: "数据库管理系统".into(),
        reason: "x".into(),
    };
    // finding overlapping nothing in GT → FP
    let f_fp = LocatedFinding {
        offset_start: 9,
        offset_end: 11,
        snippet: "创建".into(),
        reason: "y".into(),
    };
    let (tp, fp, fn_) = score_case(text, &targets, &[f_tp, f_fp]);
    assert_eq!((tp, fp, fn_), (1, 1, 0));
    // no findings → the single target is a FN
    let (tp, fp, fn_) = score_case(text, &targets, &[]);
    assert_eq!((tp, fp, fn_), (0, 0, 1));
    // one GT, one finding overlapping → perfect
    assert!(overlaps((0, 7), (3, 10)));
    assert!(!overlaps((0, 7), (7, 10)));
}
