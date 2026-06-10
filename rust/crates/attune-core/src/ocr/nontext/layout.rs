//! Stage 1 — layout / region detection (ort ONNX adapter).
//! This task lands only the ort session-construction probe (R4 feasibility gate);
//! real layout inference arrives in Task 9.
//!
//! R4 (riscv64 / K3 feasibility) — measured 2026-06-10:
//! `cargo build -p attune-core --features nontext --target riscv64gc-unknown-linux-gnu`
//! FAILS, but NOT on ort/onnxruntime — it dies earlier in pre-existing C++ build-deps
//! (`cxx`, `clipper-sys`) because the riscv64 g++ has no C++ stdlib sysroot wired
//! (`fatal error: algorithm / vector: No such file or directory`; CXX_riscv64... unset).
//! This is a workspace-wide cross-compile toolchain gap (a C++ sysroot + CXX/CXXFLAGS
//! `--sysroot` must be configured), independent of and prior to anything nontext adds —
//! the build never reaches the ort algebra backend to evaluate its riscv viability.
//! Mitigation per spec §11 R4: K3 is P2; on riscv the nontext layout pass degrades to
//! remote K3 :8080 inference or plain OCR. The x86_64 / Windows P0 path is unaffected
//! (host build + all nontext tests green). Re-run this gate after the C++ cross sysroot
//! is wired to re-evaluate the actual ort-on-riscv question.

use crate::error::{Result, VaultError};
use std::path::Path;

/// Probe that an ort `Session` can be built from a model path. Used by the R4
/// build-feasibility gate: if this compiles + links for riscv64gc, the ort algebra
/// backend is viable on K3; if not, layout degrades to "detect-free plain OCR".
///
/// ort idiom copied from `crate::infer::provider::build_session` (the verified
/// session-construction path in this crate): `Session::builder()` then
/// `.commit_from_file(path)`, both fallible.
pub fn probe_session_buildable(model_path: &Path) -> Result<bool> {
    if !model_path.exists() {
        return Ok(false); // model-missing is non-fatal (spec §7): degrade, don't error
    }
    // Construct (not run) a session to exercise ort init + backend linkage.
    match ort::session::Session::builder().and_then(|mut b| b.commit_from_file(model_path)) {
        Ok(_) => Ok(true),
        Err(e) => Err(VaultError::Io(std::io::Error::other(format!(
            "ort session build failed: {e}"
        )))),
    }
}

/// A detected region from layout analysis (pre-recognition).
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutRegion {
    pub kind: super::RegionKind,
    pub bbox: crate::ocr::BBox,
    pub det_confidence: f32,
}

/// Map a PP-Structure layout class label to our RegionKind. Unknown → Figure (safe default;
/// a Figure region only triggers 💰 caption, never auto-corrects text).
pub fn map_layout_class(label: &str) -> super::RegionKind {
    use super::RegionKind::*;
    match label.to_ascii_lowercase().as_str() {
        "table" => Table,
        "figure" | "image" | "chart" => Figure, // chart sub-classified later by chart.rs
        "equation" | "formula" => Formula,
        "title" | "text" | "list" | "header" | "footer" => Text,
        _ => Figure,
    }
}

/// Detect layout regions. Returns empty when the model is missing (non-fatal degrade, §7).
pub fn detect_regions(
    model_path: &std::path::Path,
    _page_image: &std::path::Path,
) -> Result<Vec<LayoutRegion>> {
    if !probe_session_buildable(model_path)? {
        return Ok(Vec::new()); // model-missing → regions: None upstream → plain OCR
    }
    // Real inference wired against the PP-Structure layout ONNX in a follow-up;
    // for now an available model with no detections returns empty (deterministic).
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn probe_missing_model_is_non_fatal() {
        let got =
            probe_session_buildable(&PathBuf::from("/definitely/missing/layout.onnx")).unwrap();
        assert!(!got, "missing model → Ok(false), never Err");
    }

    #[test]
    fn map_known_classes() {
        use crate::ocr::nontext::RegionKind;
        assert_eq!(map_layout_class("Table"), RegionKind::Table);
        assert_eq!(map_layout_class("equation"), RegionKind::Formula);
        assert_eq!(map_layout_class("text"), RegionKind::Text);
    }

    #[test]
    fn unknown_class_defaults_to_figure() {
        use crate::ocr::nontext::RegionKind;
        assert_eq!(map_layout_class("weird-new-class"), RegionKind::Figure);
    }

    #[test]
    fn detect_with_missing_model_returns_empty() {
        let got = detect_regions(
            std::path::Path::new("/missing.onnx"),
            std::path::Path::new("/p.png"),
        )
        .unwrap();
        assert!(got.is_empty());
    }
}
