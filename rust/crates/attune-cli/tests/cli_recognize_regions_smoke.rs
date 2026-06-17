//! §9.1 E2E subprocess — the agent-invocable shared visual-understanding surface
//! (`attune recognize-regions`, ADR-0008). Verifies via subprocess (no vault, no LLM,
//! no models) that the CLI is the contract a plugin dispatches:
//!   - missing image → exit non-zero, clear error (CapabilityResult exit 1 contract).
//!   - present image, no layout model → 200-equivalent: typed JSON envelope with empty
//!     regions + zero cost + empty correction_report; exit 0 (R1 degrade-to-plain-OCR).
//!
//! Built with `--features nontext` (the subprocess inherits the test's feature set);
//! the test is itself cfg-gated so it only runs in the nontext build.
#![cfg(feature = "nontext")]

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn attune_cmd() -> Command {
    Command::cargo_bin("attune").expect("attune binary should build")
}

/// A minimal valid 4x4 white PNG (no external fixtures). Returns a tempfile kept alive
/// by the caller.
fn tiny_png() -> tempfile::NamedTempFile {
    // Pre-encoded 4x4 white PNG (IHDR + IDAT + IEND), produced once and inlined so the
    // test has zero image-encoding deps.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR len+type
        0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, // 4x4
        0x08, 0x02, 0x00, 0x00, 0x00, 0x26, 0x93, 0x09, // bitdepth/colortype + crc
        0x29, 0x00, 0x00, 0x00, 0x16, 0x49, 0x44, 0x41, // IDAT
        0x54, 0x78, 0x9C, 0x63, 0xFC, 0xFF, 0xFF, 0x3F,
        0x03, 0x35, 0x80, 0x91, 0x81, 0x81, 0x91, 0x11,
        0x00, 0x36, 0xF0, 0x05, 0xFE, 0x6E, 0x9F, 0x9A,
        0x0F, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND
        0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let mut f = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .expect("tempfile");
    f.write_all(PNG).expect("write png");
    f.flush().expect("flush");
    f
}

#[test]
fn recognize_regions_missing_image_exits_nonzero() {
    attune_cmd()
        .args(["recognize-regions", "/nonexistent/path/xyz123.png"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("image file not found"));
}

#[test]
fn recognize_regions_no_models_degrades_to_empty_envelope() {
    let png = tiny_png();
    attune_cmd()
        .args(["recognize-regions"])
        .arg(png.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""capability": "visual-understanding""#))
        .stdout(predicate::str::contains(r#""regions": []"#))
        .stdout(predicate::str::contains(r#""local_regions": 0"#))
        .stdout(predicate::str::contains(r#""total": 0"#));
}

/// Real-model agent-invocable E2E (Task 5): with a layout model bundled at the data_dir
/// path + a real document image, the CLI surface a plugin/agent dispatches must return a
/// FUNCTIONAL engine status + non-empty regions. Env-gated + #[ignore] (the *.onnx model is
/// gitignored and not committed). Run with:
///   ATTUNE_TEST_LAYOUT_MODEL pointing at a layout.onnx already copied to
///   <data_dir>/models/ppocr/layout/layout.onnx, plus ATTUNE_TEST_LAYOUT_IMAGE=<doc.png>:
///   cargo test -p attune-cli --features nontext --release \
///     recognize_regions_real_model_is_functional -- --ignored --nocapture
#[test]
#[ignore]
fn recognize_regions_real_model_is_functional() {
    let image = match std::env::var("ATTUNE_TEST_LAYOUT_IMAGE") {
        Ok(i) => i,
        Err(_) => {
            eprintln!("skip: set ATTUNE_TEST_LAYOUT_IMAGE (and ensure the layout model is at <data_dir>/models/ppocr/layout/layout.onnx)");
            return;
        }
    };
    attune_cmd()
        .args(["recognize-regions"])
        .arg(&image)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""engine_status": "functional""#))
        // at least one detected region (regions array is non-empty)
        .stdout(predicate::str::contains(r#""kind""#));
}
