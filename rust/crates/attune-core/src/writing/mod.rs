//! Writing Engine — OSS attune general-purpose *grounded narrative generation*.
//!
//! Per spec `docs/superpowers/specs/2026-06-19-writing-engine.md`. attune today can
//! read / retrieve / extract / annotate / summarize the knowledge base but has **no
//! "write"** surface (the only generation agent is pro `legal_drafter`). This module
//! adds a grounded narrative-generation chain so any user can turn an outline + KB
//! material into a draft that is **source-attributable, fact-faithful, cost-visible,
//! and iterable**.
//!
//! ## MVP scope (this slice)
//!   - **W1 draft** ([`draft`]) — outline + KB material → narrative draft.
//!   - **W2 rewrite** ([`rewrite`]) — adjust tone / length / audience while
//!     **preserving facts** (a rewrite must not introduce un-sourced facts).
//!
//! W3 outline / W4 cite / W5 synthesis / W6 terms are later slices (spec §2.3).
//!
//! ## Reuse (no fork — every line extends an already-shipped primitive)
//!   - [`crate::pii::llm_chat_redacted_hardened`] — §4.5 schema-guided JSON +
//!     retry-with-validation (≤3) + few-shot + PII redact/restore. The generation
//!     call rides this exact stack (same one `ai_annotator` uses).
//!   - [`crate::chat_reliability::evaluate_response`] — token-overlap grounding.
//!     The writing grounding validator ([`grounding`]) reuses its tokenizer notion
//!     via a thin re-implementation kept in lock-step (CJK 2-gram + ≥3-char ASCII).
//!   - [`crate::document_intelligence::token_bill::TokenBill`] — cost accounting,
//!     attached to every [`WritingResult`]. No secret field (sentinel-guarded).
//!   - [`crate::document_intelligence::model_routing::ModelRole`] — cheap (rewrite-short)
//!     vs reasoning (narrative) stage selection (wired at the route layer).
//!
//! ## Cost contract (CLAUDE.md §"成本感知与触发契约")
//!
//! Generation (W1/W2) is **tier-3 💰** — must be user-triggered, never background.
//! The module itself does not gate (the route layer does via `enforce_gate`); but it
//! never auto-runs and the `LlmProvider` is only invoked when the caller asks.
//!
//! ## Hallucination red line (spec §11 risk A / D)
//!
//! Every generated *factual* segment carries `grounding: Vec<GroundingRef>`; a segment
//! that cannot be tied back to any source is reported in `unverified_spans` (and marked
//! with the template's placeholder, e.g. `[需核实]`). KB material is screened for
//! injection instructions ([`source_has_injection_instruction`]) **before** any model
//! call so a poisoned source cannot steer generation or fabricate citations.

pub mod cite;
pub mod draft;
pub mod grounding;
pub mod outline;
pub mod rewrite;
pub mod synthesis;
pub mod templates;

use serde::{Deserialize, Serialize};

pub use cite::{build_citations, find_inline_anchors, CiteError, CiteStyle, Citation, InlineAnchor, SourceMeta};
pub use grounding::{
    ground_segments, ground_segments_with_judge, source_has_injection_instruction, GroundingConfig,
    JudgeConfig, JudgeGroundOutcome,
};
pub use outline::{outline_forward, outline_reverse, OutlineNode, OutlineResult};
pub use synthesis::{synthesize, SynthLlms, SynthesisRequest, SynthesisStructure};
pub use templates::{
    fill_template, FillResult, GeneralTemplate, RedLine, TemplateRegistry, WorkedExample,
    WritingTemplate, OSS_TEMPLATE_IDS,
};

use crate::document_intelligence::token_bill::TokenBill;

/// Schema version of the [`WritingResult`] envelope (spec §10 — additive evolution).
pub const WRITING_SCHEMA_VERSION: u32 = 1;

/// The generation mode a [`WritingResult`] was produced by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritingMode {
    /// W1 — generate a draft from outline + KB material.
    Draft,
    /// W2 — rewrite/polish selected text (tone/length/audience), preserving facts.
    Rewrite,
    /// W5 — multi-source synthesis / literature review.
    Synthesis,
}

/// Where a generated segment's facts come from (grounding — a first-class citizen).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingRef {
    /// What kind of source this ref points at.
    pub kind: GroundingKind,
    /// KB source item id (`None` for external / user_input).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Char offset `[start, end)` **within the source material** the segment grounds
    /// to. UTF-16 code units (CJK-safe, parity with `ai_annotator`). `None` when the
    /// match is item-level without a pinned span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_offset: Option<[u32; 2]>,
    /// User-supplied external source identifier (DOI / URL / bibliographic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,
    /// Number of tokens the segment shares with the source (grounding strength;
    /// the same token-overlap notion as `chat_reliability`).
    pub overlap_tokens: u32,
    /// `true` iff this ref was credited by the LLM-judge fallback (semantic grounding)
    /// rather than the deterministic token-overlap path. A judge-elevated ref is always
    /// re-link verified: its evidence quote is a real substring of the source (the
    /// no-fabrication guard, spec §11 A). Additive (`#[serde(default)]`) — old clients
    /// read `false` and treat it like any grounded ref; no schema bump (spec §10).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub judge_elevated: bool,
}

/// The flavor of a [`GroundingRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingKind {
    /// A KB item (vault).
    KbItem,
    /// A user-supplied external reference.
    External,
    /// User free-text input (e.g. the outline itself).
    UserInput,
}

/// One generated segment with its grounding verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// The generated text of this segment.
    pub text: String,
    /// Char offset `[start, end)` of this segment **within `WritingResult.content`**
    /// (UTF-16 code units).
    pub offset: [u32; 2],
    /// Source refs this segment grounds to. Empty ⇒ unverified.
    pub grounding: Vec<GroundingRef>,
    /// True iff at least one [`GroundingRef`] met the overlap threshold OR the segment
    /// carries no factual claim (a connective / framing sentence is "verified" trivially).
    pub verified: bool,
}

/// A per-sentence rewrite suggestion (rewrite "review" output mode). Offsets are into
/// the *original* text the caller passed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewAnnotation {
    /// Char offset `[start, end)` of the original span (UTF-16 code units).
    pub offset: [u32; 2],
    /// The proposed replacement.
    pub suggestion: String,
    /// Short reason for the change.
    pub reason: String,
}

/// The unified writing-engine response envelope (all endpoints share this).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WritingResult {
    /// Schema version (additive evolution, spec §10).
    pub schema_version: u32,
    /// Which mode produced this result.
    pub mode: WritingMode,
    /// The full narrative text.
    pub content: String,
    /// Structured breakdown: one entry per segment with grounding.
    pub segments: Vec<Segment>,
    /// Review-mode per-sentence suggestions (rewrite only; empty otherwise).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<ReviewAnnotation>,
    /// Spans of `content` (UTF-16 offsets) that could not be grounded — UI surfaces
    /// these as `[需核实]` / red warning. Empty ⇒ everything grounded.
    pub unverified_spans: Vec<[u32; 2]>,
    /// Cost accounting (reused; no secret field).
    pub token_bill: TokenBill,
}

impl WritingResult {
    /// True iff **no** factual segment is grounded — a strong "likely all hallucinated"
    /// signal. The route layer turns this into a 200 + red UI warning (spec §7), not an
    /// error: the user may still want the draft, but must be told it is unverified.
    pub fn is_entirely_unverified(&self) -> bool {
        !self.segments.is_empty() && self.segments.iter().all(|s| !s.verified)
    }
}

/// A KB source passed into the writing engine. Mirrors `chat_reliability::RetrievedChunk`
/// but is the writing module's own owned input type (kept independent so the writing API is
/// stable even if the chat retrieval shape changes — spec §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMaterial {
    /// KB item id (empty ⇒ external / user-supplied, not eligible for KB grounding).
    pub item_id: String,
    /// The source text (post-decryption; caller redacts per privacy tier — the engine
    /// PII-redacts again on the outbound LLM call via the hardened helper).
    pub text: String,
}

impl SourceMaterial {
    /// Convenience constructor for tests / fixtures.
    pub fn new(item_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            item_id: item_id.into(),
            text: text.into(),
        }
    }
}

/// Audience / tone / length knobs shared by draft & rewrite. All optional; `None` ⇒
/// the template default. These are *advisory* prompt hints — they never change grounding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleTarget {
    /// e.g. "formal" / "casual" / "academic".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    /// e.g. "shorter" / "longer" / "concise".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<String>,
    /// e.g. "expert" / "beginner".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// freeform extra style hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

impl StyleTarget {
    /// Render a compact Chinese style-hint clause for the prompt. Empty when no knob set.
    pub fn prompt_clause(&self) -> String {
        let mut parts = Vec::new();
        if let Some(t) = self.tone.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("语气={t}"));
        }
        if let Some(l) = self.length.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("长度={l}"));
        }
        if let Some(a) = self.audience.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("受众={a}"));
        }
        if let Some(s) = self.style.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("风格={s}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("（要求：{}）", parts.join("，"))
        }
    }
}

/// Error returned by the writing engine (mapped to stable kebab `code` at the route layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WritingError {
    /// Both outline and source material are empty — nothing to write from.
    NoSourceMaterial,
    /// Empty input text passed to rewrite / 0-node outline etc.
    EmptyInput,
    /// LLM not configured / unavailable.
    LlmUnavailable,
    /// Generation failed after the hardened retry loop (schema invalid ×3, etc.).
    GenerationUnavailable(String),
    /// A KB source contained an injection instruction (spec §11 risk D).
    SourceInjection,
}

impl WritingError {
    /// Stable kebab error code (spec §7) for the route layer.
    pub fn code(&self) -> &'static str {
        match self {
            WritingError::NoSourceMaterial => "no-source-material",
            WritingError::EmptyInput => "empty-input",
            WritingError::LlmUnavailable => "llm-unavailable",
            WritingError::GenerationUnavailable(_) => "generation-unavailable",
            WritingError::SourceInjection => "source-injection-detected",
        }
    }
}

impl std::fmt::Display for WritingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WritingError::NoSourceMaterial => write!(f, "no source material: both outline and item_ids are empty"),
            WritingError::EmptyInput => write!(f, "empty input"),
            WritingError::LlmUnavailable => write!(f, "no LLM provider is configured"),
            WritingError::GenerationUnavailable(e) => write!(f, "generation unavailable after retries: {e}"),
            WritingError::SourceInjection => write!(f, "a source contains an injection instruction; refusing to generate"),
        }
    }
}

impl std::error::Error for WritingError {}

/// Result alias for the writing engine.
pub type WritingResultT<T> = std::result::Result<T, WritingError>;

/// Split a generated narrative into segments at sentence/paragraph boundaries, computing
/// UTF-16 offsets into `content`. CJK-aware: breaks on `。！？\n` and ASCII `.!?` followed
/// by whitespace, keeping the terminator with the segment.
pub fn split_segments(content: &str) -> Vec<(String, [u32; 2])> {
    let mut out = Vec::new();
    let mut seg_start_u16 = 0u32; // utf-16 offset of current segment start
    let mut cur = String::new();
    let mut u16_pos = 0u32;
    let chars: Vec<char> = content.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        cur.push(ch);
        let ch_u16 = ch.len_utf16() as u32;
        u16_pos += ch_u16;
        let is_cjk_term = matches!(ch, '。' | '！' | '？' | '\n');
        let is_ascii_term = matches!(ch, '.' | '!' | '?')
            && chars
                .get(i + 1)
                .map(|n| n.is_whitespace())
                .unwrap_or(true);
        if is_cjk_term || is_ascii_term {
            let trimmed = cur.trim();
            if !trimmed.is_empty() {
                out.push((cur.clone(), [seg_start_u16, u16_pos]));
            }
            cur.clear();
            seg_start_u16 = u16_pos;
        }
    }
    let trimmed = cur.trim();
    if !trimmed.is_empty() {
        out.push((cur.clone(), [seg_start_u16, u16_pos]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_target_prompt_clause_empty_when_no_knobs() {
        assert_eq!(StyleTarget::default().prompt_clause(), "");
    }

    #[test]
    fn style_target_prompt_clause_renders_set_knobs() {
        let st = StyleTarget {
            tone: Some("formal".into()),
            length: Some("shorter".into()),
            audience: None,
            style: None,
        };
        let clause = st.prompt_clause();
        assert!(clause.contains("语气=formal"));
        assert!(clause.contains("长度=shorter"));
        assert!(!clause.contains("受众"));
    }

    #[test]
    fn split_segments_cjk_and_offsets() {
        let segs = split_segments("第一句。第二句！");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].0, "第一句。");
        assert_eq!(segs[0].1, [0, 4]); // 4 CJK chars = 4 utf-16 units
        assert_eq!(segs[1].1, [4, 8]);
    }

    #[test]
    fn split_segments_handles_emoji_utf16_surrogates() {
        // 😀 is 2 UTF-16 code units. Offsets must account for it (CJK-safe contract).
        let segs = split_segments("hi 😀. ok.");
        // "hi 😀." then " ok."
        assert_eq!(segs.len(), 2);
        // "hi " = 3, 😀 = 2, "." = 1 → end offset 6
        assert_eq!(segs[0].1, [0, 6]);
    }

    #[test]
    fn split_segments_empty_input() {
        assert!(split_segments("").is_empty());
        assert!(split_segments("   \n  ").is_empty());
    }

    #[test]
    fn is_entirely_unverified_true_when_all_unverified() {
        let r = WritingResult {
            schema_version: WRITING_SCHEMA_VERSION,
            mode: WritingMode::Draft,
            content: "x".into(),
            segments: vec![Segment {
                text: "x".into(),
                offset: [0, 1],
                grounding: vec![],
                verified: false,
            }],
            annotations: vec![],
            unverified_spans: vec![[0, 1]],
            token_bill: TokenBill::default(),
        };
        assert!(r.is_entirely_unverified());
    }

    #[test]
    fn is_entirely_unverified_false_when_empty_segments() {
        let r = WritingResult {
            schema_version: WRITING_SCHEMA_VERSION,
            mode: WritingMode::Draft,
            content: String::new(),
            segments: vec![],
            annotations: vec![],
            unverified_spans: vec![],
            token_bill: TokenBill::default(),
        };
        assert!(!r.is_entirely_unverified());
    }

    #[test]
    fn error_codes_are_stable_kebab() {
        assert_eq!(WritingError::NoSourceMaterial.code(), "no-source-material");
        assert_eq!(WritingError::EmptyInput.code(), "empty-input");
        assert_eq!(WritingError::LlmUnavailable.code(), "llm-unavailable");
        assert_eq!(WritingError::SourceInjection.code(), "source-injection-detected");
        assert_eq!(
            WritingError::GenerationUnavailable("x".into()).code(),
            "generation-unavailable"
        );
    }
}
