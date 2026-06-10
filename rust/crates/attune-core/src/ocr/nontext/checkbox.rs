//! R7 checkbox — 🆓 binary checked/unchecked via interior dark-pixel ratio.
//! Zero model, zero GPU; cross-platform + riscv-safe.

use super::{CostTier, RegionCtx, RegionKind, RegionRecognizer, RegionResult};
use crate::error::Result;
use image::DynamicImage;

/// Fraction of dark pixels (luma < 128) inside the inner 60% of the box, above
/// which the checkbox is considered "checked".
pub const CHECKED_DARK_RATIO: f32 = 0.10;

pub struct CheckboxRecognizer;

/// Pure helper (testable without constructing a DynamicImage): given the dark-pixel
/// ratio of the inner region, decide checked.
pub fn is_checked(inner_dark_ratio: f32) -> bool {
    inner_dark_ratio >= CHECKED_DARK_RATIO
}

/// Compute inner dark ratio of a grayscale crop (inner 60% window to exclude the border).
pub fn inner_dark_ratio(img: &DynamicImage) -> f32 {
    let gray = img.to_luma8();
    let (w, h) = (gray.width(), gray.height());
    if w == 0 || h == 0 {
        return 0.0;
    }
    let (x0, x1) = ((w as f32 * 0.2) as u32, (w as f32 * 0.8) as u32);
    let (y0, y1) = ((h as f32 * 0.2) as u32, (h as f32 * 0.8) as u32);
    let mut dark = 0u64;
    let mut total = 0u64;
    for y in y0..y1.max(y0 + 1) {
        for x in x0..x1.max(x0 + 1) {
            total += 1;
            if gray.get_pixel(x, y).0[0] < 128 {
                dark += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        dark as f32 / total as f32
    }
}

impl RegionRecognizer for CheckboxRecognizer {
    fn kind(&self) -> RegionKind {
        RegionKind::Checkbox
    }
    fn recognize(&self, region_crop: &DynamicImage, _ctx: &RegionCtx) -> Result<RegionResult> {
        Ok(RegionResult::CheckboxV1 {
            checked: is_checked(inner_dark_ratio(region_crop)),
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

    fn solid(w: u32, h: u32, v: u8) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([v, v, v])))
    }

    #[test]
    fn threshold_helper() {
        assert!(is_checked(0.10));
        assert!(is_checked(0.5));
        assert!(!is_checked(0.09));
        assert!(!is_checked(0.0));
    }

    #[test]
    fn all_white_box_is_unchecked() {
        let r = CheckboxRecognizer
            .recognize(&solid(40, 40, 255), &RegionCtx { ocr_lines: vec![], page: 0 })
            .unwrap();
        assert!(matches!(r, RegionResult::CheckboxV1 { checked: false }));
    }

    #[test]
    fn all_black_box_is_checked() {
        let r = CheckboxRecognizer
            .recognize(&solid(40, 40, 0), &RegionCtx { ocr_lines: vec![], page: 0 })
            .unwrap();
        assert!(matches!(r, RegionResult::CheckboxV1 { checked: true }));
    }

    #[test]
    fn zero_size_crop_is_unchecked_not_panic() {
        let r = CheckboxRecognizer
            .recognize(&solid(0, 0, 0), &RegionCtx { ocr_lines: vec![], page: 0 })
            .unwrap();
        assert!(matches!(r, RegionResult::CheckboxV1 { checked: false }));
    }
}
