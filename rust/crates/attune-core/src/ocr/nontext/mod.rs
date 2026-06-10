//! Non-text content recognition — 7 region recognizers (⚡ local) + 🆓 OCR cross-validation
//! + 💰 VLM escalation. Extends the OCR pipeline; never replaces PP-OCRv5.
//! All native model deps live here behind `feature="nontext"` (keeps wasm leaf clean).
//!
//! R6 lock-order note: any ort `Session` used by recognizers in this module lives behind
//! its own independent `Mutex` (one per recognizer instance). It is NEVER held across the
//! search hot-path locks (fulltext / vectors / vault), so it cannot form an ABBA cycle.

use crate::ocr::{BBox, RawLine};
use serde::{Deserialize, Serialize};

pub mod checkbox;
pub mod layout;
pub mod stamp_signature;

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
}
