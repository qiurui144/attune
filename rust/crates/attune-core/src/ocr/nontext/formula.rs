//! R4 formula — local detection + region OCR raw text; LaTeX is 💰 VLM (spec §3.2 v1 倾向).
use super::{CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::Result;
use image::DynamicImage;

pub struct FormulaRecognizer;
impl RegionRecognizer for FormulaRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Formula
    }
    fn recognize(&self, _crop: &DynamicImage, ctx: &RegionCtx) -> Result<RegionResult> {
        let raw: String = ctx
            .ocr_lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let raw_ocr = if raw.is_empty() { None } else { Some(raw) };
        Ok(RegionResult::FormulaV1 {
            latex: None,
            raw_ocr,
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
    fn latex_none_raw_from_ocr() {
        let ctx = RegionCtx {
            ocr_lines: vec![RawLine {
                text: "E=mc^2".into(),
                bbox: BBox { x: 0, y: 0, w: 1, h: 1 },
                confidence: 0.9,
            }],
            page: 0,
        };
        let r = FormulaRecognizer
            .recognize(&DynamicImage::new_rgb8(1, 1), &ctx)
            .unwrap();
        assert!(matches!(
            r,
            RegionResult::FormulaV1 {
                latex: None,
                raw_ocr: Some(_)
            }
        ));
    }
}
