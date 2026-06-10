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

/// Typed output of the shared visual-understanding pass (ADR-0008): the recognized regions
/// plus the OCR cross-validation correction report. This is the SINGLE result type every
/// caller of the capability sees — REST handler, CLI subcommand, and any plugin invoking
/// the capability. Industry semantics (e.g. "this table is a contract-clause table") are
/// layered by pro plugins ON TOP of this generic output; this struct stays zero-industry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecognizePageResult {
    pub regions: Vec<Region>,
    pub correction_report: OcrCorrectionReport,
    /// 🆓/⚡ local regions vs 💰 VLM-escalated regions (spec §8 cost surfacing).
    pub local_regions: u32,
    pub escalated_regions: u32,
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
    let detected = layout::detect_regions(layout_model, image_path).unwrap_or_default();
    let ctx = RegionCtx {
        ocr_lines: ocr_lines.to_vec(),
        page: 0,
    };
    // Placeholder crop until per-region cropping lands with real layout inference; the
    // local recognizers tolerate it and the dispatch path is what is exercised today.
    let crop = DynamicImage::new_rgb8(1, 1);
    let regions: Vec<Region> = detected
        .iter()
        .filter_map(|lr| recognize_region(lr, &crop, &ctx, table_model).ok())
        .collect();

    let ocr_texts: Vec<Option<String>> = regions.iter().map(|_| None).collect();
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
        };
        let j = serde_json::to_string(&out).unwrap();
        assert!(j.contains(r#""schema":"checkbox_v1""#), "got {j}");
        assert!(j.contains(r#""local_regions":1"#));
    }
}
