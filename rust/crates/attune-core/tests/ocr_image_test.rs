//! Image → OCR routing + quality tests (SDLC dimension: Image→OCR routing/quality).
//!
//! Two layers, by design (per CLAUDE.md §6.1 + the "no false-green" rule):
//!
//!   1. ROUTING (default lane, model-state-independent, fast):
//!      Asserts that `parser::parse_file` / `parse_bytes` DISPATCH `.png/.jpg/...`
//!      straight to the OCR branch (images skip the `needs_ocr` text-layer gate
//!      that PDFs use — they always go to OCR). Proven WITHOUT a working OCR
//!      model by using corrupt/empty fixtures whose error is OCR-domain and is
//!      NOT the generic "unsupported file format" text-branch error. Holds
//!      whether PP-OCR models are present (it errors on garbage) or absent
//!      (`detect_default_provider()` → None → "OCR provider unavailable").
//!
//!   2. REAL OCR (env-gated `ATTUNE_TEST_REAL_OCR=1`, `#[ignore]`):
//!      Runs the genuine PP-OCRv5 leg on `known_text.png` / `known_text.jpg`
//!      (rendered text, NO text layer) and records CER against ground truth.
//!      Skipped (not failed) when the models are absent.
//!
//! Fixtures are produced by the committed `fixtures/ocr_image/gen_ocr_image_fixtures.py`
//! and are byte-deterministic. See that script's header.
//!
//! Run:
//!   cargo test -p attune-core --test ocr_image_test                  # routing only
//!   ATTUNE_TEST_REAL_OCR=1 cargo test -p attune-core --test ocr_image_test -- --ignored --nocapture

use std::path::{Path, PathBuf};

use attune_core::parser::{is_supported, parse_bytes, parse_file};

/// Ground truth rendered in known_text.{png,jpg} (must match KNOWN_TEXT in
/// fixtures/ocr_image/gen_ocr_image_fixtures.py).
const KNOWN_TEXT: &str = "ATTUNE OCR TEST 2026";

/// Acceptance ceiling for the real-OCR leg on this clean synthetic image.
/// Synthetic black-on-white ASCII is the easy case for PP-OCRv5 mobile.
const MAX_CER: f64 = 0.30;

/// Image extensions parser.rs routes to the OCR branch.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tiff", "tif", "gif"];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ocr_image")
}

// ───────────────────────── Layer 1: ROUTING (model-independent) ─────────────────────────

/// Every image extension is a supported (routable) type, case-insensitively.
#[test]
fn image_extensions_are_supported() {
    for ext in IMAGE_EXTS {
        let p = format!("scan.{ext}");
        assert!(is_supported(Path::new(&p)), ".{ext} must be supported");
        let up = format!("SCAN.{}", ext.to_uppercase());
        assert!(
            is_supported(Path::new(&up)),
            ".{} (uppercase) must be supported",
            ext.to_uppercase()
        );
    }
}

/// Bytes-API routing: an image extension must NOT fall through to the text
/// branch. The error must be OCR-domain, never "unsupported file format". We
/// feed a non-image payload so a working model (if present) rejects it fast.
#[test]
fn image_bytes_route_to_ocr_not_text_branch() {
    let not_image = std::fs::read(fixtures_dir().join("not_an_image.png"))
        .expect("not_an_image.png fixture must exist (run gen_ocr_image_fixtures.py)");

    for ext in IMAGE_EXTS {
        let filename = format!("masquerade.{ext}");
        let result = parse_bytes(&not_image, &filename);
        let err = result
            .expect_err(&format!(".{ext} non-image payload must Err, not Ok"))
            .to_string()
            .to_lowercase();
        assert!(
            !err.contains("unsupported file format"),
            ".{ext} must NOT hit the unsupported-text branch; got: {err}"
        );
        assert!(
            is_ocr_domain_error(&err),
            ".{ext} error must be OCR-domain (ocr/pp-ocr/provider); got: {err}"
        );
    }
}

/// File-API routing on a real on-disk 0-byte `.png`: exercises `parse_file` and
/// the empty-file edge. Must be a graceful OCR-domain Err, never a panic.
#[test]
fn image_file_zero_byte_routes_to_ocr_graceful_err() {
    let path = fixtures_dir().join("zero_byte.png");
    assert_eq!(
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(1),
        0,
        "zero_byte.png fixture must be empty"
    );
    let err = parse_file(&path)
        .expect_err("0-byte .png must return a graceful Err, never panic")
        .to_string()
        .to_lowercase();
    assert!(
        !err.contains("unsupported file format"),
        "0-byte .png routed to wrong branch; got: {err}"
    );
    assert!(
        is_ocr_domain_error(&err),
        "0-byte .png error must be OCR-domain; got: {err}"
    );
}

/// 0-byte image via the bytes API across all image extensions — the empty-input
/// edge for every alias.
#[test]
fn image_empty_bytes_err_gracefully_all_exts() {
    for ext in IMAGE_EXTS {
        let filename = format!("empty.{ext}");
        let err = parse_bytes(b"", &filename)
            .expect_err(&format!("empty .{ext} must Err"))
            .to_string()
            .to_lowercase();
        assert!(
            !err.contains("unsupported file format"),
            "empty .{ext} must route to OCR branch; got: {err}"
        );
        assert!(
            is_ocr_domain_error(&err),
            "empty .{ext} error must be OCR-domain; got: {err}"
        );
    }
}

/// Negative control: a non-image binary type (.exe) must be rejected as
/// unsupported, proving image routing is specific — not "any binary → OCR".
#[test]
fn executable_extension_is_not_routed_to_ocr() {
    assert!(!is_supported(Path::new("tool.exe")), ".exe must NOT be supported");
    let err = parse_bytes(b"MZ\x90\x00", "tool.exe")
        .expect_err(".exe must Err")
        .to_string()
        .to_lowercase();
    assert!(
        err.contains("unsupported"),
        ".exe must hit the unsupported branch (not OCR); got: {err}"
    );
}

/// An OCR-domain error names the OCR pipeline, distinguishing it from the text
/// branch. Covers both states:
///   - models absent  → "ocr provider unavailable"
///   - models present → "ocr returned empty text", PP-OCR decode errors, etc.
fn is_ocr_domain_error(err_lower: &str) -> bool {
    err_lower.contains("ocr")
        || err_lower.contains("pp-ocr")
        || err_lower.contains("provider")
        || err_lower.contains("image")
}

// ───────────────────────── Layer 2: REAL OCR (env-gated) ─────────────────────────

/// Real PP-OCRv5 on rendered known text, scored by CER. Gated by
/// `ATTUNE_TEST_REAL_OCR=1` AND model presence — self-skips (not fails) when
/// PP-OCR models are absent (the case in this env).
#[test]
#[ignore]
fn real_ocr_reads_known_text_within_cer() {
    if std::env::var("ATTUNE_TEST_REAL_OCR").ok().as_deref() != Some("1") {
        eprintln!("skip: set ATTUNE_TEST_REAL_OCR=1 to run the real-model leg");
        return;
    }
    if attune_core::ocr::ppocr::PpOcrProvider::new().is_none() {
        eprintln!("skip: PP-OCR models absent (PpOcrProvider::new == None)");
        return;
    }

    let want = normalize(KNOWN_TEXT);
    for name in ["known_text.png", "known_text.jpg"] {
        let path = fixtures_dir().join(name);
        if !path.exists() {
            eprintln!("skip {name}: fixture missing — run gen_ocr_image_fixtures.py");
            continue;
        }
        let (title, content) = parse_file(&path)
            .unwrap_or_else(|e| panic!("real OCR on {name} must succeed: {e}"));
        let got = normalize(&content);
        let cer = char_error_rate(&want, &got);
        eprintln!("real OCR {name}: title={title:?}");
        eprintln!("real OCR {name}: want = {want:?}");
        eprintln!("real OCR {name}: got  = {got:?}");
        eprintln!("real OCR {name}: CER  = {cer:.3} (ceiling {MAX_CER:.2})");
        assert!(
            cer <= MAX_CER,
            "real OCR {name} CER {cer:.3} exceeded ceiling {MAX_CER:.2}\n want={want:?}\n got={got:?}"
        );
    }
}

// ───────────────────────── helpers ─────────────────────────

/// Lowercase, drop non-alphanumerics, collapse whitespace — compare content.
fn normalize(s: &str) -> String {
    let cleaned: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Character Error Rate = Levenshtein(want, got) / len(want). 0.0 = perfect.
fn char_error_rate(want: &str, got: &str) -> f64 {
    let w: Vec<char> = want.chars().collect();
    let g: Vec<char> = got.chars().collect();
    if w.is_empty() {
        return if g.is_empty() { 0.0 } else { 1.0 };
    }
    let dist = levenshtein(&w, &g);
    dist as f64 / w.len() as f64
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod cer_self_tests {
    use super::*;

    #[test]
    fn cer_identical_is_zero() {
        assert_eq!(char_error_rate("attune ocr test 2026", "attune ocr test 2026"), 0.0);
    }

    #[test]
    fn cer_handles_insertions() {
        // "ab" vs "axb" → 1 insertion / 2 chars
        assert!((char_error_rate("ab", "axb") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn normalize_collapses_and_lowercases() {
        assert_eq!(normalize("ATTUNE  OCR\nTEST, 2026!"), "attune ocr test 2026");
    }
}
