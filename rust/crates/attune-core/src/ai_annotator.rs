// AI 批注生成器 —— Batch A.2
//
// ## 成本/触发契约
//
// 这是 💰 层代码（要 LLM）。只能被用户显式触发：
//   - POST /api/v1/annotations/ai  — 阅读视图里点"🤖 AI 分析 · 角度"按钮
//
// **绝不**被以下路径调用：
//   - upload / ingest / 文件夹监听（建库）
//   - classify worker（基础分类，它走更便宜的 prompt 不产生批注）
//   - skill evolver（后台定期）
//
// ## 输出策略
//
// 让 LLM 返回"verbatim snippets + reason"的 JSON，backend 再在原文里字符串搜索
// 定位出 offset。不让 LLM 直接输出 offset —— 经验上 token 级偏移不可靠，
// 即便提示词要求也经常算错。搜不到的 snippet 静默丢弃（不注入错位批注）。

use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;
use crate::plugin_loader::{AnnotationAngleConfig, LoadedPlugin};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// ── 内置 AI 批注角度插件（编译时嵌入）─────────────────────────────────────
//
// 4 个角度的 YAML + prompt.md 分别嵌入二进制；启动后按需解析成 AnnotationAngleConfig。
// 用户 / PluginHub 可通过文件系统覆盖同名 id 的插件实现自定义。

const RISK_YAML: &str = include_str!("../assets/plugins/ai_annotation_risk/plugin.yaml");
const RISK_PROMPT: &str = include_str!("../assets/plugins/ai_annotation_risk/prompt.md");
const OUTDATED_YAML: &str = include_str!("../assets/plugins/ai_annotation_outdated/plugin.yaml");
const OUTDATED_PROMPT: &str = include_str!("../assets/plugins/ai_annotation_outdated/prompt.md");
const HIGHLIGHTS_YAML: &str = include_str!("../assets/plugins/ai_annotation_highlights/plugin.yaml");
const HIGHLIGHTS_PROMPT: &str = include_str!("../assets/plugins/ai_annotation_highlights/prompt.md");
const QUESTIONS_YAML: &str = include_str!("../assets/plugins/ai_annotation_questions/plugin.yaml");
const QUESTIONS_PROMPT: &str = include_str!("../assets/plugins/ai_annotation_questions/prompt.md");

/// 可选的分析角度。每个角度对应一个内置插件（YAML + prompt.md），编译期嵌入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiAngle {
    Risk,
    Outdated,
    Highlights,
    Questions,
}

impl AiAngle {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "risk" => Some(Self::Risk),
            "outdated" => Some(Self::Outdated),
            "highlights" => Some(Self::Highlights),
            "questions" => Some(Self::Questions),
            _ => None,
        }
    }

    /// 加载对应的内置插件配置（惰性解析，首次调用时从内嵌 YAML 构造）。
    pub fn config(&self) -> &'static AnnotationAngleConfig {
        match self {
            Self::Risk => {
                static CFG: OnceLock<AnnotationAngleConfig> = OnceLock::new();
                CFG.get_or_init(|| load_builtin(RISK_YAML, RISK_PROMPT, "risk"))
            }
            Self::Outdated => {
                static CFG: OnceLock<AnnotationAngleConfig> = OnceLock::new();
                CFG.get_or_init(|| load_builtin(OUTDATED_YAML, OUTDATED_PROMPT, "outdated"))
            }
            Self::Highlights => {
                static CFG: OnceLock<AnnotationAngleConfig> = OnceLock::new();
                CFG.get_or_init(|| load_builtin(HIGHLIGHTS_YAML, HIGHLIGHTS_PROMPT, "highlights"))
            }
            Self::Questions => {
                static CFG: OnceLock<AnnotationAngleConfig> = OnceLock::new();
                CFG.get_or_init(|| load_builtin(QUESTIONS_YAML, QUESTIONS_PROMPT, "questions"))
            }
        }
    }

    /// 标签前缀（UI 直接显示）—— 从插件读
    pub fn label_prefix(&self) -> &'static str {
        &self.config().label_prefix
    }

    /// 默认色 —— 从插件读
    pub fn default_color(&self) -> &'static str {
        &self.config().default_color
    }
}

/// 内置插件加载 —— YAML 解析失败 panic（编译时嵌入，失败说明构建有问题）。
fn load_builtin(yaml: &str, prompt: &str, angle_tag: &str) -> AnnotationAngleConfig {
    let loaded = LoadedPlugin::from_strings(yaml, prompt)
        .unwrap_or_else(|e| panic!("builtin ai_annotation_{angle_tag} plugin yaml broken: {e}"));
    AnnotationAngleConfig::from_loaded(&loaded)
        .unwrap_or_else(|e| panic!("builtin ai_annotation_{angle_tag} config broken: {e}"))
}

/// LLM 原始返回里的一个 finding。
/// 接受 `snippet`/`snpshot`（LLM 常见拼写错误）两种字段名。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFinding {
    /// 原文精确片段（verbatim）
    #[serde(alias = "snpshot", alias = "text", alias = "quote")]
    pub snippet: String,
    /// 解读/建议（20-80 字）
    #[serde(alias = "comment", alias = "note", default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawResponse {
    #[serde(default, alias = "items", alias = "results")]
    findings: Vec<RawFinding>,
}

/// 后端处理后的 finding，带字符偏移
#[derive(Debug, Clone)]
pub struct LocatedFinding {
    pub offset_start: i64,
    pub offset_end: i64,
    pub snippet: String,
    pub reason: String,
}

/// 大正文截断上限（字符数，插件级常量；各角度插件可选择不同上限未来扩展）
const MAX_CONTENT_LEN_FOR_LLM: usize = 8000;

/// 调 LLM 分析指定内容，返回定位好的 findings 列表。
///
/// `content_scope`：传给 LLM 的正文（可能是整篇，也可能是用户选中的一段）。
/// `content_full`：搜索 snippet 时用的完整原文（offset 总是相对此）。
/// 当 scope == full 时两者相同。
///
/// 限额（max_findings / max_snippet_chars / min_snippet_chars）从 plugin 配置读。
pub fn generate_annotations(
    llm: &dyn LlmProvider,
    content_scope: &str,
    content_full: &str,
    scope_offset_base: i64,  // content_scope 在 content_full 里的起始偏移（选区模式非零）
    angle: AiAngle,
) -> Result<Vec<LocatedFinding>> {
    if !llm.is_available() {
        return Err(VaultError::InvalidInput("LLM not available".into()));
    }
    let cfg = angle.config();

    // 截断超长正文。真实场景用户若要分析长文，应分段分析（UI 可引导）。
    let truncated = if content_scope.chars().count() > MAX_CONTENT_LEN_FOR_LLM {
        content_scope.chars().take(MAX_CONTENT_LEN_FOR_LLM).collect::<String>()
    } else {
        content_scope.to_string()
    };

    // System prompt 直接用插件 prompt（不再动态拼接说明，插件 prompt.md 是完整的）
    let system = &cfg.prompt;
    let user = format!("笔记内容:\n{truncated}");
    let raw = llm.chat(system, &user)?;
    let parsed = parse_response(&raw)?;

    let mut located = Vec::new();
    let mut dropped = 0usize;
    for f in parsed.findings.into_iter().take(cfg.max_findings) {
        let snip = f.snippet.trim();
        if snip.chars().count() < cfg.min_snippet_chars || snip.chars().count() > cfg.max_snippet_chars {
            log::debug!("ai_annotator: skip snippet out of length range: {snip:?}");
            dropped += 1;
            continue;
        }
        // 两阶段匹配：
        //   1. verbatim —— 完全相等（首选）
        //   2. relaxed  —— 规范化空白/引号后匹配（容忍 LLM 轻度改写）
        if let Some((u16_start, u16_end)) = locate_snippet(content_full, snip) {
            let _ = scope_offset_base;  // 忽略 —— 已在 content_full 全局搜索
            located.push(LocatedFinding {
                offset_start: u16_start as i64,
                offset_end: u16_end as i64,
                snippet: snip.to_string(),
                reason: f.reason.trim().to_string(),
            });
        } else {
            log::warn!("ai_annotator: snippet not found even after relaxed match, skip: {snip:?}");
            dropped += 1;
        }
    }
    if dropped > 0 {
        log::info!("ai_annotator: dropped {dropped} findings (snippet length or not-found)");
    }
    Ok(located)
}

/// 三阶段 snippet 定位：verbatim → relaxed → prefix anchor。
/// 返回 UTF-16 code unit 区间。
///
/// - Phase 1 verbatim: content.find(snip)
/// - Phase 2 relaxed: 归一化空白/全角半角标点后搜索
/// - Phase 3 prefix anchor: 对长 snippet (>= 20 chars)，仅用前 10 个字符作 anchor，
///   在原文找到 anchor 位置后，取后续 min(snip.len, 原文剩余) 作高亮。
///   牺牲终点精度换召回率 —— 避免 LLM 改写 snippet 尾部导致整条丢失。
fn locate_snippet(content: &str, snip: &str) -> Option<(usize, usize)> {
    // Phase 1
    if let Some(byte_idx) = content.find(snip) {
        return Some(byte_range_to_utf16(content, byte_idx, byte_idx + snip.len()));
    }
    // Phase 2
    let norm_content = normalize_for_match(content);
    let norm_snip = normalize_for_match(snip);
    if norm_snip.len() >= 4 {
        if let Some(rel_byte) = norm_content.find(&norm_snip) {
            if let Some((s, e)) = map_normalized_byte_to_original(
                content, rel_byte, rel_byte + norm_snip.len(),
            ) {
                return Some(byte_range_to_utf16(content, s, e));
            }
        }
    }
    // Phase 3: prefix anchor —— 仅对较长 snippet 生效（短 snippet 前缀碰撞风险太高）
    //
    // 终点策略：从 anchor 起，按 snippet 原字符数向后计数，但遇到**段落边界**（双换行）
    // 立即截断 —— LLM 极少一次引用跨段，这个 cap 防止"AI 改写尾部"导致高亮误扩散到
    // 下一段甚至下一节。容忍单 `\n`（同段内换行），只在 `\n\n` 或本段末尾停。
    let snip_chars: Vec<char> = snip.chars().collect();
    if snip_chars.len() < 20 { return None; }
    let anchor_len = 10usize;
    let anchor: String = snip_chars[..anchor_len].iter().collect();
    let anchor_norm = normalize_for_match(&anchor);
    if anchor_norm.chars().count() < 6 { return None; }
    let rel = norm_content.find(&anchor_norm)?;
    let (orig_s, _) = map_normalized_byte_to_original(content, rel, rel + anchor_norm.len())?;

    let max_chars = snip_chars.len();
    let mut end_byte = orig_s;
    let mut ch_count = 0usize;
    let mut prev_newline = false;
    for ch in content[orig_s..].chars() {
        if ch_count >= max_chars { break; }
        // 段落边界：连续两个 \n 立即停（不含末尾 \n，避免截过短）
        if ch == '\n' && prev_newline && ch_count >= anchor_len {
            break;
        }
        prev_newline = ch == '\n';
        end_byte += ch.len_utf8();
        ch_count += 1;
    }
    log::info!("ai_annotator: prefix-anchor match for snippet starting {anchor:?}");
    Some(byte_range_to_utf16(content, orig_s, end_byte))
}

/// 归一化规则：
///   · 所有空白折叠成单个 ASCII 空格
///   · 全角标点转半角（"（（""。？）） 等常见几种）
///   · 去掉首尾空白
fn normalize_for_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        let mapped = match ch {
            '（' => '(', '）' => ')',
            '【' => '[', '】' => ']',
            '，' => ',', '。' => '.', '；' => ';', '：' => ':',
            '！' => '!', '？' => '?',
            '\u{201C}' | '\u{201D}' | '「' | '」' => '"',
            '\u{2018}' | '\u{2019}' => '\'',
            c if c.is_whitespace() => ' ',
            c => c,
        };
        if mapped == ' ' {
            if !prev_space && !out.is_empty() { out.push(' '); }
            prev_space = true;
        } else {
            out.push(mapped);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// 把归一化字符串里的字节区间映射回原始字符串的字节区间。
/// 实现：遍历原始 chars，同步累积归一化后的字节位置，找到首次 >= norm_start 和 > norm_end 的点。
fn map_normalized_byte_to_original(
    original: &str, norm_start: usize, norm_end: usize,
) -> Option<(usize, usize)> {
    let mut orig_byte = 0usize;
    let mut norm_byte = 0usize;
    let mut orig_start = None;
    let mut orig_end = None;
    let mut prev_space_in_norm = false;
    // 循环遍历：模拟 normalize_for_match 但同时跟踪原始 byte 位置
    for ch in original.chars() {
        let ch_len = ch.len_utf8();
        let mapped = match ch {
            '（' => '(', '）' => ')',
            '【' => '[', '】' => ']',
            '，' => ',', '。' => '.', '；' => ';', '：' => ':',
            '！' => '!', '？' => '?',
            '\u{201C}' | '\u{201D}' | '「' | '」' => '"',
            '\u{2018}' | '\u{2019}' => '\'',
            c if c.is_whitespace() => ' ',
            c => c,
        };
        let produces_char = !(mapped == ' ' && (prev_space_in_norm || norm_byte == 0));
        let mapped_len = if produces_char { mapped.len_utf8() } else { 0 };

        if orig_start.is_none() && norm_byte >= norm_start && produces_char {
            orig_start = Some(orig_byte);
        }
        norm_byte += mapped_len;
        orig_byte += ch_len;
        if mapped == ' ' && produces_char { prev_space_in_norm = true; }
        else if produces_char { prev_space_in_norm = false; }

        if norm_byte >= norm_end && orig_end.is_none() {
            orig_end = Some(orig_byte);
            break;
        }
    }
    match (orig_start, orig_end) {
        (Some(s), Some(e)) if e > s => Some((s, e)),
        _ => None,
    }
}

// build_system_prompt 已删除：prompt 从 plugin.yaml 关联的 prompt.md 读取，
// 见 `AiAngle::config()` 和 `generate_annotations` 直接使用 `cfg.prompt`。

/// 解析 LLM 响应。LLM 输出常见问题：
///   1. 包了前后说明文字 — 提取第一个 '{' 到最后 '}' 区间
///   2. JSON 被截断（Ollama 默认 max_tokens）— 从中抽出能成功解析的单条 findings
///   3. 字段名拼错（snpshot）— serde alias 处理
///   4. snippet 里有未转义的换行 — 逐条 findings object 单独尝试解析
fn parse_response(raw: &str) -> Result<RawResponse> {
    // 找 JSON 片段起点。即便没有闭合 `}` 也尝试（salvage 路径按 `{...}` 对扫描）。
    let start = match raw.find('{') {
        Some(s) => s,
        None => return Ok(RawResponse { findings: vec![] }),
    };
    let end = raw.rfind('}').unwrap_or(raw.len() - 1);
    let json_text = &raw[start..=end.max(start)];
    // 首选：整体解析
    if let Ok(p) = serde_json::from_str::<RawResponse>(json_text) {
        return Ok(p);
    }
    // 兜底：扫描所有匹配的 `{ ... }` 对象（含嵌套），逐个当成 RawFinding 解析。
    // 用栈跟踪未闭合的 `{`；遇到 `}` 弹栈，取出 [start, i] 区间尝试解析。
    // 注意跳过字符串内的大括号（简单状态机：双引号内不计数）。
    let mut salvaged = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_str = false;
    let mut escape = false;
    for (i, ch) in json_text.char_indices() {
        if escape { escape = false; continue; }
        if ch == '\\' && in_str { escape = true; continue; }
        if ch == '"' { in_str = !in_str; continue; }
        if in_str { continue; }
        if ch == '{' { stack.push(i); }
        else if ch == '}' {
            if let Some(s) = stack.pop() {
                let end = i + ch.len_utf8();
                let candidate = &json_text[s..end];
                // 只尝试解析含 snippet 字段的对象，避免把 `{"findings": [...]}` 当失败
                if candidate.contains("snippet") || candidate.contains("snpshot")
                   || candidate.contains("\"text\"") || candidate.contains("\"quote\"")
                {
                    if let Ok(f) = serde_json::from_str::<RawFinding>(candidate) {
                        if !f.snippet.trim().is_empty() {
                            salvaged.push(f);
                        }
                    }
                }
            }
        }
    }
    if !salvaged.is_empty() {
        log::info!("ai_annotator: salvaged {} findings from malformed JSON", salvaged.len());
        return Ok(RawResponse { findings: salvaged });
    }
    log::warn!("ai_annotator: failed to parse LLM JSON response; raw={raw:?}");
    Ok(RawResponse { findings: vec![] })
}

/// 把字节索引区间转成 UTF-16 code unit 索引（前端 String.length 用的语义）。
fn byte_range_to_utf16(s: &str, byte_start: usize, byte_end: usize) -> (usize, usize) {
    let mut utf16_before_start: usize = 0;
    let mut utf16_in_range: usize = 0;
    let mut i = 0usize;
    for ch in s.chars() {
        let byte_len = ch.len_utf8();
        if i + byte_len <= byte_start {
            utf16_before_start += ch.len_utf16();
        } else if i < byte_end {
            utf16_in_range += ch.len_utf16();
        } else {
            break;
        }
        i += byte_len;
    }
    (utf16_before_start, utf16_before_start + utf16_in_range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmProvider;

    #[test]
    fn parse_angle() {
        assert_eq!(AiAngle::parse("risk"), Some(AiAngle::Risk));
        assert_eq!(AiAngle::parse("outdated"), Some(AiAngle::Outdated));
        assert_eq!(AiAngle::parse("highlights"), Some(AiAngle::Highlights));
        assert_eq!(AiAngle::parse("questions"), Some(AiAngle::Questions));
        assert_eq!(AiAngle::parse("unknown"), None);
    }

    #[test]
    fn each_angle_has_unique_label_and_color() {
        let angles = [AiAngle::Risk, AiAngle::Outdated, AiAngle::Highlights, AiAngle::Questions];
        let labels: std::collections::HashSet<_> = angles.iter().map(|a| a.label_prefix()).collect();
        assert_eq!(labels.len(), 4, "labels must be unique per angle");
    }

    #[test]
    fn utf16_conversion_ascii_only() {
        let s = "hello world";
        // "world" starts at byte 6 (也是 utf-16 idx 6)
        let (a, b) = byte_range_to_utf16(s, 6, 11);
        assert_eq!(a, 6);
        assert_eq!(b, 11);
    }

    #[test]
    fn utf16_conversion_cjk() {
        // "数据库" = 3 chars, each 3 bytes UTF-8, each 1 UTF-16 code unit.
        // content: "hello 数据库 world"
        let s = "hello 数据库 world";
        // "数据库" byte range: from after "hello " (byte 6) to byte 6+9=15
        let (a, b) = byte_range_to_utf16(s, 6, 15);
        // UTF-16: "hello " = 6 units, "数据库" = 3 units
        assert_eq!(a, 6);
        assert_eq!(b, 9);
    }

    #[test]
    fn utf16_conversion_emoji() {
        // emoji "⭐" 是 BMP 字符（1 utf-16 unit），"🔥" 是 SMP（2 utf-16 units，surrogate pair）
        let s = "A⭐🔥B";
        // byte 偏移：A=0, ⭐=1..4 (3 bytes), 🔥=4..8 (4 bytes), B=8
        // utf-16 索引：A=0..1, ⭐=1..2, 🔥=2..4, B=4..5
        let (a, b) = byte_range_to_utf16(s, 4, 8);  // 就 🔥
        assert_eq!(a, 2);
        assert_eq!(b, 4);  // 2 code units for 🔥
    }

    #[test]
    fn parse_empty_on_non_json() {
        let r = parse_response("这不是 JSON，我想").unwrap();
        assert!(r.findings.is_empty());
    }

    #[test]
    fn parse_extracts_json_from_wrapper_text() {
        let r = parse_response(r#"好的，以下是分析：{"findings":[{"snippet":"数据库","reason":"核心概念"}]}"#).unwrap();
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].snippet, "数据库");
    }

    fn mock_with(response: &str) -> MockLlmProvider {
        let m = MockLlmProvider::new("mock-model");
        m.push_response(response);
        m
    }

    #[test]
    fn generate_returns_empty_when_snippet_not_found() {
        let content = "数据库管理系统 (DBMS) 完成数据的创建";
        let mock = mock_with(r#"{"findings":[{"snippet":"完全不在原文里的字符串 XYZ","reason":"test"}]}"#);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 0, "snippet not in content must be dropped");
    }

    #[test]
    fn generate_locates_snippet_correctly() {
        let content = "数据库管理系统 (DBMS) 完成数据的创建";
        let mock = mock_with(r#"{"findings":[{"snippet":"数据库管理系统","reason":"核心概念"}]}"#);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].snippet, "数据库管理系统");
        assert_eq!(result[0].offset_start, 0);
        assert_eq!(result[0].offset_end, 7);  // 7 utf-16 code units
        assert_eq!(result[0].reason, "核心概念");
    }

    #[test]
    fn generate_caps_findings_to_max() {
        // 插件 max_findings=5，超出的被截断
        let content = "abcd 1234 wxyz foo1 foo2 foo3 foo4 foo5 foo6 foo7 foo8";
        let big = r#"{"findings":[
            {"snippet":"abcd","reason":"r"},
            {"snippet":"1234","reason":"r"},
            {"snippet":"wxyz","reason":"r"},
            {"snippet":"foo1","reason":"r"},
            {"snippet":"foo2","reason":"r"},
            {"snippet":"foo3","reason":"r"},
            {"snippet":"foo4","reason":"r"},
            {"snippet":"foo5","reason":"r"}
        ]}"#;
        let mock = mock_with(big);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Risk).unwrap();
        let cfg_max = AiAngle::Risk.config().max_findings;
        assert_eq!(result.len(), cfg_max,
            "must cap at plugin.constraints.max_findings regardless of LLM output size");
    }

    #[test]
    fn snippet_too_short_rejected() {
        // 插件 min_snippet_chars=4，"xx" 被过滤
        let content = "xx 1234567890";
        let mock = mock_with(r#"{"findings":[{"snippet":"xx","reason":"r"}]}"#);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 0, "snippet below plugin.min_snippet_chars rejected");
    }

    #[test]
    fn snippet_too_long_rejected() {
        // 插件 max_snippet_chars=150，200 chars 被过滤
        let big_snippet = "x".repeat(200);
        let content = big_snippet.clone();
        let payload = format!(r#"{{"findings":[{{"snippet":"{}","reason":"r"}}]}}"#, big_snippet);
        let mock = mock_with(&payload);
        let result = generate_annotations(&mock, &content, &content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 0, "snippet above plugin.max_snippet_chars rejected");
    }

    #[test]
    fn each_angle_has_embedded_plugin_config() {
        // 内置 4 个 plugin.yaml 应全部解析成功 + label_prefix 非空
        for a in [AiAngle::Risk, AiAngle::Outdated, AiAngle::Highlights, AiAngle::Questions] {
            let c = a.config();
            assert!(!c.label_prefix.is_empty(), "angle {:?} label_prefix empty", a);
            assert!(!c.default_color.is_empty(), "angle {:?} default_color empty", a);
            assert!(!c.prompt.is_empty(), "angle {:?} prompt empty", a);
            assert!(c.max_findings > 0);
        }
    }

    #[test]
    fn generate_tolerates_bad_json_from_llm() {
        let content = "whatever content 1234";
        let mock = mock_with("lol, not json, sorry");
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Risk).unwrap();
        assert_eq!(result.len(), 0, "garbage LLM output → empty results, not Err");
    }

    #[test]
    fn relaxed_match_whitespace_normalization() {
        // 原文多空格，LLM 返回标准空格 — 两阶段匹配应成功定位
        let content = "数据库   管理\n  系统 (DBMS) 完成";
        let mock = mock_with(r#"{"findings":[{"snippet":"数据库 管理 系统 (DBMS) 完成","reason":"核心"}]}"#);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 1, "relaxed match must locate snippet despite whitespace differences");
    }

    #[test]
    fn relaxed_match_fullwidth_punctuation() {
        // 原文用全角括号，LLM 返回半角 — relaxed 应匹配
        let content = "数据库管理系统（DBMS）完成数据操作的创建";
        let mock = mock_with(r#"{"findings":[{"snippet":"数据库管理系统(DBMS)完成","reason":"核心"}]}"#);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn accepts_snpshot_field_typo() {
        let content = "数据库管理系统 (DBMS) 完成数据的创建";
        // LLM 常见拼写错误 snpshot，应通过 serde alias 接收
        let mock = mock_with(r#"{"findings":[{"snpshot":"数据库管理系统","reason":"核心"}]}"#);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Risk).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].snippet, "数据库管理系统");
    }

    #[test]
    fn salvages_partial_findings_from_truncated_json() {
        // Ollama max_tokens 经常截断，JSON 末尾可能少 `]` 或 `}`。尽量抢救前面成功的 finding。
        let content = "数据库管理系统 (DBMS) 完成数据的创建，以及数据库应用程序的使用";
        let truncated = r#"{"findings": [
            {"snippet": "数据库管理系统", "reason": "核心概念"},
            {"snippet": "数据库应用程序", "reason": "用户界面层"}
        "#;  // 缺 `]}`
        let mock = mock_with(truncated);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 2, "both findings should be salvaged from truncated JSON");
    }

    #[test]
    fn prefix_anchor_match_when_llm_paraphrases_tail() {
        // LLM 引用了原文前半段，但后半段改写 —— prefix anchor 应定位前 10 字符
        let content = "数据库管理系统（DBMS）主要是进行数据的创建、读取、更新、删除等数据操作，当然还要完成其他一些功能。";
        // LLM 返回的 snippet 前 10 字符在原文里，但尾部改写了
        let mock = mock_with(r#"{"findings":[{"snippet":"数据库管理系统（DBMS）主要负责数据 CRUD 操作，并完成其他核心任务","reason":"核心"}]}"#);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 1, "prefix-anchor should locate paraphrased snippet");
    }

    #[test]
    fn prefix_anchor_stops_at_paragraph_boundary() {
        // Anchor 在第 1 段（前 10 字符必须在原文可定位），snippet 原长 > 第 1 段长度
        // —— phase 3 必须在 `\n\n` 截断，避免高亮误扩散到下一段/下一小节。
        let content = "第一节\n\n数据库管理系统负责 CRUD 操作的核心模块。\n\n第二节\n\n完全不相关的后续内容很多很多很多很多很多很多。";
        // 前 10 字符 "数据库管理系统负责 C" 原文可匹配；整个 snippet 40 字符 > 第一段长度 22
        let snippet = "数据库管理系统负责 CRUD 操作的核心模块这个系统设计用于处理企业级数据服务并且完全兼容 SQL 标准";
        let payload = format!(r#"{{"findings":[{{"snippet":"{snippet}","reason":"核心"}}]}}"#);
        let mock = mock_with(&payload);
        let result = generate_annotations(&mock, content, content, 0, AiAngle::Highlights).unwrap();
        assert_eq!(result.len(), 1, "prefix-anchor should still locate the snippet");
        // 高亮文本不得包含"第二节"字样
        let hl = substring_by_utf16_chars(content, result[0].offset_start as usize, result[0].offset_end as usize);
        assert!(!hl.contains("第二节"),
            "highlight crossed paragraph boundary: {hl:?}");
    }

    // 测试辅助：按 UTF-16 code unit 索引切取字符串
    fn substring_by_utf16_chars(s: &str, start: usize, end: usize) -> String {
        let units: Vec<u16> = s.encode_utf16().collect();
        if end > units.len() { return String::new(); }
        String::from_utf16_lossy(&units[start..end])
    }
}
