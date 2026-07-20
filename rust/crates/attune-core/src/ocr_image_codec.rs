//! Model-independent image input adapter for Scheduler OCR.
//!
//! Attune accepts several common raster formats, while the Scheduler OCR wire
//! contract accepts one decoded page encoded as PNG. This module performs the
//! format bridge without invoking any local OCR/model path. All untrusted input
//! is decoded under fixed resource ceilings and PNG output is written through a
//! bounded sink so malformed or highly expanding images fail closed.

use crate::error::{Result, VaultError};
use image_wire::{GenericImageView, ImageEncoder};
use std::io::{Cursor, Write};

pub(crate) const MAX_ENCODED_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const CANONICAL_CONTENT_TYPE: &str = "image/png";
pub(crate) const MAX_IMAGE_DIMENSION: u32 = 8_192;
pub(crate) const MAX_IMAGE_PIXELS: u64 = 16_000_000;
const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Canonical page bytes plus a conservative, model-independent blank-page
/// observation.  `visually_blank` is true only when every composited RGB
/// channel is uniform (within one 8-bit level), so an empty OCR result can be
/// distinguished from a successfully processed, genuinely blank PDF page.
pub(crate) struct CanonicalOcrImage {
    pub(crate) png: Vec<u8>,
    pub(crate) visually_blank: bool,
}

#[derive(Clone, Copy)]
struct CanonicalizationLimits {
    max_encoded_input_bytes: usize,
    max_image_dimension: u32,
    max_image_pixels: u64,
    max_decoded_bytes: u64,
    max_png_output_bytes: usize,
}

impl CanonicalizationLimits {
    fn scheduler(max_png_output_bytes: usize) -> Self {
        Self {
            max_encoded_input_bytes: MAX_ENCODED_INPUT_BYTES,
            max_image_dimension: MAX_IMAGE_DIMENSION,
            max_image_pixels: MAX_IMAGE_PIXELS,
            max_decoded_bytes: MAX_DECODED_BYTES,
            max_png_output_bytes,
        }
    }
}

/// Decode an advertised Attune raster input and return a canonical RGBA8 PNG.
///
/// `max_png_output_bytes` comes from the Scheduler JSON/body budget. A zero
/// limit is rejected before decoding. Format detection is signature-based, so
/// an extension cannot trick the decoder or the downstream media type.
pub(crate) fn canonicalize_for_scheduler(
    encoded: &[u8],
    max_png_output_bytes: usize,
) -> Result<Vec<u8>> {
    canonicalize_with_limits_analyzed(
        encoded,
        CanonicalizationLimits::scheduler(max_png_output_bytes),
    )
    .map(|image| image.png)
}

pub(crate) fn canonicalize_for_scheduler_with_analysis(
    encoded: &[u8],
    max_png_output_bytes: usize,
) -> Result<CanonicalOcrImage> {
    canonicalize_with_limits_analyzed(
        encoded,
        CanonicalizationLimits::scheduler(max_png_output_bytes),
    )
}

#[cfg(test)]
fn canonicalize_with_limits(encoded: &[u8], limits: CanonicalizationLimits) -> Result<Vec<u8>> {
    canonicalize_with_limits_analyzed(encoded, limits).map(|image| image.png)
}

fn canonicalize_with_limits_analyzed(
    encoded: &[u8],
    limits: CanonicalizationLimits,
) -> Result<CanonicalOcrImage> {
    if encoded.is_empty() {
        return Err(invalid("OCR image is empty"));
    }
    if encoded.len() > limits.max_encoded_input_bytes {
        return Err(invalid(format!(
            "OCR image encoded input exceeds {} bytes",
            limits.max_encoded_input_bytes
        )));
    }
    if limits.max_png_output_bytes == 0 {
        return Err(invalid("Scheduler OCR PNG output budget is empty"));
    }

    let mut reader = image_wire::io::Reader::new(Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|error| invalid(format!("OCR image format detection failed: {error}")))?;
    let format = reader
        .format()
        .ok_or_else(|| invalid("OCR image format is corrupt or unsupported"))?;
    if !matches!(
        format,
        image_wire::ImageFormat::Png
            | image_wire::ImageFormat::Jpeg
            | image_wire::ImageFormat::WebP
            | image_wire::ImageFormat::Bmp
            | image_wire::ImageFormat::Tiff
            | image_wire::ImageFormat::Gif
    ) {
        return Err(invalid(format!(
            "OCR image format {format:?} is not accepted"
        )));
    }

    // Width/height are strict decoder limits in image-rs. max_alloc is an
    // additional decoder-side guard; total_bytes and pixel-product checks below
    // are explicit and performed before the output pixel allocation.
    let mut decoder_limits = image_wire::io::Limits::default();
    decoder_limits.max_image_width = Some(limits.max_image_dimension);
    decoder_limits.max_image_height = Some(limits.max_image_dimension);
    decoder_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(decoder_limits.clone());
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| invalid(format!("OCR image header/limits rejected: {error}")))?;
    if width == 0 || height == 0 {
        return Err(invalid("OCR image dimensions must be non-zero"));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| invalid("OCR image dimensions overflow"))?;
    if pixels > limits.max_image_pixels {
        return Err(invalid(format!(
            "OCR image expands to {pixels} pixels, above the {} pixel limit",
            limits.max_image_pixels
        )));
    }
    let decoded_bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| invalid("OCR image decoded byte count overflows"))?;
    if decoded_bytes > limits.max_decoded_bytes {
        return Err(invalid(format!(
            "OCR image expands to {decoded_bytes} decoded bytes, above the {} byte limit",
            limits.max_decoded_bytes
        )));
    }

    let mut decode_reader = image_wire::io::Reader::with_format(Cursor::new(encoded), format);
    decode_reader.limits(decoder_limits);
    let decoded = decode_reader
        .decode()
        .map_err(|error| invalid(format!("OCR image decode failed: {error}")))?;
    let (decoded_width, decoded_height) = decoded.dimensions();
    if (decoded_width, decoded_height) != (width, height) {
        return Err(invalid("OCR image dimensions changed during decode"));
    }
    let rgba = decoded.to_rgba8();
    let visually_blank = rgba_is_uniform_after_white_composite(rgba.as_raw());

    let mut output = BoundedWriter::new(limits.max_png_output_bytes);
    image_wire::codecs::png::PngEncoder::new(&mut output)
        .write_image(rgba.as_raw(), width, height, image_wire::ColorType::Rgba8)
        .map_err(|error| invalid(format!("OCR canonical PNG encode failed: {error}")))?;
    let png = output.into_inner();
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(invalid(
            "OCR canonical PNG encoder returned an invalid signature",
        ));
    }
    Ok(CanonicalOcrImage {
        png,
        visually_blank,
    })
}

fn rgba_is_uniform_after_white_composite(rgba: &[u8]) -> bool {
    let mut minima = [u8::MAX; 3];
    let mut maxima = [u8::MIN; 3];
    for pixel in rgba.chunks_exact(4) {
        let alpha = u16::from(pixel[3]);
        for channel in 0..3 {
            // OCR ultimately observes the rendered pixel, not hidden RGB
            // values underneath transparency. Composite onto the white page
            // background with integer rounding before measuring uniformity.
            let composited = (u16::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255;
            let composited = composited as u8;
            minima[channel] = minima[channel].min(composited);
            maxima[channel] = maxima[channel].max(composited);
        }
    }
    !rgba.is_empty() && (0..3).all(|channel| maxima[channel].saturating_sub(minima[channel]) <= 1)
}

fn invalid(message: impl Into<String>) -> VaultError {
    VaultError::InvalidInput(message.into())
}

struct BoundedWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl BoundedWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(max_bytes.min(64 * 1024)),
            max_bytes,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.max_bytes.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "canonical PNG exceeds Scheduler OCR body budget",
            ));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generous_limits() -> CanonicalizationLimits {
        CanonicalizationLimits {
            max_encoded_input_bytes: 1024 * 1024,
            max_image_dimension: 1024,
            max_image_pixels: 1024 * 1024,
            max_decoded_bytes: 4 * 1024 * 1024,
            max_png_output_bytes: 1024 * 1024,
        }
    }

    fn encode_fixture(format: image_wire::ImageFormat) -> Vec<u8> {
        let image =
            image_wire::DynamicImage::ImageRgba8(image_wire::RgbaImage::from_fn(8, 6, |x, y| {
                image_wire::Rgba([(x * 23) as u8, (y * 31) as u8, ((x + y) * 17) as u8, 255])
            }));
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, format).unwrap();
        encoded.into_inner()
    }

    #[test]
    fn real_jpeg_is_decoded_and_reencoded_as_png() {
        let jpeg = encode_fixture(image_wire::ImageFormat::Jpeg);
        assert!(jpeg.starts_with(&[0xff, 0xd8, 0xff]));

        let png = canonicalize_with_limits(&jpeg, generous_limits()).unwrap();
        assert!(png.starts_with(PNG_SIGNATURE));
        assert_eq!(CANONICAL_CONTENT_TYPE, "image/png");
        assert_eq!(
            image_wire::load_from_memory(&png).unwrap().dimensions(),
            (8, 6)
        );
    }

    #[test]
    fn every_advertised_codec_canonicalizes_to_png() {
        for format in [
            image_wire::ImageFormat::Png,
            image_wire::ImageFormat::Jpeg,
            image_wire::ImageFormat::WebP,
            image_wire::ImageFormat::Bmp,
            image_wire::ImageFormat::Tiff,
            image_wire::ImageFormat::Gif,
        ] {
            let encoded = encode_fixture(format);
            let png = canonicalize_with_limits(&encoded, generous_limits())
                .unwrap_or_else(|error| panic!("{format:?} failed: {error}"));
            assert!(png.starts_with(PNG_SIGNATURE), "format={format:?}");
        }
    }

    #[test]
    fn blank_page_analysis_is_conservative_and_uses_visible_pixels() {
        let limits = generous_limits();
        let uniform = image_wire::DynamicImage::ImageRgba8(image_wire::RgbaImage::from_pixel(
            8,
            6,
            image_wire::Rgba([255, 255, 255, 255]),
        ));
        let mut encoded = Cursor::new(Vec::new());
        uniform
            .write_to(&mut encoded, image_wire::ImageFormat::Png)
            .unwrap();
        let analyzed = canonicalize_with_limits_analyzed(&encoded.into_inner(), limits).unwrap();
        assert!(analyzed.visually_blank);

        let structured = encode_fixture(image_wire::ImageFormat::Png);
        let analyzed = canonicalize_with_limits_analyzed(&structured, limits).unwrap();
        assert!(!analyzed.visually_blank);

        let transparent_noise =
            image_wire::DynamicImage::ImageRgba8(image_wire::RgbaImage::from_fn(8, 6, |x, y| {
                image_wire::Rgba([(x * 31) as u8, (y * 29) as u8, 7, 0])
            }));
        let mut encoded = Cursor::new(Vec::new());
        transparent_noise
            .write_to(&mut encoded, image_wire::ImageFormat::Png)
            .unwrap();
        let analyzed = canonicalize_with_limits_analyzed(&encoded.into_inner(), limits).unwrap();
        assert!(analyzed.visually_blank);
    }

    #[test]
    fn blank_page_analysis_has_explicit_contrast_and_alpha_boundaries() {
        let analyze = |image: image_wire::RgbaImage| {
            let mut encoded = Cursor::new(Vec::new());
            image_wire::DynamicImage::ImageRgba8(image)
                .write_to(&mut encoded, image_wire::ImageFormat::Png)
                .unwrap();
            canonicalize_with_limits_analyzed(&encoded.into_inner(), generous_limits())
                .unwrap()
                .visually_blank
        };

        let mut one_level =
            image_wire::RgbaImage::from_pixel(8, 6, image_wire::Rgba([255, 255, 255, 255]));
        one_level.put_pixel(0, 0, image_wire::Rgba([254, 255, 255, 255]));
        assert!(analyze(one_level), "one quantization level is tolerated");

        let mut two_levels =
            image_wire::RgbaImage::from_pixel(8, 6, image_wire::Rgba([255, 255, 255, 255]));
        two_levels.put_pixel(0, 0, image_wire::Rgba([253, 255, 255, 255]));
        assert!(!analyze(two_levels), "two visible levels must not be blank");

        let mut semitransparent =
            image_wire::RgbaImage::from_pixel(8, 6, image_wire::Rgba([255, 255, 255, 255]));
        semitransparent.put_pixel(0, 0, image_wire::Rgba([0, 0, 0, 128]));
        assert!(
            !analyze(semitransparent),
            "visible semi-transparent content must not be blank"
        );
    }

    #[test]
    fn corrupt_image_fails_closed() {
        let error = canonicalize_with_limits(b"\xff\xd8\xfftruncated", generous_limits())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("decode") || error.contains("header"),
            "{error}"
        );
    }

    #[test]
    fn oversized_encoded_input_fails_before_decode() {
        let mut limits = generous_limits();
        limits.max_encoded_input_bytes = 8;
        let error = canonicalize_with_limits(&[0; 9], limits)
            .unwrap_err()
            .to_string();
        assert!(error.contains("encoded input exceeds"), "{error}");
    }

    #[test]
    fn oversized_canonical_output_is_never_buffered_past_limit() {
        let png = encode_fixture(image_wire::ImageFormat::Png);
        let mut limits = generous_limits();
        limits.max_png_output_bytes = PNG_SIGNATURE.len();
        let error = canonicalize_with_limits(&png, limits)
            .unwrap_err()
            .to_string();
        assert!(error.contains("body budget"), "{error}");
    }

    #[test]
    fn decompression_bomb_dimensions_fail_before_pixel_allocation() {
        let bomb = png_with_dimensions(50_000, 50_000);
        let error = canonicalize_with_limits(&bomb, generous_limits())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("limits") || error.contains("pixels"),
            "{error}"
        );
    }

    fn png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_png_chunk(&mut png, b"IHDR", &ihdr);
        push_png_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn push_png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(kind.len() + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
