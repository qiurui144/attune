//! Stage 4 — 💰 VLM escalation. Reuses the existing `VlmProvider` (vqa) with a
//! schema-guided JSON prompt + a retry-validate loop (≤3, spec §4.5 B) + failure
//! telemetry (§4.5 F).
//!
//! ## Egress is STRUCTURALLY (type-) enforced — not a doc-comment promise
//!
//! The ONLY way to call the VLM is [`escalate_region`], and it requires a
//! [`VlmEgressToken`] (borrowed, so one gated token can be reused across failover
//! candidates by [`super::vlm_router::VlmRouter`] without re-gating). A token can ONLY
//! be minted by [`gate_vlm_egress`],
//! which (a) runs the [`OutboundGate`] on the redacted string descriptor AND
//! (b) takes an image-level refuse/allow decision on the ACTUAL bytes that leave
//! the device ([`ImageEgressDecision`]). There is no public `VlmEgressToken`
//! constructor, so a future wiring of Stage4 CANNOT call the VLM without first
//! passing the gate — the compiler refuses. See `egress_without_gate_is_impossible`
//! and the `tests` module for the proofs.

use super::grounding::{validate_grounding, GroundingFail, GroundingRef};
use super::{RegionKind, RegionResult, Series};
use crate::error::{Result, VaultError};
use crate::ocr::profile::VlmEscalationPolicy;
use crate::ocr::BBox;
use crate::outbound_gate::{OutboundError, OutboundGate, OutboundKind, OutboundPolicy};
use crate::pii::Redactor;
use crate::vlm::VlmProvider;
use std::path::{Path, PathBuf};

/// Max retries when VLM JSON is invalid (spec §4.5 B).
pub const MAX_RETRIES: u32 = 3;

/// Test-only: mint a real (gated) [`VlmEgressToken`] for use by other modules' unit tests
/// (notably `vlm_router`). It goes through the real [`gate_vlm_egress`] under an allow policy, so the
/// type-enforcement invariant is preserved even in tests (no fabricated tokens). The downscaled crop
/// is written under a temp dir that is intentionally leaked so the path stays valid for the duration
/// of the test (the tiny PNG is cleaned by the OS temp reaper; tests never read its bytes via a
/// scripted provider).
#[cfg(test)]
pub(crate) fn test_egress_token() -> VlmEgressToken {
    let dir = Box::leak(Box::new(tempfile::tempdir().expect("tempdir"))).path();
    let redactor = Redactor::new();
    let src = dir.join("src.png");
    image::DynamicImage::new_rgb8(8, 8).save(&src).expect("png");
    let dst = dir.join("egress.png");
    gate_vlm_egress(
        true,
        true,
        false,
        Some(&redactor),
        "region-crop",
        &src,
        ImageEgressDecision::AllowDownscaled(dst),
    )
    .expect("gate should mint a token under allow policy")
}

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
/// ## L0-egress integration (C3 image-egress ⨯ develop's L0-egress gate)
///
/// The VLM is a **cloud** destination, so the policy passes `local_destination: false`. The
/// `source_contains_l0` flag threads the source document's privacy tier into the gate: when the
/// region/source carries `PrivacyTier::L0` ("🔒 永不出网") content, the gate refuses the call with
/// [`OutboundError::L0CloudBlocked`] (the cloud VLM must NEVER see L0 content — even redacted /
/// downscaled). The recognize pipeline is responsible for passing the document's real tier; until
/// the tier is plumbed to this call site, callers MUST pass `true` so the gate **fails closed**
/// (refuse on unknown) rather than silently leaking. The invariant "L0 content never reaches the
/// cloud VLM" holds for every caller, known-tier or not.
pub fn gate_vlm_egress(
    enabled: bool,
    vault_unlocked: bool,
    source_contains_l0: bool,
    redactor: Option<&Redactor>,
    crop_descriptor: &str,
    region_crop_path: &Path,
    image_decision: ImageEgressDecision,
) -> std::result::Result<VlmEgressToken, EgressError> {
    // (1) String-descriptor gate — fails closed on disabled / vault-locked / L0-to-cloud /
    //     no-redactor. The VLM is a cloud endpoint (`local_destination: false`), so any L0
    //     content (`source_contains_l0`) is refused with OutboundError::L0CloudBlocked.
    let policy = OutboundPolicy {
        kind: OutboundKind::Llm,
        enabled,
        vault_unlocked,
        redactor,
        local_destination: false,
        contains_l0: source_contains_l0,
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
///
/// I1 grounding (spec §E2): the prompt asks the VLM to locate each extracted value with a
/// `grounding.sub_bbox` in CROP-RELATIVE pixel coordinates (top-left origin of the region crop it is
/// shown). The caller stamps the authoritative `region_bbox`/`page` (it knows the page coords; the
/// VLM, seeing only the crop, cannot) and validates the sub_bbox lands inside the crop. A value the
/// VLM cannot locate must be omitted — never invent a value or a location.
pub fn escalation_prompt(kind: RegionKind) -> &'static str {
    match kind {
        RegionKind::Chart => "Extract the chart as JSON: {\"chart_type\":\"bar|line|pie\",\"series\":[{\"name\":\"...\",\"values\":[..],\"grounding\":{\"sub_bbox\":{\"x\":int,\"y\":int,\"w\":int,\"h\":int}}}],\"axis_labels\":[..]}. The sub_bbox locates each series in the image (top-left origin pixels). Only report a series you can locate. Reply with ONLY the JSON object.",
        RegionKind::Formula => "Transcribe the formula to LaTeX as JSON: {\"latex\":\"...\",\"grounding\":{\"sub_bbox\":{\"x\":int,\"y\":int,\"w\":int,\"h\":int}}}. The sub_bbox locates the formula in the image (top-left origin pixels). Reply with ONLY the JSON object.",
        RegionKind::Handwriting => "Transcribe the handwriting as JSON: {\"text\":\"...\",\"grounding\":{\"sub_bbox\":{\"x\":int,\"y\":int,\"w\":int,\"h\":int}}}. The sub_bbox locates the handwriting in the image (top-left origin pixels). Reply with ONLY the JSON object.",
        RegionKind::Figure => "Caption the figure as JSON: {\"caption\":\"...\",\"grounding\":{\"sub_bbox\":{\"x\":int,\"y\":int,\"w\":int,\"h\":int}}}. The sub_bbox locates the captioned subject (top-left origin pixels). Reply with ONLY the JSON object.",
        RegionKind::Stamp => "Read the stamp as JSON: {\"text\":\"...\",\"stamp_type\":\"...\",\"grounding\":{\"sub_bbox\":{\"x\":int,\"y\":int,\"w\":int,\"h\":int}}}. The sub_bbox locates the stamp in the image (top-left origin pixels). Reply with ONLY the JSON object.",
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
            grounding: parse_grounding(&v),
        },
        RegionKind::Handwriting => RegionResult::HandwritingV1 {
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
            grounding: parse_grounding(&v),
        },
        RegionKind::Figure => RegionResult::FigureV1 {
            class: "figure".into(),
            caption: v.get("caption").and_then(|x| x.as_str()).map(str::to_string),
            grounding: parse_grounding(&v),
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
            grounding: parse_grounding(&v),
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
                    // I1: per-series grounding, if the VLM provided a source location for this series.
                    grounding: parse_grounding(s),
                })
            })
            .collect(),
    )
}

/// I1: parse an optional `grounding` object out of a VLM JSON value (region/series level). The VLM
/// is asked (via the escalation prompt) to locate each extracted value with a crop-relative
/// `sub_bbox`; if it does we capture the ref so the caller can stamp the authoritative region_bbox
/// and [`ground_region`] can validate the sub_bbox lands inside the crop. A `grounding` object with
/// NO usable location (no `sub_bbox`) → `None` (the validator then flags `Missing` and drives a
/// retry, spec §E2). `region_bbox`/`page` in the JSON are ignored: the VLM sees a crop and cannot
/// know absolute page coords; the caller ([`stamp_grounding`]) overwrites them.
fn parse_grounding(v: &serde_json::Value) -> Option<GroundingRef> {
    let g = v.get("grounding")?;
    let bbox = |key: &str| -> Option<BBox> {
        let b = g.get(key)?;
        Some(BBox {
            x: b.get("x")?.as_u64()? as u32,
            y: b.get("y")?.as_u64()? as u32,
            w: b.get("w")?.as_u64()? as u32,
            h: b.get("h")?.as_u64()? as u32,
        })
    };
    // A grounding object is only useful if it carries a sub_bbox (the crop-relative location). The
    // region_bbox is a placeholder here; stamp_grounding overwrites it with the crop-origin region.
    let sub_bbox = bbox("sub_bbox")?;
    Some(GroundingRef {
        region_bbox: BBox { x: 0, y: 0, w: 0, h: 0 }, // placeholder, overwritten by stamp_grounding
        page: 0,
        sub_bbox: Some(sub_bbox),
        ocr_line_ref: g
            .get("ocr_line_ref")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

/// I1 grounding check (spec §E2): every extracted value in `result` must carry a [`GroundingRef`]
/// that lands inside `region` on `page`. Returns the first [`GroundingFail`] found (so the retry
/// loop can feed a specific reason back to the VLM), or `Ok(())` when all values are grounded.
///
/// Only VLM-extractable, value-bearing variants are checked. A `ChartV1` with an EMPTY series list
/// has no values to ground (so it cannot fabricate an ungrounded value) → vacuously grounded; a
/// chart WITH series requires each series to be grounded. Variants that carry no extracted content
/// (`UnrecognizedV1`, `CheckboxV1`, `SignatureV1`, `TableV1` — TableV1 cell grounding is a later
/// increment, spec §5.3 marks it `Option`) are not grounding-gated here.
pub fn ground_region(result: &RegionResult, region: &BBox, page: u32) -> std::result::Result<(), GroundingFail> {
    match result {
        RegionResult::ChartV1 { series, .. } => {
            for s in series {
                validate_grounding(s.grounding.as_ref(), region, page)?;
            }
            Ok(())
        }
        RegionResult::FormulaV1 { latex, grounding, .. } => {
            // Only require grounding when a value was actually extracted (latex present).
            if latex.is_some() {
                validate_grounding(grounding.as_ref(), region, page)?;
            }
            Ok(())
        }
        RegionResult::HandwritingV1 { text, grounding } => {
            if text.is_some() {
                validate_grounding(grounding.as_ref(), region, page)?;
            }
            Ok(())
        }
        RegionResult::FigureV1 { caption, grounding, .. } => {
            if caption.is_some() {
                validate_grounding(grounding.as_ref(), region, page)?;
            }
            Ok(())
        }
        RegionResult::StampV1 { text, grounding, .. } => {
            if text.is_some() {
                validate_grounding(grounding.as_ref(), region, page)?;
            }
            Ok(())
        }
        // No extracted free value to ground (or grounding is a later increment).
        _ => Ok(()),
    }
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

/// Geometry of the region being escalated, used to ground the VLM's extracted values (I1).
///
/// The VLM is shown only the region CROP (top-left origin), so it returns `sub_bbox` in crop-relative
/// pixels. The caller stamps the authoritative `page_bbox`/`page` (the absolute page coords it knows
/// but the VLM can't) and validates each `sub_bbox` lands inside the crop (`crop_w`×`crop_h`).
#[derive(Debug, Clone, Copy)]
pub struct RegionGeom {
    /// The region's absolute bbox on the page (stamped into each GroundingRef).
    pub page_bbox: BBox,
    /// The page index.
    pub page: u32,
    /// Width of the crop the VLM was shown (the coordinate system its sub_bbox uses).
    pub crop_w: u32,
    /// Height of the crop the VLM was shown.
    pub crop_h: u32,
}

impl RegionGeom {
    /// The crop-origin region used to validate the VLM's crop-relative sub_bbox.
    fn crop_region(&self) -> BBox {
        BBox { x: 0, y: 0, w: self.crop_w, h: self.crop_h }
    }
}

/// Stamp the authoritative `region_bbox`/`page` into every GroundingRef the VLM returned, keeping
/// the VLM-supplied `sub_bbox`/`ocr_line_ref`. The VLM cannot know absolute page coords (it sees a
/// crop), so we OVERWRITE whatever `region_bbox` it guessed with the crop-origin region — then
/// [`ground_region`] validates the sub_bbox lies inside it. (Validation runs in crop coords; the
/// page_bbox is what downstream UI uses to map back to the original page.)
fn stamp_grounding(result: &mut RegionResult, geom: &RegionGeom) {
    let crop = geom.crop_region();
    let restamp = |g: &mut Option<GroundingRef>| {
        if let Some(gr) = g {
            gr.region_bbox = crop;
            gr.page = geom.page;
        }
    };
    match result {
        RegionResult::ChartV1 { series, .. } => series.iter_mut().for_each(|s| restamp(&mut s.grounding)),
        RegionResult::FormulaV1 { grounding, .. } => restamp(grounding),
        RegionResult::HandwritingV1 { grounding, .. } => restamp(grounding),
        RegionResult::FigureV1 { grounding, .. } => restamp(grounding),
        RegionResult::StampV1 { grounding, .. } => restamp(grounding),
        _ => {}
    }
}

/// Rewrite the page_bbox of every grounding ref to the region's absolute page bbox, AFTER grounding
/// validation passed in crop coords. Downstream consumers (UI highlight, OCR-line jump) need the
/// absolute page rectangle, not the crop-origin one used for validation.
fn finalize_grounding_page_coords(result: &mut RegionResult, geom: &RegionGeom) {
    let set = |g: &mut Option<GroundingRef>| {
        if let Some(gr) = g {
            gr.region_bbox = geom.page_bbox;
        }
    };
    match result {
        RegionResult::ChartV1 { series, .. } => series.iter_mut().for_each(|s| set(&mut s.grounding)),
        RegionResult::FormulaV1 { grounding, .. } => set(grounding),
        RegionResult::HandwritingV1 { grounding, .. } => set(grounding),
        RegionResult::FigureV1 { grounding, .. } => set(grounding),
        RegionResult::StampV1 { grounding, .. } => set(grounding),
        _ => {}
    }
}

/// The telemetry `error_kind` value set when a value is KEPT-but-ungrounded after the retry budget
/// is spent (spec §7). The Stage4 caller maps this to a `validation_warnings: ["ungrounded"]` on the
/// Region (validation_warnings lives on Region, not RegionResult), so the value is surfaced as
/// "could not be located back to source" — never dropped, never fabricated.
pub const UNGROUNDED_ERROR_KIND: &str = "grounding";

/// Escalate one region to VLM. Retries up to MAX_RETRIES feeding the validator error (parse OR
/// I1 grounding_fail) back into the question (spec §4.5 B / §E2). Returns (result, telemetry).
///
/// I1: after a successful parse, the extracted values' grounding is stamped + validated against the
/// crop. A `grounding_fail` (missing/out-of-bounds/empty source location) is fed back to the VLM
/// like a parse error and retried; after MAX_RETRIES the value is KEPT but the telemetry error_kind
/// is `grounding` and `ungrounded` is set so the caller flags it (never drop, never fabricate, §7).
///
/// SECURITY (C2): requires a [`VlmEgressToken`] — the ONLY way to obtain one is
/// [`gate_vlm_egress`]. The token is borrowed (`&`) so the [`super::vlm_router::VlmRouter`] can hand
/// the SAME gated bytes to each failover candidate without re-gating (I3); ownership of a token still
/// cannot be forged (no public constructor), so VLM egress stays type-enforced: no token ⇒ this
/// function is uncallable, so the gate cannot be bypassed. The VLM only ever sees the token's
/// minimized (downscaled, PII-considered) crop path — never a caller-supplied raw original.
pub fn escalate_region(
    vlm: &dyn VlmProvider,
    token: &VlmEgressToken,
    kind: RegionKind,
    model_name: &str,
    geom: RegionGeom,
) -> (Result<RegionResult>, VlmTelemetry) {
    // The bytes that leave the device are the gate's minimized copy, not a raw original.
    let region_crop_path = token.egress_crop_path().to_path_buf();
    let base = escalation_prompt(kind);
    let crop = geom.crop_region();
    let mut last_err = String::new();
    // Holds the most recent successfully-parsed-but-ungrounded result, so after the retry budget is
    // spent we can KEEP the value (flagged ungrounded) rather than dropping it (spec §7).
    let mut last_ungrounded: Option<(RegionResult, GroundingFail)> = None;
    for attempt in 0..MAX_RETRIES {
        let q = if attempt == 0 {
            base.to_string()
        } else {
            format!("{base}\nPrevious answer was invalid: {last_err}. Return ONLY valid JSON.")
        };
        match vlm.vqa(&region_crop_path, &q) {
            Ok(answer) => match parse_vlm_answer(kind, &answer) {
                Ok(mut r) => {
                    stamp_grounding(&mut r, &geom);
                    match ground_region(&r, &crop, geom.page) {
                        Ok(()) => {
                            // Grounded: finalize page coords for downstream consumers + return.
                            finalize_grounding_page_coords(&mut r, &geom);
                            return (
                                Ok(r),
                                VlmTelemetry {
                                    region_kind: kind,
                                    vlm_model: model_name.into(),
                                    error_kind: None,
                                    retry_count: attempt,
                                },
                            );
                        }
                        Err(gf) => {
                            // I1: grounding failed — feed the specific reason back and retry.
                            last_err = gf.as_reason().to_string();
                            last_ungrounded = Some((r, gf));
                        }
                    }
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
    // Retry budget spent. If we have a parsed-but-ungrounded value, KEEP it (spec §7: never drop /
    // fabricate); telemetry error_kind = grounding so the failure is visible.
    if let Some((mut r, _gf)) = last_ungrounded {
        finalize_grounding_page_coords(&mut r, &geom);
        return (
            Ok(r),
            VlmTelemetry {
                region_kind: kind,
                vlm_model: model_name.into(),
                error_kind: Some(UNGROUNDED_ERROR_KIND.into()),
                retry_count: MAX_RETRIES,
            },
        );
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

    /// The crop the gate produces from an 8×8 src is 8×8 (≤ EGRESS_MAX_EDGE → not downscaled), so a
    /// grounding sub_bbox inside 8×8 is valid. Geom for these tests: tiny page bbox + 8×8 crop.
    fn test_geom() -> RegionGeom {
        RegionGeom {
            page_bbox: BBox { x: 100, y: 200, w: 8, h: 8 },
            page: 0,
            crop_w: 8,
            crop_h: 8,
        }
    }

    /// A grounded handwriting answer: text + a sub_bbox inside the 8×8 crop. region_bbox/page in the
    /// JSON are ignored (the caller stamps the authoritative crop region), so we omit them.
    const GROUNDED_HW: &str = r#"{"text":"ok","grounding":{"sub_bbox":{"x":1,"y":1,"w":3,"h":3}}}"#;

    /// Write a tiny real PNG so the gate's downscale step has bytes to act on.
    fn write_test_png(dir: &std::path::Path, name: &str, edge: u32) -> PathBuf {
        let p = dir.join(name);
        image::DynamicImage::new_rgb8(edge, edge).save(&p).unwrap();
        p
    }

    /// Mint a token the ONLY legal way — through the gate. Used to drive escalate_region tests.
    /// Non-L0 source (`source_contains_l0=false`) so a cloud VLM call is allowed.
    fn gated_token(dir: &std::path::Path) -> VlmEgressToken {
        let redactor = Redactor::new();
        let src = write_test_png(dir, "src.png", 8);
        let dst = dir.join("egress.png");
        gate_vlm_egress(
            true,
            true,
            false, // source_contains_l0: non-L0 region → cloud egress permitted
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
            RegionResult::HandwritingV1 { text, .. } => assert_eq!(text.as_deref(), Some("hello")),
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
        let vlm = script(&[GROUNDED_HW]);
        let (res, tel) =
            escalate_region(&vlm, &gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl", test_geom());
        assert!(res.is_ok());
        assert_eq!(tel.retry_count, 0);
        assert_eq!(tel.error_kind, None);
    }
    #[test]
    fn retries_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&["garbage", GROUNDED_HW]);
        let (res, tel) =
            escalate_region(&vlm, &gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl", test_geom());
        assert!(res.is_ok());
        assert_eq!(tel.retry_count, 1);
    }
    #[test]
    fn three_failures_gives_parse_fail_telemetry() {
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&["bad1", "bad2", "bad3", "bad4"]);
        let (res, tel) =
            escalate_region(&vlm, &gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl", test_geom());
        assert!(res.is_err());
        assert_eq!(tel.retry_count, MAX_RETRIES);
        assert_eq!(tel.error_kind.as_deref(), Some("parse"));
    }

    // ── I1 grounding integration (spec §E2 / §7) ──────────────────────────────

    #[test]
    fn grounded_answer_succeeds_and_stamps_page_coords() {
        // A value with a valid in-crop sub_bbox is grounded; the returned ref carries the
        // authoritative PAGE bbox (not the crop origin) for downstream UI highlight.
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&[GROUNDED_HW]);
        let (res, tel) =
            escalate_region(&vlm, &gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl", test_geom());
        let r = res.unwrap();
        assert_eq!(tel.error_kind, None);
        match r {
            RegionResult::HandwritingV1 { text, grounding } => {
                assert_eq!(text.as_deref(), Some("ok"));
                let g = grounding.expect("grounded value must carry a GroundingRef");
                // Finalized to the absolute page bbox the caller supplied (geom.page_bbox).
                assert_eq!(g.region_bbox, BBox { x: 100, y: 200, w: 8, h: 8 });
                assert_eq!(g.sub_bbox, Some(BBox { x: 1, y: 1, w: 3, h: 3 }));
            }
            _ => panic!("expected HandwritingV1"),
        }
    }

    #[test]
    fn ungrounded_answer_retries_then_keeps_value_flagged() {
        // A value WITHOUT grounding (the VLM omitted sub_bbox) fails the grounding validator and is
        // retried; if the VLM keeps omitting it, after MAX_RETRIES the value is KEPT (never dropped,
        // §7) with telemetry error_kind = "grounding".
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&[r#"{"text":"ok"}"#, r#"{"text":"ok"}"#, r#"{"text":"ok"}"#]);
        let (res, tel) =
            escalate_region(&vlm, &gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl", test_geom());
        let r = res.expect("value must be KEPT, not dropped (spec §7)");
        assert_eq!(tel.retry_count, MAX_RETRIES);
        assert_eq!(tel.error_kind.as_deref(), Some(UNGROUNDED_ERROR_KIND));
        match r {
            RegionResult::HandwritingV1 { text, .. } => assert_eq!(text.as_deref(), Some("ok")),
            _ => panic!("ungrounded value must still be returned"),
        }
    }

    #[test]
    fn ungrounded_then_grounded_succeeds_on_retry() {
        // First answer omits grounding (grounding_fail → retry), second answer includes a valid
        // sub_bbox → success. Proves the grounding error feeds back into the SAME retry loop.
        let dir = tempfile::tempdir().unwrap();
        let vlm = script(&[r#"{"text":"ok"}"#, GROUNDED_HW]);
        let (res, tel) =
            escalate_region(&vlm, &gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl", test_geom());
        assert!(res.is_ok());
        assert_eq!(tel.retry_count, 1);
        assert_eq!(tel.error_kind, None);
    }

    #[test]
    fn out_of_bounds_grounding_is_retried_as_grounding_fail() {
        // A sub_bbox outside the 8×8 crop (x=50) is out-of-bounds → grounding_fail every attempt →
        // value kept + flagged grounding (not parse — the JSON parsed fine, only grounding failed).
        let dir = tempfile::tempdir().unwrap();
        let oob = r#"{"text":"ok","grounding":{"sub_bbox":{"x":50,"y":50,"w":3,"h":3}}}"#;
        let vlm = script(&[oob, oob, oob]);
        let (res, tel) =
            escalate_region(&vlm, &gated_token(dir.path()), RegionKind::Handwriting, "qwen-vl", test_geom());
        assert!(res.is_ok());
        assert_eq!(tel.error_kind.as_deref(), Some(UNGROUNDED_ERROR_KIND));
    }

    #[test]
    fn ground_region_empty_chart_series_is_vacuously_grounded() {
        // A chart with NO series has no value to ground → ground_region passes (can't fabricate an
        // ungrounded value out of nothing).
        let crop = BBox { x: 0, y: 0, w: 8, h: 8 };
        let r = RegionResult::ChartV1 { chart_type: "bar".into(), series: vec![], axis_labels: vec![] };
        assert!(ground_region(&r, &crop, 0).is_ok());
    }

    #[test]
    fn ground_region_chart_series_missing_grounding_fails() {
        let crop = BBox { x: 0, y: 0, w: 8, h: 8 };
        let r = RegionResult::ChartV1 {
            chart_type: "bar".into(),
            series: vec![Series { name: "Q1".into(), values: vec![1.0], grounding: None }],
            axis_labels: vec![],
        };
        assert_eq!(ground_region(&r, &crop, 0), Err(GroundingFail::Missing));
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
        let vlm = script(&[GROUNDED_HW]);
        let (res, _) = escalate_region(&vlm, &token, RegionKind::Handwriting, "qwen-vl", test_geom());
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

    // gate_vlm_egress tests use the REAL OutboundGate API: Disabled / VaultLocked /
    // L0CloudBlocked / RedactorRequired. A local-only / 保守-governor destination is modeled
    // by enabled=false (refuse cloud egress); an L0-classified source is modeled by
    // source_contains_l0=true (refuse cloud egress — the VLM is a cloud endpoint).
    #[test]
    fn gate_blocks_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        // enabled=false models a local-only / privacy-off destination → refuse cloud egress.
        let err = gate_vlm_egress(
            false, true, false, None, "region-crop", &src,
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
            true, false, false, None, "region-crop", &src,
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::Gate(OutboundError::VaultLocked)));
    }

    // ---- L0-egress integration: an L0-classified source NEVER reaches the cloud VLM ----

    #[test]
    fn l0_source_is_refused_egress_to_cloud_vlm() {
        // SECURITY (C3 ⨯ develop L0-egress gate): a region from an L0-tagged ("永不出网")
        // source must NEVER egress to the cloud VLM — even fully enabled, vault-unlocked,
        // with a redactor and an Allow image decision. The gate refuses BEFORE any bytes
        // are minimized/written: L0CloudBlocked fires ahead of the image step.
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let redactor = Redactor::new();
        let dst = dir.path().join("e.png");
        let err = gate_vlm_egress(
            true, true, /* source_contains_l0 */ true, Some(&redactor), "region-crop", &src,
            ImageEgressDecision::AllowDownscaled(dst.clone()),
        )
        .unwrap_err();
        assert!(
            matches!(err, EgressError::Gate(OutboundError::L0CloudBlocked)),
            "L0-classified source must be refused egress to the cloud VLM; got {err:?}"
        );
        // Fail-closed: the gate refused before minimizing/writing any egress bytes.
        assert!(!dst.exists(), "no egress crop may be materialized for an L0-refused call");
    }

    #[test]
    fn non_l0_source_is_allowed_egress_downscaled() {
        // The other side of the invariant: a non-L0 source IS allowed (and the bytes that
        // leave are the downscaled minimized copy per C3), so the gate is not over-blocking.
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let redactor = Redactor::new();
        let dst = dir.path().join("e.png");
        let tok = gate_vlm_egress(
            true, true, /* source_contains_l0 */ false, Some(&redactor), "region-crop", &src,
            ImageEgressDecision::AllowDownscaled(dst.clone()),
        )
        .expect("non-L0 source under allow policy must mint a token");
        assert_eq!(tok.egress_crop_path(), dst.as_path());
        assert!(tok.egress_crop_path().exists(), "minimized egress crop must be materialized");
    }

    #[test]
    fn unknown_tier_default_fails_closed() {
        // Documents the fail-closed contract: callers that have NOT plumbed the source tier
        // MUST pass source_contains_l0=true, which the gate refuses — never egress when unsure.
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let redactor = Redactor::new();
        let err = gate_vlm_egress(
            true, true, /* unknown tier → conservative */ true, Some(&redactor), "region-crop", &src,
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::Gate(OutboundError::L0CloudBlocked)));
    }
    #[test]
    fn gate_fails_closed_without_redactor_on_nonempty() {
        // R7 / MEMORY P0: gate is non-no-op — a non-empty payload with no redactor fails closed.
        let dir = tempfile::tempdir().unwrap();
        let src = write_test_png(dir.path(), "s.png", 8);
        let err = gate_vlm_egress(
            true, true, false, None, "sensitive crop descriptor", &src,
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
            true, true, false, Some(&redactor), "联系电话 13800138000", &src,
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
            true, true, false, Some(&redactor), "region-crop", &src,
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
            true, true, false, Some(&redactor), "region-crop", &src,
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
            true, true, false, Some(&redactor), "region-crop",
            std::path::Path::new("/nonexistent/crop.png"),
            ImageEgressDecision::AllowDownscaled(dir.path().join("e.png")),
        )
        .unwrap_err();
        assert!(matches!(err, EgressError::Io(_)));
    }
}
