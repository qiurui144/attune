//! Stage 4 — 💰 VLM escalation. Reuses the existing `VlmProvider` (vqa) with a
//! schema-guided JSON prompt + a retry-validate loop (≤3, spec §4.5 B) + failure
//! telemetry (§4.5 F).
//!
//! ## Egress is STRUCTURALLY (type-) enforced — not a doc-comment promise
//!
//! The ONLY way to call the VLM is [`escalate_region`], and it requires a
//! [`VlmEgressToken`] by value. A token can ONLY be minted by [`gate_vlm_egress`],
//! which (a) runs the [`OutboundGate`] on the redacted string descriptor AND
//! (b) takes an image-level refuse/allow decision on the ACTUAL bytes that leave
//! the device ([`ImageEgressDecision`]). There is no public `VlmEgressToken`
//! constructor, so a future wiring of Stage4 CANNOT call the VLM without first
//! passing the gate — the compiler refuses. See `egress_without_gate_is_impossible`
//! and the `tests` module for the proofs.

use super::{RegionKind, RegionResult, Series};
use crate::error::{Result, VaultError};
use crate::ocr::profile::VlmEscalationPolicy;
use crate::outbound_gate::{OutboundError, OutboundGate, OutboundKind, OutboundPolicy};
use crate::pii::Redactor;
use crate::vlm::VlmProvider;
use std::path::{Path, PathBuf};

/// Max retries when VLM JSON is invalid (spec §4.5 B).
pub const MAX_RETRIES: u32 = 3;

/// Default longest-edge (px) the gate downscales a region crop to before it leaves the
/// device (C3 image-level minimization). 1024 keeps chart/formula legibility while shedding
/// scanner-resolution detail (faces / fingerprints / handwriting strokes) the VLM never needs.
pub const EGRESS_MAX_EDGE: u32 = 1024;

/// Should this region escalate, given policy + the cross-validate decision + budget?
/// `discrepant` = the region was flagged conflict/discrepancy or low-confidence.
pub fn should_escalate(policy: VlmEscalationPolicy, discrepant: bool, used: u32, budget: u32) -> bool {
    if used >= budget {
        return false; // R2 budget cap (escalate_budget)
    }
    match policy {
        VlmEscalationPolicy::Off => false, // build-stage / 保守 → never (§8)
        VlmEscalationPolicy::OnDiscrepancy => discrepant,
        VlmEscalationPolicy::Aggressive => true,
    }
}

/// What the gate decided to do with the IMAGE bytes that actually leave the device (C3).
/// The privacy control acts on the bytes, not just the string descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageEgressDecision {
    /// Local-only / refused: the image must NOT leave. Caller degrades to local (R7).
    Refuse,
    /// May leave, after downscaling to a minimized crop written at this path.
    /// The minimized copy (`EGRESS_MAX_EDGE` longest edge) is what goes to the wire —
    /// never the full-resolution original.
    AllowDownscaled(PathBuf),
}

/// Reasons the image-level egress decision refused / failed.
#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    /// The string-descriptor outbound gate (disabled / vault-locked / redactor) refused.
    #[error(transparent)]
    Gate(#[from] OutboundError),
    /// Policy forbids the image leaving (local-only destination); degrade to local.
    #[error("image-egress-refused: policy forbids the region image leaving the device")]
    ImageRefused,
    /// Could not read / decode / write the crop for minimization → fail closed (don't leak).
    #[error("image-egress-io: {0}")]
    Io(String),
}

/// A capability proving the egress gate was passed for ONE region crop. It carries the
/// minimized image path that may go to the wire + the redacted string descriptor.
///
/// SECURITY (C2): there is **no public constructor**. The only way to obtain a token is
/// [`gate_vlm_egress`], which runs the real [`OutboundGate`] + image-level decision first.
/// [`escalate_region`] consumes a token by value, so VLM egress is type-enforced: code that
/// has not passed the gate has no token and therefore cannot call the VLM. A future wiring
/// of Stage4 is structurally UNABLE to bypass the gate.
///
/// The following does NOT compile — the private fields (`_gated: ()` in particular) make a
/// struct literal impossible outside this module, so there is no way to fabricate a token
/// and call `escalate_region` without going through `gate_vlm_egress`:
///
/// ```compile_fail
/// use attune_core::ocr::nontext::vlm_escalate::VlmEgressToken;
/// // error[E0451]: field `_gated` of struct `VlmEgressToken` is private
/// let _t = VlmEgressToken { egress_crop_path: "/x.png".into(), redacted_descriptor: String::new(), _gated: () };
/// ```
#[derive(Debug)]
pub struct VlmEgressToken {
    /// The minimized (downscaled) crop path that is actually sent — never the original.
    egress_crop_path: PathBuf,
    /// The redacted descriptor (PII stripped) that may accompany the image.
    redacted_descriptor: String,
    /// Private unit field forces construction through this module only (no struct literal
    /// outside `gate_vlm_egress`). Belt-and-suspenders with the private named fields.
    _gated: (),
}

impl VlmEgressToken {
    /// The minimized crop path that is sent to the VLM (downscaled per `EGRESS_MAX_EDGE`).
    pub fn egress_crop_path(&self) -> &Path {
        &self.egress_crop_path
    }
    /// The redacted string descriptor that accompanies the image.
    pub fn redacted_descriptor(&self) -> &str {
        &self.redacted_descriptor
    }
}

/// Enforce the outbound gate for a VLM (cloud) escalation call AND act on the IMAGE bytes
/// that actually leave the device (C2 type-enforcement + C3 image-level control).
///
/// Steps:
///   1. String-descriptor gate — the real [`OutboundGate`] (disabled / vault-locked /
///      redactor fail-closed). VLM egress is tracked under `OutboundKind::Llm`.
///   2. Image-level decision — `image_decision` (per-policy + OutboundGate) may `Refuse`
///      (local-only destination → degrade to local, R7) or allow a `AllowDownscaled` copy.
///      The privacy control acts on the BYTES: the original is downscaled to a minimized
///      crop and ONLY that minimized copy's path is placed in the token.
///
/// Returns a [`VlmEgressToken`] — the ONLY way to obtain one. `escalate_region` requires it.
///
/// NOTE(plan-recon 2026-06-10): the plan's Task 13 referenced OutboundPolicy fields
/// `local_destination` / `contains_l0` and an `OutboundError::L0CloudBlocked` variant that
/// do NOT exist in the real outbound_gate.rs API. The real `OutboundPolicy` is
/// { kind, enabled, vault_unlocked, redactor } and errors are { Disabled, VaultLocked,
/// RedactorRequired }. We adapt to the real API while preserving R7's intent: the caller
/// maps privacy settings to `enabled` (local-only sets enabled=false) and to the image-level
/// `image_decision`; supplies a Redactor so PII is stripped before the descriptor leaves.
pub fn gate_vlm_egress(
    enabled: bool,
    vault_unlocked: bool,
    redactor: Option<&Redactor>,
    crop_descriptor: &str,
    region_crop_path: &Path,
    image_decision: ImageEgressDecision,
) -> std::result::Result<VlmEgressToken, EgressError> {
    // (1) String-descriptor gate — fails closed on disabled / vault-locked / no-redactor.
    let policy = OutboundPolicy {
        kind: OutboundKind::Llm,
        enabled,
        vault_unlocked,
        redactor,
    };
    let redacted_descriptor = OutboundGate::enforce(&policy, crop_descriptor)?;

    // (2) Image-level control on the bytes that actually leave (C3).
    let egress_crop_path = match image_decision {
        ImageEgressDecision::Refuse => return Err(EgressError::ImageRefused),
        ImageEgressDecision::AllowDownscaled(dst) => {
            downscale_for_egress(region_crop_path, &dst, EGRESS_MAX_EDGE)?;
            dst
        }
    };

    Ok(VlmEgressToken {
        egress_crop_path,
        redacted_descriptor,
        _gated: (),
    })
}

/// Downscale the region crop to `max_edge` longest edge and write a minimized copy to `dst`.
/// This is the byte-level minimization hook (C3): the VLM only ever sees the downscaled copy,
/// shedding scanner-resolution detail. Fails closed (no leak) on any IO/decode error.
fn downscale_for_egress(
    src: &Path,
    dst: &Path,
    max_edge: u32,
) -> std::result::Result<(), EgressError> {
    let img = image::open(src).map_err(|e| EgressError::Io(format!("open {}: {e}", src.display())))?;
    let (w, h) = (img.width(), img.height());
    let minimized = if w.max(h) > max_edge {
        // Preserves aspect ratio; `thumbnail` is fast nearest-ish and good enough for a VLM.
        img.thumbnail(max_edge, max_edge)
    } else {
        img
    };
    minimized
        .save(dst)
        .map_err(|e| EgressError::Io(format!("save {}: {e}", dst.display())))?;
    Ok(())
}

/// One telemetry record per VLM call attempt (spec §7 / §4.5 F).
#[derive(Debug, Clone, PartialEq)]
pub struct VlmTelemetry {
    pub region_kind: RegionKind,
    pub vlm_model: String,
    pub error_kind: Option<String>, // None = success; "parse"/"provider"
    pub retry_count: u32,
}

/// Build the schema-guided VQA question for a region kind (spec §3.2 / §4.5 A).
pub fn escalation_prompt(kind: RegionKind) -> &'static str {
    match kind {
        RegionKind::Chart => "Extract the chart as JSON: {\"chart_type\":\"bar|line|pie\",\"series\":[{\"name\":\"...\",\"values\":[..]}],\"axis_labels\":[..]}. Reply with ONLY the JSON object.",
        RegionKind::Formula => "Transcribe the formula to LaTeX as JSON: {\"latex\":\"...\"}. Reply with ONLY the JSON object.",
        RegionKind::Handwriting => "Transcribe the handwriting as JSON: {\"text\":\"...\"}. Reply with ONLY the JSON object.",
        RegionKind::Figure => "Caption the figure as JSON: {\"caption\":\"...\"}. Reply with ONLY the JSON object.",
        RegionKind::Stamp => "Read the stamp as JSON: {\"text\":\"...\",\"stamp_type\":\"...\"}. Reply with ONLY the JSON object.",
        _ => "Describe this region as JSON: {\"text\":\"...\"}. Reply with ONLY the JSON object.",
    }
}

/// Validate + parse the VLM JSON answer into a RegionResult. Err on invalid JSON
/// or missing required field (drives the retry loop). Never fabricates.
pub fn parse_vlm_answer(kind: RegionKind, answer: &str) -> Result<RegionResult> {
    let json = extract_json(answer)
        .ok_or_else(|| VaultError::Io(std::io::Error::other("vlm-parse-fail: no json object")))?;
    let v: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| VaultError::Io(std::io::Error::other(format!("vlm-parse-fail: {e}"))))?;
    Ok(match kind {
        RegionKind::Formula => RegionResult::FormulaV1 {
            latex: v.get("latex").and_then(|x| x.as_str()).map(str::to_string),
            raw_ocr: None,
        },
        RegionKind::Handwriting => RegionResult::HandwritingV1 {
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
        },
        RegionKind::Figure => RegionResult::FigureV1 {
            class: "figure".into(),
            caption: v.get("caption").and_then(|x| x.as_str()).map(str::to_string),
        },
        RegionKind::Chart => RegionResult::ChartV1 {
            chart_type: v
                .get("chart_type")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string(),
            series: v.get("series").and_then(parse_series).unwrap_or_default(),
            axis_labels: v
                .get("axis_labels")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        },
        RegionKind::Stamp => RegionResult::StampV1 {
            present: true,
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
            stamp_type: v
                .get("stamp_type")
                .and_then(|x| x.as_str())
                .map(str::to_string),
        },
        _ => RegionResult::UnrecognizedV1 {
            reason: "vlm-unsupported-kind".into(),
        },
    })
}

fn parse_series(v: &serde_json::Value) -> Option<Vec<Series>> {
    Some(
        v.as_array()?
            .iter()
            .filter_map(|s| {
                Some(Series {
                    name: s.get("name")?.as_str()?.to_string(),
                    values: s
                        .get("values")?
                        .as_array()?
                        .iter()
                        .filter_map(|x| x.as_f64())
                        .collect(),
                })
            })
            .collect(),
    )
}

/// Extract the first {...} JSON object from a possibly-chatty answer.
fn extract_json(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(s[start..=end].to_string())
    } else {
        None
    }
}

/// Escalate one region to VLM. Retries up to MAX_RETRIES on parse failure, feeding the
/// validator error back into the question (spec §4.5 B). Returns (result, telemetry).
///
/// SECURITY (C2): requires a [`VlmEgressToken`] by value — the ONLY way to obtain one is
/// [`gate_vlm_egress`]. This makes VLM egress type-enforced: no token ⇒ this function is
/// uncallable, so the gate cannot be bypassed. The VLM only ever sees the token's minimized
/// (downscaled, PII-considered) crop path — never a caller-supplied raw original.
pub fn escalate_region(
    vlm: &dyn VlmProvider,
    token: VlmEgressToken,
    kind: RegionKind,
    model_name: &str,
) -> (Result<RegionResult>, VlmTelemetry) {
    // The bytes that leave the device are the gate's minimized copy, not a raw original.
    let region_crop_path = token.egress_crop_path().to_path_buf();
    let base = escalation_prompt(kind);
    let mut last_err = String::new();
    for attempt in 0..MAX_RETRIES {
        let q = if attempt == 0 {
            base.to_string()
        } else {
            format!("{base}\nPrevious answer was invalid: {last_err}. Return ONLY valid JSON.")
        };
        match vlm.vqa(&region_crop_path, &q) {
            Ok(answer) => match parse_vlm_answer(kind, &answer) {
                Ok(r) => {
                    return (
                        Ok(r),
                        VlmTelemetry {
                            region_kind: kind,
                            vlm_model: model_name.into(),
                            error_kind: None,
                            retry_count: attempt,
                        },
                    )
                }
                Err(e) => last_err = e.to_string(),
            },
            Err(e) => {
                return (
                    Err(e),
                    VlmTelemetry {
                        region_kind: kind,
                        vlm_model: model_name.into(),
                        error_kind: Some("provider".into()),
                        retry_count: attempt,
                    },
                )
            }
        }
    }
    (
        Err(VaultError::Io(std::io::Error::other(
            "vlm-parse-fail after retries",
        ))),
        VlmTelemetry {
            region_kind: kind,
            vlm_model: model_name.into(),
            error_kind: Some("parse".into()),
            retry_count: MAX_RETRIES,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vlm::VlmProvider;
    use std::sync::Mutex;

    /// VLM mock that returns scripted answers in order (to drive retry).
    /// Uses Mutex (not RefCell) because VlmProvider requires Send + Sync.
    struct ScriptVlm {
        answers: Mutex<Vec<String>>,
    }
    impl VlmProvider for ScriptVlm {
        fn caption(&self, _: &Path) -> Result<String> {
            Ok("x".into())
        }
        fn vqa(&self, _: &Path, _: &str) -> Result<String> {
            Ok(self.answers.lock().unwrap().pop().unwrap_or_default())
        }
    }
    // ScriptVlm pops from the end → push in reverse order of expected calls.
    fn script(a: &[&str]) -> ScriptVlm {
        ScriptVlm {
            answers: Mutex::new(a.iter().rev().map(|s| s.to_string()).collect()),
        }
    }

    /// Write a tiny real PNG so the gate's downscale step has bytes to act on.
    fn write_test_png(dir: &std::path::Path, name: &str, edge: u32) -> PathBuf {
        let p = dir.join(name);
        image::DynamicImage::new_rgb8(edge, edge).save(&p).unwrap();
        p
    }

    /// Mint a token the ONLY legal way — through the gate. Used to drive escalate_region tests.
    fn gated_token(dir: &std::path::Path) -> VlmEgressToken {
        let redactor = Redactor::new();
        let src = write_test_png(dir, "src.png", 8);
        let dst = dir.join("egress.png");
        gate_vlm_egress(
            true,
            true,
            Some(&redactor),
            "region-crop",
            &src,
            ImageEgressDecision::AllowDownscaled(dst),
        )
        .expect("gate should mint a token under allow policy")
    }

    #[test]
    fn parse_valid_formula_json() {
        let r = parse_vlm_answer(RegionKind::Formula, r#"{"latex":"E=mc^2"}"#).unwrap();
        assert!(matches!(r, RegionResult::FormulaV1 { latex: Some(_), .. }));
    }
    #[test]
    fn parse_chatty_answer_extracts_json() {
        let r = parse_vlm_answer(RegionKind::Handwriting, r#"Sure! {"text":"hello"} done"#).unwrap();
        match r {
            RegionResult::HandwritingV1 { text } => assert_eq!(text.as_deref(), Some("hello")),
            _ => panic!(),
        }
    }
    #[test]
    fn invalid_json_errs() {
        assert!(parse_vlm_answer(RegionKind::Formula, "not json").is_err());
    }
    #[test]
    fn first_try_success_zero_retries() {
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&[r#"{"text":"ok"}"#]);
        let (res, tel) =
            escalate_region(&vlm, gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl");
        assert!(res.is_ok());
        assert_eq!(tel.retry_count, 0);
        assert_eq!(tel.error_kind, None);
    }
    #[test]
    fn retries_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&["garbage", r#"{"text":"ok"}"#]);
        let (res, tel) =
            escalate_region(&vlm, gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl");
        assert!(res.is_ok());
        assert_eq!(tel.retry_count, 1);
    }
    #[test]
    fn three_failures_gives_parse_fail_telemetry() {
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&["bad1", "bad2", "bad3", "bad4"]);
        let (res, tel) =
            escalate_region(&vlm, gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl");
        assert!(res.is_err());
        assert_eq!(tel.retry_count, MAX_RETRIES);
        assert_eq!(tel.error_kind.as_deref(), Some("parse"));
    }

    /// C2 proof — type-enforcement. There is NO public way to build a `VlmEgressToken`
    /// other than `gate_vlm_egress`; `escalate_region` consumes one by value. This test
    /// documents (and the compiler enforces) that the gate is the only egress path.
    /// The accompanying `egress_without_gate_is_impossible` doc-test confirms the negative.
    #[test]
    fn escalate_only_with_gated_token_and_sends_minimized_crop() {
        let dir = tempfile::tempdir().unwrap();
        let token = gated_token(dir.path());
        // The egress crop is the gate's downscaled copy, written under our temp dir —
        // NOT some caller-supplied raw original outside the gate.
        assert!(token.egress_crop_path().starts_with(dir.path()));
        assert!(token.egress_crop_path().exists(), "minimized crop must be materialized");
        let vlm = script(&[r#"{"text":"ok"}"#]);
        let (res, _) = escalate_region(&vlm, token, RegionKind::Handwriting, "qwen-vl");
        assert!(res.is_ok());
    }

    #[test]
    fn off_policy_never_escalates() {
        assert!(!should_escalate(VlmEscalationPolicy::Off, true, 0, 100));
    }
    #[test]
    fn on_discrepancy_only_when_flagged() {
        assert!(should_escalate(VlmEscalationPolicy::OnDiscrepancy, true, 0, 8));
        assert!(!should_escalate(VlmEscalationPolicy::OnDiscrepancy, false, 0, 8));
    }
    #[test]
    fn budget_cap_blocks_when_exhausted() {
        assert!(!should_escalate(VlmEscalationPolicy::Aggressive, true, 8, 8));
        assert!(should_escalate(VlmEscalationPolicy::Aggressive, true, 7, 8));
    }

    // gate_vlm_egress tests use the REAL OutboundGate API (Disabled / VaultLocked /
    // RedactorRequired), not the plan's hypothetical L0CloudBlocked. A local-only /
    // 保守-governor destination is modeled by enabled=false (refuse cloud egress).
    #[test]
    fn gate_blocks_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        // enabled=false models a local-only / privacy-off destination → refuse cloud egress.
        let err = gate_vlm_egress(
            false, true, None, "region-crop", &src,
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::Gate(OutboundError::Disabled(OutboundKind::Llm))));
    }
    #[test]
    fn gate_blocks_when_vault_locked() {
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let err = gate_vlm_egress(
            true, false, None, "region-crop", &src,
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::Gate(OutboundError::VaultLocked)));
    }
    #[test]
    fn gate_fails_closed_without_redactor_on_nonempty() {
        // R7 / MEMORY P0: gate is non-no-op — a non-empty payload with no redactor fails closed.
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let err = gate_vlm_egress(
            true, true, None, "sensitive crop descriptor", &src,
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::Gate(OutboundError::RedactorRequired)));
    }
    #[test]
    fn gate_allows_with_redactor_and_redacts() {
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let redactor = Redactor::new();
        // PII in the descriptor is stripped before it leaves the device.
        let tok = gate_vlm_egress(
            true, true, Some(&redactor), "联系电话 13800138000", &src,
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap();
        assert!(
            !tok.redacted_descriptor().contains("13800138000"),
            "phone must be redacted; got {}",
            tok.redacted_descriptor()
        );
    }

    // ---- C3: image-level egress control acts on the actual BYTES ----

    #[test]
    fn image_refuse_decision_blocks_egress() {
        // A local-only destination refuses the IMAGE (not just the descriptor) → degrade local.
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let redactor = Redactor::new();
        let err = gate_vlm_egress(
            true, true, Some(&redactor), "region-crop", &src,
            ImageEgressDecision::Refuse,
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::ImageRefused));
    }

    #[test]
    fn allowed_image_is_downscaled_below_max_edge() {
        // The bytes that leave are a MINIMIZED copy: a 4000px crop must shrink to ≤ EGRESS_MAX_EDGE.
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "big.png", 4000);
        let dst = dir.path().join("egress.png");
        let redactor = Redactor::new();
        let tok = gate_vlm_egress(
            true, true, Some(&redactor), "region-crop", &src,
            ImageEgressDecision::AllowDownscaled(dst.clone()),
        )
        .unwrap();
        assert_eq!(tok.egress_crop_path(), dst.as_path());
        let leaving = image::open(tok.egress_crop_path()).unwrap();
        assert!(
            leaving.width().max(leaving.height()) <= EGRESS_MAX_EDGE,
            "egress crop must be downscaled to ≤ {EGRESS_MAX_EDGE}px; got {}x{}",
            leaving.width(),
            leaving.height()
        );
        // The original full-resolution file is untouched (still 4000px) and was NOT what left.
        assert_eq!(image::open(&src).unwrap().width(), 4000);
    }

    #[test]
    fn downscale_missing_source_fails_closed() {
        // Can't read the crop → fail closed (EgressError::Io), never emit an ungated token.
        let dir = tempfile::tempdir().unwrap();
        let redactor = Redactor::new();
        let err = gate_vlm_egress(
            true, true, Some(&redactor), "region-crop",
            std::path::Path::new("/nonexistent/crop.png"),
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::Io(_)));
    }
}
