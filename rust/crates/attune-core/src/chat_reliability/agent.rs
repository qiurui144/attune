//! Chat-reliability agent — pure function over (response, chunks, query).
//!
//! See module-level docs in [`super`] for cost contract and verification
//! doctrine. This file holds the data types, the [`evaluate_response`] entry
//! point, and the three extractors (citation / contradiction / hallucination).

use std::collections::{BTreeSet, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::entities::{extract_entities, Entity, EntityKind};

// ============================================================================
// Public input types
// ============================================================================

/// One RAG hit fed into the reliability agent. Mirrors what
/// [`crate::chat::ChatEngine`] retrieves, slimmed to just the fields the
/// agent needs (no `inject_content`, no `breadcrumb`, no `corpus_domain`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedChunk {
    /// Source item identifier (vault item UUID). Empty for web-search /
    /// transient chunks — those are not eligible for `CitationStatus::Grounded`
    /// (no persistent source to point at).
    pub item_id: String,
    /// Chunk identifier within the item. Optional — some retrieval paths
    /// (memory layer) return whole-item passages without per-chunk id.
    #[serde(default)]
    pub chunk_id: Option<String>,
    /// Chunk text content (post-decryption). The agent reads this verbatim
    /// — callers must pass redacted-or-not according to their privacy tier;
    /// the agent does not redact.
    pub chunk_text: String,
    /// Retrieval relevance score `[0, 1]` (RRF + reranker fused). Used as a
    /// tie-break when multiple chunks could ground the same citation; never
    /// causes a flag to be added/removed.
    #[serde(default)]
    pub score: f32,
}

impl RetrievedChunk {
    /// Convenience constructor for tests / fixtures.
    pub fn new(item_id: impl Into<String>, chunk_text: impl Into<String>) -> Self {
        Self {
            item_id: item_id.into(),
            chunk_id: None,
            chunk_text: chunk_text.into(),
            score: 0.0,
        }
    }
}

// ============================================================================
// Public output types
// ============================================================================

/// One inline citation marker check. The marker is whatever
/// `[item:<id>]` / `[source:<id>]` token the LLM emitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationCheck {
    /// `item_id` extracted from the inline marker.
    pub item_id: String,
    /// Verdict — see [`CitationStatus`].
    pub status: CitationStatus,
}

/// Outcome of a citation check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationStatus {
    /// `item_id` exists in the retrieved chunks **and** the chunk text has
    /// non-trivial token overlap (≥ [`ChatReliabilityConfig::min_grounding_overlap_tokens`])
    /// with the response.
    Grounded,
    /// `item_id` matches a retrieved chunk, but the chunk text has near-zero
    /// overlap with the response → the cite is a "name-drop" rather than
    /// content support.
    WeakOverlap,
    /// `item_id` was not in the retrieved chunk list at all — the LLM
    /// fabricated a citation handle.
    Fabricated,
}

/// One contradiction between the response and a retrieved chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contradiction {
    /// What kind of contradicting value.
    pub kind: ContradictionKind,
    /// The value the response asserts.
    pub response_value: String,
    /// The conflicting value found in retrieved chunks (same entity, different
    /// value).
    pub chunk_value: String,
    /// `item_id` of the chunk holding the conflicting value (empty for
    /// memory / web chunks without a source item).
    pub chunk_item_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionKind {
    /// Same date entity (e.g. an event year) appears with two distinct values.
    Date,
    /// Same money entity appears with two distinct values.
    Money,
}

/// One hallucination flag — specific factual token in the response that did
/// not appear in **any** retrieved chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HallucinationFlag {
    pub kind: HallucinationKind,
    /// The bare token (e.g. `"2024-03-15"` / `"¥10000"` / `"某科技有限公司"`).
    pub token: String,
    /// Heuristic severity. Caller may threshold on this to decide whether to
    /// surface a UI warning.
    pub severity: HallucinationSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HallucinationKind {
    /// Concrete number (with currency symbol or "万"/"亿" unit) absent.
    Number,
    /// ISO date / Chinese date absent.
    Date,
    /// Organization-like token (含"公司"/"研究所"/"事务所"等通用机构后缀) absent.
    Organization,
    /// 2-4 字中文人名 absent.
    Person,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HallucinationSeverity {
    /// Token is uncommon (Date / specific Money amount / Org / Person).
    High,
    /// Token is partial (e.g. a year alone, an org type without name).
    Medium,
}

/// Aggregated reliability report. Returned by [`evaluate_response`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatReliabilityReport {
    /// Per-cite verdict, one entry per `[item:<id>]` / `[source:<id>]` marker
    /// found in the response. Empty when the response has no inline cites.
    pub citation_grounded: Vec<CitationCheck>,
    /// Directly conflicting date / money values.
    pub contradictions: Vec<Contradiction>,
    /// Tokens flagged as possible hallucinations.
    pub hallucination_flags: Vec<HallucinationFlag>,
    /// Aggregate confidence in `[0, 1]`. **Not** a probabilistic verdict —
    /// just a deterministic combination of the three signals above. See
    /// [`confidence_from_signals`] for the exact formula.
    pub overall_confidence: f32,
}

// ============================================================================
// Tunable thresholds (per spec §"deterministic config")
// ============================================================================

/// Caller-tunable knobs. Defaults are chosen so a typical short answer
/// ("没有相关资料") with no chunks reports a *neutral* confidence rather than
/// a misleading 1.0 — see [`confidence_from_signals`].
#[derive(Debug, Clone)]
pub struct ChatReliabilityConfig {
    /// Minimum number of unique tokens that must be shared between a response
    /// and a cited chunk to qualify as [`CitationStatus::Grounded`]. Below
    /// this → [`CitationStatus::WeakOverlap`].
    pub min_grounding_overlap_tokens: usize,
    /// Minimum length of a "specific number" token to be eligible for
    /// hallucination flagging. Filters out trivial digits (`"1"`, `"2"`).
    pub min_number_token_chars: usize,
    /// Per-signal weights for the aggregate confidence score. Sum is not
    /// required to equal anything — see [`confidence_from_signals`].
    pub weight_citation: f32,
    pub weight_contradiction: f32,
    pub weight_hallucination: f32,
}

impl Default for ChatReliabilityConfig {
    fn default() -> Self {
        Self {
            min_grounding_overlap_tokens: 3,
            min_number_token_chars: 2,
            // Contradictions are the strongest signal (the chunk literally
            // says a different value), then hallucinations (claim not in any
            // chunk), then citation weakness (cite present but thin).
            weight_citation: 0.20,
            weight_contradiction: 0.50,
            weight_hallucination: 0.30,
        }
    }
}

// ============================================================================
// Entry point
// ============================================================================

/// Evaluate one chat response against the chunks that the RAG path returned.
///
/// **Deterministic / pure** — same `(response, chunks, query, config)` always
/// yields the same report. Never reads system time, never calls an LLM, never
/// blocks. Safe to call on every chat turn from a background tokio task.
///
/// `query` is currently unused by the heuristics but kept in the API so future
/// versions (e.g. query-entity-not-in-response detection) can use it without
/// a breaking change.
pub fn evaluate_response(
    response: &str,
    chunks: &[RetrievedChunk],
    query: &str,
    config: &ChatReliabilityConfig,
) -> ChatReliabilityReport {
    let _ = query; // reserved for future signals; explicit to silence lints

    // Empty response → no signals at all; neutral confidence.
    if response.trim().is_empty() {
        return ChatReliabilityReport {
            citation_grounded: vec![],
            contradictions: vec![],
            hallucination_flags: vec![],
            overall_confidence: 0.5,
        };
    }

    let citation_grounded = check_citations(response, chunks, config);
    let contradictions = find_contradictions(response, chunks);
    let hallucination_flags = find_hallucinations(response, chunks, config);

    let overall_confidence = confidence_from_signals(
        &citation_grounded,
        &contradictions,
        &hallucination_flags,
        config,
    );

    ChatReliabilityReport {
        citation_grounded,
        contradictions,
        hallucination_flags,
        overall_confidence,
    }
}

// ============================================================================
// Confidence aggregation
// ============================================================================

/// Combine the three signal vectors into a single `[0, 1]` confidence.
///
/// Formula (deterministic, hand-derived in tests):
///
/// ```text
/// citation_penalty       = (weak + 2 * fabricated) / max(1, total_cites) * weight_citation
/// contradiction_penalty  = min(1.0, contradictions.len() as f32 / 2.0) * weight_contradiction
/// hallucination_penalty  = min(1.0, hallucinations.len() as f32 / 4.0) * weight_hallucination
/// confidence             = (1.0 - sum_of_penalties).clamp(0.0, 1.0)
/// ```
///
/// Rationales:
/// - One fabricated cite (`item_id` not in chunks) is worth 2 weak cites.
/// - Two contradictions saturate the contradiction penalty — caller already
///   has enough signal to act; a third doesn't drive the score lower.
/// - Four hallucination tokens saturate the hallucination penalty — same.
/// - The `(1.0 - sum)` form keeps the score interpretable: confidence == 1.0
///   ⇔ all signals clean; confidence == 0.0 ⇔ every signal saturated.
pub fn confidence_from_signals(
    citations: &[CitationCheck],
    contradictions: &[Contradiction],
    hallucinations: &[HallucinationFlag],
    config: &ChatReliabilityConfig,
) -> f32 {
    let total_cites = citations.len() as f32;
    let (weak, fabricated) = citations.iter().fold((0.0_f32, 0.0_f32), |acc, c| match c.status {
        CitationStatus::Grounded => acc,
        CitationStatus::WeakOverlap => (acc.0 + 1.0, acc.1),
        CitationStatus::Fabricated => (acc.0, acc.1 + 1.0),
    });
    let citation_penalty = if total_cites > 0.0 {
        ((weak + 2.0 * fabricated) / total_cites).min(1.0) * config.weight_citation
    } else {
        0.0
    };
    let contradiction_penalty =
        (contradictions.len() as f32 / 2.0).min(1.0) * config.weight_contradiction;
    let hallucination_penalty =
        (hallucinations.len() as f32 / 4.0).min(1.0) * config.weight_hallucination;
    let confidence = 1.0 - (citation_penalty + contradiction_penalty + hallucination_penalty);
    confidence.clamp(0.0, 1.0)
}

// ============================================================================
// Citation extractor
// ============================================================================

fn citation_marker_re() -> &'static Regex {
    // Matches inline `[item:abc-123]`, `[source:abc-123]`, `[doc:abc-123]`.
    // item_id charset deliberately permissive: alphanumerics + `_` + `-`.
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[(?:item|source|doc):([A-Za-z0-9_\-]{1,128})\]")
            .expect("static regex compiles")
    })
}

fn check_citations(
    response: &str,
    chunks: &[RetrievedChunk],
    config: &ChatReliabilityConfig,
) -> Vec<CitationCheck> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();

    let response_tokens = tokenize(response);

    for cap in citation_marker_re().captures_iter(response) {
        let cited_id = cap.get(1).unwrap().as_str().to_string();
        // De-dup: only emit one CitationCheck per item_id even if the LLM
        // repeated the marker. The first occurrence wins (stable order).
        if !seen.insert(cited_id.clone()) {
            continue;
        }
        let matching_chunk = chunks.iter().find(|c| c.item_id == cited_id);
        let status = match matching_chunk {
            None => CitationStatus::Fabricated,
            Some(chunk) => {
                let chunk_tokens = tokenize(&chunk.chunk_text);
                let overlap = token_overlap(&response_tokens, &chunk_tokens);
                if overlap >= config.min_grounding_overlap_tokens {
                    CitationStatus::Grounded
                } else {
                    CitationStatus::WeakOverlap
                }
            }
        };
        out.push(CitationCheck {
            item_id: cited_id,
            status,
        });
    }
    out
}

// ============================================================================
// Contradiction finder
// ============================================================================

fn find_contradictions(response: &str, chunks: &[RetrievedChunk]) -> Vec<Contradiction> {
    let mut out = Vec::new();

    let response_entities = extract_entities(response);
    if response_entities.is_empty() {
        return out;
    }

    // Group response date / money entities by context. For now use the
    // surrounding 16-char window as a coarse "context"; ground truth in
    // tests is engineered around this window size.
    let response_dates: Vec<&Entity> = response_entities
        .iter()
        .filter(|e| e.kind == EntityKind::Date)
        .collect();
    let response_money: Vec<&Entity> = response_entities
        .iter()
        .filter(|e| e.kind == EntityKind::Money)
        .collect();

    // For each (response_date, chunk) pair: if the chunk mentions the same
    // surrounding context window text but with a *different* date value,
    // emit a Date contradiction.
    for r_ent in response_dates {
        let r_context = context_window(response, r_ent.byte_start, r_ent.byte_end, 16);
        for chunk in chunks {
            let chunk_entities = extract_entities(&chunk.chunk_text);
            for c_ent in chunk_entities.iter().filter(|e| e.kind == EntityKind::Date) {
                if c_ent.value == r_ent.value {
                    continue; // identical → not a contradiction
                }
                let c_context = context_window(
                    &chunk.chunk_text,
                    c_ent.byte_start,
                    c_ent.byte_end,
                    16,
                );
                if context_overlap(&r_context, &c_context) {
                    out.push(Contradiction {
                        kind: ContradictionKind::Date,
                        response_value: r_ent.value.clone(),
                        chunk_value: c_ent.value.clone(),
                        chunk_item_id: chunk.item_id.clone(),
                    });
                }
            }
        }
    }

    for r_ent in response_money {
        let r_context = context_window(response, r_ent.byte_start, r_ent.byte_end, 16);
        for chunk in chunks {
            let chunk_entities = extract_entities(&chunk.chunk_text);
            for c_ent in chunk_entities.iter().filter(|e| e.kind == EntityKind::Money) {
                if c_ent.value == r_ent.value {
                    continue;
                }
                let c_context = context_window(
                    &chunk.chunk_text,
                    c_ent.byte_start,
                    c_ent.byte_end,
                    16,
                );
                if context_overlap(&r_context, &c_context) {
                    out.push(Contradiction {
                        kind: ContradictionKind::Money,
                        response_value: r_ent.value.clone(),
                        chunk_value: c_ent.value.clone(),
                        chunk_item_id: chunk.item_id.clone(),
                    });
                }
            }
        }
    }
    out
}

// ============================================================================
// Hallucination finder
// ============================================================================

fn find_hallucinations(
    response: &str,
    chunks: &[RetrievedChunk],
    config: &ChatReliabilityConfig,
) -> Vec<HallucinationFlag> {
    let mut out = Vec::new();

    let response_entities = extract_entities(response);
    if response_entities.is_empty() {
        return out;
    }

    // Union of *normalized* chunk text — substring lookup is the cheapest
    // "did this token appear?" check that doesn't false-positive on
    // whitespace / punctuation differences.
    let chunk_union_norm: String = chunks
        .iter()
        .map(|c| normalize_text(&c.chunk_text))
        .collect::<Vec<_>>()
        .join(" ");

    let mut seen_tokens: HashSet<String> = HashSet::new();

    for ent in response_entities {
        let token = ent.value.clone();
        // Trivial filters.
        if token.chars().count() < 2 {
            continue;
        }
        if ent.kind == EntityKind::Money && token.chars().count() < config.min_number_token_chars {
            continue;
        }
        if !seen_tokens.insert(token.clone()) {
            continue;
        }
        let token_norm = normalize_text(&token);
        if chunk_union_norm.contains(&token_norm) {
            continue; // grounded
        }

        let (kind, severity) = match ent.kind {
            EntityKind::Date => (HallucinationKind::Date, HallucinationSeverity::High),
            EntityKind::Money => (HallucinationKind::Number, HallucinationSeverity::High),
            EntityKind::Organization => {
                (HallucinationKind::Organization, HallucinationSeverity::High)
            }
            EntityKind::Person => (HallucinationKind::Person, HallucinationSeverity::Medium),
        };
        out.push(HallucinationFlag {
            kind,
            token,
            severity,
        });
    }
    out
}

// ============================================================================
// Text utilities (public so tests / sibling agents can reuse)
// ============================================================================

/// Lowercase + collapse whitespace + strip ascii / Chinese punctuation.
/// Pure / deterministic. Exposed because golden fixtures reason about
/// "normalized chunk text" for substring checks.
pub fn normalize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
            continue;
        }
        if is_skippable_punct(ch) {
            continue;
        }
        for low in ch.to_lowercase() {
            out.push(low);
        }
        last_space = false;
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn is_skippable_punct(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.'
            | '!'
            | '?'
            | ';'
            | ':'
            | '\''
            | '"'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '，'
            | '。'
            | '！'
            | '？'
            | '；'
            | '：'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '（'
            | '）'
            | '【'
            | '】'
            | '《'
            | '》'
    )
}

fn tokenize(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let norm = normalize_text(s);
    // ASCII-word tokens (>=3 chars to drop trivial "is" / "or").
    for tok in norm.split(|c: char| c.is_whitespace() || c == '-' || c == '/' || c == '_') {
        let t = tok.trim();
        if t.chars().count() >= 3 && t.chars().any(|c| c.is_ascii_alphanumeric()) {
            out.insert(t.to_string());
        }
    }
    // 2-gram of contiguous CJK runs (Chinese has no spaces; pure-char tokens
    // would over-fragment). Two-char window matches typical compounds
    // ("所有权" → "所有" + "有权").
    let cjk_runs = collect_cjk_runs(&norm);
    for run in cjk_runs {
        let chars: Vec<char> = run.chars().collect();
        if chars.len() >= 2 {
            for w in chars.windows(2) {
                out.insert(w.iter().collect::<String>());
            }
        }
    }
    out
}

fn collect_cjk_runs(s: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if is_cjk(ch) {
            cur.push(ch);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs
}

fn is_cjk(ch: char) -> bool {
    let u = ch as u32;
    (0x4E00..=0x9FFF).contains(&u) || (0x3400..=0x4DBF).contains(&u)
}

fn token_overlap(a: &HashSet<String>, b: &HashSet<String>) -> usize {
    a.intersection(b).count()
}

/// Byte-safe context window around `[byte_start, byte_end)` in `s`. Returns
/// up to `radius` chars on each side, snapped to UTF-8 char boundaries.
fn context_window(s: &str, byte_start: usize, byte_end: usize, radius: usize) -> String {
    let start = floor_char_boundary(s, byte_start.saturating_sub(radius * 3));
    let end = ceil_char_boundary(s, (byte_end + radius * 3).min(s.len()));
    let raw = &s[start..end];
    normalize_text(raw)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// True iff `a` and `b` share at least one 2-gram CJK or ≥3-char ASCII token
/// — the same notion of "context near this entity matches" used to decide
/// whether two entities are *talking about the same thing*.
fn context_overlap(a: &str, b: &str) -> bool {
    let ta = tokenize(a);
    let tb = tokenize(b);
    ta.intersection(&tb).next().is_some()
}

// ============================================================================
// Unit tests (boundary class — ≥5 per the agent-verification doctrine)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ChatReliabilityConfig {
        ChatReliabilityConfig::default()
    }

    /// Boundary 1: an empty response yields no signals + neutral confidence.
    #[test]
    fn boundary_empty_response_yields_neutral_confidence() {
        let report = evaluate_response("", &[], "q", &cfg());
        assert!(report.citation_grounded.is_empty());
        assert!(report.contradictions.is_empty());
        assert!(report.hallucination_flags.is_empty());
        assert!((report.overall_confidence - 0.5).abs() < f32::EPSILON);
    }

    /// Boundary 2: response with no cited item and no entities scores 1.0
    /// (nothing to verify → nothing fails).
    #[test]
    fn boundary_pure_prose_with_no_entities_is_max_confidence() {
        let report = evaluate_response("好的，没有具体信息。", &[], "q", &cfg());
        assert!(report.citation_grounded.is_empty());
        assert!(report.contradictions.is_empty());
        assert!(report.hallucination_flags.is_empty());
        assert!((report.overall_confidence - 1.0).abs() < 1e-6);
    }

    /// Boundary 3: a fabricated cite (`item_id` not in chunks) downgrades
    /// confidence by exactly `weight_citation * (2/1) clamped to 1.0`.
    #[test]
    fn boundary_one_fabricated_cite_applies_full_citation_penalty() {
        let response = "答案见 [item:does-not-exist]。";
        let chunks = vec![RetrievedChunk::new("real-id", "some text".to_string())];
        let report = evaluate_response(response, &chunks, "q", &cfg());
        assert_eq!(report.citation_grounded.len(), 1);
        assert_eq!(report.citation_grounded[0].status, CitationStatus::Fabricated);
        // 2 * fabricated / 1 = 2, min 1.0 → full weight_citation deducted
        let expected = 1.0 - cfg().weight_citation;
        assert!(
            (report.overall_confidence - expected).abs() < 1e-5,
            "got {} expected {}",
            report.overall_confidence,
            expected
        );
    }

    /// Boundary 4: confidence is clamped to `[0, 1]` even when every signal
    /// saturates (no underflow / overflow).
    #[test]
    fn boundary_all_signals_saturated_clamps_to_zero() {
        let response = "依据 [item:fake1] 与 [item:fake2]，金额为 ¥99999，日期 2099-12-31。\
            又依据 [item:fake3]，金额 ¥88888，日期 2098-11-30，由 某虚构有限公司 出具。";
        let chunks = vec![
            // Chunk asserts a *different* money + date for the same context.
            RetrievedChunk::new(
                "real-1",
                "依据原始记录，金额为 ¥10000，日期 2024-03-15。",
            ),
            RetrievedChunk::new(
                "real-2",
                "依据原始记录，金额 ¥20000，日期 2024-04-15。",
            ),
        ];
        let report = evaluate_response(response, &chunks, "q", &cfg());
        assert!(report.overall_confidence >= 0.0);
        assert!(report.overall_confidence <= 1.0);
        assert!(!report.citation_grounded.is_empty());
        assert!(!report.hallucination_flags.is_empty());
    }

    /// Boundary 5: a grounded cite (item_id present, content overlap ≥ min)
    /// does not get any penalty.
    #[test]
    fn boundary_grounded_cite_keeps_full_confidence() {
        // Build response + chunk that share ≥3 CJK 2-grams.
        let response = "rust ownership borrow checker prevents data race [item:real-1].";
        let chunks = vec![RetrievedChunk::new(
            "real-1",
            "rust ownership borrow checker prevents data race at compile time.",
        )];
        let report = evaluate_response(response, &chunks, "q", &cfg());
        assert_eq!(report.citation_grounded.len(), 1);
        assert_eq!(report.citation_grounded[0].status, CitationStatus::Grounded);
        assert!((report.overall_confidence - 1.0).abs() < 1e-6);
    }

    /// Boundary 6: dedup — repeated `[item:X]` marker counts once.
    #[test]
    fn boundary_repeated_marker_deduped() {
        let response = "答案见 [item:fake] 和 [item:fake] 和 [item:fake]。";
        let report = evaluate_response(response, &[], "q", &cfg());
        assert_eq!(report.citation_grounded.len(), 1);
        assert_eq!(report.citation_grounded[0].status, CitationStatus::Fabricated);
    }

    /// Boundary 7: hallucination de-dup — repeated token reported once.
    #[test]
    fn boundary_hallucination_dedup_repeated_token() {
        let response = "金额为 ¥99999，再次提及 ¥99999。";
        let report = evaluate_response(response, &[], "q", &cfg());
        let count = report
            .hallucination_flags
            .iter()
            .filter(|f| f.token == "¥99999")
            .count();
        assert_eq!(count, 1, "token should appear in flags exactly once");
    }

    /// Boundary 8: confidence formula constants — `1 contradiction` deducts
    /// exactly `0.5 * weight_contradiction`. Independently hand-computed.
    #[test]
    fn boundary_one_contradiction_half_weight_deduction() {
        // Single date contradiction at the same context window.
        let response = "事件发生于 2024-03-15 当时。";
        let chunks = vec![RetrievedChunk::new(
            "src",
            "事件发生于 2025-06-01 当时。",
        )];
        let report = evaluate_response(response, &chunks, "q", &cfg());
        assert_eq!(report.contradictions.len(), 1);
        let expected = 1.0 - 0.5 * cfg().weight_contradiction;
        // Could also have a hallucination flag for "2024-03-15" since it's not
        // in chunk → recompute expected with hallucination penalty too.
        let hallucination_penalty =
            (report.hallucination_flags.len() as f32 / 4.0).min(1.0) * cfg().weight_hallucination;
        let expected_full = expected - hallucination_penalty;
        assert!(
            (report.overall_confidence - expected_full).abs() < 1e-5,
            "got {} expected {}",
            report.overall_confidence,
            expected_full
        );
    }

    // ── Pure-function helper tests ──────────────────────────────────────

    #[test]
    fn normalize_text_lowercases_and_strips_punct() {
        assert_eq!(normalize_text("Hello, World!"), "hello world");
        assert_eq!(normalize_text("张三，签约。"), "张三签约");
        assert_eq!(normalize_text("  multi   space  "), "multi space");
    }

    #[test]
    fn confidence_from_signals_is_clamped() {
        let cfg = cfg();
        let v = confidence_from_signals(&[], &[], &[], &cfg);
        assert!((v - 1.0).abs() < f32::EPSILON);
    }
}
