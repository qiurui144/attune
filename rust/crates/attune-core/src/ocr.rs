//! OCR backend for image-based (scanned) PDFs.
//!
//! Design philosophy: 复用用户系统已装的 tesseract CLI，与 BrowserSearchProvider
//! 复用系统 Chrome 同一策略。理由：
//!
//! - 跨平台成熟：apt/brew/choco 都有 tesseract 官方包
//! - 中英双语：tesseract 的 `chi_sim+eng` 训练数据广泛验证
//! - 不引入 C/C++ FFI 依赖（避免交叉编译时的 libtesseract 版本地狱）
//! - OCR 本就是偶发操作（用户不会天天 ingest 扫描版 PDF），启动子进程可接受
//!
//! PDF → 图片：调用 pdftoppm（poppler-utils，与 pdftotext 同一 package）
//! 图片 → 文字：tesseract
//!
//! 可选择性使用：若 `detect_ocr_backend()` 返回 None，parser.rs 自动降级
//! 到纯文字 PDF 解析，不报错。

use crate::error::{Result, VaultError};
use std::path::Path;
use std::process::Command;

/// OCR backend 能力探测结果
#[derive(Debug, Clone)]
pub struct OcrBackend {
    pub tesseract_path: String,
    pub pdftoppm_path: String,
    pub languages: Vec<String>,  // 已安装的训练数据
}

impl OcrBackend {
    /// 是否支持中文 OCR
    pub fn has_chinese(&self) -> bool {
        self.languages.iter().any(|l| l.starts_with("chi_sim") || l.starts_with("chi_tra"))
    }
    /// 是否支持英文 OCR
    pub fn has_english(&self) -> bool {
        self.languages.iter().any(|l| l == "eng")
    }
    /// tesseract `-l` 参数值（优先中英双栈）
    pub fn lang_arg(&self) -> String {
        let mut parts = Vec::new();
        if self.has_chinese() { parts.push("chi_sim"); }
        if self.has_english() { parts.push("eng"); }
        if parts.is_empty() && !self.languages.is_empty() {
            parts.push(self.languages[0].as_str());
        }
        parts.join("+")
    }
}

/// 探测系统是否装了 tesseract + pdftoppm + 所需语言包
///
/// 返回 None 表示 OCR 不可用（parser.rs 降级为纯 pdf_extract）
pub fn detect_ocr_backend() -> Option<OcrBackend> {
    let tesseract_path = which_bin("tesseract")?;
    let pdftoppm_path = which_bin("pdftoppm")?;
    let languages = list_tesseract_languages(&tesseract_path).unwrap_or_default();
    if languages.is_empty() {
        return None;
    }
    Some(OcrBackend { tesseract_path, pdftoppm_path, languages })
}

fn which_bin(name: &str) -> Option<String> {
    // 跨平台 PATH 查找：Linux/macOS 等价 `which`，Windows 等价 `where`，
    // 使用 `which` crate 避免 Win 上 `Command::new("which")` 失败。
    which::which(name)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

fn list_tesseract_languages(tesseract: &str) -> Result<Vec<String>> {
    let out = Command::new(tesseract).arg("--list-langs").output()
        .map_err(VaultError::Io)?;
    // tesseract 把语言列表写到 stderr，每行一个；首行是 "List of available languages..."
    let text = String::from_utf8_lossy(&out.stderr).to_string()
        + &String::from_utf8_lossy(&out.stdout);
    let langs: Vec<String> = text.lines()
        .filter(|l| !l.is_empty() && !l.contains(':') && !l.contains("List"))
        .map(|l| l.trim().to_string())
        .collect();
    Ok(langs)
}

/// 扫描版 PDF → 文字（图片 OCR）
///
/// 流程：
///   1. pdftoppm 把 PDF 每页转为 PNG（临时目录，300 DPI）
///   2. 遍历每页 PNG 调用 tesseract OCR，拼接文字
///   3. 返回合并后的字符串
///
/// 设计注意：
///   - 大 PDF（如 100 页）耗时可能 1-5 min，调用方应在后台线程或 spawn_blocking 执行
///   - 临时目录使用 tempfile::TempDir 保证 panic 时也清理
///   - 每页失败不终止整个 OCR，记录 warn 继续（残缺文本比全失败好）
pub fn ocr_pdf(backend: &OcrBackend, pdf_path: &Path) -> Result<String> {
    let tmp = tempfile::TempDir::new()
        .map_err(VaultError::Io)?;
    let prefix = tmp.path().join("page");
    let prefix_str = prefix.to_str()
        .ok_or_else(|| VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "non-UTF8 temp path")))?;

    // 1. PDF → 多页 PNG（-r 300 = 300 DPI 清晰度，-png 指定格式）
    let status = Command::new(&backend.pdftoppm_path)
        .args(["-r", "300", "-png"])
        .arg(pdf_path)
        .arg(prefix_str)
        .status()
        .map_err(VaultError::Io)?;
    if !status.success() {
        return Err(VaultError::Io(std::io::Error::other(
            format!("pdftoppm failed: exit {}", status.code().unwrap_or(-1)),
        )));
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

    // 3. 每页 OCR，tesseract 输出到 stdout，收集
    let lang_arg = backend.lang_arg();
    let mut all_text = String::with_capacity(pages.len() * 2000);
    let mut failed = 0usize;
    for (idx, png) in pages.iter().enumerate() {
        let out = Command::new(&backend.tesseract_path)
            .arg(png).arg("-").arg("-l").arg(&lang_arg).arg("--psm").arg("3")
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                all_text.push_str(text.trim());
                all_text.push_str("\n\n");
            }
            Ok(o) => {
                log::warn!("tesseract page {} failed: {}", idx + 1,
                    String::from_utf8_lossy(&o.stderr));
                failed += 1;
            }
            Err(e) => {
                log::warn!("tesseract page {} error: {}", idx + 1, e);
                failed += 1;
            }
        }
    }
    log::info!("ocr_pdf: {} pages ok, {} failed, {} bytes text",
        pages.len() - failed, failed, all_text.len());
    Ok(all_text)
}

/// 判断 PDF 是否需要 OCR（pdf_extract 产出文字量低于阈值）
pub fn needs_ocr(extracted_text: &str) -> bool {
    // 按字符数量判断（注意中文每字符 3 字节 UTF-8，直接 .len() 会夸大）
    extracted_text.chars().filter(|c| !c.is_whitespace()).count() < 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_backend_has_languages() {
        // 仅在 CI/dev 机器装了 tesseract 时断言
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
        // 中文测试：100 个汉字（300 字节 UTF-8）应该被判定为有文字
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

    #[test]
    fn ocr_backend_english_only() {
        let b = OcrBackend {
            tesseract_path: "/usr/bin/tesseract".into(),
            pdftoppm_path: "/usr/bin/pdftoppm".into(),
            languages: vec!["eng".into()],
        };
        assert!(!b.has_chinese());
        assert!(b.has_english());
        assert_eq!(b.lang_arg(), "eng");
    }
}
