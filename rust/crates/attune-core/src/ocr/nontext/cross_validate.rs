//! Stage 3 — OCR cross-validation (🆓 pure comparison logic).
//! Compares PP-OCR's structure/content against the local recognizer's second opinion;
//! never silently rewrites原文 (per spec §7: present both values, applied=false until user accepts).

use super::{
    Agreement, CorrectionEntry, CorrectionSummary, OcrCorrectionReport, Region, RegionResult,
    RegionSource,
};

/// High-confidence terminal threshold (spec §3.4).
pub const TAU_HIGH: f32 = 0.85;
/// Low-confidence forced-escalation threshold (spec §3.4, aligns FieldValue<0.6 UI line).
pub const TAU_LOW: f32 = 0.60;

/// Decision for a single region after cross-validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// confidence ≥ τ_high and agreement → terminal, do not escalate.
    Terminal,
    /// low confidence / conflict / discrepancy → escalate candidate.
    Escalate,
}

/// Decide per spec §3 Stage 3: agree+high → Terminal; else Escalate.
pub fn decide(agreement: Agreement, confidence: f32) -> Decision {
    match agreement {
        Agreement::Agree if confidence >= TAU_HIGH => Decision::Terminal,
        Agreement::ContentConflict | Agreement::StructureDiscrepancy => Decision::Escalate,
        _ if confidence < TAU_LOW => Decision::Escalate,
        _ => Decision::Terminal, // 0.60..0.85 agree → terminal but UI may flag
    }
}

/// Compare PP-OCR text vs local recognizer text for a content region.
/// Normalizes whitespace; classifies common OCR-error pairs as ContentConflict.
pub fn compare_content(ocr: &str, local: &str) -> Agreement {
    let n = |s: &str| s.split_whitespace().collect::<String>();
    if n(ocr) == n(local) {
        Agreement::Agree
    } else {
        Agreement::ContentConflict
    }
}

/// Compare two table structures by (row_count, col_count). Diff ≥1 in either → discrepancy.
pub fn compare_table_structure(
    ocr_rows: u32,
    ocr_cols: u32,
    local_rows: u32,
    local_cols: u32,
) -> Agreement {
    if ocr_rows.abs_diff(local_rows) >= 1 || ocr_cols.abs_diff(local_cols) >= 1 {
        Agreement::StructureDiscrepancy
    } else {
        Agreement::Agree
    }
}

/// Build the correction report from finalized regions. `ocr_texts[i]` is the PP-OCR original
/// for `regions[i]` (None if not a content region).
pub fn build_report(regions: &[Region], ocr_texts: &[Option<String>]) -> OcrCorrectionReport {
    let mut entries = Vec::new();
    let mut summary = CorrectionSummary {
        total: regions.len() as u32,
        ..Default::default()
    };
    for (i, region) in regions.iter().enumerate() {
        let original = ocr_texts.get(i).cloned().flatten();
        let corrected = region_text(&region.result);
        let agreement = match (&original, &corrected) {
            (Some(o), Some(c)) => compare_content(o, c),
            _ => Agreement::Agree,
        };
        match agreement {
            Agreement::Agree => summary.confirmed += 1,
            Agreement::ContentConflict => summary.conflicts += 1,
            Agreement::StructureDiscrepancy => summary.discrepancies += 1,
        }
        if region.source == RegionSource::Vlm {
            summary.escalated += 1;
        }
        entries.push(CorrectionEntry {
            region_idx: i,
            bbox: region.bbox,
            kind: region.kind,
            original,
            corrected: if matches!(agreement, Agreement::Agree) {
                None
            } else {
                corrected
            },
            agreement,
            source: region.source,
            confidence: region.confidence,
            applied: false, // never auto-apply (spec §7)
        });
    }
    OcrCorrectionReport {
        schema_version: 1,
        entries,
        summary,
    }
}

/// Best-effort text extraction from a RegionResult for content comparison.
fn region_text(r: &RegionResult) -> Option<String> {
    match r {
        RegionResult::HandwritingV1 { text } => text.clone(),
        RegionResult::FormulaV1 { latex, raw_ocr } => latex.clone().or_else(|| raw_ocr.clone()),
        RegionResult::StampV1 { text, .. } => text.clone(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::nontext::{Region, RegionKind, RegionResult, RegionSource};
    use crate::ocr::BBox;

    fn region(
        kind: RegionKind,
        result: RegionResult,
        source: RegionSource,
        conf: f32,
    ) -> Region {
        Region {
            kind,
            bbox: BBox { x: 0, y: 0, w: 1, h: 1 },
            page: 0,
            det_confidence: 0.9,
            result,
            source,
            confidence: conf,
            validation_warnings: vec![],
        }
    }

    #[test]
    fn high_conf_agree_is_terminal() {
        assert_eq!(decide(Agreement::Agree, 0.9), Decision::Terminal);
    }
    #[test]
    fn low_conf_agree_escalates() {
        assert_eq!(decide(Agreement::Agree, 0.5), Decision::Escalate);
    }
    #[test]
    fn conflict_always_escalates() {
        assert_eq!(decide(Agreement::ContentConflict, 0.99), Decision::Escalate);
        assert_eq!(decide(Agreement::StructureDiscrepancy, 0.99), Decision::Escalate);
    }
    #[test]
    fn content_compare_normalizes_whitespace() {
        assert_eq!(compare_content("12 345", "12345"), Agreement::Agree);
        assert_eq!(compare_content("100", "1OO"), Agreement::ContentConflict);
    }
    #[test]
    fn table_structure_diff_is_discrepancy() {
        assert_eq!(compare_table_structure(3, 4, 3, 4), Agreement::Agree);
        assert_eq!(compare_table_structure(3, 4, 4, 4), Agreement::StructureDiscrepancy);
    }
    #[test]
    fn build_report_marks_agree_no_correction() {
        let regions = vec![region(
            RegionKind::Handwriting,
            RegionResult::HandwritingV1 { text: Some("foo".into()) },
            RegionSource::CrossConfirmed,
            0.9,
        )];
        let report = build_report(&regions, &[Some("foo".into())]);
        assert_eq!(report.summary.confirmed, 1);
        assert_eq!(report.entries[0].corrected, None);
        assert!(!report.entries[0].applied);
    }
    #[test]
    fn build_report_conflict_keeps_both_values() {
        let regions = vec![region(
            RegionKind::Handwriting,
            RegionResult::HandwritingV1 { text: Some("1OO".into()) },
            RegionSource::Vlm,
            0.7,
        )];
        let report = build_report(&regions, &[Some("100".into())]);
        assert_eq!(report.summary.conflicts, 1);
        assert_eq!(report.entries[0].original.as_deref(), Some("100"));
        assert_eq!(report.entries[0].corrected.as_deref(), Some("1OO"));
        assert_eq!(report.summary.escalated, 1);
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use crate::ocr::nontext::{Region, RegionKind, RegionResult, RegionSource};
    use crate::ocr::BBox;
    use proptest::prelude::*;

    proptest! {
        /// R5: build_report never sets applied=true (no silent auto-correct).
        #[test]
        fn report_never_auto_applies(text in ".*", ocr in ".*", conf in 0.0f32..1.0) {
            let regions = vec![Region {
                kind: RegionKind::Handwriting, bbox: BBox { x:0,y:0,w:1,h:1 }, page: 0,
                det_confidence: 0.5,
                result: RegionResult::HandwritingV1 { text: Some(text) },
                source: RegionSource::Local, confidence: conf, validation_warnings: vec![],
            }];
            let report = build_report(&regions, &[Some(ocr)]);
            prop_assert!(report.entries.iter().all(|e| !e.applied));
        }

        /// decide() is total + never panics across the confidence range.
        #[test]
        fn decide_total(conf in -1.0f32..2.0) {
            let _ = decide(Agreement::Agree, conf);
            let _ = decide(Agreement::ContentConflict, conf);
            let _ = decide(Agreement::StructureDiscrepancy, conf);
        }

        /// summary totals are internally consistent (confirmed+conflicts+discrepancies == total).
        #[test]
        fn summary_partition_sums_to_total(n in 0usize..20) {
            let regions: Vec<Region> = (0..n).map(|_| Region {
                kind: RegionKind::Handwriting, bbox: BBox { x:0,y:0,w:1,h:1 }, page: 0,
                det_confidence: 0.5,
                result: RegionResult::HandwritingV1 { text: Some("x".into()) },
                source: RegionSource::Local, confidence: 0.9, validation_warnings: vec![],
            }).collect();
            let ocr: Vec<Option<String>> = (0..n).map(|i| Some(if i % 2 == 0 { "x".into() } else { "y".into() })).collect();
            let report = build_report(&regions, &ocr);
            prop_assert_eq!(
                report.summary.confirmed + report.summary.conflicts + report.summary.discrepancies,
                report.summary.total
            );
        }
    }
}
