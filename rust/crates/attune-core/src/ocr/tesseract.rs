//! Tesseract subprocess OCR provider — v0.6.x default。
//!
//! Trade-offs：
//! - 中英双语成熟（apt 包覆盖广），但中文准确率仅 70-85%（PP-OCR 94-96%）
//! - 子进程模型不引入 C/C++ FFI 依赖
//! - 偶发 OCR（用户不会天天 ingest 扫描 PDF）启动子进程可接受
//!
//! v0.7+ 路由优先 PP-OCR，本 provider 作 fallback。

use crate::error::{Result, VaultError};
use std::path::Path;
use std::process::Command;

use super::OcrProvider;

/// Tesseract backend 能力探测结果（v0.6 旧 API，保留向后兼容）。
#[derive(Debug, Clone)]
pub struct OcrBackend {
    pub tesseract_path: String,
    pub pdftoppm_path: String,
    pub languages: Vec<String>, // 已安装的训练数据
}

impl OcrBackend {
    /// 是否支持中文 OCR
    pub fn has_chinese(&self) -> bool {
        self.languages
            .iter()
            .any(|l| l.starts_with("chi_sim") || l.starts_with("chi_tra"))
    }
    /// 是否支持英文 OCR
    pub fn has_english(&self) -> bool {
        self.languages.iter().any(|l| l == "eng")
    }
    /// tesseract `-l` 参数值（优先中英双栈）
    pub fn lang_arg(&self) -> String {
        let mut parts = Vec::new();
        if self.has_chinese() {
            parts.push("chi_sim");
        }
        if self.has_english() {
            parts.push("eng");
        }
        if parts.is_empty() && !self.languages.is_empty() {
            parts.push(self.languages[0].as_str());
        }
        parts.join("+")
    }
}

/// Trait 实现：让 OcrBackend 直接当 OcrProvider 用。
impl OcrProvider for OcrBackend {
    fn name(&self) -> &str {
        "tesseract"
    }
    fn has_chinese(&self) -> bool {
        OcrBackend::has_chinese(self)
    }
    fn extract_text_from_image(&self, image_path: &Path) -> Result<String> {
        let lang_arg = self.lang_arg();
        let out = Command::new(&self.tesseract_path)
            .arg(image_path)
            .arg("-")
            .arg("-l")
            .arg(&lang_arg)
            .arg("--psm")
            .arg("3")
            .output()
            .map_err(VaultError::Io)?;
        if !out.status.success() {
            return Err(VaultError::Io(std::io::Error::other(format!(
                "tesseract failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ))));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
    fn extract_text_from_pdf(&self, pdf_path: &Path) -> Result<String> {
        ocr_pdf(self, pdf_path)
    }
}

// ── v0.6 旧 API（向后兼容）──────────────────────────────────────────

/// 探测系统是否装了 tesseract + pdftoppm + 所需语言包。
/// 返回 None 表示 OCR 不可用（parser.rs 降级为纯 pdf_extract）。
pub fn detect_ocr_backend() -> Option<OcrBackend> {
    let tesseract_path = which_bin("tesseract")?;
    let pdftoppm_path = which_bin("pdftoppm")?;
    let languages = list_tesseract_languages(&tesseract_path).unwrap_or_default();
    if languages.is_empty() {
        return None;
    }
    Some(OcrBackend {
        tesseract_path,
        pdftoppm_path,
        languages,
    })
}

/// 新 API 包装：返回 OcrProvider trait object（v0.7+ 走这里）。
pub fn detect() -> Option<OcrBackend> {
    detect_ocr_backend()
}

fn which_bin(name: &str) -> Option<String> {
    which::which(name).ok().map(|p| p.to_string_lossy().into_owned())
}

fn list_tesseract_languages(tesseract: &str) -> Result<Vec<String>> {
    let out = Command::new(tesseract)
        .arg("--list-langs")
        .output()
        .map_err(VaultError::Io)?;
    let text = String::from_utf8_lossy(&out.stderr).to_string()
        + &String::from_utf8_lossy(&out.stdout);
    let langs: Vec<String> = text
        .lines()
        .filter(|l| !l.is_empty() && !l.contains(':') && !l.contains("List"))
        .map(|l| l.trim().to_string())
        .collect();
    Ok(langs)
}

/// 扫描版 PDF → 文字（图片 OCR）—— 旧 API。
///
/// 流程：
///   1. pdftoppm 把 PDF 每页转为 PNG（临时目录，300 DPI）
///   2. 遍历每页 PNG 调用 tesseract OCR，拼接文字
///   3. 返回合并后的字符串
pub fn ocr_pdf(backend: &OcrBackend, pdf_path: &Path) -> Result<String> {
    let tmp = tempfile::TempDir::new().map_err(VaultError::Io)?;
    let prefix = tmp.path().join("page");
    let prefix_str = prefix.to_str().ok_or_else(|| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "non-UTF8 temp path",
        ))
    })?;

    // 1. PDF → 多页 PNG（-r 300 = 300 DPI 清晰度，-png 指定格式）
    let status = Command::new(&backend.pdftoppm_path)
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

    // 2. 收集生成的 PNG（按文件名排序保页序）
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

    // 3. 每页 OCR
    let lang_arg = backend.lang_arg();
    let mut all_text = String::with_capacity(pages.len() * 2000);
    let mut failed = 0usize;
    for (idx, png) in pages.iter().enumerate() {
        let out = Command::new(&backend.tesseract_path)
            .arg(png)
            .arg("-")
            .arg("-l")
            .arg(&lang_arg)
            .arg("--psm")
            .arg("3")
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                all_text.push_str(text.trim());
                all_text.push_str("\n\n");
            }
            Ok(o) => {
                log::warn!(
                    "tesseract page {} failed: {}",
                    idx + 1,
                    String::from_utf8_lossy(&o.stderr)
                );
                failed += 1;
            }
            Err(e) => {
                log::warn!("tesseract page {} error: {}", idx + 1, e);
                failed += 1;
            }
        }
    }
    log::info!(
        "ocr_pdf: {} pages ok, {} failed, {} bytes text",
        pages.len() - failed,
        failed,
        all_text.len()
    );
    Ok(all_text)
}

/// 通用 PDF pipeline — provider-agnostic：pdftoppm 切页 + 调 provider 抽文字。
/// 用作 OcrProvider trait 的默认 PDF 实现（PP-OCR 等也会走这里）。
pub fn default_pdf_pipeline(provider: &dyn OcrProvider, pdf_path: &Path) -> Result<String> {
    let pdftoppm = which_bin("pdftoppm").ok_or_else(|| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pdftoppm not found (apt install poppler-utils)",
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

    let mut all = String::with_capacity(pages.len() * 2000);
    let mut failed = 0usize;
    for (idx, png) in pages.iter().enumerate() {
        match provider.extract_text_from_image(png) {
            Ok(text) => {
                all.push_str(text.trim());
                all.push_str("\n\n");
            }
            Err(e) => {
                log::warn!(
                    "{} page {} error: {}",
                    provider.name(),
                    idx + 1,
                    e
                );
                failed += 1;
            }
        }
    }
    log::info!(
        "{} pdf_pipeline: {} pages ok, {} failed, {} bytes text",
        provider.name(),
        pages.len() - failed,
        failed,
        all.len()
    );
    Ok(all)
}

/// 判断 PDF 是否需要 OCR（pdf_extract 产出文字量低于阈值）—— 旧 API。
pub fn needs_ocr(extracted_text: &str) -> bool {
    extracted_text.chars().filter(|c| !c.is_whitespace()).count() < 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_backend_has_languages() {
        if let Some(b) = detect_ocr_backend() {
            assert!(!b.languages.is_empty());
            assert!(!b.lang_arg().is_empty());
        }
    }

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
    fn ocr_backend_chinese_and_english_detection() {
        let b = OcrBackend {
            tesseract_path: "/usr/bin/tesseract".into(),
            pdftoppm_path: "/usr/bin/pdftoppm".into(),
            languages: vec!["chi_sim".into(), "eng".into(), "osd".into()],
        };
        assert!(b.has_chinese());
        assert!(b.has_english());
        assert_eq!(b.lang_arg(), "chi_sim+eng");
    }
}
