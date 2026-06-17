//! I1 — Grounding contract + validator (spec 2026-06-16 §I1 / §5.3 / §E2).
//!
//! Every structured value a VLM (or local recognizer) extracts — a chart series data point,
//! a formula LaTeX string, a table cell, a caption, a handwriting transcription, a stamp text —
//! must be **locatable back to its source**: the region bbox on the page (always available) and,
//! optionally, a finer sub-bbox in the original image or a reference to the OCR line it came from.
//!
//! This module is pure 🆓 logic (no model, no IO, no VLM call): it defines [`GroundingRef`] and a
//! [`validate_grounding`] check that the VLM-claimed source actually lands inside the input region.
//! A value whose grounding is missing / out-of-bounds / points at an empty area is flagged
//! [`GroundingFail`], which the existing retry-validate loop (`vlm_escalate::escalate_region`)
//! feeds back to the model to re-extract (≤ MAX_RETRIES). After retries are exhausted the value is
//! KEPT but marked `ungrounded` (never dropped, never fabricated — spec §7).
//!
//! Mirrors the doc-intelligence text-grounding pattern (citation substring ⊂ source): there, an
//! extracted citation must be a substring of the source text; here, an extracted value's source ref
//! must lie inside the source region. Same invariant ("the claim must point at real input"), adapted
//! from text offsets to image bboxes / OCR-line refs.

use crate::ocr::BBox;
use serde::{Deserialize, Serialize};

/// Where an extracted structured value came from. Carried alongside each value in a
/// [`super::RegionResult`] variant (all grounding fields are `#[serde(default)]` for back-compat,
/// spec §10). `region_bbox` + `page` are always present (region-level, already known); `sub_bbox`
/// and `ocr_line_ref` are optional refinements that locate the value to a pixel area / OCR line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundingRef {
    /// The region the value belongs to (region-level bbox, already known from layout detection).
    pub region_bbox: BBox,
    /// Page index the region is on.
    pub page: u32,
    /// Optional: refine to a pixel area inside the region (a bar / cell / formula fragment).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sub_bbox: Option<BBox>,
    /// Optional: locate the value back to a source OCR line (e.g. `"line:12"`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ocr_line_ref: Option<String>,
}

impl GroundingRef {
    /// A region-level grounding ref (no sub-bbox / line ref) — the always-available baseline.
    pub fn region(region_bbox: BBox, page: u32) -> Self {
        Self {
            region_bbox,
            page,
            sub_bbox: None,
            ocr_line_ref: None,
        }
    }

    /// Attach a finer sub-bbox refinement (builder-style).
    pub fn with_sub_bbox(mut self, sub: BBox) -> Self {
        self.sub_bbox = Some(sub);
        self
    }

    /// Attach an OCR line reference (builder-style).
    pub fn with_ocr_line_ref(mut self, line_ref: impl Into<String>) -> Self {
        self.ocr_line_ref = Some(line_ref.into());
        self
    }
}

/// Why a grounding check failed. Returned by [`validate_grounding`] and surfaced to the
/// retry-validate loop's error feedback so the VLM is told *what* to fix on re-extraction
/// (spec §E2 — a new validate-failure class alongside the existing parse failures).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundingFail {
    /// The value claims to exist but carries no [`GroundingRef`] at all.
    Missing,
    /// A `sub_bbox` ref lies (even partially) outside the region it claims to belong to.
    OutOfBounds,
    /// A `sub_bbox` ref has zero area — it points at no pixels (an empty/degenerate area).
    EmptyArea,
}

impl GroundingFail {
    /// Human/model-readable reason, fed back into the retry prompt (spec §4.5 B) and used as the
    /// `ungrounded` validation_warnings string after retries are exhausted (spec §7).
    pub fn as_reason(&self) -> &'static str {
        match self {
            GroundingFail::Missing => "grounding-fail: extracted value has no source location (GroundingRef). Provide where in the image this value came from.",
            GroundingFail::OutOfBounds => "grounding-fail: source location lies outside the region. The sub-area must be inside the region's bounds.",
            GroundingFail::EmptyArea => "grounding-fail: source location has zero area. Point at the actual pixels the value came from.",
        }
    }
}

/// Validate that an (optional) grounding ref actually locates the value inside `region`.
///
/// Contract (spec §E2):
/// - `None` → [`GroundingFail::Missing`] (a claimed value with no source ref).
/// - `Some` with `region_bbox` / `page` mismatching the input region → treated as out-of-bounds
///   (the ref must point at *this* region, not an arbitrary one).
/// - `Some` whose `sub_bbox` has zero area → [`GroundingFail::EmptyArea`].
/// - `Some` whose `sub_bbox` is not fully contained in `region_bbox` → [`GroundingFail::OutOfBounds`].
/// - Otherwise `Ok(())` — the value is grounded.
///
/// Region-level grounding (no `sub_bbox`) whose `region_bbox`/`page` match the input is accepted:
/// not every kind can refine to a sub-area (caption / handwriting ground at region level, spec §11 R1).
pub fn validate_grounding(grounding: Option<&GroundingRef>, region: &BBox, page: u32) -> Result<(), GroundingFail> {
    let g = grounding.ok_or(GroundingFail::Missing)?;
    // The ref must point at THIS region (defends against a model echoing a value with a bogus /
    // copy-pasted ref from another region — the doc-intel "citation must come from THIS source" rule).
    if g.region_bbox != *region || g.page != page {
        return Err(GroundingFail::OutOfBounds);
    }
    if let Some(sub) = &g.sub_bbox {
        if sub.w == 0 || sub.h == 0 {
            return Err(GroundingFail::EmptyArea);
        }
        if !bbox_contains(region, sub) {
            return Err(GroundingFail::OutOfBounds);
        }
    }
    Ok(())
}

/// True when `inner` is fully contained within `outer` (inclusive edges). Pure integer geometry.
/// Uses saturating/u64 arithmetic so a huge `inner` near `u32::MAX` can't overflow into a false pass.
fn bbox_contains(outer: &BBox, inner: &BBox) -> bool {
    let (ox, oy) = (outer.x as u64, outer.y as u64);
    let (ox2, oy2) = (ox + outer.w as u64, oy + outer.h as u64);
    let (ix, iy) = (inner.x as u64, inner.y as u64);
    let (ix2, iy2) = (ix + inner.w as u64, iy + inner.h as u64);
    ix >= ox && iy >= oy && ix2 <= ox2 && iy2 <= oy2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> BBox {
        BBox { x: 100, y: 100, w: 200, h: 200 }
    }

    // ── happy path ───────────────────────────────────────────────────────────

    #[test]
    fn region_level_grounding_matching_region_is_valid() {
        // A caption/handwriting value grounds at region level (no sub_bbox) — accepted when the
        // region_bbox/page match the input region.
        let g = GroundingRef::region(region(), 0);
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Ok(()));
    }

    #[test]
    fn sub_bbox_fully_inside_region_is_valid() {
        let g = GroundingRef::region(region(), 0).with_sub_bbox(BBox { x: 120, y: 120, w: 30, h: 30 });
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Ok(()));
    }

    #[test]
    fn sub_bbox_flush_with_region_edges_is_valid() {
        // Inclusive containment: a sub-bbox exactly filling the region is grounded.
        let g = GroundingRef::region(region(), 0).with_sub_bbox(region());
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Ok(()));
    }

    // ── grounding failures (drive the retry loop) ─────────────────────────────

    #[test]
    fn missing_grounding_is_missing_fail() {
        assert_eq!(validate_grounding(None, &region(), 0), Err(GroundingFail::Missing));
    }

    #[test]
    fn sub_bbox_partially_outside_region_is_out_of_bounds() {
        // Extends past the right edge (x 120 + w 300 = 420 > region right 300).
        let g = GroundingRef::region(region(), 0).with_sub_bbox(BBox { x: 120, y: 120, w: 300, h: 30 });
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::OutOfBounds));
    }

    #[test]
    fn sub_bbox_entirely_outside_region_is_out_of_bounds() {
        let g = GroundingRef::region(region(), 0).with_sub_bbox(BBox { x: 9000, y: 9000, w: 10, h: 10 });
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::OutOfBounds));
    }

    #[test]
    fn zero_area_sub_bbox_is_empty_area() {
        let g = GroundingRef::region(region(), 0).with_sub_bbox(BBox { x: 120, y: 120, w: 0, h: 30 });
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::EmptyArea));
        let g2 = GroundingRef::region(region(), 0).with_sub_bbox(BBox { x: 120, y: 120, w: 30, h: 0 });
        assert_eq!(validate_grounding(Some(&g2), &region(), 0), Err(GroundingFail::EmptyArea));
    }

    #[test]
    fn ref_pointing_at_a_different_region_is_out_of_bounds() {
        // A model that echoes a value with a ref copied from ANOTHER region must be caught:
        // the region_bbox in the ref doesn't match the region we're validating against.
        let other = BBox { x: 500, y: 500, w: 50, h: 50 };
        let g = GroundingRef::region(other, 0);
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::OutOfBounds));
    }

    #[test]
    fn ref_on_wrong_page_is_out_of_bounds() {
        let g = GroundingRef::region(region(), 3); // ref says page 3
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::OutOfBounds));
    }

    // ── boundary cases ────────────────────────────────────────────────────────

    #[test]
    fn sub_bbox_one_pixel_over_edge_is_out_of_bounds() {
        // x 100 + w 201 = 301 > region right 300 → one pixel past the edge is OOB (tight check).
        let g = GroundingRef::region(region(), 0).with_sub_bbox(BBox { x: 100, y: 100, w: 201, h: 10 });
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::OutOfBounds));
    }

    #[test]
    fn huge_sub_bbox_near_u32_max_does_not_overflow_into_false_pass() {
        // Adversarial: a sub_bbox whose x+w would overflow u32 must NOT wrap to a small value and
        // sneak past the containment check. u64 arithmetic in bbox_contains prevents the wrap.
        let g = GroundingRef::region(region(), 0)
            .with_sub_bbox(BBox { x: u32::MAX - 5, y: 100, w: 100, h: 10 });
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::OutOfBounds));
    }

    #[test]
    fn empty_takes_precedence_over_out_of_bounds() {
        // A zero-area sub_bbox that's also outside is reported as EmptyArea (checked first) — a
        // stable, deterministic classification so the retry feedback is consistent.
        let g = GroundingRef::region(region(), 0).with_sub_bbox(BBox { x: 9000, y: 9000, w: 0, h: 0 });
        assert_eq!(validate_grounding(Some(&g), &region(), 0), Err(GroundingFail::EmptyArea));
    }

    // ── serde back-compat (spec §10) ──────────────────────────────────────────

    #[test]
    fn grounding_ref_omits_optional_fields_when_none() {
        let g = GroundingRef::region(region(), 0);
        let j = serde_json::to_string(&g).unwrap();
        assert!(!j.contains("sub_bbox"), "None sub_bbox must be skipped: {j}");
        assert!(!j.contains("ocr_line_ref"), "None ocr_line_ref must be skipped: {j}");
    }

    #[test]
    fn grounding_ref_round_trips_with_refinements() {
        let g = GroundingRef::region(region(), 2)
            .with_sub_bbox(BBox { x: 110, y: 110, w: 20, h: 20 })
            .with_ocr_line_ref("line:12");
        let j = serde_json::to_string(&g).unwrap();
        let back: GroundingRef = serde_json::from_str(&j).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.ocr_line_ref.as_deref(), Some("line:12"));
    }

    #[test]
    fn old_payload_without_optional_fields_deserializes() {
        // An old serialized ref (region-level only) must deserialize (sub_bbox/ocr_line_ref → None).
        let json = r#"{"region_bbox":{"x":1,"y":2,"w":3,"h":4},"page":0}"#;
        let g: GroundingRef = serde_json::from_str(json).unwrap();
        assert_eq!(g.sub_bbox, None);
        assert_eq!(g.ocr_line_ref, None);
    }

    #[test]
    fn fail_reasons_are_distinct_and_nonempty() {
        for f in [GroundingFail::Missing, GroundingFail::OutOfBounds, GroundingFail::EmptyArea] {
            assert!(f.as_reason().starts_with("grounding-fail:"), "{f:?}");
        }
        assert_ne!(GroundingFail::Missing.as_reason(), GroundingFail::OutOfBounds.as_reason());
    }
}
