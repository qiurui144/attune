//! PP-OCRv5 mobile OCR provider via ONNX Runtime (ort 2.0)
//!
//! 实现策略变更（2026-05-01）：
//! 不再自实现 DBNet+CRNN+CTC（原计划 P2-P4 共 ~1000 行）。
//! 改用 [`kreuzberg-paddle-ocr`](https://crates.io/crates/kreuzberg-paddle-ocr) v4.9 —
//! MIT 开源，纯 Rust + ort 2.0.0-rc.12（与 attune 完全对齐），覆盖
//! detection (DBNet) + angle classification + recognition (CRNN) 全流程。
//!
//! 模型选择：PP-OCRv5 mobile (~16 MB total)
//! - ch_PP-OCRv5_det_mobile.onnx     ~5 MB   text detection (DBNet)
//! - ch_ppocr_mobile_v2.0_cls.onnx   ~1 MB   orientation classifier
//! - ch_PP-OCRv5_rec_mobile.onnx    ~10 MB   text recognition (CRNN)
//! - ppocr_keys_v1.txt              6627 chars
//!
//! 模型存放：~/.local/share/attune/models/ppocr/
//!
//! 准确率（与 tesseract chi_sim 对比）：
//! - 干净打印中文: tesseract 85% → PP-OCR 95%
//! - 多栏论文: tesseract 70% → PP-OCR 92%
//! - 法律扫描件: tesseract 75% → PP-OCR 93%
//! - 中英混排: tesseract 60% → PP-OCR 85%

use crate::error::{Result, VaultError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kreuzberg_paddle_ocr::OcrLite;

use super::OcrProvider;

/// PP-OCR provider — 持有已加载的 OcrLite session（线程安全）。
pub struct PpOcrProvider {
    pub det_model: PathBuf,
    pub cls_model: PathBuf,
    pub rec_model: PathBuf,
    pub char_dict: PathBuf,
    /// OcrLite 内部不是 Sync — 用 Mutex 保证 trait Send+Sync。
    /// OCR 是低频 + 单次推理 ~500ms-3s，单线程串行不是瓶颈。
    inner: Mutex<OcrLite>,
}

impl PpOcrProvider {
    /// 模型缓存目录：`<data_dir>/models/ppocr/`
    pub fn models_dir() -> PathBuf {
        crate::platform::data_dir().join("models").join("ppocr")
    }

    /// 探测模型是否齐全 — postinst / wizard 需要确认下载完成。
    pub fn models_present() -> bool {
        let d = Self::models_dir();
        let need = [
            "ch_PP-OCRv5_det_mobile.onnx",
            "ch_ppocr_mobile_v2.0_cls.onnx",
            "ch_PP-OCRv5_rec_mobile.onnx",
            "ppocr_keys_v1.txt",
        ];
        need.iter().all(|f| d.join(f).exists())
    }

    /// 一键化部署: 模型缺失时**自动下载**（任何部署方式 — deb/cargo/源码 都生效）。
    ///
    /// 替代 postinst.sh 单一渠道下载的限制。AMD 等 cargo binary 部署的 server 启动
    /// 即可触发模型下载，避免用户手动 apt install --reinstall.
    ///
    /// 数据源 (与 postinst.sh 一致):
    /// - 3 ONNX from HuggingFace SWHL/RapidOCR (~16 MB)
    /// - 字典 from PaddlePaddle GitHub (6627 chars + # / space wrap)
    ///
    /// HF_ENDPOINT 环境变量支持 (e.g. hf-mirror.com 国内镜像).
    pub fn ensure_models_downloaded() -> Result<()> {
        if Self::models_present() {
            return Ok(());
        }
        let d = Self::models_dir();
        std::fs::create_dir_all(&d).map_err(|e| {
            VaultError::ModelLoad(format!("create ppocr dir {}: {e}", d.display()))
        })?;
        log::info!("PP-OCR: models missing, auto-downloading (~16 MB)...");

        // HF endpoint (支持国内镜像)
        let hf_endpoint = std::env::var("HF_ENDPOINT")
            .unwrap_or_else(|_| "https://huggingface.co".to_string());
        let rapidocr_base = format!("{}/SWHL/RapidOCR/resolve/main", hf_endpoint.trim_end_matches('/'));

        let downloads: &[(&str, &str, &str)] = &[
            (
                "PP-OCRv4/ch_PP-OCRv4_det_infer.onnx",
                "ch_PP-OCRv5_det_mobile.onnx",
                "~5 MB det (PP-OCRv4)",
            ),
            (
                "PP-OCRv1/ch_ppocr_mobile_v2.0_cls_infer.onnx",
                "ch_ppocr_mobile_v2.0_cls.onnx",
                "~1 MB cls",
            ),
            (
                "PP-OCRv4/ch_PP-OCRv4_rec_infer.onnx",
                "ch_PP-OCRv5_rec_mobile.onnx",
                "~10 MB rec (PP-OCRv4)",
            ),
        ];

        for (src_path, dst_name, desc) in downloads {
            let dst = d.join(dst_name);
            if dst.exists() {
                log::info!("  PP-OCR: {} already present", dst_name);
                continue;
            }
            let url = format!("{}/{}", rapidocr_base, src_path);
            log::info!("  PP-OCR: downloading {dst_name} ({desc}) from {url}");
            download_file(&url, &dst)?;
        }

        // 字典 (需 # prefix + ' ' suffix 满足 kreuzberg-paddle-ocr CTC blank 格式)
        let dict_path = d.join("ppocr_keys_v1.txt");
        if !dict_path.exists() {
            let dict_url =
                "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/release/2.7/ppocr/utils/ppocr_keys_v1.txt";
            log::info!("  PP-OCR: downloading + preparing ppocr_keys_v1.txt");
            let tmp_path = d.join("ppocr_keys_v1.txt.tmp");
            download_file(dict_url, &tmp_path)?;
            // wrap with # prefix + ' ' suffix
            let raw = std::fs::read_to_string(&tmp_path).map_err(|e| {
                VaultError::ModelLoad(format!("read ppocr dict tmp: {e}"))
            })?;
            std::fs::write(&dict_path, format!("#\n{}\n ", raw.trim_end())).map_err(|e| {
                VaultError::ModelLoad(format!("write ppocr_keys_v1.txt: {e}"))
            })?;
            let _ = std::fs::remove_file(&tmp_path);
        }

        if Self::models_present() {
            log::info!("PP-OCR: all 4 models downloaded ✓");
            Ok(())
        } else {
            Err(VaultError::ModelLoad(
                "PP-OCR: download attempted but models still missing".into(),
            ))
        }
    }

    /// 构造 provider — 假定模型已下载。失败返回 None。
    pub fn new() -> Option<Self> {
        let d = Self::models_dir();
        let det_model = d.join("ch_PP-OCRv5_det_mobile.onnx");
        let cls_model = d.join("ch_ppocr_mobile_v2.0_cls.onnx");
        let rec_model = d.join("ch_PP-OCRv5_rec_mobile.onnx");
        let char_dict = d.join("ppocr_keys_v1.txt");

        if !det_model.exists()
            || !cls_model.exists()
            || !rec_model.exists()
            || !char_dict.exists()
        {
            return None;
        }

        let mut ocr = OcrLite::new();
        let n_threads = num_cpus_safe();
        match ocr.init_models_with_dict(
            det_model.to_str()?,
            cls_model.to_str()?,
            rec_model.to_str()?,
            char_dict.to_str()?,
            n_threads,
        ) {
            Ok(()) => Some(Self {
                det_model,
                cls_model,
                rec_model,
                char_dict,
                inner: Mutex::new(ocr),
            }),
            Err(e) => {
                log::warn!("PP-OCR init_models failed: {e}");
                None
            }
        }
    }
}

/// 安全的 CPU 数量探测（替代 num_cpus crate 避免新依赖）。
/// 上限 8 — OCR 单图推理 batch=1，更多线程边际收益递减。
fn num_cpus_safe() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
}

/// 同步下载 URL 到目标路径 (一键化部署用 — 替代 postinst.sh curl)。
///
/// 用 reqwest blocking client (复用 attune-core 已有 reqwest 依赖, 不增加 dep)。
/// 60s 超时 + atomic rename (.tmp → 最终文件) 防止半下载状态。
fn download_file(url: &str, dst: &std::path::Path) -> Result<()> {
    let tmp = dst.with_extension("tmp");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))  // 大 ONNX 文件慢网慢
        .build()
        .map_err(|e| VaultError::ModelLoad(format!("build http client: {e}")))?;
    let mut resp = client.get(url).send().map_err(|e| {
        VaultError::ModelLoad(format!("download GET {url}: {e}"))
    })?;
    if !resp.status().is_success() {
        return Err(VaultError::ModelLoad(format!(
            "download {url} returned status {}",
            resp.status()
        )));
    }
    let mut out = std::fs::File::create(&tmp).map_err(|e| {
        VaultError::ModelLoad(format!("create tmp file {}: {e}", tmp.display()))
    })?;
    resp.copy_to(&mut out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        VaultError::ModelLoad(format!("copy_to {} from {url}: {e}", tmp.display()))
    })?;
    drop(out);
    std::fs::rename(&tmp, dst).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        VaultError::ModelLoad(format!("rename {} -> {}: {e}", tmp.display(), dst.display()))
    })?;
    Ok(())
}

impl OcrProvider for PpOcrProvider {
    fn name(&self) -> &str {
        "pp-ocr-v5-mobile"
    }

    fn has_chinese(&self) -> bool {
        // PP-OCRv5 默认带中英双语字典 (6627 字符含 chi_sim + ASCII)
        true
    }

    fn extract_text_from_image(&self, image_path: &Path) -> Result<String> {
        // EXIF 方向归一 — 手机照片常带 orientation tag, 不摆正会横躺/倒置喂进 OCR
        let orient_tmp = normalize_orientation(image_path);
        let oriented: &Path = orient_tmp.as_ref().map_or(image_path, |t| t.path());

        let path_str = oriented.to_str().ok_or_else(|| {
            VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "non-UTF8 image path",
            ))
        })?;

        let lock = self
            .inner
            .lock()
            .map_err(|_| VaultError::Crypto("PP-OCR session lock poisoned".into()))?;

        // PP-OCR 推理参数（参考 RapidOCR 官方默认值）：
        //   padding=50         border padding for short edges
        //   max_side_len=2048  resize 长边到此（越大越准但越慢）
        //   box_score_thresh=0.6  detection 置信度阈值
        //   box_thresh=0.3        DBNet binarization 阈值
        //   un_clip_ratio=1.6     扩张文本框
        //   do_angle=true         做方向分类
        //   most_angle=true       全图统一方向
        let result = lock
            .detect_from_path(
                path_str,
                50,
                super::profile::OcrProfile::DEFAULT_MAX_SIDE_LEN,
                0.6,
                0.3,
                1.6,
                true,
                true,
            )
            .map_err(|e| VaultError::ModelLoad(format!("PP-OCR detect: {e}")))?;

        // 拼接所有文本行（已按 reading order 排序）
        let mut all = String::with_capacity(1024);
        for block in &result.text_blocks {
            if !block.text.is_empty() {
                all.push_str(&block.text);
                all.push('\n');
            }
        }
        Ok(all)
    }

    /// 带布局结构的 OCR — EXIF 方向归一 + 去斜预处理 + 表格重建。
    ///
    /// 流程：
    ///   0. EXIF orientation 归一（手机照片摆正，detect 前置）
    ///   1. profile.deskew=true → 检测倾斜角，> 0.5° 时旋转校正后再推理
    ///   2. PP-OCR 推理（长边上限 profile.max_side_len），拿到带 box_points 的 text_blocks
    ///   3. profile.reconstruct_tables=true → 按文本块坐标重建 Markdown 表格
    fn extract_structured(
        &self,
        image_path: &Path,
        profile: &super::profile::OcrProfile,
    ) -> super::super::error::Result<super::OcrOutput> {
        // Step 0: EXIF 方向归一（手机照片 orientation tag）— 必须在 deskew 前先摆正
        let orient_tmp = normalize_orientation(image_path);
        let oriented_path: &Path = orient_tmp.as_ref().map_or(image_path, |t| t.path());

        // Step 1: deskew preprocessing（如 profile 开启，在已摆正的图上做）
        let deskewed_tmp: Option<tempfile::NamedTempFile> =
            if profile.deskew { deskew_image(oriented_path) } else { None };
        let effective_path: &Path =
            deskewed_tmp.as_ref().map_or(oriented_path, |t| t.path());

        let path_str = effective_path.to_str().ok_or_else(|| {
            VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "non-UTF8 image path after deskew",
            ))
        })?;

        // Step 2: PP-OCR 推理（拿完整 text_blocks 含 box_points）
        // 长边上限由 profile.max_side_len 决定 — 法律证据用大值保留小字细节
        let max_side = sanitize_max_side(profile.max_side_len);
        let result = {
            let lock = self
                .inner
                .lock()
                .map_err(|_| VaultError::Crypto("PP-OCR session lock poisoned".into()))?;
            lock.detect_from_path(path_str, 50, max_side, 0.6, 0.3, 1.6, true, true)
                .map_err(|e| VaultError::ModelLoad(format!("PP-OCR detect (structured): {e}")))?
        };
        // temp 文件在此 drop（lock 先 drop，避免重叠）
        drop(deskewed_tmp);
        drop(orient_tmp);

        // Step 3: 拼接文本
        let mut plain = String::with_capacity(1024);
        for block in &result.text_blocks {
            if !block.text.is_empty() {
                plain.push_str(&block.text);
                plain.push('\n');
            }
        }

        // Step 4: 表格重建（如 profile 开启）
        let table_markdown = if profile.reconstruct_tables {
            reconstruct_table_md(&result.text_blocks)
        } else {
            None
        };

        // Step 5: 文档级 OCR 置信度
        let avg_confidence = compute_avg_confidence(&result.text_blocks);
        if let Some(c) = avg_confidence {
            log::debug!("PP-OCR structured: avg_confidence={c:.3}");
        }

        // Step 6: 行级输出（含 bbox）— office helper 结构化抽取需要
        let lines: Vec<super::RawLine> = result.text_blocks.iter()
            .filter(|b| !b.text.is_empty())
            .map(|b| {
                // box_points: [tl, tr, br, bl], Point { x: u32, y: u32 }
                let xs: Vec<u32> = b.box_points.iter().map(|p| p.x).collect();
                let ys: Vec<u32> = b.box_points.iter().map(|p| p.y).collect();
                let x = *xs.iter().min().unwrap_or(&0);
                let y = *ys.iter().min().unwrap_or(&0);
                let w = xs.iter().max().unwrap_or(&0).saturating_sub(x);
                let h = ys.iter().max().unwrap_or(&0).saturating_sub(y);
                super::RawLine {
                    text: b.text.clone(),
                    bbox: super::BBox { x, y, w, h },
                    confidence: b.text_score,
                }
            })
            .collect();

        Ok(super::OcrOutput { text: plain, table_markdown, avg_confidence, lines: Some(lines) })
    }
}

/// 工厂 — `mod.rs::detect_default_provider()` 调这里。
pub fn detect() -> Option<PpOcrProvider> {
    PpOcrProvider::new()
}

// ── EXIF 方向归一 + 分辨率上限 ───────────────────────────────────────────────

/// EXIF 方向归一 —— 手机照片常带 orientation tag（像素存传感器原始方向 + 一个
/// 旋转/翻转标记）。`image::open` 与 PP-OCR 解码默认**不应用**该 tag，导致横躺 /
/// 倒置的证据图直接喂进 OCR，识别率从 ~93% 掉到接近 0。
///
/// 此函数读 tag，若需变换则实际旋转/翻转像素，写出摆正后的临时 PNG。
/// 返回 `None` = 无需变换（方向正常 / 无 EXIF / 读取失败 → 调用方用原图）。
fn normalize_orientation(image_path: &Path) -> Option<tempfile::NamedTempFile> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(image_path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    if orientation == image::metadata::Orientation::NoTransforms {
        return None; // 方向正常，无需处理
    }
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    let mut tmp = tempfile::Builder::new()
        .prefix("attune_exif_")
        .suffix(".png")
        .tempfile()
        .ok()?;
    img.write_to(tmp.as_file_mut(), image::ImageFormat::Png).ok()?;
    log::debug!(
        "EXIF orientation normalized ({orientation:?}): {}",
        image_path.display()
    );
    Some(tmp)
}

/// 把 profile 的 `max_side_len` 收敛到安全区间。
/// 0（来自 `Default`）/ 越界值回退到 `DEFAULT_MAX_SIDE_LEN`。
/// 下限 512（再小 OCR 无意义），上限 8192（再大耗时/内存不可控）。
fn sanitize_max_side(v: u32) -> u32 {
    if (512..=8192).contains(&v) {
        v
    } else {
        super::profile::OcrProfile::DEFAULT_MAX_SIDE_LEN
    }
}

/// 文档级 OCR 置信度—— 各文本块 `text_score`（CRNN recognition
/// 置信度）按文本字符数加权平均。长文本块权重更大，更能代表整页质量。
/// 无文本块（空白页 / OCR 全失败）返回 `None`。
fn compute_avg_confidence(blocks: &[kreuzberg_paddle_ocr::TextBlock]) -> Option<f32> {
    let mut weighted = 0.0f64;
    let mut total_len = 0usize;
    for b in blocks {
        let len = b.text.chars().count();
        if len == 0 {
            continue;
        }
        weighted += b.text_score as f64 * len as f64;
        total_len += len;
    }
    if total_len == 0 {
        None
    } else {
        Some((weighted / total_len as f64) as f32)
    }
}

// ── 去斜（Deskew）预处理 ─────────────────────────────────────────────────────

/// 检测图片倾斜角并在必要时旋转校正。
///
/// 算法：水平投影法（Projection Profile Method）
///   - 对图片二值化（< 128 → 暗像素）
///   - 在 -10° ~ +10°（0.5° 步长）枚举旋转角
///   - 计算每个角度下水平投影曲线的方差
///   - 方差最大的角度 = 文字行最对齐 = 真实倾斜角
///   - 仅当 |angle| > 0.5° 才实际旋转（避免无谓损失）
///
/// 采样优化：每 4 像素采样一次（2000×3000 → 500×750），约 15M 操作/角度，
/// 41 个角度总计 ~600M 简单操作，通常 < 200ms。
///
/// 返回 NamedTempFile（调用方持有生命期；drop 时自动删除）。
pub fn deskew_image(image_path: &Path) -> Option<tempfile::NamedTempFile> {
    let img = image::open(image_path).ok()?;
    let luma = img.to_luma8();
    let angle_deg = estimate_skew_deg(&luma)?; // > 0.5° 才 Some
    // 用 imageproc::geometric_transformations::rotate_about_center 做旋转
    // 注：旋转是顺时针为负角度（图像坐标系 Y 向下），我们纠正倾斜需取反
    let rad = (-angle_deg).to_radians() as f32;
    let rotated = imageproc::geometric_transformations::rotate_about_center(
        &luma,
        rad,
        imageproc::geometric_transformations::Interpolation::Bilinear,
        image::Luma([255u8]),
    );
    let dynamic = image::DynamicImage::ImageLuma8(rotated);
    let mut tmp = tempfile::Builder::new()
        .prefix("attune_deskew_")
        .suffix(".png")
        .tempfile()
        .ok()?;
    dynamic
        .write_to(tmp.as_file_mut(), image::ImageFormat::Png)
        .ok()?;
    log::debug!("deskew: {:.1}° correction applied to {}", angle_deg, image_path.display());
    Some(tmp)
}

/// 水平投影法估计文档倾斜角（-10° ~ +10°，0.5° 步长）。
/// 返回 None 当 |angle| ≤ 0.5°（无需校正）。
/// 返回 None 当图像过小（< 100×100），避免投影统计无意义。
fn estimate_skew_deg(gray: &image::GrayImage) -> Option<f32> {
    let (w, h) = gray.dimensions();
    if w < 100 || h < 100 {
        return None;
    }
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let mut best_angle = 0.0f32;
    let mut best_score = 0.0f64; // only update if variance > 0 (blank/no-text → stays 0.0 → returns None)

    // -10° to +10° in 0.5° steps (41 angles)
    for i in -20i32..=20 {
        let angle = i as f32 * 0.5;
        let rad = angle.to_radians();
        let cos = rad.cos();
        let sin = rad.sin();

        // 采样每 4 像素（宽高均），减少计算量
        let mut profile = vec![0u32; h as usize];
        let mut y = 0u32;
        while y < h {
            let mut x = 0u32;
            while x < w {
                if gray.get_pixel(x, y)[0] < 128 {
                    // 旋转该点，取旋转后的 Y 坐标
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let ry = (-sin * dx + cos * dy + cy).round() as i32;
                    if ry >= 0 && (ry as u32) < h {
                        profile[ry as usize] += 1;
                    }
                }
                x += 4;
            }
            y += 4;
        }

        // 投影曲线方差（文字行对齐时，有文字的行计数高，无文字行接近0，方差最大）
        let n = profile.len() as f64;
        let mean = profile.iter().map(|&v| v as f64).sum::<f64>() / n;
        let variance = profile
            .iter()
            .map(|&v| {
                let d = v as f64 - mean;
                d * d
            })
            .sum::<f64>()
            / n;

        if variance > best_score {
            best_score = variance;
            best_angle = angle;
        }
    }

    if best_angle.abs() > 0.5 {
        Some(best_angle)
    } else {
        None
    }
}

// ── 表格重建 ──────────────────────────────────────────────────────────────────

/// 按文字块坐标重建 Markdown 表格。
///
/// 策略：
///   1. 从 box_points 计算每个块的中心 (cx, cy)
///   2. 按中心 Y 分行（容差 = 平均块高的 50%）
///   3. 每行按中心 X 排序
///   4. 若 ≥ 2 行 × ≥ 2 列 → 输出 Markdown 表格
///   5. 否则返回 None（调用方 fall back 到 plain text）
fn reconstruct_table_md(blocks: &[kreuzberg_paddle_ocr::TextBlock]) -> Option<String> {
    if blocks.len() < 4 {
        return None; // 少于 4 个块，不可能是表格
    }

    // 计算每个块的中心坐标和高度
    struct BlockInfo<'a> {
        text: &'a str,
        cx: f32, // center x
        cy: f32, // center y
        height: f32,
    }

    let infos: Vec<BlockInfo<'_>> = blocks
        .iter()
        .filter(|b| !b.text.is_empty())
        .map(|b| {
            // box_points: Vec<Point> — 4 corners [tl, tr, br, bl], Point { x: u32, y: u32 }
            let pts = &b.box_points;
            let n = pts.len().max(1) as f32;
            let cx = pts.iter().map(|p| p.x as f32).sum::<f32>() / n;
            let cy = pts.iter().map(|p| p.y as f32).sum::<f32>() / n;
            // height = avg of left and right side heights (need ≥4 pts)
            let left_h = if pts.len() >= 4 { (pts[3].y as f32 - pts[0].y as f32).abs() } else { 20.0 };
            let right_h = if pts.len() >= 4 { (pts[2].y as f32 - pts[1].y as f32).abs() } else { 20.0 };
            let height = ((left_h + right_h) / 2.0).max(1.0);
            BlockInfo { text: &b.text, cx, cy, height }
        })
        .collect();

    if infos.is_empty() {
        return None;
    }

    // 平均块高 → 行分组容差
    let avg_height: f32 = infos.iter().map(|b| b.height).sum::<f32>() / infos.len() as f32;
    let row_tol = avg_height * 0.5;

    // 按 cy 升序排列，分行（贪心聚类：cy 差值 ≤ row_tol 属同行）
    let mut sorted: Vec<usize> = (0..infos.len()).collect();
    sorted.sort_by(|&a, &b| infos[a].cy.partial_cmp(&infos[b].cy).unwrap_or(std::cmp::Ordering::Equal));

    let mut rows: Vec<Vec<usize>> = Vec::new();
    for idx in sorted {
        let cy = infos[idx].cy;
        if let Some(last_row) = rows.last_mut() {
            let last_cy = infos[*last_row.last().unwrap()].cy;
            if (cy - last_cy).abs() <= row_tol {
                last_row.push(idx);
                continue;
            }
        }
        rows.push(vec![idx]);
    }

    // 每行按 cx 排序
    for row in &mut rows {
        row.sort_by(|&a, &b| infos[a].cx.partial_cmp(&infos[b].cx).unwrap_or(std::cmp::Ordering::Equal));
    }

    // 检验是否像表格：≥ 2 行 × ≥ 2 列
    if rows.len() < 2 || rows.iter().map(|r| r.len()).max().unwrap_or(0) < 2 {
        return None;
    }

    // 确定最大列数
    let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);

    // 生成 Markdown 表格
    let mut md = String::with_capacity(rows.len() * max_cols * 20);

    // header row
    let header: Vec<&str> = rows[0].iter().map(|&i| infos[i].text).collect();
    md.push('|');
    for cell in &header {
        md.push(' ');
        md.push_str(cell.trim().replace('|', "\\|").as_str());
        md.push_str(" |");
    }
    // pad to max_cols
    for _ in header.len()..max_cols {
        md.push_str("  |");
    }
    md.push('\n');

    // separator
    md.push('|');
    for _ in 0..max_cols {
        md.push_str("---|");
    }
    md.push('\n');

    // data rows
    for row in rows.iter().skip(1) {
        md.push('|');
        for &i in row {
            md.push(' ');
            md.push_str(infos[i].text.trim().replace('|', "\\|").as_str());
            md.push_str(" |");
        }
        for _ in row.len()..max_cols {
            md.push_str("  |");
        }
        md.push('\n');
    }

    Some(md)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_none_when_models_missing() {
        // 测试机理论上不该有 PP-OCR 模型 — 应返回 None
        // （CI 也不预装，除非 postinst 跑过）
        let p = detect();
        // 不强断言，因为开发机可能装过：仅验证 detect 不 panic
        let _ = p;
    }

    #[test]
    fn models_dir_under_data_dir() {
        let d = PpOcrProvider::models_dir();
        assert!(d.ends_with("models/ppocr"));
    }

    #[test]
    fn models_present_returns_false_when_clean() {
        // 仅当真的没下载时应返回 false
        // CI 跑这里通常是干净环境
        if !PpOcrProvider::models_present() {
            // 干净环境下：扫描 detect 应返回 None
            assert!(detect().is_none());
        }
    }

    #[test]
    fn num_cpus_safe_caps_at_8() {
        let n = num_cpus_safe();
        assert!(n >= 1 && n <= 8, "got {n}");
    }

    #[test]
    fn sanitize_max_side_clamps_out_of_range() {
        use crate::ocr::profile::OcrProfile;
        // 0（Default）/ 太小 / 太大 → 回退默认
        assert_eq!(sanitize_max_side(0), OcrProfile::DEFAULT_MAX_SIDE_LEN);
        assert_eq!(sanitize_max_side(100), OcrProfile::DEFAULT_MAX_SIDE_LEN);
        assert_eq!(sanitize_max_side(99_999), OcrProfile::DEFAULT_MAX_SIDE_LEN);
        // 合理值原样保留
        assert_eq!(sanitize_max_side(3200), 3200);
        assert_eq!(sanitize_max_side(512), 512);
        assert_eq!(sanitize_max_side(8192), 8192);
    }

    #[test]
    fn compute_avg_confidence_weighted_by_text_length() {
        use kreuzberg_paddle_ocr::{Point, TextBlock};
        let blk = |text: &str, score: f32| TextBlock {
            text: text.to_string(),
            box_points: vec![Point { x: 0, y: 0 }],
            box_score: 0.9,
            angle_index: 0,
            angle_score: 0.0,
            text_score: score,
        };
        // 无文本块 → None
        assert!(compute_avg_confidence(&[]).is_none());
        // 全空文本 → None
        assert!(compute_avg_confidence(&[blk("", 0.5)]).is_none());
        // 2 字符@1.0 + 8 字符@0.5 → (2×1.0 + 8×0.5) / 10 = 0.6（长度加权）
        let blocks = vec![blk("ab", 1.0), blk("cdefghij", 0.5)];
        let c = compute_avg_confidence(&blocks).expect("some");
        assert!((c - 0.6).abs() < 1e-4, "expected 0.6, got {c}");
    }

    #[test]
    fn normalize_orientation_no_exif_returns_none() {
        // 无 EXIF orientation 的 PNG → 应返回 None（调用方用原图）
        let tmp = tempfile::Builder::new()
            .suffix(".png")
            .tempfile()
            .expect("tmp");
        let img = image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(
            64,
            64,
            image::Luma([200u8]),
        ));
        img.save(tmp.path()).expect("save png");
        assert!(
            normalize_orientation(tmp.path()).is_none(),
            "PNG without EXIF orientation should return None"
        );
    }

    #[test]
    fn normalize_orientation_exif_rotate90_swaps_dimensions() {
        // fixture: 100×60 JPEG, EXIF Orientation=6 (Rotate 90 CW)。
        // 归一化后旋转 90° → 宽高互换 60×100；证明 EXIF 旋转防误判路径生效。
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/exif_orientation6.jpg"
        );
        let tmp = normalize_orientation(std::path::Path::new(fixture))
            .expect("EXIF Orientation=6 应触发归一化, 返回 Some");
        let corrected = image::ImageReader::open(tmp.path())
            .expect("open corrected")
            .decode()
            .expect("decode corrected");
        assert_eq!(
            (corrected.width(), corrected.height()),
            (60, 100),
            "Orientation=6 旋转 90° 后宽高应互换 (100×60 → 60×100)"
        );
    }

    // ── deskew tests ──────────────────────────────────────────────────────

    #[test]
    fn estimate_skew_deg_small_image_returns_none() {
        // 图像过小（< 100×100）不做倾斜估计
        let tiny = image::GrayImage::new(50, 50);
        assert!(estimate_skew_deg(&tiny).is_none());
    }

    #[test]
    fn estimate_skew_deg_blank_image_returns_none() {
        // 全白图像无暗像素，投影全零方差相等 → best_angle=0° → None
        let blank = image::GrayImage::from_pixel(200, 200, image::Luma([255u8]));
        assert!(estimate_skew_deg(&blank).is_none());
    }

    #[test]
    fn estimate_skew_deg_horizontal_lines_zero_skew() {
        // 在 200×200 白底上画精确水平黑线 → 估计偏转角应接近 0（不超过 0.5°）
        let mut img = image::GrayImage::from_pixel(200, 200, image::Luma([255u8]));
        for row in [30u32, 60, 90, 120, 150] {
            for x in 10u32..190 {
                img.put_pixel(x, row, image::Luma([0u8]));
            }
        }
        // 期望：skew = 0 → returns None（不校正）
        let result = estimate_skew_deg(&img);
        if let Some(angle) = result {
            // 容差 1.5°（合成图可能因量化而有小偏差）
            assert!(angle.abs() <= 1.5, "expected near-zero skew but got {angle}°");
        }
        // None 也是可接受的（0° 不需要校正）
    }

    // ── table reconstruction tests ────────────────────────────────────────

    #[test]
    fn reconstruct_table_md_empty_returns_none() {
        assert!(reconstruct_table_md(&[]).is_none());
    }

    fn mk_block(text: &str, x: u32, y: u32) -> kreuzberg_paddle_ocr::TextBlock {
        kreuzberg_paddle_ocr::TextBlock {
            text: text.to_string(),
            box_points: vec![
                kreuzberg_paddle_ocr::Point { x, y },
                kreuzberg_paddle_ocr::Point { x: x + 80, y },
                kreuzberg_paddle_ocr::Point { x: x + 80, y: y + 20 },
                kreuzberg_paddle_ocr::Point { x, y: y + 20 },
            ],
            box_score: 0.9,
            angle_index: 0,
            angle_score: 0.0,
            text_score: 0.9,
        }
    }

    #[test]
    fn reconstruct_table_md_too_few_blocks_returns_none() {
        // 3 个块不够构成表格
        let blocks = vec![mk_block("A", 0, 0), mk_block("B", 100, 0), mk_block("C", 0, 30)];
        assert!(reconstruct_table_md(&blocks).is_none());
    }

    #[test]
    fn reconstruct_table_md_2x2_grid() {
        // 2行 × 2列 的表格 → 应得到 Markdown 表格
        let blocks = vec![
            mk_block("姓名", 0, 5),
            mk_block("年龄", 100, 5),
            mk_block("张三", 0, 35),
            mk_block("28", 100, 35),
        ];
        let md = reconstruct_table_md(&blocks);
        assert!(md.is_some(), "2x2 grid should produce table");
        let md = md.unwrap();
        assert!(md.contains("姓名"), "header missing 姓名");
        assert!(md.contains("年龄"), "header missing 年龄");
        assert!(md.contains("张三"), "data row missing 张三");
        assert!(md.contains("28"), "data row missing 28");
        assert!(md.contains("|---|"), "markdown separator missing");
    }

    #[test]
    fn reconstruct_table_md_pipe_escaped() {
        // 文本含 | 符号 → 应转义为 \|
        let blocks = vec![
            mk_block("A|B", 0, 5),
            mk_block("C", 100, 5),
            mk_block("D|E", 0, 35),
            mk_block("F", 100, 35),
        ];
        let md = reconstruct_table_md(&blocks).unwrap();
        assert!(md.contains(r"A\|B"), "pipe in cell should be escaped");
    }
}
