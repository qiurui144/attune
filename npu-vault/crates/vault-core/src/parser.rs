// npu-vault/crates/vault-core/src/parser.rs

use std::path::Path;
use crate::error::{Result, VaultError};

/// 代码文件扩展名
const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".js", ".ts", ".rs", ".go", ".java", ".c", ".cpp", ".h",
    ".rb", ".php", ".swift", ".kt", ".scala", ".sh", ".bash", ".zsh",
    ".toml", ".yaml", ".yml", ".json", ".xml", ".html", ".css",
];

/// 解析文件 → (title, content)
pub fn parse_file(path: &Path) -> Result<(String, String)> {
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
        ".pdf" => parse_pdf_file(path, &stem),
        ".docx" => parse_docx_file(path, &stem),
        _ => {
            // Text-based files (md, txt, code)
            let content = std::fs::read_to_string(path)
                .map_err(|e| VaultError::Io(e))?;
            parse_content(&content, &filename)
        }
    }
}

/// 从内存解析 → (title, content)
pub fn parse_bytes(data: &[u8], filename: &str) -> Result<(String, String)> {
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());

    match ext.as_str() {
        ".pdf" => {
            let content = pdf_extract::extract_text_from_mem(data)
                .map_err(|e| VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("PDF extract failed: {e}"),
                )))?;
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
        _ => {
            let content = String::from_utf8_lossy(data).to_string();
            parse_content(&content, filename)
        }
    }
}

fn parse_pdf_file(path: &Path, stem: &str) -> Result<(String, String)> {
    let bytes = std::fs::read(path)?;
    let content = pdf_extract::extract_text_from_mem(&bytes)
        .map_err(|e| VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("PDF extract failed: {e}"),
        )))?;
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
    matches!(ext.as_str(), ".md" | ".txt" | ".pdf" | ".docx")
        || CODE_EXTENSIONS.iter().any(|e| *e == ext)
}

/// 计算文件的 SHA-256 hash
pub fn file_hash(path: &Path) -> Result<String> {
    use sha2::{Sha256, Digest};
    let data = std::fs::read(path)?;
    let hash = Sha256::digest(&data);
    Ok(hex::encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        assert!(!is_supported(Path::new("image.png")));
        assert!(!is_supported(Path::new("video.mp4")));
    }

    #[test]
    fn parse_pdf_bytes_invalid() {
        let result = parse_bytes(b"not a real pdf", "test.pdf");
        assert!(result.is_err(), "Should error on invalid PDF data");
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
