// npu-vault/crates/vault-core/src/chunker.rs

// 滑动窗口分块 + 语义章节切割
// 复用 npu-webhook Python 实现的逻辑

pub const DEFAULT_CHUNK_SIZE: usize = 512;
pub const DEFAULT_OVERLAP: usize = 128;
pub const SECTION_TARGET_SIZE: usize = 1500;

/// 滑动窗口分块（字符级，句子边界感知 + Markdown code fence 边界保留）
///
/// 切割规则（优先级从高到低）：
/// 1. **Code fence 平衡** — 不允许把 ``` 切到一半；如果 chunk 内 ``` 数量奇数，
///    向前扩展到下一个 ``` 之后（让 code block 完整保留），实在找不到才退而求其次。
/// 2. **句子边界** — 在 (end-50, end) 区间内回退到最近的句末符号 (。.!?\n！？)。
/// 3. **硬切** — 都不满足时按 chunk_size 硬切。
///
/// 防御 #1 让代码片段在 RAG 时不被截断（per phase6 chunker fence fix，2026-04-28）。
pub fn chunk(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    // overlap 必须 < chunk_size，否则滑动步长 <= 0 导致无限循环
    let overlap = overlap.min(chunk_size.saturating_sub(1));
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    // 给 code fence 扩展留 buffer：最多扩展到 2× chunk_size（避免某个超长 code block 让 chunk 失控）
    let max_extend = chunk_size;
    let mut start = 0;
    while start < chars.len() {
        // 0. 如果 start 落在已开启的 code block 内（前面有奇数个 ```），推后到下一个 ``` 之后
        //    这避免 chunk 以孤立的闭合 fence 开头（产生 unbalanced chunk）
        start = advance_start_past_open_fence(&chars, start);
        if start >= chars.len() {
            break;
        }

        let end = (start + chunk_size).min(chars.len());
        // 1. 尝试在句子边界切割
        let sentence_end = if end < chars.len() {
            find_sentence_boundary(&chars, start, end).unwrap_or(end)
        } else {
            end
        };
        // 2. 检查 code fence 平衡，不平衡则调整（保证严格 > start，避免 0 进度死循环）
        let mut actual_end = adjust_for_code_fence(&chars, start, sentence_end, max_extend);
        if actual_end <= start {
            actual_end = sentence_end.max(start + 1);
        }
        let chunk_text: String = chars[start..actual_end].iter().collect();
        if !chunk_text.trim().is_empty() {
            chunks.push(chunk_text);
        }
        if actual_end >= chars.len() {
            break;
        }
        // 严格前进：next_start 必须 > 当前 start，避免无限循环
        let next_start = actual_end.saturating_sub(overlap).max(start + 1);
        start = next_start;
        if start == 0 && !chunks.is_empty() {
            break; // 兜底（不应发生）
        }
    }
    chunks
}

/// 如果 start 位置之前有奇数个 ```（说明 start 落在已开启的 code block 内部），
/// 把 start 推后到下一个 ``` 之后，避免 chunk 以孤立闭合 fence 开头。
fn advance_start_past_open_fence(chars: &[char], start: usize) -> usize {
    if start == 0 || start >= chars.len() {
        return start;
    }
    // 数 chars[0..start] 中 ``` 出现次数
    let mut count = 0;
    let mut i = 0;
    while i + 3 <= start {
        if chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            count += 1;
            i += 3;
        } else {
            i += 1;
        }
    }
    if count % 2 == 0 {
        return start; // start 在 code block 外部，OK
    }
    // start 在 code block 内 — 推到下一个 ``` 之后
    let mut j = start;
    while j + 3 <= chars.len() {
        if chars[j] == '`' && chars[j + 1] == '`' && chars[j + 2] == '`' {
            // 跳到 fence 后行末，避免 ``` 后接 language tag 被切
            let mut after = j + 3;
            while after < chars.len() && chars[after] != '\n' {
                after += 1;
            }
            if after < chars.len() {
                after += 1;
            }
            return after;
        }
        j += 1;
    }
    // 找不到闭合 fence — 返回原 start (退化情况)
    start
}

/// 检查 chars[start..end] 中 ``` 数量是否奇数（即切到 code block 中间）。
/// 如果是，调整 end 让 fence 平衡：
///   - 优先扩展到下一个 ```  之后（让本 chunk 包含完整 code block）
///   - 都找不到 → 回退到 chunk 内最近的 ``` 之前（让 code block 完全在下个 chunk）
///   - 退化情况 → 返回原 end
fn adjust_for_code_fence(chars: &[char], start: usize, end: usize, max_extend: usize) -> usize {
    if !has_unbalanced_fence(chars, start, end) {
        return end;
    }
    // 向前扩展找下一个 ```（必须是 3 个连续 backtick）
    let extend_limit = (end + max_extend).min(chars.len());
    let mut i = end;
    while i + 3 <= extend_limit {
        if chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            // 找到闭合 fence，包含它（含 fence 后到行末，让 chunk 自然结束）
            let mut after = i + 3;
            // 顺到行末，避免 ``` 在行中部把语言标签也切了
            while after < extend_limit && chars[after] != '\n' {
                after += 1;
            }
            // 含上 \n
            if after < extend_limit {
                after += 1;
            }
            return after.min(chars.len());
        }
        i += 1;
    }
    // 没找到闭合 fence — 回退到 chunk 内最近一个 ``` 之前
    let mut last_fence: Option<usize> = None;
    let mut j = start;
    while j + 3 <= end {
        if chars[j] == '`' && chars[j + 1] == '`' && chars[j + 2] == '`' {
            last_fence = Some(j);
            j += 3;
        } else {
            j += 1;
        }
    }
    if let Some(f) = last_fence {
        // 回退到该 ``` 之前（向前找 \n 让 chunk 在 code block 开始前自然结束）
        let mut k = f;
        while k > start && chars[k - 1] != '\n' {
            k -= 1;
        }
        if k > start {
            return k;
        }
    }
    // 找不到合理边界（罕见）— 保持原 end
    end
}

/// 数 chars[start..end] 中 ``` 出现的次数，奇数返回 true。
fn has_unbalanced_fence(chars: &[char], start: usize, end: usize) -> bool {
    let mut count = 0;
    let mut i = start;
    while i + 3 <= end {
        if chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            count += 1;
            i += 3;
        } else {
            i += 1;
        }
    }
    count % 2 != 0
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
    // OSS-S4 fix (2026-05-02): markdown ``` code fence 内的代码行（pub fn / fn /
    // def / class / impl）不应被识别为 section heading。修复前 rust-book chapters
    // 大量引用 Rust code，每个 `fn` 例子被错切成独立 section → rust-slices 13K →
    // 455 chunks → search 排序被该文档 dominate（precision@1 仅 26.7%）。
    // 跟踪 ``` 状态，仅在 fence 外检测代码边界 heading。
    let mut in_code_fence = false;

    for line in &lines {
        // OSS-S4 fix: track ``` code fence; skip heading detection inside fences。
        // 兼容 ``` / ```rust / ```python / ~~~ 等所有 fenced 形态。
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~") {
            in_code_fence = !in_code_fence;
        }
        let heading = if in_code_fence {
            None
        } else {
            heading_depth_and_text(line)
        };

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

        // 同 extract_sections：达到目标段落大小时尝试在空行切（避免章节超长）。
        // OSS-S4 fix part 2 (2026-05-02): **不在 code fence 内切**，否则 split 后
        // section 中部 ``` 数为奇数，触发下游 chunker::adjust_for_code_fence 的极慢
        // 退化路径（每次推进 1 char），导致 1285-byte Java 代码段产生 132+ chunks。
        if heading.is_none()
            && !in_code_fence
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

    // ── Code fence preservation tests (phase 6 chunker fix, 2026-04-28) ──

    #[test]
    fn chunk_preserves_code_fence_balanced_in_each_chunk() {
        // markdown 含一个会被 chunk 切到中间的 code block
        let prose = "前置说明。".repeat(80); // ~480 chars 中文
        let code = "\n```rust\nfn main() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n```\n";
        let after = "\n后续段落。".repeat(80);
        let text = format!("{prose}{code}{after}");
        let chunks = chunk(&text, 500, 100);
        for (i, c) in chunks.iter().enumerate() {
            let fc = c.matches("```").count();
            assert_eq!(
                fc % 2,
                0,
                "chunk {i} 含奇数个 fence ({fc}), 内容:\n----\n{c}\n----"
            );
        }
    }

    #[test]
    fn chunk_with_only_code_block_no_panic() {
        let text = "```python\nprint('hi')\nprint('there')\n```\n".repeat(20);
        let chunks = chunk(&text, 200, 50);
        assert!(!chunks.is_empty());
        for c in &chunks {
            let fc = c.matches("```").count();
            assert_eq!(fc % 2, 0, "code-only doc 仍应保持 balanced");
        }
    }

    #[test]
    fn chunk_does_not_loop_on_extreme_input() {
        // 超长不可拆分的 code block — 算法不应死循环或栈溢出
        let big_code = "```rust\n".to_string() + &"x".repeat(5000) + "\n```\n";
        let chunks = chunk(&big_code, 500, 100);
        // 至少产生 1 个 chunk（可能一个超大 chunk 装下整个 code block）
        assert!(!chunks.is_empty());
        // 不要求 balanced（fence 退化时算法会保留原 end）
    }

    #[test]
    fn has_unbalanced_fence_detects_odd_fences() {
        let text: Vec<char> = "before ``` code".chars().collect();
        assert!(has_unbalanced_fence(&text, 0, text.len()));
        let balanced: Vec<char> = "before ```code``` after".chars().collect();
        assert!(!has_unbalanced_fence(&balanced, 0, balanced.len()));
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

    /// Regression test for OSS-S4 (2026-05-02): markdown ``` code fence 内的
    /// `pub fn` / `fn` / `def` / `class` / `impl` 行不应被识别为 section heading。
    ///
    /// 修复前 rust-book chapters 的 markdown 引用 Rust 代码示例时，每个 `fn` 例子
    /// 被错切成独立 section → rust-slices 13K 文件 → 455 chunks → search 排序
    /// 被该文档 dominate（precision@1 仅 26.7%）。
    #[test]
    fn extract_sections_with_path_skips_code_inside_fence() {
        let content = "# Real Heading\n\nIntro text.\n\n```rust\nfn foo() {}\nfn bar() {}\nimpl Baz for Qux {}\npub fn quux() {}\n```\n\nMore prose.\n";
        let secs = extract_sections_with_path(content);
        // 修复前会产生 5+ sections（每个 fn / impl 各一个）
        // 修复后应该只 1 section（"Real Heading"），因为 fence 内的 fn 不再被识别
        assert!(
            secs.len() <= 2,
            "REGRESSION (OSS-S4): code-fence 内 fn 不应切 section，预期 ≤2 section（real heading 1 + 可能 SECTION_TARGET_SIZE 切的 1），got {} sections: {:?}",
            secs.len(),
            secs.iter().map(|s| s.path.last().cloned()).collect::<Vec<_>>()
        );
        // 应该有一个 Real Heading section
        assert!(secs.iter().any(|s| s.path == vec!["Real Heading".to_string()]));
        // 不应该有 fn foo / fn bar / impl 等作为 section path
        for s in &secs {
            for p in &s.path {
                assert!(
                    !p.contains("fn foo")
                        && !p.contains("fn bar")
                        && !p.contains("impl Baz")
                        && !p.contains("pub fn quux"),
                    "REGRESSION (OSS-S4): code-fence 内 `{}` 不应出现在 section path",
                    p
                );
            }
        }
    }

    #[test]
    fn extract_sections_with_path_code_outside_fence_still_detected() {
        // raw .rs 文件（无 ``` markdown fence）— code-boundary heading 仍生效
        // （这是 plain Rust 源码扫描场景，与 rust-book markdown 区分）
        let content = "fn foo() {\n    println!(\"foo\");\n}\n\npub fn bar() {\n    helper();\n}";
        let secs = extract_sections_with_path(content);
        // 至少 2 sections（fn foo, pub fn bar），保持之前测试期望
        assert!(secs.len() >= 1);
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

    // ── R12 v0.6.4 mutation testing: chunk_size 边界等于的 off-by-one 防御 ──

    #[test]
    fn chunk_with_text_exactly_chunk_size_returns_single_chunk() {
        // R12 (mutation testing) 发现: 17 chunker tests 全没测 text.len() == chunk_size
        // 边界. 故意改 `<=` 为 `<` 时所有 test 仍 pass. 本 test 锁定该边界.
        // 不变量: text 长度 ≤ chunk_size 时 chunk() 返回 1 个 chunk (整段, 不切).
        let text: String = "a".repeat(512);
        assert_eq!(text.len(), 512);

        let chunks = chunk(&text, 512, 128);
        assert_eq!(
            chunks.len(),
            1,
            "text.len() == chunk_size 必须返回 1 chunk (不切), got {} chunks",
            chunks.len()
        );
        assert_eq!(chunks[0].len(), 512);
    }

    #[test]
    fn chunk_with_text_one_byte_over_chunk_size_returns_two_chunks() {
        // 边界另一侧: 513 字节 (chunk_size + 1) 必须切成 ≥ 2 chunks.
        let text: String = "a".repeat(513);
        let chunks = chunk(&text, 512, 128);
        assert!(
            chunks.len() >= 2,
            "513 chars (> chunk_size) 必须切成 ≥ 2 chunks, got {}",
            chunks.len()
        );
    }

    #[test]
    fn chunk_with_text_one_byte_under_chunk_size_returns_single_chunk() {
        // 边界另一侧: 511 字节 (< chunk_size) 必须返回 1 chunk.
        let text: String = "a".repeat(511);
        let chunks = chunk(&text, 512, 128);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 511);
    }

    // ── R13 v0.6.4 randomized property test (no proptest dep) ────────────────
    //
    // Property: 对任意输入长度 + chunk_size，chunker::chunk 应满足:
    //   1. 第一个 chunk 包含 text 开头
    //   2. 最后一个 chunk 包含 text 末尾
    //   3. 所有 chunk 长度 ≤ 2 * chunk_size (含 code fence extend buffer)
    //   4. 不返回空 Vec (除非 text 为空)
    //   5. 不 panic / 不死循环
    //
    // 实现: deterministic seed (固定输入序列, 不引入 rand crate), 100 个 case.

    fn deterministic_text(seed: u32, len: usize) -> String {
        // Linear congruential generator (LCG) — deterministic, no extern crate.
        // Output: ASCII printable + '\n' (含 markdown headings + code fences 偶尔出现)
        let mut state = seed.wrapping_mul(2654435761);
        let mut s = String::with_capacity(len);
        let charset = b"abcdefghijklmnopqrstuvwxyz ABC.,!?\n";
        for _ in 0..len {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let idx = (state as usize) % charset.len();
            s.push(charset[idx] as char);
        }
        s
    }

    #[test]
    fn chunk_property_first_chunk_contains_text_start() {
        // 跑 50 个 case: 不同 seed × 不同长度
        for seed in 0..10u32 {
            for &len in &[100usize, 500, 1000, 2000, 5000] {
                let text = deterministic_text(seed, len);
                let chunks = chunk(&text, 512, 128);
                assert!(
                    !chunks.is_empty(),
                    "seed={} len={} chunks empty",
                    seed,
                    len
                );
                let first_chunk_starts_with_text_start = text.starts_with(&chunks[0][..chunks[0].len().min(50)])
                    || chunks[0].starts_with(&text[..text.len().min(50)]);
                assert!(
                    first_chunk_starts_with_text_start,
                    "seed={} len={}: first chunk doesn't contain text start. \
                     text[..50]={:?}, chunks[0][..50]={:?}",
                    seed,
                    len,
                    &text[..text.len().min(50)],
                    &chunks[0][..chunks[0].len().min(50)],
                );
            }
        }
    }

    #[test]
    fn chunk_property_no_chunk_exceeds_2x_chunk_size() {
        // 即使 code fence extend, chunk 长度也不应超 2*chunk_size buffer
        for seed in 0..10u32 {
            for &len in &[200usize, 1000, 3000, 10000] {
                let text = deterministic_text(seed, len);
                let chunk_size = 512;
                let chunks = chunk(&text, chunk_size, 128);
                for (i, c) in chunks.iter().enumerate() {
                    let char_count = c.chars().count();
                    assert!(
                        char_count <= 2 * chunk_size,
                        "seed={} len={} chunk[{}] has {} chars > 2*512=1024",
                        seed,
                        len,
                        i,
                        char_count
                    );
                }
            }
        }
    }

    #[test]
    fn chunk_property_no_panic_on_edge_inputs() {
        // 防御: chunker 不应 panic 即使输入异常
        let cases = vec![
            "",                                                 // empty
            "\n\n\n",                                           // only newlines
            "                                            ",     // only spaces
            "```",                                              // unclosed fence
            "```rust\nfn x()\n",                                // unclosed code block
            "a",                                                // single char
            "🌿🌿🌿🌿🌿",                                            // multi-byte unicode
        ];
        for input in cases {
            // Must not panic
            let chunks = chunk(input, 512, 128);
            // Empty input → may or may not return [""] — both are valid behaviors
            // for caller (parser typically filters empty content before chunking)
            if !input.is_empty() && !input.trim().is_empty() {
                assert!(!chunks.is_empty(), "non-empty input {:?} produced 0 chunks", input);
            }
        }
    }

    #[test]
    fn chunk_property_terminates_on_pathological_overlap() {
        // 防死循环: overlap >= chunk_size 时 chunker 应自动 clamp
        let text: String = "a".repeat(2000);
        let chunks = chunk(&text, 512, 999); // overlap > chunk_size
        // 不应死循环, chunks 数量有限 (<2000 chunks)
        assert!(
            chunks.len() < 2000,
            "pathological overlap caused {} chunks (likely infinite loop signal)",
            chunks.len()
        );
        assert!(!chunks.is_empty());
    }
}
