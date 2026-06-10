//! Stage 1 — layout / region detection (PicoDet-LCNet PP-Structure via ort ONNX).
//!
//! Model: `Desperado-JT/RapidLayout-PP-Structure` (Apache-2.0, a RapidAI export of
//! PaddleOCR's PP-Structure layout PicoDet). `layout_cdla.onnx` (10 Chinese-Document-
//! Layout-Analysis classes: table / figure / *_caption / header / footer / reference /
//! equation / text / title). Input `image` f32 NCHW [1,3,800,608]; outputs are the RAW
//! PicoDet head with NO baked-in NMS — 4 classification maps (post-sigmoid probs, C classes)
//! and 4 DFL box-distribution maps (4 sides times 8 reg bins) at strides 8/16/32/64.
//! We run the post-processing here (DFL decode into boxes, then score-threshold and NMS),
//! verified against the python onnxruntime reference on real document scans before this port.
//!
//! R4 (riscv64 / K3 feasibility) — re-measured 2026-06-11 with the real ort layout path:
//! `cargo build -p attune-core --features nontext --target riscv64gc-unknown-linux-gnu`
//! FAILS, but NOT on ort/onnxruntime — it dies earlier in pre-existing C++ build-deps
//! (`cxx`, `clipper-sys`) because the riscv64 g++ has no C++ stdlib sysroot wired. This is
//! a workspace-wide cross-compile toolchain gap, independent of and prior to anything
//! nontext adds. Mitigation per spec §11 R4: K3 is P2; on riscv the nontext layout pass
//! degrades to remote K3 :8080 inference or plain OCR. The x86_64 / Windows P0 path is
//! unaffected (host build + all nontext tests green).
//!
//! 💰 VLM Stage4 escalation (NOT wired here; gate is type-enforced in vlm_escalate.rs):
//! TODO when escalation lands, route 💰 tier to qwen3.6/3.7 multimodal — qwen3.7-max /
//! qwen3.7-plus / qwen3.6-plus / qwen3.6-flash via DashScope. Do NOT call VLM from Stage1.

use crate::error::{Result, VaultError};
use std::path::{Path, PathBuf};

// ── Model geometry (fixed for the RapidLayout PicoDet export) ────────────────────────────
const IN_W: usize = 608;
const IN_H: usize = 800;
const STRIDES: [usize; 4] = [8, 16, 32, 64];
/// DFL reg bins per box side (output channel count 32 = 4 sides × 8 bins).
const REG_MAX: usize = 8;
/// ImageNet normalization (PaddleOCR PP-Structure preprocessing).
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// CDLA label order (matches the RapidLayout PP-Structure CDLA export). Index = class id.
const CDLA_LABELS: [&str; 10] = [
    "table",
    "figure",
    "figure_caption",
    "table_caption",
    "header",
    "footer",
    "reference",
    "equation",
    "text",
    "title",
];

/// Default detection score threshold (below → dropped). 0.4 matches the verified python
/// reference; tables/figures land well above it, noise below.
const SCORE_THRESH: f32 = 0.4;
/// NMS IoU threshold.
const NMS_IOU: f32 = 0.5;

/// Probe that an ort `Session` can be built from a model path. Used by the R4
/// build-feasibility gate and by `detect_regions` before inference.
///
/// ort idiom copied from `crate::infer::provider::build_session`: `Session::builder()`
/// then `.commit_from_file(path)`, both fallible.
pub fn probe_session_buildable(model_path: &Path) -> Result<bool> {
    if !model_path.exists() {
        return Ok(false); // model-missing is non-fatal (spec §7): degrade, don't error
    }
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
        "title" | "text" | "list" | "header" | "footer" | "reference" | "figure_caption"
        | "table_caption" => Text,
        _ => Figure,
    }
}

/// Default layout model path: `<data_dir>/models/ppocr/layout/layout.onnx` (the path the
/// CLI + REST handler look up — see `run_recognize_regions` / `ocr_recognize`).
pub fn default_model_path() -> PathBuf {
    crate::platform::data_dir()
        .join("models")
        .join("ppocr")
        .join("layout")
        .join("layout.onnx")
}

/// One-shot deployment: download the layout ONNX if absent (mirrors PpOcr's auto-download
/// so any deploy path — deb / cargo / source — gets a functional layout engine).
///
/// Source: `Desperado-JT/RapidLayout-PP-Structure` (Apache-2.0). `HF_ENDPOINT` env honored
/// for mirrors (e.g. hf-mirror.com). Atomic write via .tmp rename. No-op when already present.
pub fn ensure_model_downloaded() -> Result<()> {
    let dst = default_model_path();
    if dst.exists() {
        return Ok(());
    }
    let dir = dst.parent().ok_or_else(|| {
        VaultError::ModelLoad("layout model path has no parent dir".into())
    })?;
    std::fs::create_dir_all(dir)
        .map_err(|e| VaultError::ModelLoad(format!("create layout dir {}: {e}", dir.display())))?;
    let hf = std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let url = format!(
        "{}/Desperado-JT/RapidLayout-PP-Structure/resolve/main/layout_cdla.onnx",
        hf.trim_end_matches('/')
    );
    log::info!("layout: model missing, auto-downloading CDLA PicoDet (~7 MB) from {url}");
    let tmp = dst.with_extension("onnx.tmp");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| VaultError::ModelLoad(format!("build http client: {e}")))?;
    let mut resp = client
        .get(&url)
        .send()
        .map_err(|e| VaultError::ModelLoad(format!("download GET {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(VaultError::ModelLoad(format!(
            "download {url} returned status {}",
            resp.status()
        )));
    }
    let mut out = std::fs::File::create(&tmp)
        .map_err(|e| VaultError::ModelLoad(format!("create tmp {}: {e}", tmp.display())))?;
    resp.copy_to(&mut out)
        .map_err(|e| VaultError::ModelLoad(format!("copy_to {}: {e}", tmp.display())))?;
    drop(out);
    std::fs::rename(&tmp, &dst).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        VaultError::ModelLoad(format!("rename {} -> {}: {e}", tmp.display(), dst.display()))
    })?;
    log::info!("layout: model downloaded ✓ {}", dst.display());
    Ok(())
}

/// Whether a real layout ONNX model is present at `model_path`. Lets the orchestrator
/// surface an HONEST `EngineStatus` (scaffold-no-layout-model vs functional) — the C1 truth.
pub fn layout_model_present(model_path: &std::path::Path) -> bool {
    model_path.exists()
}

/// Detect layout regions via real PicoDet inference. Returns empty when the model is
/// missing (non-fatal degrade, §7). A genuine inference error returns `Err` so the
/// orchestrator surfaces `EngineStatus::LayoutError` (never masked as an empty page, I1).
pub fn detect_regions(
    model_path: &std::path::Path,
    page_image: &std::path::Path,
) -> Result<Vec<LayoutRegion>> {
    if !model_path.exists() {
        return Ok(Vec::new()); // model-missing → regions: None upstream → plain OCR
    }
    // Decode + preprocess (resize to fixed input, ImageNet-normalize, NCHW).
    let img = image::open(page_image)
        .map_err(|e| VaultError::Io(std::io::Error::other(format!("open page image: {e}"))))?;
    let (orig_w, orig_h) = (img.width() as f32, img.height() as f32);
    let input = preprocess(&img);

    // Build session + run. ort idiom mirrors `infer::embedding` (Tensor::from_array, inputs!,
    // run, try_extract_tensor) and `infer::provider::build_session` (commit_from_file).
    let mut session = ort::session::Session::builder()
        .and_then(|mut b| b.commit_from_file(model_path))
        .map_err(|e| VaultError::ModelLoad(format!("layout session build: {e}")))?;
    let input_tensor = ort::value::Tensor::<f32>::from_array((vec![1usize, 3, IN_H, IN_W], input))
        .map_err(|e| VaultError::ModelLoad(format!("layout input tensor: {e}")))?;
    // Input name is fixed "image" for the RapidLayout PicoDet export (verified via onnx graph).
    let outputs = session
        .run(ort::inputs! { "image" => input_tensor })
        .map_err(|e| VaultError::ModelLoad(format!("layout ort run: {e}")))?;

    // Pair cls/reg outputs by NAME (ort's `outputs` is a map — iteration order is NOT graph
    // order). The RapidLayout PicoDet head names cls maps transpose_{0,2,4,6}.tmp_0 and reg
    // maps transpose_{1,3,5,7}.tmp_0; level L uses cls=2L, reg=2L+1.
    let extract = |name: &str| -> Result<(Vec<usize>, Vec<f32>)> {
        let val = outputs.get(name).ok_or_else(|| {
            VaultError::ModelLoad(format!("layout output '{name}' missing"))
        })?;
        let (shape, flat) = val
            .try_extract_tensor::<f32>()
            .map_err(|e| VaultError::ModelLoad(format!("layout extract '{name}': {e}")))?;
        Ok((shape.iter().map(|&d| d as usize).collect(), flat.to_vec()))
    };
    let mut tensors: Vec<(Vec<usize>, Vec<f32>)> = Vec::with_capacity(8);
    for lvl in 0..4 {
        tensors.push(extract(&format!("transpose_{}.tmp_0", lvl * 2))?); // cls
        tensors.push(extract(&format!("transpose_{}.tmp_0", lvl * 2 + 1))?); // reg
    }

    let dets = decode_picodet(&tensors)?;
    let kept = nms(dets, NMS_IOU);

    // Scale boxes from model input space back to original image pixels, clamp, map to RegionKind.
    let sx = orig_w / IN_W as f32;
    let sy = orig_h / IN_H as f32;
    let mut regions = Vec::with_capacity(kept.len());
    for d in kept {
        let x1 = (d.x1 * sx).clamp(0.0, orig_w);
        let y1 = (d.y1 * sy).clamp(0.0, orig_h);
        let x2 = (d.x2 * sx).clamp(0.0, orig_w);
        let y2 = (d.y2 * sy).clamp(0.0, orig_h);
        if x2 <= x1 || y2 <= y1 {
            continue; // degenerate box
        }
        let label = CDLA_LABELS.get(d.class).copied().unwrap_or("figure");
        regions.push(LayoutRegion {
            kind: map_layout_class(label),
            bbox: crate::ocr::BBox {
                x: x1 as u32,
                y: y1 as u32,
                w: (x2 - x1) as u32,
                h: (y2 - y1) as u32,
            },
            det_confidence: d.score,
        });
    }
    Ok(regions)
}

// ── PicoDet post-processing (deterministic; verified against python onnxruntime) ─────────

/// One decoded detection in model-input pixel space.
#[derive(Debug, Clone, Copy)]
struct Det {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    class: usize,
}

/// Resize to fixed [IN_H, IN_W], ImageNet-normalize, return CHW f32 (len 3*IN_H*IN_W).
fn preprocess(img: &image::DynamicImage) -> Vec<f32> {
    let resized = img
        .resize_exact(IN_W as u32, IN_H as u32, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut out = vec![0f32; 3 * IN_H * IN_W];
    let plane = IN_H * IN_W;
    for (x, y, px) in resized.enumerate_pixels() {
        let (x, y) = (x as usize, y as usize);
        let idx = y * IN_W + x;
        for c in 0..3 {
            let v = px[c] as f32 / 255.0;
            out[c * plane + idx] = (v - MEAN[c]) / STD[c];
        }
    }
    out
}

/// Decode the 4-level raw PicoDet head into thresholded detections (pre-NMS).
/// `tensors` = 8 (shape, flat) pairs in graph output order: cls@even idx, reg@odd idx.
fn decode_picodet(tensors: &[(Vec<usize>, Vec<f32>)]) -> Result<Vec<Det>> {
    let mut dets = Vec::new();
    for (lvl, &stride) in STRIDES.iter().enumerate() {
        let (cls_shape, cls) = &tensors[lvl * 2];
        let (_reg_shape, reg) = &tensors[lvl * 2 + 1];
        // cls shape [1, A, C]; reg shape [1, A, 4*REG_MAX]
        if cls_shape.len() != 3 {
            return Err(VaultError::ModelLoad(format!(
                "layout cls level {lvl} rank {} != 3",
                cls_shape.len()
            )));
        }
        let anchors = cls_shape[1];
        let n_class = cls_shape[2];
        let fw = IN_W.div_ceil(stride);
        let fh = IN_H.div_ceil(stride);
        if fw * fh != anchors {
            return Err(VaultError::ModelLoad(format!(
                "layout level {lvl} grid {fw}x{fh}={} != anchors {anchors}",
                fw * fh
            )));
        }
        for a in 0..anchors {
            // grid center for anchor a (row-major: a = gy*fw + gx)
            let gx = a % fw;
            let gy = a / fw;
            let cx = (gx as f32 + 0.5) * stride as f32;
            let cy = (gy as f32 + 0.5) * stride as f32;

            // best class (cls already post-sigmoid probabilities)
            let cls_base = a * n_class;
            let mut best_c = 0usize;
            let mut best_s = cls[cls_base];
            for c in 1..n_class {
                let s = cls[cls_base + c];
                if s > best_s {
                    best_s = s;
                    best_c = c;
                }
            }
            if best_s < SCORE_THRESH {
                continue;
            }

            // DFL decode: reg[a] is 4*REG_MAX, softmax each side then expected value * stride.
            let reg_base = a * 4 * REG_MAX;
            let dfl_dist = |side: usize| -> f32 {
                let bins = &reg[reg_base + side * REG_MAX..reg_base + (side + 1) * REG_MAX];
                let maxv = bins.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let exps: [f32; REG_MAX] = std::array::from_fn(|k| (bins[k] - maxv).exp());
                let sum: f32 = exps.iter().sum();
                let acc: f32 = exps.iter().enumerate().map(|(k, &e)| (e / sum) * k as f32).sum();
                acc * stride as f32
            };
            let (dl, dt, dr, db) = (dfl_dist(0), dfl_dist(1), dfl_dist(2), dfl_dist(3));
            dets.push(Det {
                x1: cx - dl,
                y1: cy - dt,
                x2: cx + dr,
                y2: cy + db,
                score: best_s,
                class: best_c,
            });
        }
    }
    Ok(dets)
}

/// Greedy NMS (score-desc, class-agnostic — matches the verified python reference).
fn nms(mut dets: Vec<Det>, iou_th: f32) -> Vec<Det> {
    dets.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut keep: Vec<Det> = Vec::new();
    'outer: for d in dets {
        for k in &keep {
            if iou(&d, k) > iou_th {
                continue 'outer;
            }
        }
        keep.push(d);
    }
    keep
}

fn iou(a: &Det, b: &Det) -> f32 {
    let x1 = a.x1.max(b.x1);
    let y1 = a.y1.max(b.y1);
    let x2 = a.x2.min(b.x2);
    let y2 = a.y2.min(b.y2);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = (a.x2 - a.x1).max(0.0) * (a.y2 - a.y1).max(0.0);
    let area_b = (b.x2 - b.x1).max(0.0) * (b.y2 - b.y1).max(0.0);
    let union = area_a + area_b - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
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
        assert_eq!(map_layout_class("title"), RegionKind::Text);
        assert_eq!(map_layout_class("figure_caption"), RegionKind::Text);
        assert_eq!(map_layout_class("figure"), RegionKind::Figure);
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

    #[test]
    fn default_model_path_under_data_dir() {
        let p = default_model_path();
        assert!(p.ends_with("models/ppocr/layout/layout.onnx"), "{p:?}");
    }

    #[test]
    fn nms_suppresses_overlapping_lower_score() {
        // Two near-identical boxes → only the higher-score one survives.
        let dets = vec![
            Det { x1: 0.0, y1: 0.0, x2: 100.0, y2: 100.0, score: 0.9, class: 0 },
            Det { x1: 5.0, y1: 5.0, x2: 105.0, y2: 105.0, score: 0.6, class: 0 },
            Det { x1: 500.0, y1: 500.0, x2: 600.0, y2: 600.0, score: 0.7, class: 1 },
        ];
        let kept = nms(dets, 0.5);
        assert_eq!(kept.len(), 2, "overlapping low-score box should be suppressed");
        assert!((kept[0].score - 0.9).abs() < 1e-6, "highest score kept first");
    }

    #[test]
    fn iou_identical_is_one_disjoint_is_zero() {
        let a = Det { x1: 0.0, y1: 0.0, x2: 10.0, y2: 10.0, score: 1.0, class: 0 };
        let b = a;
        assert!((iou(&a, &b) - 1.0).abs() < 1e-6);
        let c = Det { x1: 100.0, y1: 100.0, x2: 110.0, y2: 110.0, score: 1.0, class: 0 };
        assert_eq!(iou(&a, &c), 0.0);
    }

    #[test]
    fn decode_thresholds_and_dfl_decodes_a_box() {
        // Synthetic single-level-shaped tensor set is awkward (decode iterates all 4 levels);
        // instead validate DFL math via a minimal 1-anchor-per-level construction where only
        // level-0 anchor 0 exceeds threshold. Each level needs grid-consistent anchor counts,
        // so we build full-size zero tensors and inject one high-confidence anchor at L0/a0.
        let mut tensors: Vec<(Vec<usize>, Vec<f32>)> = Vec::new();
        for &stride in &STRIDES {
            let fw = IN_W.div_ceil(stride);
            let fh = IN_H.div_ceil(stride);
            let a = fw * fh;
            let n_class = 10usize;
            tensors.push((vec![1, a, n_class], vec![0.01f32; a * n_class])); // cls
            tensors.push((vec![1, a, 4 * REG_MAX], vec![0.0f32; a * 4 * REG_MAX])); // reg
        }
        // Inject: L0 anchor 0, class "table"(0) prob 0.95. reg all-zero → softmax uniform →
        // each side dist = mean(0..8) * stride = 3.5 * 8 = 28 px from center (cx=cy=4).
        let (_cs, cls0) = &mut tensors[0];
        cls0[0] = 0.95; // anchor 0, class 0
        let dets = decode_picodet(&tensors).unwrap();
        assert_eq!(dets.len(), 1, "exactly one anchor above threshold");
        let d = dets[0];
        assert_eq!(d.class, 0);
        assert!((d.score - 0.95).abs() < 1e-6);
        // center (4,4), uniform DFL expected value 3.5*stride(8)=28 each side
        assert!((d.x1 - (4.0 - 28.0)).abs() < 1e-3, "x1={}", d.x1);
        assert!((d.x2 - (4.0 + 28.0)).abs() < 1e-3, "x2={}", d.x2);
    }

    /// Real-model inference smoke test. Env-gated + #[ignore] (the *.onnx model is gitignored
    /// and not committed; see `ensure_model_downloaded`). Run with:
    ///   ATTUNE_TEST_LAYOUT_MODEL=~/.local/share/attune/models/ppocr/layout/layout.onnx \
    ///   ATTUNE_TEST_LAYOUT_IMAGE=/path/to/doc_with_table.png \
    ///   cargo test -p attune-core --features nontext --release layout_real_inference -- --ignored --nocapture
    #[test]
    #[ignore]
    fn layout_real_inference_finds_regions() {
        let model = match std::env::var("ATTUNE_TEST_LAYOUT_MODEL") {
            Ok(m) => PathBuf::from(m),
            Err(_) => {
                eprintln!("skip: set ATTUNE_TEST_LAYOUT_MODEL");
                return;
            }
        };
        let image = match std::env::var("ATTUNE_TEST_LAYOUT_IMAGE") {
            Ok(i) => PathBuf::from(i),
            Err(_) => {
                eprintln!("skip: set ATTUNE_TEST_LAYOUT_IMAGE");
                return;
            }
        };
        let regions = detect_regions(&model, &image).expect("inference ok");
        eprintln!("detected {} regions:", regions.len());
        for r in &regions {
            eprintln!("  {:?} {:?} conf={:.3}", r.kind, r.bbox, r.det_confidence);
        }
        assert!(!regions.is_empty(), "a real document should yield ≥1 region");
        // every region has a positive-area bbox and a plausible confidence
        for r in &regions {
            assert!(r.bbox.w > 0 && r.bbox.h > 0, "positive area: {:?}", r.bbox);
            assert!(r.det_confidence >= SCORE_THRESH, "above threshold");
        }
    }
}
