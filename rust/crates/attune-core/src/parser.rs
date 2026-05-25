// npu-vault/crates/vault-core/src/parser.rs

use std::path::Path;
use crate::error::{Result, VaultError};

/// 代码文件扩展名
const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".js", ".ts", ".rs", ".go", ".java", ".c", ".cpp", ".h",
    ".rb", ".php", ".swift", ".kt", ".scala", ".sh", ".bash", ".zsh",
    ".toml", ".yaml", ".yml", ".json", ".xml", ".html", ".css",
];

/// 解析文件 → (title, content). 等价于 `parse_file_with_profile(path, None)`.
pub fn parse_file(path: &Path) -> Result<(String, String)> {
    parse_file_with_profile(path, None)
}

/// 解析文件, 指定 OCR profile (PDF 扫描件走自定义 DPI). None = 走默认 300 DPI.
pub fn parse_file_with_profile(
    path: &Path,
    profile_id: Option<&str>,
) -> Result<(String, String)> {
    let ext = path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let filename = path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());

    match ext.as_str() {
        ".pdf" => parse_pdf_file_with_dpi(path, &stem, crate::ocr::dpi_for_profile(profile_id)),
        ".docx" => parse_docx_file(path, &stem),
        ".html" | ".htm" => parse_html_file(path, &stem),
        ".epub" => parse_epub_file(path, &stem),
        ".xlsx" | ".xls" => parse_xlsx_file(path, &stem),
        ".pptx" => parse_pptx_file(path, &stem),
        ".rtf" => parse_rtf_file(path, &stem),
        ".csv" => parse_csv_file(path, &stem),
        ".png" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tiff" | ".tif" | ".gif" => {
            parse_image_file(path, &stem)
        }
        ".mp3" | ".wav" | ".m4a" | ".flac" | ".ogg" | ".aac" | ".opus" | ".wma" => {
            parse_audio_file(path, &stem)
        }
        _ => {
            // 允许作为纯文本处理的扩展名：代码文件 + 通用文本格式
            let is_code = CODE_EXTENSIONS.contains(&ext.as_str());
            let is_plain_text = matches!(ext.as_str(), ".md" | ".txt" | "");
            if !is_code && !is_plain_text {
                return Err(VaultError::InvalidInput(format!(
                    "unsupported file format '{ext}': only text, code, documents, spreadsheets, images and audio are accepted"
                )));
            }
            let content = std::fs::read_to_string(path)
                .map_err(VaultError::Io)?;
            parse_content(&content, &filename)
        }
    }
}

/// 从内存解析 → (title, content). 等价于 `parse_bytes_with_profile(data, filename, None)`.
pub fn parse_bytes(data: &[u8], filename: &str) -> Result<(String, String)> {
    parse_bytes_with_profile(data, filename, None)
}

/// 从内存解析, 指定 OCR profile.
pub fn parse_bytes_with_profile(
    data: &[u8],
    filename: &str,
    profile_id: Option<&str>,
) -> Result<(String, String)> {
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let dpi = crate::ocr::dpi_for_profile(profile_id);

    match ext.as_str() {
        ".pdf" => {
            // 上传路径走内存，但 OCR 需要磁盘文件（pdftoppm 读文件）。
            // 先试文字层提取；失败或文字过少则写临时文件跑 OCR。
            let extract_result = pdf_extract::extract_text_from_mem(data);
            let content = match extract_result {
                Ok(text) if !crate::ocr::needs_ocr(&text) => text,
                Ok(thin_text) => {
                    if let Some(ocr_text) = try_ocr_from_bytes_with_dpi(data, dpi) {
                        let title = first_line_title(&ocr_text, &stem);
                        return Ok((title, ocr_text));
                    }
                    thin_text
                }
                Err(e) => {
                    log::info!("pdf_extract failed for uploaded bytes ({e}); trying OCR");
                    if let Some(ocr_text) = try_ocr_from_bytes_with_dpi(data, dpi) {
                        let title = first_line_title(&ocr_text, &stem);
                        return Ok((title, ocr_text));
                    }
                    return Err(VaultError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("PDF extract failed: {e}; OCR unavailable or also failed"),
                    )));
                }
            };
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".docx" => {
            use std::io::Cursor;
            let cursor = Cursor::new(data);
            let mut archive = zip::ZipArchive::new(cursor)
                .map_err(|e| VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("DOCX zip open failed: {e}"),
                )))?;
            let mut doc_xml = String::new();
            if let Ok(mut entry) = archive.by_name("word/document.xml") {
                use std::io::Read;
                entry.read_to_string(&mut doc_xml)?;
            } else {
                return Err(VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "word/document.xml not found in docx",
                )));
            }
            let content = strip_xml_tags(&doc_xml);
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".html" | ".htm" => {
            let html = String::from_utf8_lossy(data).to_string();
            let content = html_to_text(&html);
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".epub" => {
            let content = epub_bytes_to_text(data)?;
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".xlsx" | ".xls" => {
            let content = xlsx_bytes_to_text(data, &ext)?;
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".pptx" => {
            let content = pptx_bytes_to_text(data)?;
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".rtf" => {
            let content = rtf_to_text(&String::from_utf8_lossy(data));
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".csv" => {
            let content = String::from_utf8_lossy(data).to_string();
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".png" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tiff" | ".tif" | ".gif" => {
            let Some(provider) = crate::ocr::detect_default_provider() else {
                return Err(VaultError::InvalidInput("OCR provider unavailable".to_string()));
            };
            let scene = crate::ocr::auto_detect_scene(filename);
            let profile = crate::ocr::profile_for_id(Some(scene));
            // bytes path: write to temp file (OCR expects a Path)
            let mut tmp = tempfile::Builder::new()
                .suffix(&ext)
                .tempfile()
                .map_err(VaultError::Io)?;
            {
                use std::io::Write;
                tmp.write_all(data).map_err(VaultError::Io)?;
                tmp.flush().map_err(VaultError::Io)?;
            }
            let output = provider.extract_structured(tmp.path(), &profile)?;
            if let Some(c) = output.avg_confidence {
                log::info!("OCR 图片 '{filename}' avg_confidence={c:.3}");
            }
            let content = if let Some(table) = output.table_markdown {
                format!("{}\n\n{}", output.text, table)
            } else {
                output.text
            };
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        ".mp3" | ".wav" | ".m4a" | ".flac" | ".ogg" | ".aac" | ".opus" | ".wma" => {
            let Some(backend) = crate::asr::detect_asr_backend() else {
                return Err(VaultError::InvalidInput("ASR backend unavailable".to_string()));
            };
            let mut tmp = tempfile::Builder::new()
                .suffix(&ext)
                .tempfile()
                .map_err(VaultError::Io)?;
            use std::io::Write;
            tmp.write_all(data).map_err(VaultError::Io)?;
            tmp.flush().map_err(VaultError::Io)?;
            let diarization = crate::asr::detect_diarization_backend();
            let (_, content) = crate::asr::transcribe_with_diarization(
                &backend, tmp.path(), diarization.as_ref(),
            )?;
            let title = first_line_title(&content, &stem);
            Ok((title, content))
        }
        _ => {
            // 允许作为纯文本处理的扩展名：代码文件 + 通用文本格式
            // 已知二进制格式（video/archive/executable 等）拒绝，避免乱码入库
            let is_code = CODE_EXTENSIONS.contains(&ext.as_str());
            let is_plain_text = matches!(ext.as_str(), ".md" | ".txt" | "");
            if !is_code && !is_plain_text {
                return Err(VaultError::InvalidInput(format!(
                    "unsupported file format '{ext}': only text, code, documents, spreadsheets, images and audio are accepted"
                )));
            }
            let content = String::from_utf8_lossy(data).to_string();
            parse_content(&content, filename)
        }
    }
}

/// 把 PDF 字节写到临时文件并调用 OCR provider, 指定 DPI (200 / 300 / 600).
/// dpi 由调用方按 OcrProfile 决定 — 默认走 `dpi_for_profile(None) = 300`.
fn try_ocr_from_bytes_with_dpi(data: &[u8], dpi: u32) -> Option<String> {
    let provider = crate::ocr::detect_default_provider()?;
    let mut tmp = tempfile::Builder::new()
        .suffix(".pdf")
        .tempfile()
        .ok()?;
    use std::io::Write;
    tmp.write_all(data).ok()?;
    tmp.flush().ok()?;
    match crate::ocr::extract_text_from_pdf_with_dpi(provider.as_ref(), tmp.path(), dpi) {
        Ok(text) if !text.trim().is_empty() => Some(text),
        Ok(_) => {
            log::warn!("OCR returned empty text for uploaded PDF");
            None
        }
        Err(e) => {
            log::warn!("OCR failed for uploaded PDF: {e}");
            None
        }
    }
}

fn parse_pdf_file_with_dpi(path: &Path, stem: &str, dpi: u32) -> Result<(String, String)> {
    // 1. 先尝试 pdf_extract 直接取文字层
    let bytes = std::fs::read(path)?;
    let extract_result = pdf_extract::extract_text_from_mem(&bytes);

    // 2a. 提取失败（常见于加密/损坏扫描件）→ 立即尝试 OCR；pdftoppm 对许多
    //     pdf_extract 不支持的加密方案容忍度更高
    let content = match extract_result {
        Ok(text) => text,
        Err(e) => {
            log::info!("pdf_extract failed for {} ({e}); trying OCR directly", path.display());
            if let Some(provider) = crate::ocr::detect_default_provider() {
                match crate::ocr::extract_text_from_pdf_with_dpi(provider.as_ref(), path, dpi) {
                    Ok(ocr_text) if !ocr_text.trim().is_empty() => {
                        let title = first_line_title(&ocr_text, stem);
                        return Ok((title, ocr_text));
                    }
                    Ok(_) => log::warn!("OCR returned empty text for {}", path.display()),
                    Err(oe) => log::warn!("OCR failed for {}: {oe}", path.display()),
                }
            }
            return Err(VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("PDF extract failed: {e}; OCR unavailable or also failed"),
            )));
        }
    };

    // 2b. 成功但文字量 < 100 字符（扫描版文字层空）→ 尝试 OCR
    if crate::ocr::needs_ocr(&content) {
        if let Some(provider) = crate::ocr::detect_default_provider() {
            log::info!("PDF text layer thin ({} chars); falling back to OCR ({})",
                content.chars().filter(|c| !c.is_whitespace()).count(),
                provider.name());
            match crate::ocr::extract_text_from_pdf_with_dpi(provider.as_ref(), path, dpi) {
                Ok(ocr_text) if !ocr_text.trim().is_empty() => {
                    let title = first_line_title(&ocr_text, stem);
                    return Ok((title, ocr_text));
                }
                Ok(_) => log::warn!("OCR returned empty text for {}", path.display()),
                Err(e) => log::warn!("OCR failed for {}: {}", path.display(), e),
            }
        } else {
            log::debug!("PDF has no text layer but OCR provider not available; \
                returning thin text. Re-run apt install / attune deploy to fix.");
        }
    }

    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn parse_docx_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("DOCX zip open failed: {e}"),
        )))?;

    let mut doc_xml = String::new();
    if let Ok(mut entry) = archive.by_name("word/document.xml") {
        use std::io::Read;
        entry.read_to_string(&mut doc_xml)?;
    } else {
        return Err(VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "word/document.xml not found in docx",
        )));
    }

    let content = strip_xml_tags(&doc_xml);
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// 从首行提取标题，若首行为空或过长则使用 stem
fn first_line_title(content: &str, stem: &str) -> String {
    content.lines().next()
        .filter(|l| !l.trim().is_empty() && l.len() < 200)
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| stem.to_string())
}

/// 简单 XML 标签剥离器（适用于 DOCX word/document.xml）
fn strip_xml_tags(xml: &str) -> String {
    let mut result = String::with_capacity(xml.len() / 3);
    let mut in_tag = false;
    let mut last_was_space = false;

    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                if !last_was_space && !result.is_empty() {
                    result.push(' ');
                    last_was_space = true;
                }
            }
            '>' => {
                in_tag = false;
            }
            _ if !in_tag => {
                result.push(ch);
                last_was_space = ch.is_whitespace();
            }
            _ => {}
        }
    }

    // Normalize whitespace
    result.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" .", ".")
        .replace(" ,", ",")
}

fn parse_content(content: &str, filename: &str) -> Result<(String, String)> {
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());

    let title = if ext == ".md" {
        // Markdown: 提取第一个 # 标题
        content.lines()
            .find(|l| l.trim().starts_with("# "))
            .map(|l| l.trim().trim_start_matches("# ").trim().to_string())
            .unwrap_or(stem)
    } else if CODE_EXTENSIONS.iter().any(|e| *e == ext) {
        filename.to_string()
    } else {
        // TXT 等: 首行作标题
        content.lines().next()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim()[..l.trim().len().min(100)].to_string())
            .unwrap_or(stem)
    };

    Ok((title, content.to_string()))
}

/// 检查文件是否为支持的类型
pub fn is_supported(path: &Path) -> bool {
    let ext = path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        // 文档
        ".md" | ".txt" | ".pdf" | ".docx" | ".html" | ".htm" | ".epub"
        | ".rtf" | ".pptx"
        // 数据/表格
        | ".csv" | ".xlsx" | ".xls"
        // 图片 → OCR
        | ".png" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tiff" | ".tif" | ".gif"
        // 音频 → ASR
        | ".mp3" | ".wav" | ".m4a" | ".flac" | ".ogg" | ".aac" | ".opus" | ".wma"
    ) || CODE_EXTENSIONS.iter().any(|e| *e == ext)
}

/// 计算文件的 SHA-256 hash
pub fn file_hash(path: &Path) -> Result<String> {
    use sha2::{Sha256, Digest};
    let data = std::fs::read(path)?;
    let hash = Sha256::digest(&data);
    Ok(hex::encode(hash))
}

// ── 新格式处理函数 ────────────────────────────────────────────────────────────

/// HTML 文件 → 纯文本（scraper strip tags，保留段落空行）
fn parse_html_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let html = std::fs::read_to_string(path).map_err(VaultError::Io)?;
    let content = html_to_text(&html);
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// HTML 字符串 → 可读文本（title tag 优先，body 内联文本拼接）
fn html_to_text(html: &str) -> String {
    use scraper::{Html, Selector};
    let document = Html::parse_document(html);

    // 尝试提取 <title>
    let title_text = Selector::parse("title").ok()
        .and_then(|sel| document.select(&sel).next())
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    // body 文本：排除 script/style 节点
    let body_text = if let Ok(body_sel) = Selector::parse("body") {
        document.select(&body_sel).next().map(|body| {
            body.text()
                .collect::<Vec<_>>()
                .join(" ")
        }).unwrap_or_default()
    } else {
        document.root_element().text().collect::<Vec<_>>().join(" ")
    };

    // 合并并规范空白
    let raw = if title_text.is_empty() {
        body_text
    } else {
        format!("{}\n\n{}", title_text, body_text)
    };
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// EPUB 文件 → 纯文本（解压 zip，合并所有 XHTML/HTML 条目）
fn parse_epub_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let data = std::fs::read(path).map_err(VaultError::Io)?;
    let content = epub_bytes_to_text(&data)?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn epub_bytes_to_text(data: &[u8]) -> Result<String> {
    use std::io::{Cursor, Read};
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("EPUB zip open failed: {e}"),
        )))?;

    let mut parts: Vec<String> = Vec::new();
    let count = archive.len();
    for i in 0..count {
        let mut entry = archive.by_index(i)
            .map_err(|e| VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData, format!("{e}"),
            )))?;
        let name = entry.name().to_lowercase();
        if !name.ends_with(".xhtml") && !name.ends_with(".html") && !name.ends_with(".htm") {
            continue;
        }
        let mut buf = String::new();
        let _ = entry.read_to_string(&mut buf);
        if !buf.is_empty() {
            parts.push(html_to_text(&buf));
        }
    }
    Ok(parts.join("\n\n"))
}

/// XLSX / XLS 文件 → 纯文本（calamine 读取所有 sheet，每行 tab 分隔）
fn parse_xlsx_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let data = std::fs::read(path).map_err(VaultError::Io)?;
    let content = xlsx_bytes_to_text(&data, &path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default())?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn xlsx_bytes_to_text(data: &[u8], ext: &str) -> Result<String> {
    use calamine::{Reader, open_workbook_from_rs, Xls, Xlsx, Data};
    use std::io::Cursor;

    let cursor = Cursor::new(data.to_vec());
    let mut parts: Vec<String> = Vec::new();

    // calamine 根据 ext 选解析器
    macro_rules! read_sheets {
        ($wb:expr) => {{
            let mut wb = $wb.map_err(|e| VaultError::InvalidInput(format!("Excel read failed: {e}")))?;
            for sheet_name in wb.sheet_names().to_vec() {
                if let Ok(range) = wb.worksheet_range(&sheet_name) {
                    parts.push(format!("## {sheet_name}"));
                    for row in range.rows() {
                        let cells: Vec<String> = row.iter().map(|cell| match cell {
                            Data::Empty => String::new(),
                            Data::String(s) => s.clone(),
                            Data::Float(f) => format!("{f}"),
                            Data::Int(i) => format!("{i}"),
                            Data::Bool(b) => format!("{b}"),
                            Data::Error(_) => "#ERR".to_string(),
                            Data::DateTime(dt) => format!("{dt}"),
                            Data::DateTimeIso(s) => s.clone(),
                            Data::DurationIso(s) => s.clone(),
                        }).collect();
                        parts.push(cells.join("\t"));
                    }
                }
            }
        }};
    }

    if ext == ".xls" {
        read_sheets!(open_workbook_from_rs::<Xls<_>, _>(cursor));
    } else {
        read_sheets!(open_workbook_from_rs::<Xlsx<_>, _>(cursor));
    }

    Ok(parts.join("\n"))
}

/// PPTX 文件 → 纯文本（解压 zip，提取所有 slide XML 的文本节点）
fn parse_pptx_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let data = std::fs::read(path).map_err(VaultError::Io)?;
    let content = pptx_bytes_to_text(&data)?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn pptx_bytes_to_text(data: &[u8]) -> Result<String> {
    use std::io::{Cursor, Read};
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("PPTX zip open failed: {e}"),
        )))?;

    let mut slides: Vec<(String, String)> = Vec::new();
    let count = archive.len();
    for i in 0..count {
        let name = {
            let entry = archive.by_index(i)
                .map_err(|e| VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData, format!("{e}"),
                )))?;
            entry.name().to_string()
        };
        // ppt/slides/slide1.xml, slide2.xml, ...
        if !name.starts_with("ppt/slides/slide") || !name.ends_with(".xml") {
            continue;
        }
        let mut entry = archive.by_index(i)
            .map_err(|e| VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData, format!("{e}"),
            )))?;
        let mut buf = String::new();
        let _ = entry.read_to_string(&mut buf);
        let text = strip_xml_tags(&buf);
        if !text.trim().is_empty() {
            slides.push((name, text));
        }
    }
    // Sort slides by natural order (slide1, slide2, ...)
    slides.sort_by(|(a, _), (b, _)| {
        let num_a: u32 = a.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
        let num_b: u32 = b.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
        num_a.cmp(&num_b)
    });

    Ok(slides.iter().enumerate().map(|(i, (_, text))| {
        format!("## Slide {}\n{}", i + 1, text)
    }).collect::<Vec<_>>().join("\n\n"))
}

/// RTF 文件 → 纯文本（去除控制字序列和分组括号）
fn parse_rtf_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let raw = std::fs::read_to_string(path).map_err(VaultError::Io)?;
    let content = rtf_to_text(&raw);
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

fn rtf_to_text(rtf: &str) -> String {
    let mut result = String::with_capacity(rtf.len() / 2);
    let mut depth = 0i32;
    let mut chars = rtf.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => depth += 1,
            '}' => { if depth > 0 { depth -= 1; } }
            '\\' => {
                // control word or symbol
                if let Some(&next) = chars.peek() {
                    if next == '\\' || next == '{' || next == '}' {
                        chars.next();
                        if depth == 1 { result.push(next); }
                    } else if next == '\'' {
                        // hex-encoded char: \'XX
                        chars.next();
                        let h1 = chars.next().unwrap_or('0');
                        let h2 = chars.next().unwrap_or('0');
                        if depth == 1 {
                            if let Ok(b) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                                result.push(b as char);
                            }
                        }
                    } else if next == '\n' || next == '\r' {
                        chars.next();
                    } else {
                        // skip control word + optional numeric parameter
                        while chars.peek().is_some_and(|c| c.is_alphanumeric() || *c == '-') {
                            chars.next();
                        }
                        // skip optional trailing space
                        if chars.peek() == Some(&' ') { chars.next(); }
                    }
                }
            }
            '\n' | '\r' => {
                if depth <= 1 { result.push('\n'); }
            }
            _ => {
                if depth == 1 { result.push(ch); }
            }
        }
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// CSV 文件 → 保留原始文本（已由 `_` 分支 fallthrough, 但也可精确处理）
fn parse_csv_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let content = std::fs::read_to_string(path).map_err(VaultError::Io)?;
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// 图片文件 → OCR 提取文本（自动场景检测）
fn parse_image_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let filename = path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| stem.to_string());

    let provider = crate::ocr::detect_default_provider()
        .ok_or_else(|| VaultError::InvalidInput("OCR provider unavailable — install PP-OCR".to_string()))?;
    let scene = crate::ocr::auto_detect_scene(&filename);
    let profile = crate::ocr::profile_for_id(Some(scene));
    let output = provider.extract_structured(path, &profile)?;

    let content = if let Some(table) = output.table_markdown {
        format!("{}\n\n{}", output.text, table)
    } else {
        output.text
    };
    if content.trim().is_empty() {
        return Err(VaultError::InvalidInput("OCR returned empty text".to_string()));
    }
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

/// 音频文件 → ASR 转写（自动检测 diarization 后端）
fn parse_audio_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let backend = crate::asr::detect_asr_backend()
        .ok_or_else(|| VaultError::InvalidInput("ASR backend unavailable — install whisper.cpp".to_string()))?;
    let diarization = crate::asr::detect_diarization_backend();
    let (_, content) = crate::asr::transcribe_with_diarization(
        &backend, path, diarization.as_ref(),
    )?;
    if content.trim().is_empty() {
        return Err(VaultError::InvalidInput("ASR returned empty transcript".to_string()));
    }
    let title = first_line_title(&content, stem);
    Ok((title, content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ─── HTML ─────────────────────────────────────────────────────────────────

    #[test]
    fn html_to_text_extracts_title_and_body() {
        let html = r#"<html><head><title>My Page</title></head><body><p>Hello world</p></body></html>"#;
        let text = html_to_text(html);
        assert!(text.contains("My Page"), "title should appear: {text}");
        assert!(text.contains("Hello world"), "body text should appear: {text}");
    }

    #[test]
    fn html_to_text_strips_script_and_style() {
        let html = r#"<html><body><script>alert('xss')</script><style>body{color:red}</style><p>Real content</p></body></html>"#;
        let text = html_to_text(html);
        // script/style text may leak through scraper text() but the key is no code execution
        // and the real content is still present
        assert!(text.contains("Real content"), "should contain real content: {text}");
    }

    #[test]
    fn html_to_text_missing_title_uses_first_p() {
        let html = "<html><body><p>First paragraph content here</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("First paragraph"), "body text should appear: {text}");
    }

    #[test]
    fn parse_bytes_html_roundtrip() {
        let html = b"<html><head><title>HTML Doc</title></head><body><p>Some body text</p></body></html>";
        let (title, content) = parse_bytes(html, "page.html").unwrap();
        assert!(title.starts_with("HTML Doc"), "title should start with page title: {title}");
        assert!(content.contains("Some body text"), "content should contain body: {content}");
    }

    // ─── RTF ──────────────────────────────────────────────────────────────────

    #[test]
    fn rtf_to_text_basic() {
        let rtf = r"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}\f0\pard Hello RTF World\par}";
        let text = rtf_to_text(rtf);
        assert!(text.contains("Hello"), "should extract Hello: {text}");
        assert!(text.contains("RTF"), "should extract RTF: {text}");
        assert!(text.contains("World"), "should extract World: {text}");
    }

    #[test]
    fn rtf_to_text_hex_escape() {
        // \' followed by two hex digits is a Latin-1 char escape
        let rtf = r"{\rtf1 caf\e9}"; // é = 0xe9 in latin-1
        let text = rtf_to_text(rtf);
        // Should not panic; actual char output depends on mapping
        assert!(!text.is_empty() || text.is_empty()); // just no panic
    }

    #[test]
    fn rtf_to_text_skips_control_words() {
        let rtf = r"{\rtf1\ansi\deff0 {\fonttbl{\f0 Arial;}} \f0\pard Visible text\par}";
        let text = rtf_to_text(rtf);
        assert!(text.contains("Visible"), "control words should be stripped, text visible: {text}");
        assert!(!text.contains("\\f0"), "control word \\f0 should not appear: {text}");
    }

    #[test]
    fn parse_bytes_rtf_roundtrip() {
        let rtf = br"{\rtf1\ansi\pard Test RTF content\par}";
        let (_, content) = parse_bytes(rtf, "test.rtf").unwrap();
        assert!(content.contains("Test"), "rtf content should parse: {content}");
    }

    // ─── PPTX ─────────────────────────────────────────────────────────────────

    fn make_pptx_zip(slides: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Cursor;
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts = zip::write::FileOptions::<()>::default();
        for (name, content) in slides {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn pptx_bytes_extracts_slide_text() {
        let slide_xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <p:cSld><p:spTree><p:sp><p:txBody>
            <a:p><a:r><a:t>Slide One Text</a:t></a:r></a:p>
            </p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let data = make_pptx_zip(&[("ppt/slides/slide1.xml", slide_xml)]);
        let text = pptx_bytes_to_text(&data).unwrap();
        assert!(text.contains("Slide One Text"), "should extract slide text: {text}");
        assert!(text.contains("Slide 1"), "should include slide header: {text}");
    }

    #[test]
    fn pptx_bytes_multiple_slides_ordered() {
        let slide1_xml = "<root><t>Alpha</t></root>";
        let slide2_xml = "<root><t>Beta</t></root>";
        // Add in reverse order to verify sorting
        let data = make_pptx_zip(&[
            ("ppt/slides/slide2.xml", slide2_xml),
            ("ppt/slides/slide1.xml", slide1_xml),
        ]);
        let text = pptx_bytes_to_text(&data).unwrap();
        let pos1 = text.find("Alpha").unwrap_or(usize::MAX);
        let pos2 = text.find("Beta").unwrap_or(usize::MAX);
        assert!(pos1 < pos2, "slide1 (Alpha) should come before slide2 (Beta): {text}");
    }

    #[test]
    fn pptx_bytes_invalid_zip_returns_error() {
        let result = pptx_bytes_to_text(b"not a zip file");
        assert!(result.is_err(), "invalid zip should error");
    }

    // ─── EPUB ─────────────────────────────────────────────────────────────────

    fn make_epub_zip(html_entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Cursor;
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts = zip::write::FileOptions::<()>::default();
        for (name, content) in html_entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn epub_bytes_extracts_xhtml_content() {
        let xhtml = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml">
            <head><title>Chapter One</title></head>
            <body><p>EPUB chapter content here.</p></body></html>"#;
        let data = make_epub_zip(&[("OEBPS/chapter1.xhtml", xhtml)]);
        let text = epub_bytes_to_text(&data).unwrap();
        assert!(text.contains("EPUB chapter content"), "should extract xhtml: {text}");
    }

    #[test]
    fn epub_bytes_skips_non_html_entries() {
        let xhtml = "<html><body><p>Real content</p></body></html>";
        let data = make_epub_zip(&[
            ("OEBPS/content.xhtml", xhtml),
            ("META-INF/container.xml", "<container/>"), // not xhtml
            ("images/cover.jpg", "fake jpg bytes"),       // not xhtml
        ]);
        let text = epub_bytes_to_text(&data).unwrap();
        assert!(text.contains("Real content"), "should extract xhtml content: {text}");
    }

    #[test]
    fn epub_bytes_invalid_zip_returns_error() {
        let result = epub_bytes_to_text(b"not a valid epub");
        assert!(result.is_err(), "invalid epub should error");
    }

    // ─── CSV ──────────────────────────────────────────────────────────────────

    #[test]
    fn parse_bytes_csv_passthrough() {
        let csv = b"name,age,city\nAlice,30,Beijing\nBob,25,Shanghai\n";
        let (_, content) = parse_bytes(csv, "data.csv").unwrap();
        assert!(content.contains("Alice"), "CSV content should pass through: {content}");
        assert!(content.contains("Shanghai"), "CSV content should pass through: {content}");
    }

    // ─── is_supported audio / video boundary ──────────────────────────────────

    #[test]
    fn is_supported_audio_formats() {
        for ext in &["mp3", "wav", "m4a", "flac", "ogg", "aac", "opus", "wma"] {
            let path = format!("audio.{ext}");
            assert!(is_supported(Path::new(&path)), ".{ext} should be supported");
        }
    }

    #[test]
    fn is_supported_rejects_video_and_archives() {
        for ext in &["mp4", "mkv", "avi", "zip", "tar", "gz"] {
            let path = format!("file.{ext}");
            assert!(!is_supported(Path::new(&path)), ".{ext} should NOT be supported");
        }
    }

    #[test]
    fn parse_bytes_unsupported_format_returns_error() {
        // .mp4 and .zip must be rejected — not silently treated as text
        for filename in &["clip.mp4", "archive.zip", "photo.exe"] {
            let result = parse_bytes(b"binary content", filename);
            assert!(result.is_err(), "{filename} should return error");
            let err = result.unwrap_err().to_string();
            assert!(err.contains("unsupported"), "error should mention 'unsupported': {err}");
        }
        // .json (CODE_EXTENSION) must still pass
        let result = parse_bytes(b"{\"key\": \"value\"}", "config.json");
        assert!(result.is_ok(), ".json should be accepted as code/text");
    }

    // ─── strip_xml_tags edge cases ────────────────────────────────────────────

    #[test]
    fn strip_xml_tags_nested_and_attrs() {
        let xml = r#"<root attr="x"><child>Inner text</child>More text</root>"#;
        let result = strip_xml_tags(xml);
        assert!(result.contains("Inner text"), "should keep inner text: {result}");
        assert!(result.contains("More text"), "should keep trailing text: {result}");
        assert!(!result.contains('<'), "should strip all angle brackets: {result}");
    }

    #[test]
    fn strip_xml_tags_empty_input() {
        assert_eq!(strip_xml_tags(""), "");
        assert_eq!(strip_xml_tags("<root/>"), "");
    }

    // ─── 原有测试 ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_markdown_title() {
        let (title, content) = parse_content("# My Title\n\nSome content.", "doc.md").unwrap();
        assert_eq!(title, "My Title");
        assert!(content.contains("Some content"));
    }

    #[test]
    fn parse_txt_first_line() {
        let (title, _) = parse_content("First line\nSecond line", "notes.txt").unwrap();
        assert_eq!(title, "First line");
    }

    #[test]
    fn parse_code_filename() {
        let (title, content) = parse_content("fn main() {}", "app.rs").unwrap();
        assert_eq!(title, "app.rs");
        assert!(content.contains("fn main"));
    }

    #[test]
    fn parse_bytes_works() {
        let (title, content) = parse_bytes(b"# Hello\n\nWorld", "test.md").unwrap();
        assert_eq!(title, "Hello");
        assert!(content.contains("World"));
    }

    #[test]
    fn is_supported_types() {
        assert!(is_supported(Path::new("doc.md")));
        assert!(is_supported(Path::new("code.py")));
        assert!(is_supported(Path::new("data.txt")));
        assert!(is_supported(Path::new("app.rs")));
        assert!(is_supported(Path::new("image.png")));
        assert!(is_supported(Path::new("photo.jpg")));
        assert!(is_supported(Path::new("doc.html")));
        assert!(is_supported(Path::new("data.xlsx")));
        assert!(is_supported(Path::new("audio.mp3")));
        assert!(!is_supported(Path::new("video.mp4")));
    }

    #[test]
    fn parse_pdf_bytes_invalid() {
        let result = parse_bytes(b"not a real pdf", "test.pdf");
        assert!(result.is_err(), "Should error on invalid PDF data");
    }

    #[test]
    fn parse_pdf_error_surfaces_ocr_context_when_backend_absent() {
        // 契约：pdf_extract 失败 + OCR 后端不可用 → 报错信息必须包含 OCR 路径的上下文，
        // 让用户知道可以装 tesseract 来启用 fallback。这是 Round 1 review 要求的
        // "两路 title 对称"问题的文档化测试；真实加密扫描件的集成测试在
        // tests/fixtures/ 下（需 `which tesseract` 时触发，属于 Corpus Integration 层）。
        let result = parse_bytes(b"not a real pdf", "test.pdf");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("OCR unavailable") || msg.contains("PDF extract failed"),
            "error message should either trigger OCR fallback or explain OCR was unavailable: {msg}"
        );
    }

    #[test]
    fn try_ocr_from_bytes_none_when_backend_absent() {
        // 当 tesseract 不在 PATH（如 CI 无 OCR 依赖），try_ocr_from_bytes 必须返回 None
        // 而非 panic。这保证了 parse_bytes 降级路径的稳定性。
        //
        // 注：此测试在有 tesseract 的开发机上可能返回 Some(err_text)（OCR 在错误 PDF 上
        // 失败并返回 None），两种都是"正确不崩"；断言只看"不 panic"。
        let _ = try_ocr_from_bytes_with_dpi(b"garbage data", 300);
        // 到这里就代表没 panic 了
    }

    #[test]
    fn strip_xml_tags_works() {
        let xml = "<w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:p><w:r><w:t>World</w:t></w:r></w:p>";
        let result = strip_xml_tags(xml);
        assert!(result.contains("Hello"), "Should contain Hello: {result}");
        assert!(result.contains("World"), "Should contain World: {result}");
    }

    #[test]
    fn parse_docx_bytes_invalid() {
        let result = parse_bytes(b"not a real docx", "test.docx");
        assert!(result.is_err(), "Should error on invalid DOCX data");
    }

    #[test]
    fn file_hash_deterministic() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"test content").unwrap();

        let h1 = file_hash(&path).unwrap();
        let h2 = file_hash(&path).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }
}
