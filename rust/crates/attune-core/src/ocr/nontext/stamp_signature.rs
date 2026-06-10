//! R6/R7 stamp & signature — 🆓 presence detection.
//! Stamp: red connected-ink ratio (Chinese 公章 are red). Signature: dark-stroke
//! presence in a low-fill region. Inner text/type recognition is 💰 VLM (escalation).

use super::{CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::Result;
use image::DynamicImage;

/// Red-pixel ratio above which a stamp is considered present.
pub const STAMP_RED_RATIO: f32 = 0.02;
/// Dark-stroke ratio band for "a signature is present" (low fill, not a solid block).
pub const SIG_MIN_DARK: f32 = 0.01;
pub const SIG_MAX_DARK: f32 = 0.40;

/// Fraction of pixels that are "ink red": R high, G/B low.
pub fn red_ink_ratio(img: &DynamicImage) -> f32 {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    if w == 0 || h == 0 {
        return 0.0;
    }
    let mut red = 0u64;
    let total = (w as u64) * (h as u64);
    for p in rgb.pixels() {
        let [r, g, b] = p.0;
        if r > 120 && (r as i16 - g as i16) > 60 && (r as i16 - b as i16) > 60 {
            red += 1;
        }
    }
    red as f32 / total as f32
}

/// Fraction of dark pixels (luma < 100).
pub fn dark_ratio(img: &DynamicImage) -> f32 {
    let gray = img.to_luma8();
    let total = (gray.width() as u64) * (gray.height() as u64);
    if total == 0 {
        return 0.0;
    }
    let dark = gray.pixels().filter(|p| p.0[0] < 100).count() as u64;
    dark as f32 / total as f32
}

pub struct StampRecognizer;
impl RegionRecognizer for StampRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Stamp
    }
    fn recognize(&self, crop: &DynamicImage, _ctx: &RegionCtx) -> Result<RegionResult> {
        let present = red_ink_ratio(crop) >= STAMP_RED_RATIO;
        // text/stamp_type left None → 💰 VLM fills when escalated.
        Ok(RegionResult::StampV1 {
            present,
            text: None,
            stamp_type: None,
        })
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Free
    }
}

pub struct SignatureRecognizer;
impl RegionRecognizer for SignatureRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Signature
    }
    fn recognize(&self, crop: &DynamicImage, _ctx: &RegionCtx) -> Result<RegionResult> {
        let d = dark_ratio(crop);
        let present = (SIG_MIN_DARK..=SIG_MAX_DARK).contains(&d);
        Ok(RegionResult::SignatureV1 {
            present,
            bbox_of_owner_field: None,
        })
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Free
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    fn solid(w: u32, h: u32, c: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb(c)))
    }

    #[test]
    fn red_block_detected_as_stamp() {
        let r = StampRecognizer
            .recognize(&solid(30, 30, [220, 20, 20]), &RegionCtx { ocr_lines: vec![], page: 0 })
            .unwrap();
        assert!(matches!(r, RegionResult::StampV1 { present: true, .. }));
    }

    #[test]
    fn white_block_no_stamp() {
        let r = StampRecognizer
            .recognize(&solid(30, 30, [255, 255, 255]), &RegionCtx { ocr_lines: vec![], page: 0 })
            .unwrap();
        assert!(matches!(r, RegionResult::StampV1 { present: false, .. }));
    }

    #[test]
    fn solid_black_is_not_signature() {
        // fully black = 1.0 dark > SIG_MAX_DARK → not a signature (it's a filled block)
        let r = SignatureRecognizer
            .recognize(&solid(30, 30, [0, 0, 0]), &RegionCtx { ocr_lines: vec![], page: 0 })
            .unwrap();
        assert!(matches!(r, RegionResult::SignatureV1 { present: false, .. }));
    }

    #[test]
    fn blank_is_not_signature() {
        let r = SignatureRecognizer
            .recognize(&solid(30, 30, [255, 255, 255]), &RegionCtx { ocr_lines: vec![], page: 0 })
            .unwrap();
        assert!(matches!(r, RegionResult::SignatureV1 { present: false, .. }));
    }

    #[test]
    fn zero_size_no_panic() {
        assert_eq!(red_ink_ratio(&solid(0, 0, [255, 0, 0])), 0.0);
        assert_eq!(dark_ratio(&solid(0, 0, [0, 0, 0])), 0.0);
    }
}
