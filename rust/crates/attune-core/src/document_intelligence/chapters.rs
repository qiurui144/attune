//! Chapter-by-chapter reading + cross-chapter memory (spec §3.4, T-05) with the §3.5
//! review/批阅 output-mode contract.
//!
//! Three actions (spec §5.3):
//!   - `list`           : zero-LLM chapter navigation + per-chapter extractive preview.
//!   - `summarize_chapter`: reuses the deep_summary single-chapter pipeline (member-gated).
//!   - `ask`            : RAG over this+related chapters → Reasoning-model Q&A; cross-chapter
//!                        memory = prior chapters' (cached) summaries injected into context.
//!
//! **Output-Mode Contract (spec §3.5)**: the default mode is `review/批阅` — each chapter
//! carries `annotations[]` anchored to **that chapter's char offsets** (like a margin note /
//! a teacher's mark), and an `ask` answer carries citation offsets anchoring the answer back
//! to the source text. `structured` mode returns the [`ChapterReadResult`] payload without the
//! review overlay. Cross-chapter memory is proven mechanically: an `ask` on chapter N (N>0)
//! injects chapter N-1's summary into the LLM prompt.

use crate::document_intelligence::compare::Annotation;
use crate::document_intelligence::model_routing::{ModelRole, ModelRouter};
use crate::document_intelligence::token_bill::TokenBill;
use crate::error::Result;
use crate::llm::{ChatMessage, LlmProvider};
use serde::{Deserialize, Serialize};

/// Output shaping per §3.5. Chapters default to `review`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Margin-note / 批阅: per-chapter annotations anchored to chapter offsets. DEFAULT.
    Review,
    /// Raw structured payload, no review overlay.
    Structured,
}

impl OutputMode {
    /// Per spec §3.5 the chapters default is `review`.
    pub fn default_for_chapters() -> Self {
        OutputMode::Review
    }
    fn as_str(self) -> &'static str {
        match self {
            OutputMode::Review => "review",
            OutputMode::Structured => "structured",
        }
    }
}

/// A chapter as seen by `list` (zero LLM).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChapterEntry {
    pub idx: usize,
    pub heading_path: String,
    /// Local extractive preview (zero LLM) — the免登录 free preview (spec §2.1).
    pub extractive_preview: String,
}

/// Result of a chapter `summarize` or `ask` (spec §5.3 + §3.5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChapterReadResult {
    pub output_mode: String,
    pub chapter_idx: usize,
    /// The chapter summary (summarize) or the answer (ask).
    pub result: String,
    /// Prior chapter indices whose summaries were injected (cross-chapter memory proof).
    pub cross_chapter_memory_used: Vec<usize>,
    /// review-mode overlay: annotations anchored to THIS chapter's char offsets (§3.5).
    pub annotations: Vec<Annotation>,
    /// For `ask`: citation offsets anchoring the answer back to the chapter text (§3.5).
    pub citations: Vec<Annotation>,
    pub token_bill: TokenBill,
}

const SUMMARIZE_SYSTEM_PROMPT: &str =
    "你是逐章阅读助手。基于本章正文（可能附带前序章节的摘要作为上下文），用简洁中文写出本章要点。直接输出要点。";

const ASK_SYSTEM_PROMPT: &str =
    "你是逐章问答助手。基于本章正文与前序章节的摘要回答用户问题。回答须忠于原文，可引用原文片段。直接输出答案。";

/// A chapter's text plus an optional already-cached summary (the route/store layer fills the
/// cache; unit tests pass it directly). Keeping the cache explicit makes cross-chapter memory
/// unit-testable without the full Store.
pub struct ChapterCtx {
    pub heading_path: String,
    pub content: String,
    /// Prior cached summary for this chapter (cross-chapter memory source), if any.
    pub cached_summary: Option<String>,
}

/// Split a document into chapters (zero LLM) for navigation.
pub fn split_chapters(full_text: &str) -> Vec<ChapterCtx> {
    let sections = crate::chunker::extract_sections_with_path(full_text);
    if sections.is_empty() && !full_text.trim().is_empty() {
        return vec![ChapterCtx {
            heading_path: String::new(),
            content: full_text.to_string(),
            cached_summary: None,
        }];
    }
    sections
        .into_iter()
        .map(|s| ChapterCtx {
            heading_path: s.path.join(" / "),
            content: s.content,
            cached_summary: None,
        })
        .collect()
}

/// `action=list` (zero LLM): chapter navigation + per-chapter extractive preview.
pub fn list(full_text: &str, preview_keep_ratio: f32) -> Vec<ChapterEntry> {
    split_chapters(full_text)
        .into_iter()
        .enumerate()
        .map(|(idx, ch)| {
            let heading_words: Vec<String> = ch
                .heading_path
                .split(['/', ' '])
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let preview = crate::document_intelligence::extractive::extract_candidates(
                &ch.content,
                preview_keep_ratio,
                &heading_words,
            );
            ChapterEntry {
                idx,
                heading_path: ch.heading_path,
                extractive_preview: preview,
            }
        })
        .collect()
}

/// `action=summarize` (member-gated): summarize chapter `idx`, injecting prior chapters'
/// summaries as cross-chapter memory. Review mode anchors a per-chapter annotation.
pub fn summarize_chapter(
    chapters: &[ChapterCtx],
    idx: usize,
    output_mode: OutputMode,
    reasoning: &dyn LlmProvider,
    router: &ModelRouter,
) -> Result<ChapterReadResult> {
    let reasoning_model = router.pick(ModelRole::Reasoning).to_string();
    let ch = chapters
        .get(idx)
        .ok_or_else(|| crate::error::VaultError::InvalidInput(format!("chapter idx {idx} out of range")))?;

    let (memory, used) = build_cross_chapter_memory(chapters, idx);
    let user = compose_chapter_payload(&memory, &ch.content);
    let msgs = [ChatMessage::system(SUMMARIZE_SYSTEM_PROMPT), ChatMessage::user(&user)];
    let (summary, usage) = reasoning.chat_with_history(&msgs)?;

    let mut bill = TokenBill::default();
    account(&mut bill, &usage, &user, &summary, &reasoning_model);

    let annotations = if output_mode == OutputMode::Review {
        review_annotations_for_chapter(&ch.content, &summary)
    } else {
        Vec::new()
    };

    Ok(ChapterReadResult {
        output_mode: output_mode.as_str().to_string(),
        chapter_idx: idx,
        result: summary,
        cross_chapter_memory_used: used,
        annotations,
        citations: Vec::new(),
        token_bill: bill,
    })
}

/// `action=ask` (member-gated): answer `question` about chapter `idx` using the chapter text +
/// prior chapters' summaries (cross-chapter memory). The answer carries citation offsets
/// anchoring quoted phrases back to the chapter's char offsets (§3.5).
pub fn ask(
    chapters: &[ChapterCtx],
    idx: usize,
    question: &str,
    output_mode: OutputMode,
    reasoning: &dyn LlmProvider,
    router: &ModelRouter,
) -> Result<ChapterReadResult> {
    if question.trim().is_empty() {
        return Err(crate::error::VaultError::InvalidInput("question is required for ask".into()));
    }
    let reasoning_model = router.pick(ModelRole::Reasoning).to_string();
    let ch = chapters
        .get(idx)
        .ok_or_else(|| crate::error::VaultError::InvalidInput(format!("chapter idx {idx} out of range")))?;

    let (memory, used) = build_cross_chapter_memory(chapters, idx);
    let user = format!(
        "{}\n\n【本章正文】\n{}\n\n【问题】{}",
        if memory.is_empty() { String::new() } else { format!("【前序章节记忆】\n{memory}") },
        ch.content,
        question
    );
    let msgs = [ChatMessage::system(ASK_SYSTEM_PROMPT), ChatMessage::user(&user)];
    let (answer, usage) = reasoning.chat_with_history(&msgs)?;

    let mut bill = TokenBill::default();
    account(&mut bill, &usage, &user, &answer, &reasoning_model);

    // Citations: any quoted phrase of the answer that appears verbatim in the chapter → anchor it.
    let citations = citation_offsets(&ch.content, &answer);
    let annotations = if output_mode == OutputMode::Review {
        review_annotations_for_chapter(&ch.content, &answer)
    } else {
        Vec::new()
    };

    Ok(ChapterReadResult {
        output_mode: output_mode.as_str().to_string(),
        chapter_idx: idx,
        result: answer,
        cross_chapter_memory_used: used,
        annotations,
        citations,
        token_bill: bill,
    })
}

/// Build the cross-chapter memory string from prior chapters' cached summaries.
/// Returns (memory_text, prior_indices_used). Only chapters with a `cached_summary` count.
fn build_cross_chapter_memory(chapters: &[ChapterCtx], idx: usize) -> (String, Vec<usize>) {
    let mut parts = Vec::new();
    let mut used = Vec::new();
    for (i, ch) in chapters.iter().enumerate().take(idx) {
        if let Some(sum) = &ch.cached_summary {
            let h = if ch.heading_path.is_empty() { format!("章{i}") } else { ch.heading_path.clone() };
            parts.push(format!("【{h}】{sum}"));
            used.push(i);
        }
    }
    (parts.join("\n"), used)
}

fn compose_chapter_payload(memory: &str, content: &str) -> String {
    if memory.is_empty() {
        format!("【本章正文】\n{content}")
    } else {
        format!("【前序章节记忆】\n{memory}\n\n【本章正文】\n{content}")
    }
}

/// Anchor a margin-note annotation to the chapter's lead span (the first sentence), which is
/// where a review note conventionally attaches. Char-offset into the CHAPTER text.
fn review_annotations_for_chapter(chapter: &str, note: &str) -> Vec<Annotation> {
    let chars: Vec<char> = chapter.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    // Lead span: up to the first terminator (。！？.!?\n) or 40 chars.
    let mut end = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        end = i + 1;
        if matches!(c, '。' | '！' | '？' | '.' | '!' | '?' | '\n') || end >= 40 {
            break;
        }
    }
    vec![Annotation {
        offset_start: 0,
        offset_end: end,
        kind: "note".into(),
        note: note.chars().take(120).collect(),
        severity: 1,
    }]
}

/// Extract citation offsets: phrases (≥4 chars) of `answer` enclosed in quotes "..." / 「」/
/// 『』 that appear verbatim in `chapter` → anchored to the chapter's char offsets.
fn citation_offsets(chapter: &str, answer: &str) -> Vec<Annotation> {
    let chapter_chars: Vec<char> = chapter.chars().collect();
    let mut out = Vec::new();
    for quoted in extract_quoted(answer) {
        if quoted.chars().count() < 4 {
            continue;
        }
        if let Some((s, e)) = find_char_span(&chapter_chars, &quoted) {
            out.push(Annotation {
                offset_start: s,
                offset_end: e,
                kind: "citation".into(),
                note: quoted,
                severity: 1,
            });
        }
    }
    out
}

/// Pull quoted substrings from text (ASCII "..", CJK 「」, 『』).
fn extract_quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let (open, close) = match chars[i] {
            '"' => ('"', '"'),
            '「' => ('「', '」'),
            '『' => ('『', '』'),
            _ => {
                i += 1;
                continue;
            }
        };
        let _ = open;
        let mut j = i + 1;
        let mut buf = String::new();
        while j < chars.len() && chars[j] != close {
            buf.push(chars[j]);
            j += 1;
        }
        if j < chars.len() && !buf.is_empty() {
            out.push(buf);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Char-span finder (shared shape with compare::find_char_span).
fn find_char_span(haystack_chars: &[char], needle: &str) -> Option<(usize, usize)> {
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.is_empty() || needle_chars.len() > haystack_chars.len() {
        return None;
    }
    let limit = haystack_chars.len() - needle_chars.len();
    for start in 0..=limit {
        if haystack_chars[start..start + needle_chars.len()] == needle_chars[..] {
            return Some((start, start + needle_chars.len()));
        }
    }
    None
}

fn account(
    bill: &mut TokenBill,
    usage: &crate::usage::TokenUsage,
    user: &str,
    resp: &str,
    model: &str,
) {
    use crate::context_compress::estimate_tokens;
    if usage.tokens_in == 0 {
        bill.reduce_llm_tokens.r#in = bill.reduce_llm_tokens.r#in.saturating_add(estimate_tokens(user) as u32);
        bill.reduce_llm_tokens.out = bill.reduce_llm_tokens.out.saturating_add(estimate_tokens(resp) as u32);
        if bill.reduce_llm_tokens.model.is_empty() {
            bill.reduce_llm_tokens.model = model.to_string();
        }
    } else {
        bill.reduce_llm_tokens.add(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::RecordingMockLlm;
    use serde_json::json;

    fn router() -> ModelRouter {
        ModelRouter::from_settings(&json!({
            "model_routing": { "cheap": "gpt-4o-mini", "reasoning": "gpt-4o", "vision": "gpt-4o-mini" }
        }))
    }

    fn three_chapter_doc() -> String {
        "# 第一章 引言\n\n这是引言的正文内容讲述背景。引言还有第二句。\n\n\
         # 第二章 方法\n\n方法章节描述了实验设计与步骤。方法还有细节句。\n\n\
         # 第三章 结果\n\n结果章节给出了主要发现与数据。结果有结论句。\n"
            .to_string()
    }

    #[test]
    fn test_list_zero_llm() {
        let chapters = list(&three_chapter_doc(), 0.5);
        assert_eq!(chapters.len(), 3, "three headings → three chapters");
        for c in &chapters {
            assert!(!c.extractive_preview.is_empty(), "every chapter has an extractive preview");
        }
        assert!(chapters[0].heading_path.contains("第一章"));
        // list takes no LlmProvider → structurally cannot call an LLM (zero-cost free preview).
    }

    #[test]
    fn test_summarize_chapter_review_annotations() {
        let mut chs = split_chapters(&three_chapter_doc());
        chs[0].cached_summary = Some("引言讲背景".into());
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("本章方法要点：实验设计");
        let r = summarize_chapter(&chs, 1, OutputMode::Review, &reasoning, &router()).unwrap();
        assert_eq!(r.chapter_idx, 1);
        assert_eq!(r.output_mode, "review");
        assert!(!r.annotations.is_empty(), "review mode anchors a chapter annotation");
        // annotation offset aligns to chapter 1's text.
        let ch_chars: Vec<char> = chs[1].content.chars().collect();
        let ann = &r.annotations[0];
        assert!(ann.offset_end <= ch_chars.len() && ann.offset_start < ann.offset_end);
        assert_eq!(reasoning.call_count(), 1);
        assert_eq!(r.token_bill.reduce_llm_tokens.model, "gpt-4o");
    }

    #[test]
    fn test_ask_injects_prior_chapter_memory() {
        // Cross-chapter memory: ask on chapter 2 must inject chapter 0 & 1's cached summaries.
        let mut chs = split_chapters(&three_chapter_doc());
        chs[0].cached_summary = Some("第一章记忆：引言背景XYZ".into());
        chs[1].cached_summary = Some("第二章记忆：方法步骤ABC".into());
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("基于前两章，结果是……");
        let r = ask(&chs, 2, "结果与方法的关系？", OutputMode::Review, &reasoning, &router()).unwrap();
        // The LLM prompt must contain the prior chapters' summaries (memory proven via capture).
        assert!(reasoning.any_call_contains("第一章记忆：引言背景XYZ"), "chapter 0 summary injected");
        assert!(reasoning.any_call_contains("第二章记忆：方法步骤ABC"), "chapter 1 summary injected");
        assert_eq!(r.cross_chapter_memory_used, vec![0, 1], "memory_used lists the injected prior chapters");
    }

    #[test]
    fn test_ask_no_prior_memory_when_chapter_zero() {
        let chs = split_chapters(&three_chapter_doc());
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("答案");
        let r = ask(&chs, 0, "引言讲了什么？", OutputMode::Structured, &reasoning, &router()).unwrap();
        assert!(r.cross_chapter_memory_used.is_empty(), "chapter 0 has no prior chapters");
        assert_eq!(r.output_mode, "structured");
        assert!(r.annotations.is_empty(), "structured mode has no review annotations");
    }

    #[test]
    fn test_ask_citation_offsets_align() {
        // The answer quotes a phrase verbatim from the chapter → citation anchored to chapter offset.
        let chs = split_chapters(&three_chapter_doc());
        // chapter 1 content contains "方法章节描述了实验设计与步骤"
        let reasoning = RecordingMockLlm::new("gpt-4o")
            .with_response("根据原文「方法章节描述了实验设计与步骤」可知方法清晰。");
        let r = ask(&chs, 1, "方法如何？", OutputMode::Review, &reasoning, &router()).unwrap();
        assert!(!r.citations.is_empty(), "answer quoting the chapter yields a citation");
        let ch_chars: Vec<char> = chs[1].content.chars().collect();
        for cit in &r.citations {
            let span: String = ch_chars[cit.offset_start..cit.offset_end].iter().collect();
            assert!(chs[1].content.contains(&span), "citation span exists verbatim in chapter");
            assert_eq!(cit.kind, "citation");
        }
    }

    #[test]
    fn test_invalid_chapter_idx_errors() {
        let chs = split_chapters(&three_chapter_doc());
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("x");
        assert!(summarize_chapter(&chs, 99, OutputMode::Review, &reasoning, &router()).is_err());
        assert!(ask(&chs, 99, "q", OutputMode::Review, &reasoning, &router()).is_err());
    }

    #[test]
    fn test_ask_empty_question_errors() {
        let chs = split_chapters(&three_chapter_doc());
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("x");
        assert!(ask(&chs, 0, "  ", OutputMode::Review, &reasoning, &router()).is_err());
        assert_eq!(reasoning.call_count(), 0, "empty question short-circuits before any LLM call");
    }

    #[test]
    fn test_serde_roundtrip() {
        let chs = split_chapters(&three_chapter_doc());
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("摘要");
        let r = summarize_chapter(&chs, 0, OutputMode::Review, &reasoning, &router()).unwrap();
        let js = serde_json::to_string(&r).unwrap();
        let back: ChapterReadResult = serde_json::from_str(&js).unwrap();
        assert_eq!(back, r);
    }
}
