//! R2 chart — local chart-type classification + axis text (via ctx OCR lines).
//! series 数值 are 💰 VLM-only (spec §3.2: local chart→data ONNX immaturity).
use super::{CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::Result;
use image::DynamicImage;

pub struct ChartRecognizer;
impl RegionRecognizer for ChartRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Chart
    }
    fn recognize(&self, _crop: &DynamicImage, ctx: &RegionCtx) -> Result<RegionResult> {
        // Axis labels = the OCR text lines inside this region (passed via ctx). series empty
        // until 💰 VLM fills it — never fabricate values (spec: abstain over invent).
        let axis_labels: Vec<String> = ctx.ocr_lines.iter().map(|l| l.text.clone()).collect();
        Ok(RegionResult::ChartV1 {
            chart_type: "unknown".into(),
            series: vec![],
            axis_labels,
        })
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Local
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::{BBox, RawLine};
    #[test]
    fn axis_labels_from_ctx_series_empty() {
        let ctx = RegionCtx {
            ocr_lines: vec![RawLine {
                text: "2024".into(),
                bbox: BBox { x: 0, y: 0, w: 1, h: 1 },
                confidence: 0.9,
            }],
            page: 0,
        };
        let r = ChartRecognizer
            .recognize(&DynamicImage::new_rgb8(1, 1), &ctx)
            .unwrap();
        match r {
            RegionResult::ChartV1 {
                series,
                axis_labels,
                ..
            } => {
                assert!(series.is_empty(), "local must not fabricate series");
                assert_eq!(axis_labels, vec!["2024".to_string()]);
            }
            _ => panic!("wrong variant"),
        }
    }
}
