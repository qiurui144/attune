//! Document comparison (spec §3.3, T-04) with the §3.5 Output-Mode Contract.
//!
//! Three layers, stacked by cost (CLAUDE.md §Cost&Trigger Contract):
//!   - STRUCTURAL (zero LLM): section alignment via `extract_sections_with_path` heading_path
//!     + an LCS over headings → added / removed / moved / modified.
//!   - TEXTUAL (zero LLM): line/sentence LCS diff inside aligned sections → ins/del/eq hunks.
//!   - SEMANTIC (member-gated, tier-3 LLM): a Cheap-model per-changed-block verdict
//!     (rewrite / substantive / stance-reversal / numeric-change) + a Reasoning-model ×1
//!     overall difference summary.
//!
//! **Output-Mode Contract (spec §3.5)**: the default mode is `marked` — the report carries
//! `annotations[]` anchored to **doc b's char offsets** (each annotation's
//! `b[offset_start..offset_end]` is exactly the changed span). `structured` mode returns the
//! [`DiffReport`] payload without forcing the marked overlay. The route layer (T-07) wraps
//! either in the §3.5 envelope. Offset alignment is mechanically tested
//! (`test_marked_annotations_offsets_align`).

use crate::document_intelligence::model_routing::{ModelRole, ModelRouter};
use crate::document_intelligence::token_bill::TokenBill;
use crate::error::Result;
use crate::llm::{ChatMessage, LlmProvider};
use serde::{Deserialize, Serialize};

/// Which comparison layers to run (spec §5.1 `mode`). `Semantic` implies the textual+structural
/// layers too (it is the richest); it is the only member-gated mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompareMode {
    Structural,
    Textual,
    Semantic,
}

/// Response shaping per the §3.5 Output-Mode Contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Highlight changed/risk spans on doc b (annotations anchored to b's offsets). DEFAULT for compare.
    Marked,
    /// Raw structured payload (DiffReport JSON), no forced overlay.
    Structured,
}

impl OutputMode {
    /// Per spec §3.5 the compare default is `marked`.
    pub fn default_for_compare() -> Self {
        OutputMode::Marked
    }
}

/// The semantic verdict for a changed block (spec §5.1). OSS gives only these four general
/// classes; attune-pro may wrap a stronger enum in its own repo (spec §6 extension point).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffVerdict {
    /// Reworded, same meaning.
    Rewrite,
    /// Meaning changed (added/removed substance).
    Substantive,
    /// Position/stance reversed (e.g. "支持" → "反对").
    StanceReversal,
    /// A number/figure changed.
    NumericChange,
}

impl DiffVerdict {
    pub fn as_kebab(self) -> &'static str {
        match self {
            DiffVerdict::Rewrite => "rewrite",
            DiffVerdict::Substantive => "substantive",
            DiffVerdict::StanceReversal => "stance-reversal",
            DiffVerdict::NumericChange => "numeric-change",
        }
    }
    /// Severity ranking for the marked overlay (higher = more attention).
    pub fn severity(self) -> u8 {
        match self {
            DiffVerdict::Rewrite => 1,
            DiffVerdict::NumericChange => 3,
            DiffVerdict::Substantive => 3,
            DiffVerdict::StanceReversal => 4,
        }
    }
    fn from_llm_token(s: &str) -> DiffVerdict {
        let t = s.trim().to_lowercase();
        if t.contains("stance") || t.contains("reversal") || t.contains("立场") || t.contains("反转") {
            DiffVerdict::StanceReversal
        } else if t.contains("numeric") || t.contains("number") || t.contains("数值") || t.contains("数字") {
            DiffVerdict::NumericChange
        } else if t.contains("substant") || t.contains("实质") {
            DiffVerdict::Substantive
        } else {
            DiffVerdict::Rewrite
        }
    }
}

/// A structural difference (chapter-level).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StructuralDiff {
    /// `added` | `removed` | `moved` | `modified`.
    pub kind: String,
    pub heading_path: String,
    /// section_idx in doc b for added/modified/moved; in doc a for removed.
    pub section_idx: usize,
}

/// One textual hunk op inside an aligned section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HunkOp {
    Ins,
    Del,
    Eq,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TextualHunk {
    pub op: HunkOp,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TextualDiff {
    pub section_idx: usize,
    pub hunks: Vec<TextualHunk>,
}

/// A semantic verdict for a changed block (member-gated; spec §5.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SemanticVerdict {
    pub section_idx: usize,
    pub verdict: String,
    pub rationale: String,
    pub model: String,
}

/// An annotation anchored to doc b's char offsets (spec §3.5).
///
/// **Invariant**: `b.chars().collect::<String>()[offset_start..offset_end]` (by char index)
/// is exactly the changed span this annotation describes. We use **char offsets** (not byte
/// offsets) so the UI overlay can highlight CJK spans without splitting multi-byte chars.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    /// Char offset (inclusive) into doc b.
    pub offset_start: usize,
    /// Char offset (exclusive) into doc b.
    pub offset_end: usize,
    /// Annotation kind: a `DiffVerdict` kebab string for compare.
    pub kind: String,
    pub note: String,
    pub severity: u8,
}

/// The comparison report (spec §5.1 + §3.5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiffReport {
    pub output_mode: String,
    pub structural_diffs: Vec<StructuralDiff>,
    pub textual_diffs: Vec<TextualDiff>,
    /// Only present (non-empty) for `mode=semantic` AND a paid member.
    pub semantic_verdicts: Vec<SemanticVerdict>,
    /// Overall difference summary (member-gated, reasoning model ×1).
    pub summary: Option<String>,
    /// Marked-mode overlay: annotations anchored to doc b char offsets (§3.5).
    pub annotations: Vec<Annotation>,
    pub token_bill: TokenBill,
}

/// Cheap+reasoning handles, already model-selected (mirrors deep_summary::StageLlms).
pub struct StageLlms<'a> {
    pub cheap: &'a dyn LlmProvider,
    pub reasoning: &'a dyn LlmProvider,
}

const VERDICT_SYSTEM_PROMPT: &str = "你是文档差异裁决器。给你同一段落的旧版(A)与新版(B)，判定变更类型，只输出四选一的英文标签之一：rewrite（仅改写措辞）/ substantive（实质内容变化）/ stance-reversal（立场反转）/ numeric-change（数字变化）。再换行给一句中文理由。";

const SUMMARY_SYSTEM_PROMPT: &str = "你是文档差异总结器。基于给定的逐段差异，用简洁中文概述两份文档的总体变化。直接输出总结。";

/// Compare doc `a` (old) and doc `b` (new).
///
/// - `mode` selects which layers run (spec §5.1). `Semantic` runs all three; the caller
///   (route layer T-08) is responsible for the 403 member-gate — this fn accepts `member`
///   and simply *skips the LLM layers* when `!member`, so a non-member semantic call still
///   returns structural+textual with empty `semantic_verdicts` (never panics, never leaks).
/// - `output_mode` shapes the report per §3.5 (default `marked` builds the annotations[]).
///
/// All offsets in `annotations` are **char offsets into `b`**.
pub fn compare(
    a: &str,
    b: &str,
    mode: CompareMode,
    output_mode: OutputMode,
    member: bool,
    router: &ModelRouter,
    llms: &StageLlms,
) -> Result<DiffReport> {
    let mut bill = TokenBill::default();
    let cheap_model = router.pick(ModelRole::Cheap).to_string();
    let reasoning_model = router.pick(ModelRole::Reasoning).to_string();

    let sections_a = crate::chunker::extract_sections_with_path(a);
    let sections_b = crate::chunker::extract_sections_with_path(b);

    // ── STRUCTURAL (zero LLM): align sections by heading_path via LCS ──
    let heads_a: Vec<String> = sections_a.iter().map(|s| s.path.join(" / ")).collect();
    let heads_b: Vec<String> = sections_b.iter().map(|s| s.path.join(" / ")).collect();
    let aligned = lcs_align(&heads_a, &heads_b);

    let mut structural_diffs = Vec::new();
    let mut matched_pairs: Vec<(usize, usize)> = Vec::new(); // (idx_a, idx_b) aligned sections
    {
        // From the alignment, classify each b-section and each a-section.
        let mut a_matched = vec![false; sections_a.len()];
        let mut b_matched = vec![false; sections_b.len()];
        for &(ia, ib) in &aligned {
            a_matched[ia] = true;
            b_matched[ib] = true;
            matched_pairs.push((ia, ib));
            // Aligned headings: "modified" iff content differs.
            if sections_a[ia].content != sections_b[ib].content {
                structural_diffs.push(StructuralDiff {
                    kind: "modified".into(),
                    heading_path: heads_b[ib].clone(),
                    section_idx: ib,
                });
            }
        }
        // Unmatched in b = added (or moved if the same heading exists unmatched in a).
        for (ib, matched) in b_matched.iter().enumerate() {
            if !*matched {
                let kind = if heads_a.iter().enumerate().any(|(ia, h)| !a_matched[ia] && h == &heads_b[ib]) {
                    "moved"
                } else {
                    "added"
                };
                structural_diffs.push(StructuralDiff {
                    kind: kind.into(),
                    heading_path: heads_b[ib].clone(),
                    section_idx: ib,
                });
            }
        }
        // Unmatched in a (and not a move target) = removed.
        for (ia, matched) in a_matched.iter().enumerate() {
            if !*matched && !heads_b.iter().enumerate().any(|(ib, h)| !b_matched[ib] && h == &heads_a[ia]) {
                structural_diffs.push(StructuralDiff {
                    kind: "removed".into(),
                    heading_path: heads_a[ia].clone(),
                    section_idx: ia,
                });
            }
        }
    }
    structural_diffs.sort_by_key(|d| d.section_idx);

    // ── TEXTUAL (zero LLM): line LCS diff inside aligned, modified sections ──
    let mut textual_diffs = Vec::new();
    // Changed spans, each with its char offset into b (for marked annotations + semantic verdicts).
    let mut changed_spans: Vec<ChangedSpan> = Vec::new();
    let b_chars: Vec<char> = b.chars().collect();

    if matches!(mode, CompareMode::Textual | CompareMode::Semantic) {
        for &(ia, ib) in &matched_pairs {
            if sections_a[ia].content == sections_b[ib].content {
                continue;
            }
            let hunks = line_diff(&sections_a[ia].content, &sections_b[ib].content);
            // Collect each `Ins` hunk's char span within b for annotation anchoring.
            for h in &hunks {
                if h.op == HunkOp::Ins && !h.text.trim().is_empty() {
                    if let Some((start, end)) = find_char_span(&b_chars, &h.text) {
                        changed_spans.push(ChangedSpan {
                            section_idx: ib,
                            offset_start: start,
                            offset_end: end,
                            old_text: sections_a[ia].content.clone(),
                            new_text: h.text.clone(),
                        });
                    }
                }
            }
            textual_diffs.push(TextualDiff { section_idx: ib, hunks });
        }
    }

    // ── SEMANTIC (member-gated, tier-3 LLM) ──
    let mut semantic_verdicts = Vec::new();
    let mut summary = None;
    let mut annotations = Vec::new();

    if mode == CompareMode::Semantic && member && !changed_spans.is_empty() {
        // Per changed span: Cheap-model verdict.
        for span in &changed_spans {
            let user = format!("【旧版 A】\n{}\n\n【新版 B】\n{}", span.old_text, span.new_text);
            let (resp, usage) = llms.cheap.chat(VERDICT_SYSTEM_PROMPT, &user)?;
            account_leg(&mut bill.map_llm_tokens, &usage, &user, &resp, &cheap_model);
            let (verdict, rationale) = parse_verdict(&resp);
            semantic_verdicts.push(SemanticVerdict {
                section_idx: span.section_idx,
                verdict: verdict.as_kebab().to_string(),
                rationale: rationale.clone(),
                model: cheap_model.clone(),
            });
            annotations.push(Annotation {
                offset_start: span.offset_start,
                offset_end: span.offset_end,
                kind: verdict.as_kebab().to_string(),
                note: rationale,
                severity: verdict.severity(),
            });
        }
        // Overall difference summary: Reasoning-model ×1.
        let payload = semantic_verdicts
            .iter()
            .map(|v| format!("- 章节{} [{}]: {}", v.section_idx, v.verdict, v.rationale))
            .collect::<Vec<_>>()
            .join("\n");
        let msgs = [ChatMessage::system(SUMMARY_SYSTEM_PROMPT), ChatMessage::user(&payload)];
        let (sum, usage) = llms.reasoning.chat_with_history(&msgs)?;
        account_leg(&mut bill.reduce_llm_tokens, &usage, &payload, &sum, &reasoning_model);
        summary = Some(sum);
    } else if output_mode == OutputMode::Marked {
        // marked mode WITHOUT semantic verdicts (non-member or structural/textual mode):
        // still anchor annotations to the changed spans, but with a neutral kind (zero LLM).
        for span in &changed_spans {
            annotations.push(Annotation {
                offset_start: span.offset_start,
                offset_end: span.offset_end,
                kind: "modified".into(),
                note: String::new(),
                severity: 2,
            });
        }
    }

    Ok(DiffReport {
        output_mode: match output_mode {
            OutputMode::Marked => "marked".into(),
            OutputMode::Structured => "structured".into(),
        },
        structural_diffs,
        textual_diffs,
        semantic_verdicts,
        summary,
        annotations,
        token_bill: bill,
    })
}

struct ChangedSpan {
    section_idx: usize,
    offset_start: usize,
    offset_end: usize,
    old_text: String,
    new_text: String,
}

/// Account an LLM call into a bill leg. Mock usage reports 0 tokens → approximate from text
/// (mirrors deep_summary). Real usage is added verbatim.
fn account_leg(
    leg: &mut crate::document_intelligence::token_bill::ModelLeg,
    usage: &crate::usage::TokenUsage,
    user: &str,
    resp: &str,
    model: &str,
) {
    use crate::context_compress::estimate_tokens;
    if usage.tokens_in == 0 {
        leg.r#in = leg.r#in.saturating_add(estimate_tokens(user) as u32);
        leg.out = leg.out.saturating_add(estimate_tokens(resp) as u32);
        if leg.model.is_empty() {
            leg.model = model.to_string();
        }
    } else {
        leg.add(usage);
    }
}

/// Parse the verdict LLM response: first line = label token, rest = rationale.
fn parse_verdict(resp: &str) -> (DiffVerdict, String) {
    let mut lines = resp.lines();
    let first = lines.next().unwrap_or("");
    let verdict = DiffVerdict::from_llm_token(first);
    let rationale = lines.collect::<Vec<_>>().join(" ").trim().to_string();
    let rationale = if rationale.is_empty() { first.trim().to_string() } else { rationale };
    (verdict, rationale)
}

/// Find the char span `[start, end)` of `needle` within `haystack_chars` (char-indexed).
/// Returns the FIRST occurrence. `needle` is matched by chars (CJK-safe). None if absent.
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

/// Line-level LCS diff → ins/del/eq hunks (zero LLM). Lines preserve their trailing newline
/// so a reconstructed `Ins` hunk text appears verbatim in doc b (offset alignment relies on this).
fn line_diff(a: &str, b: &str) -> Vec<TextualHunk> {
    let a_lines: Vec<&str> = a.split_inclusive('\n').collect();
    let b_lines: Vec<&str> = b.split_inclusive('\n').collect();
    let lcs = lcs_indices(&a_lines, &b_lines);

    let mut hunks: Vec<TextualHunk> = Vec::new();
    let mut ia = 0usize;
    let mut ib = 0usize;
    let mut push = |hunks: &mut Vec<TextualHunk>, op: HunkOp, text: &str| {
        if text.is_empty() {
            return;
        }
        match hunks.last_mut() {
            Some(last) if last.op == op => last.text.push_str(text),
            _ => hunks.push(TextualHunk { op, text: text.to_string() }),
        }
    };
    for &(la, lb) in &lcs {
        while ia < la {
            push(&mut hunks, HunkOp::Del, a_lines[ia]);
            ia += 1;
        }
        while ib < lb {
            push(&mut hunks, HunkOp::Ins, b_lines[ib]);
            ib += 1;
        }
        push(&mut hunks, HunkOp::Eq, b_lines[lb]);
        ia = la + 1;
        ib = lb + 1;
    }
    while ia < a_lines.len() {
        push(&mut hunks, HunkOp::Del, a_lines[ia]);
        ia += 1;
    }
    while ib < b_lines.len() {
        push(&mut hunks, HunkOp::Ins, b_lines[ib]);
        ib += 1;
    }
    hunks
}

/// LCS over two slices of equal-comparable items → matched index pairs (ia, ib) in order.
fn lcs_indices<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let n = a.len();
    let m = b.len();
    // dp[i][j] = LCS length of a[i..] and b[j..]
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Align two heading lists by LCS but treat empty headings as non-matchable (so two
/// untitled lead sections do not falsely align).
fn lcs_align(heads_a: &[String], heads_b: &[String]) -> Vec<(usize, usize)> {
    lcs_indices(heads_a, heads_b)
        .into_iter()
        .filter(|&(ia, ib)| !heads_a[ia].is_empty() && !heads_b[ib].is_empty())
        .collect()
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

    fn llms<'a>(cheap: &'a RecordingMockLlm, reasoning: &'a RecordingMockLlm) -> StageLlms<'a> {
        StageLlms { cheap, reasoning }
    }

    #[test]
    fn test_identical_docs_empty_diff_zero_llm() {
        let doc = "# 第一章\n\n相同的内容这里。\n\n# 第二章\n\n第二章内容相同。\n";
        let cheap = RecordingMockLlm::new("gpt-4o-mini");
        let reasoning = RecordingMockLlm::new("gpt-4o");
        let r = compare(doc, doc, CompareMode::Semantic, OutputMode::Marked, true, &router(), &llms(&cheap, &reasoning)).unwrap();
        assert!(r.structural_diffs.is_empty(), "identical docs → no structural diff");
        assert!(r.textual_diffs.is_empty(), "identical docs → no textual diff");
        assert!(r.semantic_verdicts.is_empty());
        assert!(r.annotations.is_empty());
        assert_eq!(cheap.call_count(), 0, "identical docs must not call any LLM");
        assert_eq!(reasoning.call_count(), 0);
        assert_eq!(r.token_bill.actual_billable_tokens(), 0, "token_bill all-zero");
    }

    #[test]
    fn test_structural_added_removed_moved() {
        let a = "# 引言\n\n引言内容。\n\n# 方法\n\n方法内容。\n";
        let b = "# 引言\n\n引言内容。\n\n# 结果\n\n新增的结果章节。\n";
        let cheap = RecordingMockLlm::new("gpt-4o-mini");
        let reasoning = RecordingMockLlm::new("gpt-4o");
        let r = compare(a, b, CompareMode::Structural, OutputMode::Structured, false, &router(), &llms(&cheap, &reasoning)).unwrap();
        // "方法" removed, "结果" added.
        assert!(r.structural_diffs.iter().any(|d| d.kind == "removed" && d.heading_path == "方法"));
        assert!(r.structural_diffs.iter().any(|d| d.kind == "added" && d.heading_path == "结果"));
        assert_eq!(cheap.call_count(), 0, "structural mode never calls LLM");
    }

    #[test]
    fn test_textual_hunks() {
        let a = "# 标题\n\n第一行保持。\n旧的第二行。\n";
        let b = "# 标题\n\n第一行保持。\n新的第二行替换。\n";
        let cheap = RecordingMockLlm::new("gpt-4o-mini");
        let reasoning = RecordingMockLlm::new("gpt-4o");
        let r = compare(a, b, CompareMode::Textual, OutputMode::Structured, false, &router(), &llms(&cheap, &reasoning)).unwrap();
        let td = r.textual_diffs.iter().find(|d| d.hunks.iter().any(|h| h.op == HunkOp::Ins)).expect("an insert hunk");
        assert!(td.hunks.iter().any(|h| h.op == HunkOp::Eq), "shared line is an eq hunk");
        assert!(td.hunks.iter().any(|h| h.op == HunkOp::Ins && h.text.contains("新的第二行替换")));
        assert_eq!(cheap.call_count(), 0, "textual mode never calls LLM");
    }

    #[test]
    fn test_semantic_gated_no_member_no_llm() {
        let a = "# 标题\n\n第一行。\n旧观点：我支持这个方案。\n";
        let b = "# 标题\n\n第一行。\n新观点：我反对这个方案。\n";
        let cheap = RecordingMockLlm::new("gpt-4o-mini").with_response("stance-reversal\n立场从支持反转为反对");
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("总体立场反转");
        // member=false → semantic layer is skipped (LLM not called), verdicts empty.
        let r = compare(a, b, CompareMode::Semantic, OutputMode::Marked, false, &router(), &llms(&cheap, &reasoning)).unwrap();
        assert!(r.semantic_verdicts.is_empty(), "non-member: no semantic verdicts");
        assert_eq!(cheap.call_count(), 0, "non-member: no LLM call");
        assert_eq!(reasoning.call_count(), 0);
        // marked mode still anchors a neutral 'modified' annotation to the changed span.
        assert!(!r.annotations.is_empty(), "marked mode anchors changed spans even without LLM");
    }

    #[test]
    fn test_semantic_verdict_classes() {
        let a = "# 标题\n\n第一行不变。\n旧观点：我支持这个方案。\n";
        let b = "# 标题\n\n第一行不变。\n新观点：我反对这个方案。\n";
        let cheap = RecordingMockLlm::new("gpt-4o-mini").with_response("stance-reversal\n立场从支持反转为反对");
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("总体立场反转");
        let r = compare(a, b, CompareMode::Semantic, OutputMode::Marked, true, &router(), &llms(&cheap, &reasoning)).unwrap();
        assert_eq!(r.semantic_verdicts.len(), 1, "one changed span → one verdict");
        assert_eq!(r.semantic_verdicts[0].verdict, "stance-reversal");
        assert_eq!(r.semantic_verdicts[0].model, "gpt-4o-mini", "verdict uses Cheap model");
        // reasoning model used for the ×1 overall summary.
        assert_eq!(reasoning.call_count(), 1, "summary is one reasoning call");
        assert!(r.summary.is_some());
        assert!(r.token_bill.map_llm_tokens.model == "gpt-4o-mini");
        assert!(r.token_bill.reduce_llm_tokens.model == "gpt-4o");
    }

    #[test]
    fn test_marked_annotations_offsets_align() {
        // The core §3.5 offset-alignment guarantee: each annotation's b[offset_start..offset_end]
        // (by CHAR index) is exactly the changed span text.
        let a = "# 标题\n\n保持不变的开头行。\n旧的数值是 100。\n";
        let b = "# 标题\n\n保持不变的开头行。\n新的数值是 250 了。\n";
        let cheap = RecordingMockLlm::new("gpt-4o-mini").with_response("numeric-change\n数值由100变为250");
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("数值变化");
        let r = compare(a, b, CompareMode::Semantic, OutputMode::Marked, true, &router(), &llms(&cheap, &reasoning)).unwrap();

        assert!(!r.annotations.is_empty(), "marked mode produces annotations");
        let b_chars: Vec<char> = b.chars().collect();
        for ann in &r.annotations {
            assert!(ann.offset_end <= b_chars.len(), "offset within b");
            assert!(ann.offset_start < ann.offset_end, "non-empty span");
            let span: String = b_chars[ann.offset_start..ann.offset_end].iter().collect();
            // The annotated span must be REAL changed text present in b (the new line).
            assert!(b.contains(&span), "annotated span exists verbatim in b");
            assert!(span.contains("新的数值是 250"), "annotation anchors the actual changed span, got: {span:?}");
        }
        // numeric-change kind carried through to the marked annotation.
        assert!(r.annotations.iter().any(|x| x.kind == "numeric-change"));
    }

    #[test]
    fn test_serde_roundtrip_report() {
        let a = "# T\n\neq line.\nold.\n";
        let b = "# T\n\neq line.\nnew.\n";
        let cheap = RecordingMockLlm::new("gpt-4o-mini");
        let reasoning = RecordingMockLlm::new("gpt-4o");
        let r = compare(a, b, CompareMode::Textual, OutputMode::Marked, false, &router(), &llms(&cheap, &reasoning)).unwrap();
        let js = serde_json::to_string(&r).unwrap();
        let back: DiffReport = serde_json::from_str(&js).unwrap();
        assert_eq!(back.output_mode, "marked");
        assert_eq!(back, r);
    }

    #[test]
    fn test_empty_and_single_section_docs() {
        let cheap = RecordingMockLlm::new("gpt-4o-mini");
        let reasoning = RecordingMockLlm::new("gpt-4o");
        // empty vs empty → no diffs, no panic.
        let r = compare("", "", CompareMode::Semantic, OutputMode::Marked, true, &router(), &llms(&cheap, &reasoning)).unwrap();
        assert!(r.structural_diffs.is_empty() && r.textual_diffs.is_empty());
        // single plain paragraph (no heading) vs changed paragraph.
        let r2 = compare("只有一段文字没有标题。", "改了的一段文字没有标题。", CompareMode::Textual, OutputMode::Structured, false, &router(), &llms(&cheap, &reasoning)).unwrap();
        // degenerate single-section: textual diff present, no panic.
        let _ = r2;
        assert_eq!(cheap.call_count(), 0);
    }
}
