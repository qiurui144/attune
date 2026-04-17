// npu-vault/crates/vault-core/src/chunker.rs

/// 滑动窗口分块 + 语义章节切割
/// 复用 npu-webhook Python 实现的逻辑

pub const DEFAULT_CHUNK_SIZE: usize = 512;
pub const DEFAULT_OVERLAP: usize = 128;
pub const SECTION_TARGET_SIZE: usize = 1500;

/// 滑动窗口分块（字符级，句子边界感知）
pub fn chunk(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    // overlap 必须 < chunk_size，否则滑动步长 <= 0 导致无限循环
    let overlap = overlap.min(chunk_size.saturating_sub(1));
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        // 尝试在句子边界切割
        let actual_end = if end < chars.len() {
            find_sentence_boundary(&chars, start, end).unwrap_or(end)
        } else {
            end
        };
        let chunk_text: String = chars[start..actual_end].iter().collect();
        if !chunk_text.trim().is_empty() {
            chunks.push(chunk_text);
        }
        if actual_end >= chars.len() {
            break;
        }
        start = if actual_end > overlap { actual_end - overlap } else { 0 };
        if start == 0 && !chunks.is_empty() {
            break; // 防止无限循环
        }
    }
    chunks
}

/// 语义章节切割: Markdown 标题 / 代码 def|class / 段落大小
pub fn extract_sections(content: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }
    let mut sections: Vec<(usize, String)> = Vec::new();
    let mut current_section = String::new();
    let mut section_idx: usize = 0;

    for line in &lines {
        let is_boundary = line.starts_with("# ")
            || line.starts_with("## ")
            || line.starts_with("### ")
            || line.starts_with("def ")
            || line.starts_with("class ")
            || line.starts_with("fn ")
            || line.starts_with("pub fn ")
            || line.starts_with("impl ");

        if is_boundary && !current_section.trim().is_empty() {
            sections.push((section_idx, current_section.clone()));
            section_idx += 1;
            current_section.clear();
        }

        current_section.push_str(line);
        current_section.push('\n');

        // 段落大小限制
        if current_section.len() >= SECTION_TARGET_SIZE && !is_boundary {
            // 尝试在空行处切割
            if line.trim().is_empty() {
                sections.push((section_idx, current_section.clone()));
                section_idx += 1;
                current_section.clear();
            }
        }
    }
    if !current_section.trim().is_empty() {
        sections.push((section_idx, current_section));
    }
    sections
}

fn find_sentence_boundary(chars: &[char], start: usize, end: usize) -> Option<usize> {
    // 从 end 往回找句子结束符
    let search_start = if end > start + 50 { end - 50 } else { start };
    for i in (search_start..end).rev() {
        let c = chars[i];
        if c == '。' || c == '.' || c == '!' || c == '?' || c == '\n' || c == '！' || c == '？' {
            return Some(i + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_short_text_single() {
        let chunks = chunk("Hello world", 512, 128);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn chunk_long_text_multiple() {
        let text = "A".repeat(1000);
        let chunks = chunk(&text, 512, 128);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].len() <= 512);
    }

    #[test]
    fn extract_sections_markdown() {
        let content = "# Title\n\nIntro paragraph.\n\n## Section 1\n\nContent 1.\n\n## Section 2\n\nContent 2.";
        let sections = extract_sections(content);
        assert!(sections.len() >= 2, "Should split on ## headings: got {}", sections.len());
        assert!(sections[0].1.contains("Title"));
    }

    #[test]
    fn extract_sections_code() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n\npub fn helper() {\n    // code\n}";
        let sections = extract_sections(content);
        assert!(sections.len() >= 2, "Should split on fn boundaries: got {}", sections.len());
    }

    #[test]
    fn extract_sections_empty() {
        let sections = extract_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn chunk_with_chinese() {
        let text = "这是一段中文内容。".repeat(100);
        let chunks = chunk(&text, 512, 128);
        assert!(chunks.len() >= 1);
        for c in &chunks {
            assert!(!c.is_empty());
        }
    }
}
