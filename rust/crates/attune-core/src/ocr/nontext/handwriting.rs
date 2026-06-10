//! R5 handwriting — local detection (layout-provided); transcription is 💰 VLM.
use super::{CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::Result;
use image::DynamicImage;

pub struct HandwritingRecognizer;
impl RegionRecognizer for HandwritingRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Handwriting
    }
    fn recognize(&self, _crop: &DynamicImage, _ctx: &RegionCtx) -> Result<RegionResult> {
        Ok(RegionResult::HandwritingV1 { text: None }) // transcription needs 💰 VLM
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Local
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handwriting_text_none_until_vlm() {
        let r = HandwritingRecognizer
            .recognize(
                &DynamicImage::new_rgb8(1, 1),
                &RegionCtx { ocr_lines: vec![], page: 0 },
            )
            .unwrap();
        assert!(matches!(r, RegionResult::HandwritingV1 { text: None }));
    }
}
