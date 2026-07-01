//! I2 — vision eval gate harness (spec 2026-06-16 §I2 / §9.2). OFFLINE only (feature `vision-eval`).
//!
//! Mirrors the agent-golden-gate model from attune-pro (`agent_golden_gate.rs`): a holdout set of
//! human-labeled fixtures (per non-text kind), each run through the real VLM escalation path N=3
//! seeds, scored with kind-appropriate metrics (see [`metrics`]), aggregated to mean ± std per
//! `(kind × model)`, and gated against a floor. Below floor → the kind is not ship-able as a default
//! on that model (spec §9.2). This is a DEV/CI gate, never the user request path (spec §8.1).
//!
//! ## Real-VLM vs mock lane
//!
//! The harness is provider-agnostic — it takes any `&dyn VlmProvider`. The mock lane (deterministic
//! `MockVlmProvider` / scripted) exercises the harness wiring + metrics in CI with no network. The
//! real-VLM lane is a `#[ignore]` (nightly) test that builds a qwen-3.6/3.7 multimodal provider via
//! the OpenAI-compatible DashScope endpoint (per §4.5H) and runs N=3 against real fixtures —
//! invoked manually under cost authorization (spec §1.3 / §8.1 `--confirm-cost`).

pub mod metrics;

use super::vlm_escalate::{escalate_region, gate_vlm_egress, ImageEgressDecision, RegionGeom};
use super::{RegionKind, RegionResult};
use crate::ocr::BBox;
use crate::pii::Redactor;
use crate::vlm::VlmProvider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default seed count for a vision eval run (spec §2.3 / §9.2: N=3 multi-seed).
pub const DEFAULT_SEEDS: usize = 3;

/// One human-labeled holdout fixture: an image of a single region + its ground-truth structured
/// value. GT is produced INDEPENDENTLY of the recognizer (per Agent 验证铁律: never agent-generated).
#[derive(Debug, Clone)]
pub struct VisionFixture {
    /// Stable id for raw-log traceability (spec §6.3): `chart-01`, `formula-03`, ...
    pub id: String,
    pub kind: RegionKind,
    /// Path to the region-crop image fed to the VLM.
    pub image_path: PathBuf,
    /// Ground truth the prediction is scored against.
    pub truth: VisionTruth,
}

/// Ground-truth structured value for a fixture (the human label). Variant matches the fixture kind.
#[derive(Debug, Clone)]
pub enum VisionTruth {
    /// chart: GT series as (name, values).
    Chart(Vec<(String, Vec<f64>)>),
    /// formula: GT LaTeX string.
    Formula(String),
    /// free-text kinds (handwriting / stamp / figure caption): GT text.
    Text(String),
}

/// Score of one prediction against one fixture's GT (a single seed run).
#[derive(Debug, Clone, Copy)]
pub struct FixtureScore {
    /// Accuracy/F1 in [0,1] for the value (chart-F1 / latex-sim / token-F1).
    pub value_score: f64,
    /// Grounding precision in [0,1] for the extracted values (spec §9.3).
    pub grounding_precision: f64,
}

/// Score one parsed `RegionResult` against a fixture's GT (pure logic, deterministic). Used per seed.
pub fn score_prediction(
    truth: &VisionTruth,
    predicted: &RegionResult,
    chart_tol: f64,
) -> FixtureScore {
    match (truth, predicted) {
        (VisionTruth::Chart(gt), RegionResult::ChartV1 { series, .. }) => {
            let pred: Vec<(String, Vec<f64>)> = series
                .iter()
                .map(|s| (s.name.clone(), s.values.clone()))
                .collect();
            let total = series.len();
            let grounded = series.iter().filter(|s| s.grounding.is_some()).count();
            FixtureScore {
                value_score: metrics::chart_series_f1(&pred, gt, chart_tol),
                grounding_precision: metrics::grounding_precision(grounded, total),
            }
        }
        (
            VisionTruth::Formula(gt),
            RegionResult::FormulaV1 {
                latex, grounding, ..
            },
        ) => {
            let pred = latex.as_deref().unwrap_or("");
            let total = usize::from(latex.is_some());
            let grounded = usize::from(grounding.is_some());
            FixtureScore {
                value_score: metrics::latex_similarity(pred, gt),
                grounding_precision: metrics::grounding_precision(grounded, total),
            }
        }
        (VisionTruth::Text(gt), pred) => {
            let (text, grounded, total) = extract_text_and_grounding(pred);
            FixtureScore {
                value_score: metrics::token_f1(&metrics::tokenize(&text), &metrics::tokenize(gt)),
                grounding_precision: metrics::grounding_precision(grounded, total),
            }
        }
        // Kind/GT mismatch (recognizer returned a different variant than the fixture expects) →
        // worst score, never silently treated as a pass.
        _ => FixtureScore {
            value_score: 0.0,
            grounding_precision: 0.0,
        },
    }
}

/// Pull the free-text value + its grounding presence out of a text-bearing RegionResult variant.
fn extract_text_and_grounding(r: &RegionResult) -> (String, usize, usize) {
    match r {
        RegionResult::HandwritingV1 { text, grounding } => {
            text_grounded(text.as_deref(), grounding.is_some())
        }
        RegionResult::StampV1 {
            text, grounding, ..
        } => text_grounded(text.as_deref(), grounding.is_some()),
        RegionResult::FigureV1 {
            caption, grounding, ..
        } => text_grounded(caption.as_deref(), grounding.is_some()),
        _ => (String::new(), 0, 0),
    }
}

fn text_grounded(text: Option<&str>, has_grounding: bool) -> (String, usize, usize) {
    match text {
        Some(t) => (t.to_string(), usize::from(has_grounding), 1),
        None => (String::new(), 0, 0),
    }
}

/// Aggregated mean ± std over N seeds for one `(kind × model)` (spec §9.2).
#[derive(Debug, Clone)]
pub struct KindModelScore {
    pub kind: RegionKind,
    pub model: String,
    pub n_seed: usize,
    pub value_f1_mean: f64,
    pub value_f1_std: f64,
    pub grounding_precision_mean: f64,
    /// number of fixtures averaged into this row.
    pub n_fixtures: usize,
}

/// Final verdict of a vision eval run (spec §5.3 `VisionEvalVerdict`).
#[derive(Debug, Clone)]
pub struct VisionEvalVerdict {
    pub per_kind_model: Vec<KindModelScore>,
    /// false if ANY default-enabled (kind × model) has value_f1_mean < floor (spec §9.2).
    pub gate_pass: bool,
    /// Floor used for the gate decision (golden-calibrated, spec §9.3).
    pub floor: f64,
    /// Minimum ship-able tier per kind (kinds whose only passing model is a higher tier, §4.5 E).
    pub min_tier_per_kind: HashMap<RegionKind, String>,
}

/// Run the eval for ONE model: every fixture × `seeds` runs through the real escalation path, scored,
/// aggregated per kind. `make_token` mints a [`super::vlm_escalate::VlmEgressToken`] per image (so the
/// egress gate is exercised the same way production does). `chart_tol` / region geom are fixed for the
/// holdout (each fixture is a single cropped region).
///
/// Deterministic given a deterministic VLM (mock lane). For the real lane, seeds drive sampling
/// variance which the mean ± std captures (spec §9.2).
pub fn run_model_eval(
    vlm: &dyn VlmProvider,
    model: &str,
    fixtures: &[VisionFixture],
    seeds: usize,
    chart_tol: f64,
) -> Vec<KindModelScore> {
    // Group scores by kind across all fixtures + seeds.
    let mut by_kind: HashMap<RegionKind, Vec<FixtureScore>> = HashMap::new();
    let mut fixtures_per_kind: HashMap<RegionKind, usize> = HashMap::new();

    for fx in fixtures {
        *fixtures_per_kind.entry(fx.kind).or_insert(0) += 1;
        let geom = geom_for_image(&fx.image_path);
        for _seed in 0..seeds {
            // Mint a fresh egress token per call (the gate downscales/redacts a temp copy).
            let Some(token) = mint_eval_token(&fx.image_path) else {
                // Can't gate the image (decode/IO) → worst score for this run, never skip silently.
                by_kind.entry(fx.kind).or_default().push(FixtureScore {
                    value_score: 0.0,
                    grounding_precision: 0.0,
                });
                continue;
            };
            let (res, _tel) = escalate_region(vlm, &token, fx.kind, model, geom);
            let score = match res {
                Ok(r) => score_prediction(&fx.truth, &r, chart_tol),
                // A hard failure (provider error / parse-fail after retries) scores zero, never skipped.
                Err(_) => FixtureScore {
                    value_score: 0.0,
                    grounding_precision: 0.0,
                },
            };
            by_kind.entry(fx.kind).or_default().push(score);
        }
    }

    let mut out = Vec::new();
    for (kind, scores) in by_kind {
        let values: Vec<f64> = scores.iter().map(|s| s.value_score).collect();
        let groundings: Vec<f64> = scores.iter().map(|s| s.grounding_precision).collect();
        let (v_mean, v_std) = metrics::mean_std(&values);
        let (g_mean, _) = metrics::mean_std(&groundings);
        out.push(KindModelScore {
            kind,
            model: model.to_string(),
            n_seed: seeds,
            value_f1_mean: v_mean,
            value_f1_std: v_std,
            grounding_precision_mean: g_mean,
            n_fixtures: *fixtures_per_kind.get(&kind).unwrap_or(&0),
        });
    }
    out.sort_by(|a, b| format!("{:?}", a.kind).cmp(&format!("{:?}", b.kind)));
    out
}

/// Compute the gate verdict from per-(kind×model) scores (spec §9.2). The `default_models` set are
/// the models enabled by default; any default (kind × model) with `value_f1_mean < floor` fails the
/// gate. `min_tier_per_kind` records, for each kind, the first model (in `tier_order`, weakest→
/// strongest) that meets the floor — the minimum ship-able tier (spec §4.5 E).
pub fn compute_verdict(
    scores: &[KindModelScore],
    floor: f64,
    default_models: &[String],
    tier_order: &[String],
) -> VisionEvalVerdict {
    let mut gate_pass = true;
    for s in scores {
        if default_models.iter().any(|m| m == &s.model) && s.value_f1_mean < floor {
            gate_pass = false;
        }
    }

    let mut min_tier_per_kind: HashMap<RegionKind, String> = HashMap::new();
    let kinds: std::collections::BTreeSet<String> =
        scores.iter().map(|s| format!("{:?}", s.kind)).collect();
    for kind_dbg in &kinds {
        // First model in weakest→strongest order that passes the floor for this kind.
        for tier in tier_order {
            if let Some(s) = scores.iter().find(|s| {
                &format!("{:?}", s.kind) == kind_dbg && &s.model == tier && s.value_f1_mean >= floor
            }) {
                min_tier_per_kind.insert(s.kind, tier.clone());
                break;
            }
        }
    }

    VisionEvalVerdict {
        per_kind_model: scores.to_vec(),
        gate_pass,
        floor,
        min_tier_per_kind,
    }
}

/// Region geom for a fixture image: the whole image is the region (single-region crops). Uses the
/// image's real dimensions as the crop coordinate system the VLM grounds against.
fn geom_for_image(path: &Path) -> RegionGeom {
    let (w, h) = image::image_dimensions(path).unwrap_or((1, 1));
    RegionGeom {
        page_bbox: BBox { x: 0, y: 0, w, h },
        page: 0,
        crop_w: w,
        crop_h: h,
    }
}

/// Mint an egress token for an eval image the SAME way production does (real gate: redact + downscale
/// + outbound gate). Non-L0 (holdout fixtures are public test images). `None` on decode/IO failure.
fn mint_eval_token(image_path: &Path) -> Option<super::vlm_escalate::VlmEgressToken> {
    let redactor = Redactor::new();
    let dst =
        std::env::temp_dir().join(format!("vision-eval-egress-{}.png", uuid_like(image_path)));
    gate_vlm_egress(
        true,
        true,
        false, // holdout fixtures are non-L0 public test images
        Some(&redactor),
        "vision-eval-region",
        image_path,
        ImageEgressDecision::AllowDownscaled(dst),
    )
    .ok()
}

/// DashScope OpenAI-compatible endpoint (per §4.5H multimodal default = qwen-3.6/3.7 via DashScope).
pub const DASHSCOPE_OPENAI_COMPAT: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

/// Build a real qwen multimodal VLM provider from env, for the real-VLM eval lane (spec §4.5H).
/// `DASHSCOPE_API_KEY` (or the OpenAI-compat overrides `*_BASE_URL`/`*_MODEL`/`*_API_KEY`) must be
/// set. Returns `None` when no key is configured → the real-VLM lane is PENDING-KEY (spec §6.3:
/// never fabricate an eval result; an absent key means the gate did not run, not that it passed).
///
/// Default model `qwen3-vl-plus` is a qwen3 multimodal model (NOT the deprecated `qwen-vl`, §4.5H
/// reverse-pattern). Override with `ATTUNE_VLM_MODEL` to eval a different tier (flash/plus/max).
pub fn qwen_vlm_from_env() -> Option<(std::sync::Arc<dyn VlmProvider>, String)> {
    let key = std::env::var("ATTUNE_VLM_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .ok()
        .filter(|k| !k.trim().is_empty())?;
    let base = std::env::var("ATTUNE_VLM_BASE_URL")
        .unwrap_or_else(|_| DASHSCOPE_OPENAI_COMPAT.to_string());
    let model = std::env::var("ATTUNE_VLM_MODEL").unwrap_or_else(|_| "qwen3-vl-plus".to_string());
    let llm = std::sync::Arc::new(crate::llm::OpenAiLlmProvider::new(&base, &key, &model));
    let vlm: std::sync::Arc<dyn VlmProvider> =
        std::sync::Arc::new(crate::vlm::LlmVlmProvider::new(llm));
    Some((vlm, model))
}

/// Cheap unique-ish suffix from a path (avoid an extra uuid dep in this offline harness).
fn uuid_like(p: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    std::time::SystemTime::now().hash(&mut h);
    format!("{:x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::nontext::Series;

    fn write_png(dir: &Path, name: &str, edge: u32) -> PathBuf {
        let p = dir.join(name);
        image::DynamicImage::new_rgb8(edge, edge).save(&p).unwrap();
        p
    }

    // ── score_prediction ──────────────────────────────────────────────────────

    #[test]
    fn score_chart_perfect_with_grounding() {
        let truth = VisionTruth::Chart(vec![("Q1".into(), vec![1.0])]);
        let pred = RegionResult::ChartV1 {
            chart_type: "bar".into(),
            series: vec![Series {
                name: "Q1".into(),
                values: vec![1.0],
                grounding: Some(crate::ocr::nontext::grounding::GroundingRef::region(
                    BBox {
                        x: 0,
                        y: 0,
                        w: 8,
                        h: 8,
                    },
                    0,
                )),
            }],
            axis_labels: vec![],
        };
        let s = score_prediction(&truth, &pred, 0.05);
        assert_eq!(s.value_score, 1.0);
        assert_eq!(s.grounding_precision, 1.0);
    }

    #[test]
    fn score_chart_ungrounded_value_lowers_grounding_precision() {
        let truth = VisionTruth::Chart(vec![("Q1".into(), vec![1.0])]);
        let pred = RegionResult::ChartV1 {
            chart_type: "bar".into(),
            series: vec![Series {
                name: "Q1".into(),
                values: vec![1.0],
                grounding: None,
            }],
            axis_labels: vec![],
        };
        let s = score_prediction(&truth, &pred, 0.05);
        assert_eq!(s.value_score, 1.0);
        assert_eq!(s.grounding_precision, 0.0); // value right, but not located
    }

    #[test]
    fn score_formula_similarity() {
        let truth = VisionTruth::Formula("E=mc^2".into());
        let pred = RegionResult::FormulaV1 {
            latex: Some("E=mc^2".into()),
            raw_ocr: None,
            grounding: None,
        };
        assert_eq!(score_prediction(&truth, &pred, 0.05).value_score, 1.0);
    }

    #[test]
    fn score_text_token_f1() {
        let truth = VisionTruth::Text("hello world".into());
        let pred = RegionResult::HandwritingV1 {
            text: Some("hello world".into()),
            grounding: None,
        };
        assert_eq!(score_prediction(&truth, &pred, 0.05).value_score, 1.0);
    }

    #[test]
    fn score_kind_mismatch_is_zero() {
        // GT is a chart but the recognizer returned formula → worst score (never a silent pass).
        let truth = VisionTruth::Chart(vec![("Q1".into(), vec![1.0])]);
        let pred = RegionResult::FormulaV1 {
            latex: Some("x".into()),
            raw_ocr: None,
            grounding: None,
        };
        let s = score_prediction(&truth, &pred, 0.05);
        assert_eq!(s.value_score, 0.0);
        assert_eq!(s.grounding_precision, 0.0);
    }

    // ── run_model_eval (mock lane) ────────────────────────────────────────────

    /// Mock VLM that always returns a fixed grounded handwriting JSON (deterministic across seeds).
    struct FixedVlm(String);
    impl VlmProvider for FixedVlm {
        fn caption(&self, _: &Path) -> crate::error::Result<String> {
            Ok(self.0.clone())
        }
        fn vqa(&self, _: &Path, _: &str) -> crate::error::Result<String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn run_model_eval_mock_lane_aggregates_n_seeds() {
        let dir = tempfile::tempdir().unwrap();
        let img = write_png(dir.path(), "hw.png", 8);
        let fixtures = vec![VisionFixture {
            id: "hw-01".into(),
            kind: RegionKind::Handwriting,
            image_path: img,
            truth: VisionTruth::Text("hello".into()),
        }];
        // Returns the exact GT text + an in-crop sub_bbox → value_score 1.0, grounding 1.0.
        let vlm = FixedVlm(
            r#"{"text":"hello","grounding":{"sub_bbox":{"x":1,"y":1,"w":2,"h":2}}}"#.into(),
        );
        let scores = run_model_eval(&vlm, "mock-qwen", &fixtures, DEFAULT_SEEDS, 0.05);
        assert_eq!(scores.len(), 1);
        let s = &scores[0];
        assert_eq!(s.kind, RegionKind::Handwriting);
        assert_eq!(s.n_seed, 3);
        assert_eq!(s.n_fixtures, 1);
        assert_eq!(s.value_f1_mean, 1.0);
        assert_eq!(s.value_f1_std, 0.0); // deterministic mock → zero variance
        assert_eq!(s.grounding_precision_mean, 1.0);
    }

    #[test]
    fn run_model_eval_wrong_answer_scores_low() {
        let dir = tempfile::tempdir().unwrap();
        let img = write_png(dir.path(), "hw.png", 8);
        let fixtures = vec![VisionFixture {
            id: "hw-02".into(),
            kind: RegionKind::Handwriting,
            image_path: img,
            truth: VisionTruth::Text("expected text".into()),
        }];
        let vlm = FixedVlm(
            r#"{"text":"totally wrong","grounding":{"sub_bbox":{"x":1,"y":1,"w":2,"h":2}}}"#.into(),
        );
        let scores = run_model_eval(&vlm, "mock-qwen", &fixtures, DEFAULT_SEEDS, 0.05);
        assert_eq!(scores[0].value_f1_mean, 0.0);
    }

    // ── compute_verdict ───────────────────────────────────────────────────────

    fn score(kind: RegionKind, model: &str, f1: f64) -> KindModelScore {
        KindModelScore {
            kind,
            model: model.into(),
            n_seed: 3,
            value_f1_mean: f1,
            value_f1_std: 0.0,
            grounding_precision_mean: 1.0,
            n_fixtures: 10,
        }
    }

    #[test]
    fn verdict_passes_when_all_default_models_meet_floor() {
        let scores = vec![score(RegionKind::Chart, "qwen-3.7-max", 0.9)];
        let v = compute_verdict(
            &scores,
            0.7,
            &["qwen-3.7-max".into()],
            &["qwen-3.7-max".into()],
        );
        assert!(v.gate_pass);
        assert_eq!(
            v.min_tier_per_kind
                .get(&RegionKind::Chart)
                .map(|s| s.as_str()),
            Some("qwen-3.7-max")
        );
    }

    #[test]
    fn verdict_fails_when_a_default_model_below_floor() {
        let scores = vec![score(RegionKind::Chart, "qwen-3.6-flash", 0.4)];
        let v = compute_verdict(
            &scores,
            0.7,
            &["qwen-3.6-flash".into()],
            &["qwen-3.6-flash".into()],
        );
        assert!(
            !v.gate_pass,
            "a default model below floor must fail the gate"
        );
    }

    #[test]
    fn verdict_min_tier_picks_weakest_passing_model() {
        // Chart: flash fails (0.4), plus passes (0.8), max passes (0.9). Weakest passing = plus.
        let scores = vec![
            score(RegionKind::Chart, "qwen-3.6-flash", 0.4),
            score(RegionKind::Chart, "qwen-3.7-plus", 0.8),
            score(RegionKind::Chart, "qwen-3.7-max", 0.9),
        ];
        let tiers = vec![
            "qwen-3.6-flash".into(),
            "qwen-3.7-plus".into(),
            "qwen-3.7-max".into(),
        ];
        // Only the strong tier is a DEFAULT, so gate passes; min_tier reports the weakest passing.
        let v = compute_verdict(&scores, 0.7, &["qwen-3.7-max".into()], &tiers);
        assert!(v.gate_pass);
        assert_eq!(
            v.min_tier_per_kind
                .get(&RegionKind::Chart)
                .map(|s| s.as_str()),
            Some("qwen-3.7-plus")
        );
    }

    #[test]
    fn verdict_floor_is_recorded() {
        let v = compute_verdict(&[], 0.75, &[], &[]);
        assert_eq!(v.floor, 0.75);
        assert!(v.gate_pass); // no default model below floor (none at all) → vacuously pass
    }

    #[test]
    fn qwen_vlm_from_env_is_none_without_key() {
        // Honesty contract (§6.3): no key → None (real-VLM lane is PENDING-KEY, never a fake pass).
        // We can't unset another test's env safely in parallel, so just assert the type contract:
        // when neither override nor DASHSCOPE_API_KEY is set the builder yields None. Run isolated:
        if std::env::var("DASHSCOPE_API_KEY").is_err()
            && std::env::var("ATTUNE_VLM_API_KEY").is_err()
        {
            assert!(qwen_vlm_from_env().is_none());
        }
    }

    // ── REAL-VLM lane (nightly, #[ignore]) — spec §9.2 / §4.5H / §1.3 ─────────
    //
    // Runs the escalation path against a REAL qwen multimodal model on DashScope, N=3 seeds, and
    // prints scores. Requires DASHSCOPE_API_KEY (or ATTUNE_VLM_* overrides) AND cost authorization
    // (spec §1.3 / §8.1): each run is a paid cloud call. NOT run in CI (ignored). This is a SMOKE of
    // the real provider wiring + N=3 aggregation on a synthetic fixture; the FULL 3-tier × human-GT
    // golden gate is PENDING the golden fixture set (spec §11 R1, plan-phase deliverable).
    #[test]
    #[ignore = "real-VLM lane: needs DASHSCOPE_API_KEY + cost authorization (§1.3); run with --ignored"]
    fn real_vlm_eval_smoke_n3() {
        let Some((vlm, model)) = qwen_vlm_from_env() else {
            eprintln!(
                "real-VLM eval PENDING-KEY: no DASHSCOPE_API_KEY/ATTUNE_VLM_API_KEY set — skipping"
            );
            return;
        };
        // A synthetic single-line handwriting-style image. (Real golden fixtures with human GT are a
        // plan-phase deliverable; this smoke only proves the real provider returns parseable output
        // through the full gate→escalate→ground→score path.)
        let dir = tempfile::tempdir().unwrap();
        let img = write_png(dir.path(), "real-hw.png", 64);
        let fixtures = vec![VisionFixture {
            id: "real-hw-01".into(),
            kind: RegionKind::Figure, // caption a blank image; we score wiring, not accuracy here
            image_path: img,
            truth: VisionTruth::Text("blank".into()),
        }];
        let scores = run_model_eval(&*vlm, &model, &fixtures, DEFAULT_SEEDS, 0.05);
        assert_eq!(scores.len(), 1, "one kind evaluated");
        let s = &scores[0];
        eprintln!(
            "real-VLM smoke: model={} kind={:?} n_seed={} value_f1={:.3}±{:.3} grounding_prec={:.3}",
            s.model, s.kind, s.n_seed, s.value_f1_mean, s.value_f1_std, s.grounding_precision_mean
        );
        assert_eq!(s.n_seed, 3);
    }

    /// REAL-VLM grounding + N=3 accuracy on a CONTENT-bearing fixture (not the blank smoke). Uses the
    /// committed `ocr_image/known_text.png` (white canvas, black "ATTUNE OCR TEST 2026") as a
    /// text-kind region: the real qwen multimodal model must (a) read the actual text — proving it
    /// does NOT over-reject true content — and (b) locate it with an in-crop `sub_bbox` — proving I1
    /// grounding works on real pixels, not just the mock lane. N=3 seeds → token-F1 mean ± std vs the
    /// known GT. Asserts a non-trivial floor (text-F1 ≥ 0.5: the model recovered most of the GT
    /// tokens) + grounding precision ≈ 1.0 (every kept value was located). Paid cloud call (§1.3).
    #[test]
    #[ignore = "real-VLM lane: needs DASHSCOPE_API_KEY + cost authorization (§1.3); run with --ignored"]
    fn real_vlm_grounding_n3_on_known_text() {
        let Some((vlm, model)) = qwen_vlm_from_env() else {
            eprintln!("real-VLM grounding PENDING-KEY: no DASHSCOPE_API_KEY/ATTUNE_VLM_API_KEY — skipping");
            return;
        };
        // Committed content fixture (CARGO_MANIFEST_DIR-relative): real text on a white canvas.
        let img = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ocr_image/known_text.png");
        assert!(
            img.exists(),
            "known_text.png fixture must be committed: {}",
            img.display()
        );
        let fixtures = vec![VisionFixture {
            id: "known-text-01".into(),
            kind: RegionKind::Handwriting, // text-transcription kind (token-F1 scored)
            image_path: img,
            truth: VisionTruth::Text("ATTUNE OCR TEST 2026".into()),
        }];
        let scores = run_model_eval(&*vlm, &model, &fixtures, DEFAULT_SEEDS, 0.05);
        assert_eq!(scores.len(), 1);
        let s = &scores[0];
        eprintln!(
            "real-VLM grounding/N=3: model={} kind={:?} n_seed={} text_f1={:.3}±{:.3} grounding_prec={:.3} (GT=\"ATTUNE OCR TEST 2026\")",
            s.model, s.kind, s.n_seed, s.value_f1_mean, s.value_f1_std, s.grounding_precision_mean
        );
        assert_eq!(s.n_seed, 3);
        // Accuracy floor: the model recovered most GT tokens (does not over-reject true content).
        assert!(
            s.value_f1_mean >= 0.5,
            "real VLM should read most of the known text (token-F1 ≥ 0.5); got {:.3}",
            s.value_f1_mean
        );
        // Grounding: every kept value carried an in-crop sub_bbox (I1 works on real pixels).
        assert!(
            s.grounding_precision_mean >= 0.99,
            "kept values must be grounded (precision ≈ 1.0); got {:.3}",
            s.grounding_precision_mean
        );
    }

    /// REAL multimodal FAILOVER (I3): a dead primary (always-erroring provider) must fail over to a
    /// LIVE qwen candidate on DashScope, and the live candidate produces a grounded result. Proves the
    /// router's provider-error → next-candidate path works against a REAL backend (the mock lane only
    /// proved it against scripted errors). Cost: exactly one real call (the dead primary errors
    /// without a network round-trip). Paid cloud call for the backup (§1.3).
    #[test]
    #[ignore = "real-VLM lane: needs DASHSCOPE_API_KEY + cost authorization (§1.3); run with --ignored"]
    fn real_vlm_failover_dead_primary_to_live_qwen() {
        use crate::ocr::nontext::vlm_router::{VlmCandidate, VlmRouter};
        use std::path::Path;

        let Some((live, model)) = qwen_vlm_from_env() else {
            eprintln!(
                "real-VLM failover PENDING-KEY: no DASHSCOPE_API_KEY/ATTUNE_VLM_API_KEY — skipping"
            );
            return;
        };

        // A dead primary: healthy probe (so it is TRIED, not skipped) but every call errors —
        // forcing the router down the provider-error failover path to the live qwen backup.
        struct DeadVlm;
        impl VlmProvider for DeadVlm {
            fn caption(&self, _: &Path) -> crate::error::Result<String> {
                Err(crate::error::VaultError::LlmUnavailable(
                    "dead primary".into(),
                ))
            }
            fn vqa(&self, _: &Path, _: &str) -> crate::error::Result<String> {
                Err(crate::error::VaultError::LlmUnavailable(
                    "dead primary".into(),
                ))
            }
            fn probe(&self) -> bool {
                true
            }
            fn vlm_model_name(&self) -> &str {
                "dead-primary"
            }
        }

        let router = VlmRouter::new(vec![
            VlmCandidate::new(std::sync::Arc::new(DeadVlm), "dead-primary", 9),
            VlmCandidate::new(live, model.clone(), 5),
        ]);

        // Mint a real gated token from the content fixture (the live qwen sees the downscaled crop).
        let img = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ocr_image/known_text.png");
        let token = mint_eval_token(&img).expect("egress token for known_text.png");
        let geom = geom_for_image(&img);
        let out = router.escalate(&token, RegionKind::Handwriting, geom);

        eprintln!(
            "real-VLM failover: tried={:?} winning_model={} failed_over={} result_ok={}",
            out.tried,
            out.winning_model(),
            out.failed_over(),
            out.result.is_ok()
        );
        assert!(
            out.failed_over(),
            "dead primary must trigger a real failover"
        );
        assert_eq!(out.tried.first().map(String::as_str), Some("dead-primary"));
        assert_eq!(out.winning_model(), model, "live qwen backup must win");
        assert!(
            out.result.is_ok(),
            "live backup must produce a parseable (grounded) result"
        );
        // The dead primary is recorded as a provider failure; the live backup is not.
        assert!(router.failure_rate(RegionKind::Handwriting, "dead-primary") > 0.0);
        assert_eq!(router.failure_rate(RegionKind::Handwriting, &model), 0.0);
    }
}
