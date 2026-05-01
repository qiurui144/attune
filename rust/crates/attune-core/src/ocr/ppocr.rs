//! PP-OCRv5 mobile OCR provider via ONNX Runtime (ort 2.0)
//!
//! 阶段化交付：
//! - **P1 (本提交)**: skeleton — trait 实现存在但抛 NotImplemented；模型路径探测
//! - P2: DBNet detection 推理 + contour 后处理
//! - P3: CLS 方向分类 + CRNN recognition + CTC greedy decode
//! - P4: pipeline (det → crop+warp → cls → rec)
//! - P5: postinst 下载 3 个 ONNX 模型 + ppocr_keys_v1.txt
//!
//! 模型选择：PP-OCRv5 mobile (~16 MB total)
//! - ch_PP-OCRv5_det_mobile.onnx  ~5 MB   text detection (DBNet)
//! - ch_ppocr_mobile_v2.0_cls.onnx ~1 MB  orientation classifier
//! - ch_PP-OCRv5_rec_mobile.onnx  ~10 MB  text recognition (CRNN)
//! - ppocr_keys_v1.txt            6627 chars
//!
//! 模型存放：~/.local/share/attune/models/ppocr/
//!
//! 参考实现：
//! - RapidOCR (Python/C++/.NET) — 算法权威
//! - PaddleOCR/deploy/cpp_infer — C++ 参考
//! - 我们用纯 Rust ort + image + imageproc

use crate::error::{Result, VaultError};
use std::path::{Path, PathBuf};

use super::OcrProvider;

/// PP-OCR provider — 模型路径绑定 + 推理 session 句柄。
///
/// P1 阶段只持有路径，不加载 session（验证模型存在 + trait 实现连通）。
/// P2 起会持有 ort::Session 句柄。
pub struct PpOcrProvider {
    pub det_model: PathBuf,
    pub cls_model: PathBuf,
    pub rec_model: PathBuf,
    pub char_dict: PathBuf,
}

impl PpOcrProvider {
    /// 模型缓存目录：`<data_dir>/models/ppocr/`
    pub fn models_dir() -> PathBuf {
        crate::platform::data_dir()
            .join("models")
            .join("ppocr")
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

    /// 构造 provider — 假定模型已下载。失败返回 None。
    pub fn new() -> Option<Self> {
        let d = Self::models_dir();
        let p = Self {
            det_model: d.join("ch_PP-OCRv5_det_mobile.onnx"),
            cls_model: d.join("ch_ppocr_mobile_v2.0_cls.onnx"),
            rec_model: d.join("ch_PP-OCRv5_rec_mobile.onnx"),
            char_dict: d.join("ppocr_keys_v1.txt"),
        };
        if !p.det_model.exists()
            || !p.cls_model.exists()
            || !p.rec_model.exists()
            || !p.char_dict.exists()
        {
            return None;
        }
        Some(p)
    }
}

impl OcrProvider for PpOcrProvider {
    fn name(&self) -> &str {
        "pp-ocr-v5-mobile"
    }

    fn has_chinese(&self) -> bool {
        // PP-OCRv5 默认带中英双语字典 (6627 字符含 chi_sim + ASCII)
        true
    }

    fn extract_text_from_image(&self, _image_path: &Path) -> Result<String> {
        // P1 skeleton — P2/P3/P4 实现完整 pipeline
        Err(VaultError::ModelLoad(
            "PP-OCR P1 skeleton: extract_text not yet implemented (P2-P4 pending)".into(),
        ))
    }
}

/// 工厂 — `mod.rs::detect_default_provider()` 调这里。
pub fn detect() -> Option<PpOcrProvider> {
    PpOcrProvider::new()
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
    fn extract_text_returns_not_implemented_p1() {
        // P1 skeleton 的预期行为：抛 NotImplemented
        let provider = PpOcrProvider {
            det_model: "/dev/null".into(),
            cls_model: "/dev/null".into(),
            rec_model: "/dev/null".into(),
            char_dict: "/dev/null".into(),
        };
        let r = provider.extract_text_from_image(Path::new("/dev/null"));
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("not yet implemented"));
    }
}
