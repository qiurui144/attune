//! W6 — citation **normalization + consistency** (spec §2.3 W6, paired with [`super::cite`]).
//!
//! **Tier 🆓 (zero LLM).** [`super::cite`] can *format* a known set of [`SourceMeta`] into one of
//! four styles, but it cannot tell, given a draft that already contains a reference list, (a) what
//! style each existing entry is written in, or (b) whether the document mixes styles. This module
//! adds that detection layer and a normalization entry point that **reuses
//! [`super::cite::build_citations`]** — it never re-implements any per-style formatting.
//!
//! Citation format is a *rule* problem, not a semantic one, so — like [`crate::terminology`] — the
//! whole module is deterministic and its public signatures carry **no `LlmProvider`** (compile-time
//! zero-LLM guard, mirroring the terminology layer's §成本契约 stance).
//!
//! ## Two cooperating dimensions of one "consistency check"
//!
//! Terminology unification ([`crate::terminology`]) and citation-style unification (this module)
//! are the two deterministic axes of a single document-consistency capability. [`check_document`]
//! exposes them together (a [`ConsistencyReport`]) without forcing either to depend on the other —
//! callers that only want one axis still call the per-axis functions directly.

use super::cite::{build_citations, CiteError, CiteStyle, Citation, SourceMeta};
use crate::terminology::{collect_variants, TermCluster};
use serde::{Deserialize, Serialize};

/// A detected citation occurrence in raw text: the line, where it sits, and the style it most
/// resembles (`None` ⇒ recognizably a reference line but no style scored above the floor).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedCitation {
    /// 0-based line index in the input text.
    pub line: usize,
    /// The trimmed citation text on that line.
    pub text: String,
    /// Best-scoring style, or `None` if ambiguous / unrecognized.
    pub style: Option<CiteStyle>,
}

/// Result of a citation-consistency scan over a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiteConsistencyReport {
    /// Every detected citation line, in document order.
    pub citations: Vec<DetectedCitation>,
    /// The distinct styles seen (sorted, deduped). `> 1` ⇒ the document mixes styles.
    pub styles_seen: Vec<CiteStyle>,
    /// The dominant (most frequent) style, if any citation had a recognizable style. Ties broken
    /// by [`CiteStyle::all`] order (deterministic).
    pub dominant: Option<CiteStyle>,
}

impl CiteConsistencyReport {
    /// `true` ⇔ the document mixes ≥ 2 recognizable citation styles (the inconsistency the user
    /// should be warned about).
    pub fn is_inconsistent(&self) -> bool {
        self.styles_seen.len() > 1
    }
}

/// Combined deterministic consistency report: terminology variants + citation styles. The two
/// halves are independent; either may be empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsistencyReport {
    /// Inconsistent terminology clusters (`> 1` spelling of one term). See [`crate::terminology`].
    pub terms: Vec<TermCluster>,
    /// Citation-style consistency scan. See [`CiteConsistencyReport`].
    pub citations: CiteConsistencyReport,
}

impl ConsistencyReport {
    /// `true` ⇔ either axis flags an inconsistency.
    pub fn has_inconsistency(&self) -> bool {
        !self.terms.is_empty() || self.citations.is_inconsistent()
    }
}

/// Minimum score (out of the style's max) a style must reach to be claimed for a line. Below this
/// the line is "looks like a citation but style unclear" (`style: None`).
const STYLE_FLOOR: i32 = 2;

/// Heuristically classify one already-isolated citation string into a [`CiteStyle`].
///
/// Pure rule scoring — no model. Returns `None` when no style clears [`STYLE_FLOOR`] or there is a
/// tie at the top (ambiguous → don't guess). Signatures, per [`super::cite::format_one`] output:
///   - **IEEE**: leading `[n]` sequence bracket **and** a quoted title with the comma *inside* the
///     quote (`"Title,"`).
///   - **APA**: `(YYYY).` — a 4-digit year (or `n.d.`) parenthesized then a period, appearing early.
///   - **MLA**: a quoted title with the period *inside* the quote (`"Title."`) and **no** leading
///     `[n]`.
///   - **GB/T 7714**: CJK present, or the `, 等` et-al marker, with an unquoted title and a bare
///     trailing `YYYY.` — i.e. none of the quote/paren signatures above.
pub fn detect_style(citation: &str) -> Option<CiteStyle> {
    let s = citation.trim();
    if s.is_empty() {
        return None;
    }
    let has_leading_seq = leading_bracket_seq(s);
    let has_quoted_comma = s.contains("\","); // "Title," (IEEE)
    let has_quoted_period = s.contains(".\""); // "Title." (MLA)
    let has_paren_year = has_parenthesized_year(s);
    let has_cjk = s.chars().any(is_cjk);
    let has_deng = s.contains("等"); // GB/T 7714 et-al marker
    let bare_trailing_year = has_bare_trailing_year(s);

    let mut scores = [
        (CiteStyle::Ieee, 0),
        (CiteStyle::Apa, 0),
        (CiteStyle::Mla, 0),
        (CiteStyle::Gbt7714, 0),
    ];
    let add = |scores: &mut [(CiteStyle, i32); 4], style: CiteStyle, n: i32| {
        for entry in scores.iter_mut() {
            if entry.0 == style {
                entry.1 += n;
            }
        }
    };

    // IEEE: bracket seq is its strongest, near-unique marker.
    if has_leading_seq {
        add(&mut scores, CiteStyle::Ieee, 2);
    }
    if has_quoted_comma {
        add(&mut scores, CiteStyle::Ieee, 1);
    }

    // APA: parenthesized year is its signature.
    if has_paren_year {
        add(&mut scores, CiteStyle::Apa, 3);
    }

    // MLA: quoted title with period inside, but not a bracket-prefixed IEEE entry and not an APA
    // entry. A parenthesized year is exclusively APA's signature, so its presence rules MLA out —
    // an APA title may itself end `."` before the container, which must not steal it for MLA.
    if has_quoted_period && !has_leading_seq && !has_paren_year {
        add(&mut scores, CiteStyle::Mla, 3);
    }

    // GB/T 7714: CJK content or the `等` marker, with a bare trailing year and no quote/paren
    // signatures (those belong to the Latin styles above).
    if (has_cjk || has_deng)
        && !has_paren_year
        && !has_quoted_comma
        && !has_quoted_period
        && !has_leading_seq
    {
        add(&mut scores, CiteStyle::Gbt7714, 2);
        if bare_trailing_year {
            add(&mut scores, CiteStyle::Gbt7714, 1);
        }
    }

    // Pick the unique top scorer ≥ floor. Tie at the top → ambiguous → None.
    scores.sort_by_key(|s| std::cmp::Reverse(s.1));
    let (top_style, top_score) = scores[0];
    if top_score < STYLE_FLOOR {
        return None;
    }
    if scores[1].1 == top_score {
        return None; // tie ⇒ don't guess
    }
    Some(top_style)
}

/// `true` iff the string starts with an IEEE-style `[n]` sequence bracket (e.g. `[1] `).
fn leading_bracket_seq(s: &str) -> bool {
    let b = s.as_bytes();
    if b.first() != Some(&b'[') {
        return false;
    }
    let mut i = 1;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    i > 1 && b.get(i) == Some(&b']')
}

/// `true` iff a `(YYYY)` or `(n.d.)` group is present (APA's signature). Scans for `(` … `)` with
/// 4 ASCII digits or the literal `n.d.` inside.
fn has_parenthesized_year(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            if let Some(rel) = s[i + 1..].find(')') {
                let inner = s[i + 1..i + 1 + rel].trim();
                if inner == "n.d." || (inner.len() == 4 && inner.bytes().all(|c| c.is_ascii_digit())) {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// `true` iff the string ends with a bare `YYYY.` (4 digits then a period, optional trailing
/// whitespace) — GB/T 7714's tail. A parenthesized year does **not** count (that's APA).
fn has_bare_trailing_year(s: &str) -> bool {
    let t = s.trim_end();
    let t = t.strip_suffix('.').unwrap_or(t);
    let tail: Vec<char> = t.chars().rev().take(4).collect();
    if tail.len() != 4 || !tail.iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // The char before the 4 digits must not be another digit (avoid matching the middle of a number)
    // and must not be a ')' (that would be a parenthesized year tail).
    let before = t.chars().rev().nth(4);
    !matches!(before, Some(c) if c.is_ascii_digit() || c == ')')
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

/// `true` iff a line looks like a bibliographic reference at all (worth style-detecting). A line
/// qualifies if it has a recognizable style **or** carries at least two weak reference cues
/// (year-like token, a comma, a quote, or an `et al.`/`等` marker). Keeps prose lines out of the
/// report.
fn looks_like_reference(line: &str) -> bool {
    if detect_style(line).is_some() {
        return true;
    }
    let mut cues = 0;
    if line.chars().filter(|c| c.is_ascii_digit()).count() >= 4 {
        cues += 1;
    }
    if line.contains(',') || line.contains('，') {
        cues += 1;
    }
    if line.contains('"') || line.contains('"') || line.contains('"') {
        cues += 1;
    }
    if line.contains("et al.") || line.contains("等") {
        cues += 1;
    }
    cues >= 2
}

/// Scan a document and report which citation styles appear, flagging mixed-style documents.
///
/// Splits on newlines; each non-blank line that [`looks_like_reference`] is tested with
/// [`detect_style`]. Deterministic, zero LLM.
pub fn scan_citations(text: &str) -> CiteConsistencyReport {
    let mut citations = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || !looks_like_reference(line) {
            continue;
        }
        citations.push(DetectedCitation {
            line: idx,
            text: line.to_string(),
            style: detect_style(line),
        });
    }

    // Distinct styles seen, in CiteStyle::all order (deterministic).
    let mut styles_seen: Vec<CiteStyle> = Vec::new();
    for style in CiteStyle::all() {
        if citations.iter().any(|c| c.style == Some(style)) {
            styles_seen.push(style);
        }
    }

    // Dominant = most frequent recognized style; ties broken by all() order.
    let mut dominant = None;
    let mut best = 0usize;
    for style in CiteStyle::all() {
        let n = citations.iter().filter(|c| c.style == Some(style)).count();
        if n > best {
            best = n;
            dominant = Some(style);
        }
    }

    CiteConsistencyReport {
        citations,
        styles_seen,
        dominant,
    }
}

/// Normalize a known set of sources to `target` style by **delegating to
/// [`build_citations`]** — this is the single source of per-style formatting truth (no fork).
///
/// Use this when you hold the structured [`SourceMeta`] (e.g. the user picked KB items). For raw
/// text where only the rendered strings exist, use [`scan_citations`] to detect the current mix,
/// then re-render once you have structured metadata.
pub fn normalize_to(sources: &[SourceMeta], target: CiteStyle) -> Result<Vec<Citation>, CiteError> {
    build_citations(sources, target)
}

/// Run both deterministic consistency axes over one document.
///
/// `term_candidates` are the (term, count) pairs the caller already extracted (tokenizer / NER /
/// user glossary) — the same shape [`collect_variants`] takes. `text` is scanned for citation
/// style mixing. Neither axis calls an LLM.
pub fn check_document(text: &str, term_candidates: &[(String, usize)]) -> ConsistencyReport {
    ConsistencyReport {
        terms: collect_variants(term_candidates),
        citations: scan_citations(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writing::cite::build_citations;

    // Helper: render one source in a given style via the *real* formatter, so detection is tested
    // against genuine cite.rs output (not hand-rolled strings that might drift).
    fn rendered(style: CiteStyle, authors: &[&str], title: &str, year: &str) -> String {
        let m = SourceMeta {
            id: "s1".into(),
            authors: authors.iter().map(|a| a.to_string()).collect(),
            title: title.into(),
            container: Some("Journal X".into()),
            year: Some(year.into()),
            url: None,
            external: false,
        };
        build_citations(std::slice::from_ref(&m), style).unwrap()[0].formatted.clone()
    }

    // ─────────────── detect_style: golden (≥10, one per style + variants) ───────────────

    #[test]
    fn detect_ieee_from_real_output() {
        let s = rendered(CiteStyle::Ieee, &["A. Smith"], "On Things", "2021");
        assert_eq!(detect_style(&s), Some(CiteStyle::Ieee), "got {s}");
    }

    #[test]
    fn detect_apa_from_real_output() {
        let s = rendered(CiteStyle::Apa, &["Smith, J."], "A Study", "2024");
        assert_eq!(detect_style(&s), Some(CiteStyle::Apa), "got {s}");
    }

    #[test]
    fn detect_mla_from_real_output() {
        let s = rendered(CiteStyle::Mla, &["Doe, John"], "On Things", "2019");
        assert_eq!(detect_style(&s), Some(CiteStyle::Mla), "got {s}");
    }

    #[test]
    fn detect_gbt7714_from_real_output() {
        let s = rendered(CiteStyle::Gbt7714, &["张三", "李四"], "深度学习综述", "2023");
        assert_eq!(detect_style(&s), Some(CiteStyle::Gbt7714), "got {s}");
    }

    #[test]
    fn detect_ieee_multi_entry() {
        // [2] prefix is still IEEE.
        let s = "[2] B. Jones, \"Another Title,\" Journal Y, 2020.";
        assert_eq!(detect_style(s), Some(CiteStyle::Ieee));
    }

    #[test]
    fn detect_apa_nd_year() {
        // (n.d.) is APA's no-date form.
        let s = "Author A. (n.d.). Some work. Container.";
        assert_eq!(detect_style(s), Some(CiteStyle::Apa));
    }

    #[test]
    fn detect_gbt7714_with_deng() {
        let s = "张三, 李四, 王五, 等. 标题. 出版社, 2020.";
        assert_eq!(detect_style(s), Some(CiteStyle::Gbt7714));
    }

    #[test]
    fn detect_mla_no_container() {
        let s = "Lee, Ann. \"Quiet Things.\" 2018.";
        assert_eq!(detect_style(s), Some(CiteStyle::Mla));
    }

    #[test]
    fn detect_apa_beats_quote_when_paren_year_present() {
        // A line with both a paren year and quotes still resolves to APA (paren year weighted 3).
        let s = "Smith, J. (2022). \"Edge title.\" Journal.";
        assert_eq!(detect_style(s), Some(CiteStyle::Apa));
    }

    #[test]
    fn detect_all_styles_roundtrip_via_real_formatter() {
        // Every style's real output classifies back to itself.
        for style in CiteStyle::all() {
            let (authors, title): (&[&str], &str) = match style {
                CiteStyle::Gbt7714 => (&["李雷", "韩梅梅"], "中文标题"),
                _ => (&["Alpha B."], "English Title"),
            };
            let s = rendered(style, authors, title, "2020");
            assert_eq!(detect_style(&s), Some(style), "style {style:?} mis-detected: {s}");
        }
    }

    // ─────────────── boundary (≥5) ───────────────

    #[test]
    fn detect_empty_is_none() {
        assert_eq!(detect_style(""), None);
        assert_eq!(detect_style("   "), None);
    }

    #[test]
    fn detect_plain_prose_is_none() {
        assert_eq!(detect_style("This is just a sentence with no citation cues"), None);
    }

    #[test]
    fn detect_bare_title_only_is_none() {
        // No year, no quote signature, no CJK → nothing clears the floor.
        assert_eq!(detect_style("Some Title Without Anything"), None);
    }

    #[test]
    fn detect_year_only_number_not_misread_as_apa() {
        // A bare 4-digit year (no parens) must not score APA.
        let s = "Report 2020";
        assert_ne!(detect_style(s), Some(CiteStyle::Apa));
    }

    #[test]
    fn detect_leading_bracket_non_numeric_is_not_ieee_seq() {
        // [abc] is not a sequence bracket.
        assert!(!leading_bracket_seq("[abc] foo"));
        assert!(leading_bracket_seq("[12] foo"));
        assert!(!leading_bracket_seq("[] foo"));
    }

    // ─────────────── error / unusual inputs (≥3) ───────────────

    #[test]
    fn normalize_to_propagates_no_sources_error() {
        let err = normalize_to(&[], CiteStyle::Apa).unwrap_err();
        assert_eq!(err, CiteError::NoSources);
    }

    #[test]
    fn normalize_to_propagates_missing_title_error() {
        let m = SourceMeta { id: "x".into(), title: "  ".into(), ..Default::default() };
        let err = normalize_to(std::slice::from_ref(&m), CiteStyle::Ieee).unwrap_err();
        assert_eq!(err, CiteError::MissingTitle("x".into()));
    }

    #[test]
    fn scan_empty_text_is_clean() {
        let r = scan_citations("");
        assert!(r.citations.is_empty());
        assert!(r.styles_seen.is_empty());
        assert!(!r.is_inconsistent());
        assert_eq!(r.dominant, None);
    }

    // ─────────────── scan / consistency (happy + mixed) ───────────────

    #[test]
    fn scan_single_style_is_consistent() {
        let text = "\
[1] A. One, \"Title A,\" J, 2020.
[2] B. Two, \"Title B,\" J, 2021.";
        let r = scan_citations(text);
        assert_eq!(r.citations.len(), 2);
        assert_eq!(r.styles_seen, vec![CiteStyle::Ieee]);
        assert!(!r.is_inconsistent());
        assert_eq!(r.dominant, Some(CiteStyle::Ieee));
    }

    #[test]
    fn scan_mixed_styles_flagged() {
        // IEEE + APA in one document → inconsistent.
        let text = "\
[1] A. One, \"Title A,\" J, 2020.
Smith, J. (2021). Title B. Journal.";
        let r = scan_citations(text);
        assert!(r.is_inconsistent());
        assert_eq!(r.styles_seen.len(), 2);
        assert!(r.styles_seen.contains(&CiteStyle::Ieee));
        assert!(r.styles_seen.contains(&CiteStyle::Apa));
    }

    #[test]
    fn scan_skips_prose_lines() {
        let text = "\
This is an introduction paragraph that should be ignored.
[1] A. One, \"Title A,\" J, 2020.
Another prose line.";
        let r = scan_citations(text);
        assert_eq!(r.citations.len(), 1);
        assert_eq!(r.citations[0].line, 1);
    }

    #[test]
    fn scan_dominant_is_most_frequent() {
        let text = "\
[1] A. One, \"Title A,\" J, 2020.
[2] B. Two, \"Title B,\" J, 2021.
Smith, J. (2021). Title C. Journal.";
        let r = scan_citations(text);
        // 2 IEEE vs 1 APA → IEEE dominant.
        assert_eq!(r.dominant, Some(CiteStyle::Ieee));
        assert!(r.is_inconsistent());
    }

    // ─────────────── check_document (combined axes) ───────────────

    #[test]
    fn check_document_reports_both_axes() {
        let text = "[1] A. One, \"Title A,\" J, 2020.\nSmith, J. (2021). Title B. Journal.";
        let terms = vec![("OpenAI".to_string(), 3), ("openai".to_string(), 2)];
        let r = check_document(text, &terms);
        assert!(r.has_inconsistency());
        assert_eq!(r.terms.len(), 1); // one term cluster
        assert!(r.citations.is_inconsistent()); // mixed styles
    }

    #[test]
    fn check_document_clean_when_both_consistent() {
        let text = "[1] A. One, \"Title A,\" J, 2020.\n[2] B. Two, \"Title B,\" J, 2021.";
        let terms = vec![("Rust".to_string(), 5)]; // single spelling
        let r = check_document(text, &terms);
        assert!(!r.has_inconsistency());
    }

    // ─────────────── regression fixtures (each pins a fixed bug) ───────────────

    #[test]
    fn regression_apa_not_misdetected_as_mla() {
        // BUG: an APA entry whose title segment happened to end `."` before a container could be
        // grabbed by MLA's quote-period rule. APA's parenthesized-year weight must win.
        let s = "Smith, J. (2024). \"A study.\" Journal X.";
        assert_eq!(detect_style(s), Some(CiteStyle::Apa));
    }

    #[test]
    fn regression_ieee_bracket_not_classified_mla() {
        // BUG: `[1] ... "Title."` (period happened inside quotes) could match MLA; leading [n]
        // must keep it IEEE (MLA rule is gated on `!has_leading_seq`).
        let s = "[1] A. Smith, \"On Things.\" Journal Y, 2021.";
        assert_eq!(detect_style(s), Some(CiteStyle::Ieee));
    }

    #[test]
    fn regression_trailing_year_inside_parens_is_not_bare() {
        // has_bare_trailing_year must reject `(2020)` so it doesn't double-count APA as GB/T.
        assert!(!has_bare_trailing_year("Foo (2020)"));
        assert!(has_bare_trailing_year("Foo, 2020."));
        assert!(has_bare_trailing_year("Foo, 2020"));
    }

    // ─────────────── proptest (≥3) ───────────────
    use proptest::prelude::*;

    proptest! {
        // ① detect_style is deterministic (pure function, same input → same output).
        #[test]
        fn prop_detect_deterministic(s in ".{0,60}") {
            prop_assert_eq!(detect_style(&s), detect_style(&s));
        }

        // ② Any real IEEE-formatted entry (over arbitrary ASCII title/author) detects as IEEE.
        //    GT is independent: it is the *style we asked the formatter to produce*, not a call to
        //    detect_style.
        #[test]
        fn prop_ieee_roundtrips(
            title in "[A-Za-z][A-Za-z ]{0,20}",
            author in "[A-Z]\\. [A-Za-z]{1,10}",
        ) {
            let m = SourceMeta {
                id: "s".into(),
                authors: vec![author],
                title,
                container: Some("Jrnl".into()),
                year: Some("2020".into()),
                url: None,
                external: false,
            };
            let s = build_citations(std::slice::from_ref(&m), CiteStyle::Ieee).unwrap()[0].formatted.clone();
            prop_assert_eq!(detect_style(&s), Some(CiteStyle::Ieee), "{}", s);
        }

        // ③ scan_citations: styles_seen is always a subset of the per-line detected styles, and
        //    is_inconsistent ⇔ ≥2 distinct styles. (Invariant computed independently from the raw
        //    citations vec, not by re-calling scan internals.)
        #[test]
        fn prop_scan_styles_seen_invariant(
            n_ieee in 0usize..4,
            n_apa in 0usize..4,
        ) {
            let mut lines = Vec::new();
            for i in 0..n_ieee {
                lines.push(format!("[{}] A. B, \"T{},\" J, 2020.", i + 1, i));
            }
            for i in 0..n_apa {
                lines.push(format!("Auth, X. ({}). Work {}. Jr.", 2000 + i, i));
            }
            let text = lines.join("\n");
            let r = scan_citations(&text);

            // Independent recompute of distinct styles from the detected citations.
            let mut distinct: Vec<CiteStyle> = r.citations.iter().filter_map(|c| c.style).collect();
            distinct.sort_by_key(|s| s.as_str().to_string());
            distinct.dedup();

            prop_assert_eq!(r.styles_seen.len(), distinct.len());
            prop_assert_eq!(r.is_inconsistent(), distinct.len() > 1);
        }
    }
}
