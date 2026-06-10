//! Non-text content recognition — 7 region recognizers (⚡ local), 🆓 OCR cross-validation,
//! and 💰 VLM escalation. Extends the OCR pipeline; never replaces PP-OCRv5.
//! All native model deps live here behind `feature="nontext"` (keeps wasm leaf clean).
//!
//! R6 lock-order note: any ort `Session` used by recognizers in this module lives behind
//! its own independent `Mutex` (one per recognizer instance). It is NEVER held across the
//! search hot-path locks (fulltext / vectors / vault), so it cannot form an ABBA cycle.

use crate::ocr::{BBox, RawLine};
use serde::{Deserialize, Serialize};

pub mod chart;
pub mod checkbox;
pub mod cross_validate;
pub mod figure;
pub mod formula;
pub mod handwriting;
pub mod layout;
pub mod stamp_signature;
pub mod table_structure;
pub mod vlm_escalate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    Text,
    Table,
    Chart,
    Figure,
    Formula,
    Handwriting,
    Stamp,
    Signature,
    Checkbox,
    FormField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionSource {
    Local,
    Vlm,
    CrossConfirmed,
    OcrOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agreement {
    Agree,
    ContentConflict,
    StructureDiscrepancy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostTier {
    Free,
    Local,
    Llm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cell {
    pub row: u32,
    pub col: u32,
    pub row_span: u32,
    pub col_span: u32,
    pub text: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "schema", rename_all = "snake_case")]
pub enum RegionResult {
    TableV1 {
        cells: Vec<Cell>,
        row_count: u32,
        col_count: u32,
    },
    ChartV1 {
        chart_type: String,
        series: Vec<Series>,
        axis_labels: Vec<String>,
    },
    FigureV1 {
        class: String,
        caption: Option<String>,
    },
    FormulaV1 {
        latex: Option<String>,
        raw_ocr: Option<String>,
    },
    HandwritingV1 {
        text: Option<String>,
    },
    StampV1 {
        present: bool,
        text: Option<String>,
        stamp_type: Option<String>,
    },
    SignatureV1 {
        present: bool,
        bbox_of_owner_field: Option<BBox>,
    },
    CheckboxV1 {
        checked: bool,
    },
    UnrecognizedV1 {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub kind: RegionKind,
    pub bbox: BBox,
    pub page: u32,
    pub det_confidence: f32,
    pub result: RegionResult,
    pub source: RegionSource,
    pub confidence: f32,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub validation_warnings: Vec<String>,
}

/// Per-region context handed to recognizers — carries the PP-OCR second opinion.
#[derive(Debug, Clone)]
pub struct RegionCtx {
    pub ocr_lines: Vec<RawLine>,
    pub page: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcrCorrectionReport {
    pub schema_version: u32,
    pub entries: Vec<CorrectionEntry>,
    pub summary: CorrectionSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorrectionEntry {
    pub region_idx: usize,
    pub bbox: BBox,
    pub kind: RegionKind,
    pub original: Option<String>,
    pub corrected: Option<String>,
    pub agreement: Agreement,
    pub source: RegionSource,
    pub confidence: f32,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct CorrectionSummary {
    pub total: u32,
    pub confirmed: u32,
    pub discrepancies: u32,
    pub conflicts: u32,
    pub escalated: u32,
    pub accepted: u32,
}

use crate::error::Result;
use image::DynamicImage;

/// Internal extension point: one recognizer per RegionKind (spec §6.4).
pub trait RegionRecognizer: Send + Sync {
    fn kind(&self) -> RegionKind;
    /// ⚡/🆓 local recognition of a cropped region.
    fn recognize(&self, region_crop: &DynamicImage, ctx: &RegionCtx) -> Result<RegionResult>;
    fn cost_tier(&self) -> CostTier;
}

/// Dispatch a single detected region to its kind's recognizer. 🆓/⚡ only — no VLM here.
/// Returns a fully-populated Region with source=Local and confidence from det_confidence.
pub fn recognize_region(
    layout: &layout::LayoutRegion,
    crop: &DynamicImage,
    ctx: &RegionCtx,
    table_model: &std::path::Path,
) -> Result<Region> {
    let result = match layout.kind {
        RegionKind::Checkbox => checkbox::CheckboxRecognizer.recognize(crop, ctx)?,
        RegionKind::Stamp => stamp_signature::StampRecognizer.recognize(crop, ctx)?,
        RegionKind::Signature => stamp_signature::SignatureRecognizer.recognize(crop, ctx)?,
        RegionKind::Table => table_structure::TableStructureRecognizer {
            model_path: table_model.to_path_buf(),
        }
        .recognize(crop, ctx)?,
        RegionKind::Chart => chart::ChartRecognizer.recognize(crop, ctx)?,
        RegionKind::Figure => figure::FigureRecognizer.recognize(crop, ctx)?,
        RegionKind::Formula => formula::FormulaRecognizer.recognize(crop, ctx)?,
        RegionKind::Handwriting => handwriting::HandwritingRecognizer.recognize(crop, ctx)?,
        RegionKind::Text | RegionKind::FormField => RegionResult::UnrecognizedV1 {
            reason: "handled-by-plain-ocr".into(),
        },
    };
    Ok(Region {
        kind: layout.kind,
        bbox: layout.bbox,
        page: ctx.page,
        det_confidence: layout.det_confidence,
        result,
        source: RegionSource::Local,
        confidence: layout.det_confidence,
        validation_warnings: vec![],
    })
}

/// HONEST status of the recognition engine, surfaced to every caller so nobody mistakes a
/// degraded/scaffold pass for a functioning one (the C1 truth from adversarial review).
///
/// SCAFFOLD: Stage1 layout detection (`layout::detect_regions`) currently returns empty
/// because the layout ONNX model is NOT bundled (pending model sourcing, spec R3/R4). When
/// no model is present the only honest answer is `ScaffoldNoLayoutModel` — recognition is
/// not yet functional, callers must not claim it recognizes structure. Once a real layout
/// model is wired the status becomes `Functional` (model present) or `LayoutError` (the
/// model is present but inference failed — surfaced, never masked as "empty page").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineStatus {
    /// Layout ONNX model is absent → Stage1 yields no regions. Recognition NOT functional.
    ScaffoldNoLayoutModel,
    /// Layout model present + inference ran (functional path). Regions reflect real detection.
    Functional,
    /// Layout model present but inference errored — surfaced (not masked as empty page, I1).
    LayoutError,
}

/// Typed output of the shared visual-understanding pass (ADR-0008): the recognized regions
/// plus the OCR cross-validation correction report. This is the SINGLE result type every
/// caller of the capability sees — REST handler, CLI subcommand, and any plugin invoking
/// the capability. Domain-specific interpretation (what a given table/stamp/figure *means*
/// in a vertical) is layered by pro plugins ON TOP of this generic output; this struct and
/// every RegionKind stay zero-vertical-binding (R8 OSS-boundary).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecognizePageResult {
    pub regions: Vec<Region>,
    pub correction_report: OcrCorrectionReport,
    /// 🆓/⚡ local regions vs 💰 VLM-escalated regions (spec §8 cost surfacing).
    pub local_regions: u32,
    pub escalated_regions: u32,
    /// HONEST engine status — callers KNOW whether recognition is functional or a scaffold.
    pub engine_status: EngineStatus,
    /// Page-level warnings (e.g. a Stage1 inference error surfaced instead of masked as empty,
    /// or a per-region recognizer error flagged on its region). Empty on the happy path.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub validation_warnings: Vec<String>,
}

/// Shared visual-understanding orchestrator (spec §3 data-flow Stage1→2→3): detect layout
/// regions, dispatch each to its 🆓/⚡ local recognizer, then 🆓 cross-validate against the
/// PP-OCR second opinion. This is the ONE entry point the capability exposes — REST, CLI,
/// and plugins all funnel through it so cost/quality/telemetry have a single tuning point.
///
/// Models missing → `detect_regions` returns empty → regions degrade to plain OCR (never
/// errors). VLM escalation (Stage4) is NOT performed here; it is the caller's gated step
/// (build-stage default Off never escalates, §8).
///
/// `ocr_lines` is the PP-OCR second opinion (empty when no engine present). `layout_model`
/// and `table_model` are the ONNX model paths (absent → graceful empty).
pub fn recognize_page(
    image_path: &std::path::Path,
    layout_model: &std::path::Path,
    table_model: &std::path::Path,
    ocr_lines: &[RawLine],
) -> RecognizePageResult {
    let mut page_warnings: Vec<String> = Vec::new();

    // I1b + HONEST status: distinguish "model absent (scaffold)" from "model present but
    // inference errored" from "real empty result". Never mask an error as an empty page.
    let model_present = layout::layout_model_present(layout_model);
    let detected = match layout::detect_regions(layout_model, image_path) {
        Ok(d) => d,
        Err(e) => {
            // I1b: a genuine Stage1 inference error is SURFACED (telemetry + warning),
            // not silently turned into an empty page via unwrap_or_default().
            page_warnings.push(format!("layout-inference-error: {e}"));
            Vec::new()
        }
    };
    let layout_errored = page_warnings.iter().any(|w| w.starts_with("layout-inference-error"));
    let engine_status = if !model_present {
        // SCAFFOLD: no layout ONNX bundled → recognition not functional (spec R3/R4, C1).
        EngineStatus::ScaffoldNoLayoutModel
    } else if layout_errored {
        EngineStatus::LayoutError
    } else {
        EngineStatus::Functional
    };

    let ctx = RegionCtx {
        ocr_lines: ocr_lines.to_vec(),
        page: 0,
    };
    // Real per-region cropping: decode the page once, then hand each recognizer the actual
    // pixels of its region bbox (falls back to a 1×1 placeholder if the page can't be decoded
    // or a bbox is degenerate — the local recognizers tolerate it).
    let page_img: Option<DynamicImage> = if detected.is_empty() {
        None
    } else {
        image::open(image_path).ok()
    };

    // I1a: a recognizer Err does NOT drop the region (spec §7 "绝不 drop region"). The
    // region stays in the output flagged as UnrecognizedV1 + a validation_warning, so the
    // caller sees the region exists but recognition failed (vs it never having been there).
    let regions: Vec<Region> = detected
        .iter()
        .map(|lr| {
            let crop = crop_region(page_img.as_ref(), &lr.bbox);
            let res = recognize_region(lr, &crop, &ctx, table_model)
                .map(|region| region.result);
            region_from_recognizer_result(lr, ctx.page, res)
        })
        .collect();

    // I2: feed the REAL PP-OCR text for each region (the second opinion) into cross-validation
    // — not hardcoded all-None. For each region we gather the OCR lines whose bbox center sits
    // inside the region bbox; that is the OCR's reading of the same area, which build_report
    // compares against the local recognizer's reading. If no OCR line falls in the region the
    // entry is None (not a content region / no overlap), and build_report treats it as Agree.
    let ocr_texts: Vec<Option<String>> = regions
        .iter()
        .map(|r| ocr_text_for_region(&r.bbox, ocr_lines))
        .collect();
    let correction_report = cross_validate::build_report(&regions, &ocr_texts);
    let local_regions = regions
        .iter()
        .filter(|r| r.source == RegionSource::Local)
        .count() as u32;
    let escalated_regions = regions
        .iter()
        .filter(|r| r.source == RegionSource::Vlm)
        .count() as u32;
    RecognizePageResult {
        regions,
        correction_report,
        local_regions,
        escalated_regions,
        engine_status,
        validation_warnings: page_warnings,
    }
}

/// I2 helper: the PP-OCR second opinion for one region — concatenated text of every OCR line
/// whose bbox center falls inside the region bbox. `None` when no OCR line overlaps (so
/// cross-validation does not invent a comparison). Lines are joined in reading order (the
/// order PP-OCR emits them, which is top-to-bottom / left-to-right).
fn ocr_text_for_region(region: &BBox, ocr_lines: &[RawLine]) -> Option<String> {
    let parts: Vec<&str> = ocr_lines
        .iter()
        .filter(|l| bbox_center_in(&l.bbox, region))
        .map(|l| l.text.as_str())
        .filter(|t| !t.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// True when the center of `inner` lies within `outer` (inclusive). Integer pixel coords.
fn bbox_center_in(inner: &BBox, outer: &BBox) -> bool {
    let cx = inner.x + inner.w / 2;
    let cy = inner.y + inner.h / 2;
    cx >= outer.x && cx <= outer.x + outer.w && cy >= outer.y && cy <= outer.y + outer.h
}

/// Crop the page image to a region bbox, clamped to image bounds. Returns a 1×1 placeholder
/// when the page is absent (couldn't decode) or the clamped bbox is degenerate — the local
/// recognizers tolerate a tiny crop and degrade gracefully rather than erroring.
fn crop_region(page: Option<&DynamicImage>, bbox: &BBox) -> DynamicImage {
    use image::GenericImageView;
    let Some(img) = page else {
        return DynamicImage::new_rgb8(1, 1);
    };
    let (iw, ih) = img.dimensions();
    let x = bbox.x.min(iw.saturating_sub(1));
    let y = bbox.y.min(ih.saturating_sub(1));
    let w = bbox.w.min(iw.saturating_sub(x));
    let h = bbox.h.min(ih.saturating_sub(y));
    if w == 0 || h == 0 {
        return DynamicImage::new_rgb8(1, 1);
    }
    img.crop_imm(x, y, w, h)
}

/// I1b: turn a per-region recognizer result into a `Region` that ALWAYS stays in the output.
/// On Ok the region carries the recognized result; on Err the region is KEPT (never dropped,
/// spec §7 "绝不 drop region") as `UnrecognizedV1{reason}` + a `validation_warnings` entry so
/// the caller sees the region exists but recognition failed (distinct from it never existing).
fn region_from_recognizer_result(
    lr: &layout::LayoutRegion,
    page: u32,
    res: Result<RegionResult>,
) -> Region {
    match res {
        Ok(result) => Region {
            kind: lr.kind,
            bbox: lr.bbox,
            page,
            det_confidence: lr.det_confidence,
            result,
            source: RegionSource::Local,
            confidence: lr.det_confidence,
            validation_warnings: vec![],
        },
        Err(e) => Region {
            kind: lr.kind,
            bbox: lr.bbox,
            page,
            det_confidence: lr.det_confidence,
            result: RegionResult::UnrecognizedV1 {
                reason: format!("recognizer-error: {e}"),
            },
            source: RegionSource::Local,
            confidence: lr.det_confidence,
            validation_warnings: vec![format!(
                "region kept despite recognizer error ({:?}): {e}",
                lr.kind
            )],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_result_tag_is_schema_snake_case() {
        let r = RegionResult::TableV1 {
            cells: vec![],
            row_count: 0,
            col_count: 0,
        };
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains(r#""schema":"table_v1""#), "got {j}");
    }

    #[test]
    fn region_kind_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&RegionKind::FormField).unwrap(),
            r#""form_field""#
        );
    }

    #[test]
    fn recognizer_trait_is_object_safe() {
        // Compile-time proof the trait is dyn-compatible (we store Box<dyn RegionRecognizer>).
        fn _assert(_: &dyn RegionRecognizer) {}
    }

    #[test]
    fn recognize_region_dispatches_checkbox() {
        use image::DynamicImage;
        let lr = layout::LayoutRegion {
            kind: RegionKind::Checkbox,
            bbox: BBox { x: 0, y: 0, w: 1, h: 1 },
            det_confidence: 0.8,
        };
        // new_rgb8 is all-black → checkbox detector reads it as checked.
        let crop = DynamicImage::new_rgb8(10, 10);
        let ctx = RegionCtx { ocr_lines: vec![], page: 2 };
        let r = recognize_region(&lr, &crop, &ctx, std::path::Path::new("/no.onnx")).unwrap();
        assert_eq!(r.page, 2);
        assert_eq!(r.source, RegionSource::Local);
        assert!(matches!(r.result, RegionResult::CheckboxV1 { .. }));
    }

    #[test]
    fn recognize_region_table_missing_model_unrecognized() {
        use image::DynamicImage;
        let lr = layout::LayoutRegion {
            kind: RegionKind::Table,
            bbox: BBox { x: 0, y: 0, w: 1, h: 1 },
            det_confidence: 0.8,
        };
        let r = recognize_region(
            &lr,
            &DynamicImage::new_rgb8(4, 4),
            &RegionCtx { ocr_lines: vec![], page: 0 },
            std::path::Path::new("/no.onnx"),
        )
        .unwrap();
        assert!(matches!(r.result, RegionResult::UnrecognizedV1 { .. }));
    }

    #[test]
    fn region_round_trips() {
        let r = Region {
            kind: RegionKind::Checkbox,
            bbox: BBox { x: 1, y: 2, w: 3, h: 4 },
            page: 0,
            det_confidence: 0.9,
            result: RegionResult::CheckboxV1 { checked: true },
            source: RegionSource::Local,
            confidence: 0.95,
            validation_warnings: vec![],
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: Region = serde_json::from_str(&j).unwrap();
        assert_eq!(back.page, 0);
        assert!(matches!(back.result, RegionResult::CheckboxV1 { checked: true }));
        // validation_warnings skipped when empty
        assert!(
            !j.contains("validation_warnings"),
            "empty warnings should be skipped: {j}"
        );
    }

    #[test]
    fn recognize_page_missing_models_degrades_to_empty() {
        // The shared capability entry point: no layout/table model present → empty regions,
        // empty report, zero cost, never errors (the 🆓 degrade-to-plain-OCR invariant, R1).
        // HONEST status: no layout model → ScaffoldNoLayoutModel (recognition not functional).
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let out = recognize_page(
            tmp.path(),
            std::path::Path::new("/nonexistent/layout.onnx"),
            std::path::Path::new("/nonexistent/slanet.onnx"),
            &[],
        );
        assert!(out.regions.is_empty());
        assert_eq!(out.local_regions, 0);
        assert_eq!(out.escalated_regions, 0);
        assert_eq!(out.correction_report.summary.total, 0);
        assert_eq!(out.engine_status, EngineStatus::ScaffoldNoLayoutModel);
        assert!(out.validation_warnings.is_empty());
    }

    #[test]
    fn scaffold_status_serializes_honestly() {
        // The C1 truth must be machine-readable: callers see engine-status = scaffold string.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let out = recognize_page(
            tmp.path(),
            std::path::Path::new("/nonexistent/layout.onnx"),
            std::path::Path::new("/nonexistent/slanet.onnx"),
            &[],
        );
        let j = serde_json::to_string(&out).unwrap();
        assert!(
            j.contains(r#""engine_status":"scaffold-no-layout-model""#),
            "callers must KNOW recognition is a scaffold; got {j}"
        );
    }

    #[test]
    fn engine_status_serde_kebab() {
        assert_eq!(
            serde_json::to_string(&EngineStatus::ScaffoldNoLayoutModel).unwrap(),
            r#""scaffold-no-layout-model""#
        );
        assert_eq!(
            serde_json::to_string(&EngineStatus::LayoutError).unwrap(),
            r#""layout-error""#
        );
    }

    #[test]
    fn recognizer_error_keeps_region_flagged_not_dropped() {
        // I1b: spec §7 "绝不 drop region". A recognizer Err must NOT drop the region; it stays
        // in output as UnrecognizedV1 + a validation_warning (distinct from never existing).
        let lr = layout::LayoutRegion {
            kind: RegionKind::Table,
            bbox: BBox { x: 5, y: 6, w: 7, h: 8 },
            det_confidence: 0.77,
        };
        let injected = Err(crate::error::VaultError::Io(std::io::Error::other("boom")));
        let region = region_from_recognizer_result(&lr, 3, injected);
        // The region is preserved (same kind/bbox/page), not dropped.
        assert_eq!(region.kind, RegionKind::Table);
        assert_eq!(region.bbox, BBox { x: 5, y: 6, w: 7, h: 8 });
        assert_eq!(region.page, 3);
        assert!(matches!(region.result, RegionResult::UnrecognizedV1 { .. }));
        assert!(
            region.validation_warnings.iter().any(|w| w.contains("recognizer error")),
            "region must carry a warning explaining recognition failed: {:?}",
            region.validation_warnings
        );
    }

    #[test]
    fn crop_region_clamps_and_placeholders() {
        // No page → 1×1 placeholder.
        let ph = crop_region(None, &BBox { x: 0, y: 0, w: 10, h: 10 });
        assert_eq!((ph.width(), ph.height()), (1, 1));
        // Real page, in-bounds bbox → exact crop.
        let page = DynamicImage::new_rgb8(100, 80);
        let c = crop_region(Some(&page), &BBox { x: 10, y: 20, w: 30, h: 40 });
        assert_eq!((c.width(), c.height()), (30, 40));
        // Bbox overflowing the page is clamped, never panics.
        let c2 = crop_region(Some(&page), &BBox { x: 90, y: 70, w: 999, h: 999 });
        assert_eq!((c2.width(), c2.height()), (10, 10));
        // Degenerate (zero area after clamp) → 1×1 placeholder.
        let c3 = crop_region(Some(&page), &BBox { x: 100, y: 80, w: 5, h: 5 });
        assert_eq!((c3.width(), c3.height()), (1, 1));
    }

    #[test]
    fn ocr_text_for_region_matches_lines_inside_bbox() {
        // I2: the real PP-OCR second opinion for a region = OCR lines whose center is inside it.
        let region = BBox { x: 0, y: 0, w: 100, h: 100 };
        let lines = vec![
            RawLine { text: "inside".into(), bbox: BBox { x: 10, y: 10, w: 20, h: 10 }, confidence: 0.9 },
            RawLine { text: "outside".into(), bbox: BBox { x: 500, y: 500, w: 20, h: 10 }, confidence: 0.9 },
        ];
        assert_eq!(ocr_text_for_region(&region, &lines).as_deref(), Some("inside"));
        // No overlapping line → None (don't invent a comparison).
        assert_eq!(ocr_text_for_region(&BBox { x: 1000, y: 1000, w: 1, h: 1 }, &lines), None);
    }

    #[test]
    fn recognize_page_result_serializes_typed() {
        // The capability output must serialize to a typed JSON envelope plugins can parse.
        let out = RecognizePageResult {
            regions: vec![Region {
                kind: RegionKind::Checkbox,
                bbox: BBox { x: 0, y: 0, w: 1, h: 1 },
                page: 0,
                det_confidence: 0.9,
                result: RegionResult::CheckboxV1 { checked: true },
                source: RegionSource::Local,
                confidence: 0.9,
                validation_warnings: vec![],
            }],
            correction_report: OcrCorrectionReport {
                schema_version: 1,
                entries: vec![],
                summary: CorrectionSummary::default(),
            },
            local_regions: 1,
            escalated_regions: 0,
            engine_status: EngineStatus::Functional,
            validation_warnings: vec![],
        };
        let j = serde_json::to_string(&out).unwrap();
        assert!(j.contains(r#""schema":"checkbox_v1""#), "got {j}");
        assert!(j.contains(r#""local_regions":1"#));
        assert!(j.contains(r#""engine_status":"functional""#));
    }
}
