// npu-vault/crates/vault-core/src/chunker.rs

// 滑动窗口分块 + 语义章节切割
// 复用 npu-webhook Python 实现的逻辑

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
        start = actual_end.saturating_sub(overlap);
        if start == 0 && !chunks.is_empty() {
            break; // 防止无限循环
        }
    }
    chunks
}

/// 语义章节切割: Markdown 标题 / 代码 def|class / 段落大小
///
/// per reviewer I5：W2 后改为 wrapper，调 [`extract_sections_with_path`] 后丢弃 path，
/// 避免两份函数维护相同的章节切分逻辑（未来一份改了另一份会漂移）。
pub fn extract_sections(content: &str) -> Vec<(usize, String)> {
    extract_sections_with_path(content)
        .into_iter()
        .map(|s| (s.section_idx, s.content))
        .collect()
}

// ── J1：Chunk 面包屑路径前缀（W2，2026-04-27）─────────────────────────────
//
// 设计来源（per docs/superpowers/specs/2026-04-27-w2-rag-quality-batch1-design.md §J1）：
//   - 吴师兄《鹅厂面试官追问：你的 RAG 能跑通 Demo？》§1
//     https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw
//     "每个 chunk 加上下文路径（产品名 > 章 > 节），让 LLM 知道 chunk 在讲什么"
//
// 与原 extract_sections 的关系：原函数保留不破坏向后兼容。新调用方用
// extract_sections_with_path，旧调用方（单元测试 + indexer 历史路径）继续工作。

/// 一个章节附加来自文档根的标题层级路径。供 J1 chunk 面包屑前缀注入用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionWithPath {
    pub section_idx: usize,
    /// 文档根开始的标题层级；例如 ["公司手册", "第三章 福利", "3.2 假期"]。
    /// 第一个标题前的内容为空 Vec（因为没有任何上下文路径可言）。
    pub path: Vec<String>,
    pub content: String,
}

impl SectionWithPath {
    /// 把面包屑作为 Markdown blockquote 前缀拼接到 content 头部，返回新字符串。
    /// 用 `> ` 前缀让 LLM prompt 自然可读（人和 LLM 都把 `>` 视为元信息）。
    /// path 为空时直接返回原 content（无前缀）。
    pub fn with_breadcrumb_prefix(&self) -> String {
        if self.path.is_empty() {
            return self.content.clone();
        }
        let crumbs = self.path.join(" > ");
        format!("> {}\n\n{}", crumbs, self.content)
    }
}

/// 标题深度：返回 (depth, title_text) 或 None（非标题行）。
///
/// Markdown：连续 `#` 数即 depth（1-6，CommonMark 标准）。
/// 代码 boundary（def/class/fn/...）：统一 depth=1（代码结构很少深嵌套，
/// 用平铺结构避免错误的"嵌套类"路径误导 LLM）。
fn heading_depth_and_text(line: &str) -> Option<(usize, String)> {
    // Markdown：H1-H6 标准支持
    let hash_count = line.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hash_count) {
        // 必须是 `#{1,6} ` 后跟内容，避免 "#tag" / "##" 单独成行误判
        let rest = &line[hash_count..];
        if let Some(stripped) = rest.strip_prefix(' ') {
            let title = stripped.trim();
            if !title.is_empty() {
                return Some((hash_count, title.to_string()));
            }
        }
    }
    // 代码：按整行作 title（含签名），depth=1
    for prefix in ["pub fn ", "fn ", "def ", "class ", "impl "] {
        if line.starts_with(prefix) {
            return Some((1, line.trim().to_string()));
        }
    }
    None
}

/// J1 主函数：按章节切并保持标题层级路径。
///
/// 返回的每个 [`SectionWithPath`]：
/// - `path` = 从文档根到本章节的标题序列（Markdown 嵌套 / 代码同级）
/// - `content` = 章节正文（首行为标题本身）
///
/// 调用方（如 indexer pipeline）用 [`SectionWithPath::with_breadcrumb_prefix`]
/// 把面包屑注入 chunk 文本前再做 embedding / 存储。
pub fn extract_sections_with_path(content: &str) -> Vec<SectionWithPath> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }
    let mut sections: Vec<SectionWithPath> = Vec::new();
    let mut current_section = String::new();
    let mut current_path_when_section_started: Vec<String> = Vec::new();
    // path_stack[i-1] = 深度 i 的最近标题；遇到深度 d 的新标题时，
    // 先把 stack 截断到长度 d-1（dedent），再 push 新标题。
    let mut path_stack: Vec<String> = Vec::new();
    let mut section_idx: usize = 0;
    let mut path_for_pending_section_set = false;

    for line in &lines {
        let heading = heading_depth_and_text(line);

        if let Some((depth, title)) = &heading {
            // 在新标题出现前，先把上一个 section 落库
            if !current_section.trim().is_empty() {
                sections.push(SectionWithPath {
                    section_idx,
                    path: current_path_when_section_started.clone(),
                    content: current_section.clone(),
                });
                section_idx += 1;
                current_section.clear();
                // path_for_pending_section_set = false 之前是显式 reset，
                // 但下一行就会重新赋 true（line 160），是 dead write
                // (per W3 batch B 遗留代码扫描清理)
            }
            // 维护栈：dedent 到 depth-1，再 push（per spec §J1 path stack maintenance）
            // 防御性：depth >= 1 已由 heading_depth_and_text 保证（H1-H6 + 代码 boundary
            // 都 >= 1），但用 .max(1).saturating_sub(1) 防止未来扩展某 prefix 写成 (0, ...)
            // 时 usize underflow → truncate(usize::MAX) 静默失效（per reviewer S1）
            debug_assert!(*depth >= 1, "heading depth must be >=1, got {depth}");
            path_stack.truncate(depth.max(&1).saturating_sub(1));
            path_stack.push(title.clone());
            // 新 section 的 path 是 push 后的快照
            current_path_when_section_started = path_stack.clone();
            path_for_pending_section_set = true;
        }

        // section 没标题前导（文档开头未匹配标题）的 path 是空 Vec
        if !path_for_pending_section_set && current_section.is_empty() {
            current_path_when_section_started = Vec::new();
            path_for_pending_section_set = true;
        }

        current_section.push_str(line);
        current_section.push('\n');

        // 同 extract_sections：达到目标段落大小时尝试在空行切（避免章节超长）
        if heading.is_none()
            && current_section.len() >= SECTION_TARGET_SIZE
            && line.trim().is_empty()
        {
            sections.push(SectionWithPath {
                section_idx,
                path: current_path_when_section_started.clone(),
                content: current_section.clone(),
            });
            section_idx += 1;
            current_section.clear();
            // 大段切割时 path 不变（仍在同一标题下）
        }
    }
    if !current_section.trim().is_empty() {
        sections.push(SectionWithPath {
            section_idx,
            path: current_path_when_section_started,
            content: current_section,
        });
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
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(!c.is_empty());
        }
    }

    // ── J1 tests（per spec §J1）──────────────────────────────────────

    #[test]
    fn extract_sections_with_path_markdown_nested() {
        let content = "# 公司手册\n\n概述。\n\n## 第三章 福利\n\n福利总览。\n\n### 3.2 假期\n\n年假 15 天。";
        let secs = extract_sections_with_path(content);
        // 4 sections: 概述（path=[公司手册]） / 福利总览（[公司手册, 第三章 福利]）/
        // 假期（[公司手册, 第三章 福利, 3.2 假期]） — 第一个标题前内容如果有则 path=空
        // 当前 content "公司手册" 直接是第一个标题，所以无 path-empty section
        assert!(secs.len() >= 3, "期望 ≥3 sections, got {}: {:?}", secs.len(), secs);
        // 验证最后一个 section 的 path 三层
        let last = &secs[secs.len() - 1];
        assert_eq!(
            last.path,
            vec!["公司手册".to_string(), "第三章 福利".to_string(), "3.2 假期".to_string()]
        );
        assert!(last.content.contains("年假 15 天"));
    }

    #[test]
    fn extract_sections_with_path_dedent_pops_stack() {
        // # A → ## B → # C 时，C 的 path 应为 [C] 不是 [A, B, C]
        let content = "# A\n内容 A\n\n## B\n内容 B\n\n# C\n内容 C";
        let secs = extract_sections_with_path(content);
        // 找 path 包含 "C" 的 section
        let c_section = secs.iter().find(|s| s.content.contains("内容 C")).expect("missing C");
        assert_eq!(c_section.path, vec!["C".to_string()], "dedent must reset to depth-1: got {:?}", c_section.path);
    }

    #[test]
    fn extract_sections_with_path_code_treated_flat() {
        // 代码 fn / impl 同级（depth=1）；不会出现 [fn outer, fn inner] 的错路径
        let content = "fn foo() {\n    println!(\"foo\");\n}\n\npub fn bar() {\n    helper();\n}";
        let secs = extract_sections_with_path(content);
        for s in &secs {
            assert!(s.path.len() <= 1, "代码 boundary 路径深度不应 > 1: {:?}", s.path);
        }
    }

    #[test]
    fn extract_sections_with_path_empty_input() {
        let secs = extract_sections_with_path("");
        assert!(secs.is_empty());
    }

    #[test]
    fn extract_sections_with_path_preamble_no_heading() {
        // 文档开头无标题的内容 → path 为空 Vec
        let content = "intro paragraph\n\n# 后来的标题\n\n章节正文";
        let secs = extract_sections_with_path(content);
        // 第一个 section 的 path 应为空（preamble）
        assert!(!secs.is_empty());
        assert!(secs[0].path.is_empty(), "preamble path 应为空，得到 {:?}", secs[0].path);
        // 第二个 section 的 path 应有 1 层
        let after = secs.iter().find(|s| s.content.contains("章节正文")).expect("missing post-heading");
        assert_eq!(after.path, vec!["后来的标题".to_string()]);
    }

    #[test]
    fn breadcrumb_prefix_formats_blockquote() {
        let s = SectionWithPath {
            section_idx: 0,
            path: vec!["A".to_string(), "B".to_string()],
            content: "正文行".to_string(),
        };
        let prefixed = s.with_breadcrumb_prefix();
        assert!(prefixed.starts_with("> A > B\n\n"), "got: {prefixed}");
        assert!(prefixed.contains("正文行"));
    }

    #[test]
    fn breadcrumb_prefix_no_path_no_prefix() {
        let s = SectionWithPath {
            section_idx: 0,
            path: vec![],
            content: "无 path 的内容".to_string(),
        };
        // 空 path 不加前缀（避免 ">  \n\n" 这种垃圾）
        assert_eq!(s.with_breadcrumb_prefix(), "无 path 的内容");
    }
}
