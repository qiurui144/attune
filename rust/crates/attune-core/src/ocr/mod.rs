//! OCR backends for image / scanned-PDF text extraction.
//!
//! 历史：v0.6.0 之前用 tesseract subprocess。
//! v0.7+ 引入 trait `OcrProvider`，支持 PP-OCR (ORT-based) + tesseract 双栈。
//!
//! 默认引擎决策：
//! - 中文场景（chi_sim 含量高）→ PP-OCR（准确率 94-96% vs tesseract 70-85%）
//! - 纯英文场景 → tesseract（成熟）
//! - PP-OCR 模型不在 → 降级 tesseract（向后兼容）
//!
//! 调用层（parser.rs / ai_stack.rs）保留旧 API（detect_ocr_backend / ocr_pdf / needs_ocr）
//! 不破坏；新代码用 `OcrProvider` trait + `detect_default_provider()`。

pub mod ppocr;
pub mod tesseract;

use crate::error::Result;
use std::path::Path;

// ── 向后兼容 re-exports（v0.6 调用方用的旧 API）─────────────────────
pub use tesseract::{detect_ocr_backend, needs_ocr, ocr_pdf, OcrBackend};

// ── 新 trait API（v0.7+）────────────────────────────────────────────

/// OCR provider 抽象 — 后端可换 tesseract 或 PP-OCR。
pub trait OcrProvider: Send + Sync {
    /// 引擎名（用于日志 / 诊断 UI）
    fn name(&self) -> &str;

    /// 是否支持中文（决定路由：chi_sim 多的文档优先这里）
    fn has_chinese(&self) -> bool;

    /// 抽取单张图片的文字
    fn extract_text_from_image(&self, image_path: &Path) -> Result<String>;

    /// 抽取 PDF 全部文字（默认走每页 PNG → extract_text_from_image，
    /// provider 可重写以走更高效路径）
    fn extract_text_from_pdf(&self, pdf_path: &Path) -> Result<String>
    where
        Self: Sized,
    {
        tesseract::default_pdf_pipeline(self, pdf_path)
    }
}

/// 选择默认 OCR provider —
/// 优先 PP-OCR（如果模型已下载），降级 tesseract。
pub fn detect_default_provider() -> Option<Box<dyn OcrProvider>> {
    // 1. 尝试 PP-OCR (ORT)
    if let Some(p) = ppocr::detect() {
        log::info!("OCR provider: PP-OCR (ORT)");
        return Some(Box::new(p));
    }
    // 2. 降级 tesseract
    if let Some(p) = tesseract::detect() {
        log::info!("OCR provider: tesseract subprocess (fallback)");
        return Some(Box::new(p));
    }
    log::warn!("OCR provider: none available — install tesseract or download PP-OCR models");
    None
}
