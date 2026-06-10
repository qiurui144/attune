//! R3 figure — region present as figure; class defaults "figure", caption 💰 VLM-only.
use super::{CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::Result;
use image::DynamicImage;

pub struct FigureRecognizer;
impl RegionRecognizer for FigureRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Figure
    }
    fn recognize(&self, _crop: &DynamicImage, _ctx: &RegionCtx) -> Result<RegionResult> {
        Ok(RegionResult::FigureV1 {
            class: "figure".into(),
            caption: None,
        })
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Local
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn figure_caption_none_until_vlm() {
        let r = FigureRecognizer
            .recognize(
                &DynamicImage::new_rgb8(1, 1),
                &RegionCtx { ocr_lines: vec![], page: 0 },
            )
            .unwrap();
        assert!(matches!(r, RegionResult::FigureV1 { caption: None, .. }));
    }
}
