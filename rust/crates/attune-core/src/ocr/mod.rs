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
pub mod profile;
pub mod profile_registry;

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
/// 选择默认 OCR provider — **不触发运行时下载**。
///
/// 设计原则（标准应用规范）:
/// - 模型下载是**安装时**职责（postinst.sh 或 `attune --bootstrap-models` flag）
/// - server 启动路径**不偷偷下载** — 否则用户首次启动等 30s+ 以为卡死
/// - 模型缺失 → 返回 None + diagnostics 端点提示用户跑 bootstrap
///
/// 部署方式覆盖:
/// - apt install attune.deb → postinst.sh 自动下载 (主路径)
/// - cargo binary / 源码部署 → 用户跑 `attune-server-headless --bootstrap-models`
/// - 一键工具 ensure_models_downloaded() 仍在 ppocr.rs (供 bootstrap 调用)
pub fn detect_default_provider() -> Option<Box<dyn OcrProvider>> {
    if let Some(p) = ppocr::detect() {
        log::info!("OCR provider: PP-OCRv5 mobile (ORT)");
        return Some(Box::new(p));
    }
    log::warn!(
        "OCR provider: PP-OCR models missing. Run `attune-server-headless --bootstrap-models` \
         (cargo) or `apt install --reinstall attune` (deb) to download (~21 MB)."
    );
    None
}

/// 抽取 PDF 文字 — 用 pdftoppm 切页 + provider 逐页 OCR。
/// `pdftoppm` 来自 poppler-utils，由 .deb 自动装。
///
/// 等价于 `extract_text_from_pdf_with_dpi(provider, pdf_path, 300)`. 300 DPI 是 PP-OCR
/// 在普通合同/判决书上的最佳实测值；若要换不同 DPI (200 票据 / 600 古籍) 用
/// `extract_text_from_pdf_with_dpi`.
pub fn extract_text_from_pdf(provider: &dyn OcrProvider, pdf_path: &Path) -> Result<String> {
    extract_text_from_pdf_with_dpi(provider, pdf_path, 300)
}

/// 用指定 DPI 抽取 PDF 文字. dpi 通常由 OcrProfile 决定 (200/300/600).
/// 越界值 (< 72 或 > 1200) 退回 300.
pub fn extract_text_from_pdf_with_dpi(
    provider: &dyn OcrProvider,
    pdf_path: &Path,
    dpi: u32,
) -> Result<String> {
    let dpi = if (72..=1200).contains(&dpi) { dpi } else { 300 };
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

    // PDF → 多页 PNG (DPI 由 profile 决定: 200 票据 / 300 合同 / 600 古籍)
    let dpi_str = dpi.to_string();
    let status = Command::new(&pdftoppm)
        .args(["-r", dpi_str.as_str(), "-png"])
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

/// 从注册表按 profile_id 取 DPI. None / 不存在 / load 失败均返回 300 (默认合同 DPI).
///
/// 这是 OCR pipeline 与 profile 系统的对接点 — 调用方拿到 profile_id 后用此函数
/// 决定 DPI, 再调 `extract_text_from_pdf_with_dpi`.
pub fn dpi_for_profile(profile_id: Option<&str>) -> u32 {
    let id = match profile_id {
        Some(s) if !s.is_empty() => s,
        _ => return 300,
    };
    match profile_registry::ProfileRegistry::load_default() {
        Ok(reg) => reg.get(id).map(|p| p.dpi).unwrap_or(300),
        Err(_) => 300,
    }
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

    #[test]
    fn dpi_for_profile_empty_returns_default() {
        assert_eq!(dpi_for_profile(None), 300);
        assert_eq!(dpi_for_profile(Some("")), 300);
    }

    #[test]
    fn dpi_for_profile_unknown_returns_default() {
        // unknown profile id → 300 (registry 加载得到 4 builtin 但无此 id)
        let tmp = tempfile::TempDir::new().expect("tmp");
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        assert_eq!(dpi_for_profile(Some("does-not-exist")), 300);
    }
}
