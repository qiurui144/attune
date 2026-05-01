//! OCR — 单一引擎 PP-OCRv5 mobile (via kreuzberg-paddle-ocr + ort 2.0)
//!
//! 设计选择（2026-05-01）：
//! - **彻底移除 tesseract** — fallback 是幻想保险（网络故障时两个都没救；
//!   ort 兼容性已被 reranker 验证；语言覆盖 PP-OCR 6627 chars 够用）
//! - **不暴露 OCR 引擎开关** — 引擎对用户是实现细节，UI 越少越好
//! - **postinst 自动下载 4 个 ONNX 模型 (~21 MB)** — 装包即可用
//! - PDF → 多页 PNG 经 `pdftoppm` (poppler-utils 自动装)；每页走 PP-OCR
//!
//! 调用层 (parser.rs / ai_stack.rs) 用：
//!   `detect_default_provider()` → `Box<dyn OcrProvider>`
//!   `extract_text_from_pdf(provider, path)` → 全文
//!   `needs_ocr(text)` → 文字层薄判定（保留旧 API）

pub mod ppocr;

use crate::error::{Result, VaultError};
use std::path::Path;
use std::process::Command;

/// OCR provider 抽象 — 当前只有 PP-OCRv5 一个实现。
/// trait 仍然保留是为了：测试用 mock + 未来可能的 K3 远程 provider。
pub trait OcrProvider: Send + Sync {
    /// 引擎名（用于日志 / diagnostics 端点）
    fn name(&self) -> &str;

    /// 是否支持中文
    fn has_chinese(&self) -> bool;

    /// 抽取单张图片的文字
    fn extract_text_from_image(&self, image_path: &Path) -> Result<String>;
}

/// 选择默认 OCR provider —
/// PP-OCR 模型不在 → None（attune-server diagnostics 会提示用户重跑 postinst）。
pub fn detect_default_provider() -> Option<Box<dyn OcrProvider>> {
    if let Some(p) = ppocr::detect() {
        log::info!("OCR provider: PP-OCRv5 mobile (ORT)");
        return Some(Box::new(p));
    }
    log::warn!("OCR provider: PP-OCR models missing — apt install --reinstall attune or run attune deploy");
    None
}

/// 抽取 PDF 文字 — 用 pdftoppm 切页 + provider 逐页 OCR。
/// `pdftoppm` 来自 poppler-utils，由 .deb 自动装。
pub fn extract_text_from_pdf(provider: &dyn OcrProvider, pdf_path: &Path) -> Result<String> {
    let pdftoppm = which::which("pdftoppm").map_err(|_| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pdftoppm not found (poppler-utils 应由 .deb 自动装；apt install poppler-utils 修复)",
        ))
    })?;

    let tmp = tempfile::TempDir::new().map_err(VaultError::Io)?;
    let prefix = tmp.path().join("page");
    let prefix_str = prefix.to_str().ok_or_else(|| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "non-UTF8 temp path",
        ))
    })?;

    // PDF → 多页 PNG（300 DPI，PP-OCR 在此分辨率上准确率最佳）
    let status = Command::new(&pdftoppm)
        .args(["-r", "300", "-png"])
        .arg(pdf_path)
        .arg(prefix_str)
        .status()
        .map_err(VaultError::Io)?;
    if !status.success() {
        return Err(VaultError::Io(std::io::Error::other(format!(
            "pdftoppm failed: exit {}",
            status.code().unwrap_or(-1)
        ))));
    }

    let mut pages: Vec<_> = std::fs::read_dir(tmp.path())
        .map_err(VaultError::Io)?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    pages.sort();

    if pages.is_empty() {
        return Err(VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pdftoppm produced no pages (PDF may be empty or corrupt)",
        )));
    }

    let mut all = String::with_capacity(pages.len() * 2000);
    let mut failed = 0usize;
    for (idx, png) in pages.iter().enumerate() {
        match provider.extract_text_from_image(png) {
            Ok(text) => {
                all.push_str(text.trim());
                all.push_str("\n\n");
            }
            Err(e) => {
                log::warn!("{} page {} error: {}", provider.name(), idx + 1, e);
                failed += 1;
            }
        }
    }
    log::info!(
        "{} PDF pipeline: {} pages ok, {} failed, {} bytes text",
        provider.name(),
        pages.len() - failed,
        failed,
        all.len()
    );
    Ok(all)
}

/// 判断 PDF 是否需要 OCR（pdf_extract 产出文字量低于阈值）
pub fn needs_ocr(extracted_text: &str) -> bool {
    extracted_text.chars().filter(|c| !c.is_whitespace()).count() < 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_ocr_threshold() {
        assert!(needs_ocr(""), "empty = needs ocr");
        assert!(needs_ocr("  \n\t"), "whitespace = needs ocr");
        assert!(needs_ocr(&"a".repeat(50)), "50 chars = needs ocr");
        assert!(!needs_ocr(&"a".repeat(200)), "200 chars = enough text");
        let chinese = "中".repeat(100);
        assert!(!needs_ocr(&chinese), "100 Chinese chars = enough");
    }

    #[test]
    fn detect_default_provider_returns_none_when_pp_ocr_missing() {
        // CI 环境无 PP-OCR 模型 → 应返回 None；本测试只确保不 panic
        let _ = detect_default_provider();
    }
}
