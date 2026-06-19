//! W4 — citation management (spec §2.1 W4, §5.2 `/cite`).
//!
//! **Tier 🆓 (zero LLM).** Given a set of sources (KB items the user picked + user-supplied
//! external refs), produce: (1) a formatted reference list in one of four academic styles, and
//! (2) inline anchors pinning a citation id to a `[start,end)` offset in the draft text. The
//! formatting is pure deterministic string work — no model is ever called. The hallucination red
//! line (spec §7) is structural here: a [`Citation`] can only be built from a real source the
//! caller supplies, so the engine cannot invent a bibliography.
//!
//! Styles: GB/T 7714 (Chinese national standard), APA, IEEE, MLA. New styles implement the same
//! formatting branch; the enum is the extension point (spec §6.2). All four are regex/format-only.

use serde::{Deserialize, Serialize};

/// A citation style. Stable serde kebab strings for the API (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiteStyle {
    /// GB/T 7714-2015 (Chinese national standard).
    Gbt7714,
    /// American Psychological Association.
    Apa,
    /// Institute of Electrical and Electronics Engineers.
    Ieee,
    /// Modern Language Association.
    Mla,
}

impl CiteStyle {
    /// Stable kebab id for the wire (matches the spec §5.2 `style` strings).
    pub fn as_str(self) -> &'static str {
        match self {
            CiteStyle::Gbt7714 => "gbt7714",
            CiteStyle::Apa => "apa",
            CiteStyle::Ieee => "ieee",
            CiteStyle::Mla => "mla",
        }
    }

    /// Parse a wire `style` string. `None` for unknown (route maps to `unsupported-cite-style`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gbt7714" | "gb/t7714" | "gb7714" => Some(CiteStyle::Gbt7714),
            "apa" => Some(CiteStyle::Apa),
            "ieee" => Some(CiteStyle::Ieee),
            "mla" => Some(CiteStyle::Mla),
            _ => None,
        }
    }

    /// All supported styles (for an error message listing valid options).
    pub fn all() -> [CiteStyle; 4] {
        [CiteStyle::Gbt7714, CiteStyle::Apa, CiteStyle::Ieee, CiteStyle::Mla]
    }
}

/// Bibliographic fields for one source. All optional except a stable `id` and `title` — a source
/// with no title at all cannot be cited (returns an error), preventing an empty/fake entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMeta {
    /// Stable id (KB item_id or a user-given external id). Used as the citation anchor key.
    pub id: String,
    /// Authors, one per entry (already in "Surname, Given" or full-name form as the user typed).
    #[serde(default)]
    pub authors: Vec<String>,
    /// Title (required — a citation with no title is rejected).
    #[serde(default)]
    pub title: String,
    /// Container / journal / publisher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Year (4-digit string; validated loosely).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    /// DOI or URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `true` ⇒ external (user-supplied) source; `false` ⇒ KB item. Recorded for grounding kind.
    #[serde(default)]
    pub external: bool,
}

/// A formatted citation entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    /// Anchor key (the source id) referenced by inline anchors.
    pub id: String,
    /// 1-based sequence number in the reference list (IEEE `[n]`, etc.).
    pub seq: u32,
    /// The fully formatted reference string in the requested style.
    pub formatted: String,
    /// The style this entry was formatted in.
    pub style: CiteStyle,
}

/// An inline citation marker pinning a [`Citation`] to a span of the draft text (UTF-16 offsets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineAnchor {
    /// Char offset `[start, end)` of the inline marker in the draft text (UTF-16 code units).
    pub offset: [u32; 2],
    /// The citation id this marker points at.
    pub citation_id: String,
}

/// Error from citation building (mapped to kebab codes at the route layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiteError {
    /// No sources supplied.
    NoSources,
    /// A source had no title (cannot form a non-empty citation — avoids fake/empty entries).
    MissingTitle(String),
}

impl CiteError {
    /// Stable kebab code.
    pub fn code(&self) -> &'static str {
        match self {
            CiteError::NoSources => "no-source-material",
            CiteError::MissingTitle(_) => "citation-missing-title",
        }
    }
}

impl std::fmt::Display for CiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiteError::NoSources => write!(f, "no sources to cite"),
            CiteError::MissingTitle(id) => write!(f, "source `{id}` has no title; cannot cite"),
        }
    }
}

impl std::error::Error for CiteError {}

fn join_authors(authors: &[String], sep: &str, max: usize, et_al: &str) -> String {
    let cleaned: Vec<&str> = authors.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).collect();
    if cleaned.is_empty() {
        return String::new();
    }
    if cleaned.len() > max {
        format!("{}{et_al}", cleaned[..max].join(sep))
    } else {
        cleaned.join(sep)
    }
}

fn year_or_nd(year: &Option<String>) -> String {
    year.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("n.d.").to_string()
}

/// Format one source in the given style (pure).
fn format_one(meta: &SourceMeta, style: CiteStyle) -> String {
    let title = meta.title.trim();
    let container = meta.container.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let url = meta.url.as_deref().map(str::trim).filter(|s| !s.is_empty());
    match style {
        // GB/T 7714: 作者. 题名[文献类型]. 出版项, 年. URL.
        CiteStyle::Gbt7714 => {
            let authors = join_authors(&meta.authors, ", ", 3, ", 等");
            let mut s = String::new();
            if !authors.is_empty() {
                s.push_str(&format!("{authors}. "));
            }
            s.push_str(title);
            s.push_str(". ");
            if let Some(c) = container {
                s.push_str(&format!("{c}, "));
            }
            s.push_str(&year_or_nd(&meta.year));
            s.push('.');
            if let Some(u) = url {
                s.push_str(&format!(" {u}."));
            }
            s
        }
        // APA: Author (Year). Title. Container. URL
        CiteStyle::Apa => {
            let authors = join_authors(&meta.authors, ", ", 20, ", et al.");
            let mut s = String::new();
            if !authors.is_empty() {
                s.push_str(&format!("{authors} "));
            }
            s.push_str(&format!("({}). ", year_or_nd(&meta.year)));
            s.push_str(title);
            s.push('.');
            if let Some(c) = container {
                s.push_str(&format!(" {c}."));
            }
            if let Some(u) = url {
                s.push_str(&format!(" {u}"));
            }
            s
        }
        // IEEE: Author, "Title," Container, Year. (the leading [n] is added by the seq wrapper)
        CiteStyle::Ieee => {
            let authors = join_authors(&meta.authors, ", ", 6, " et al.");
            let mut s = String::new();
            if !authors.is_empty() {
                s.push_str(&format!("{authors}, "));
            }
            s.push_str(&format!("\"{title},\""));
            if let Some(c) = container {
                s.push_str(&format!(" {c},"));
            }
            s.push_str(&format!(" {}.", year_or_nd(&meta.year)));
            if let Some(u) = url {
                s.push_str(&format!(" {u}"));
            }
            s
        }
        // MLA: Author. "Title." Container, Year, URL.
        CiteStyle::Mla => {
            let authors = join_authors(&meta.authors, ", ", 2, ", et al.");
            let mut s = String::new();
            if !authors.is_empty() {
                s.push_str(&format!("{authors}. "));
            }
            s.push_str(&format!("\"{title}.\""));
            if let Some(c) = container {
                s.push_str(&format!(" {c},"));
            }
            s.push_str(&format!(" {}", year_or_nd(&meta.year)));
            if let Some(u) = url {
                s.push_str(&format!(", {u}"));
            }
            s.push('.');
            s
        }
    }
}

/// Build the formatted reference list for `sources` in `style`.
///
/// Order is preserved (caller controls source order). Each citation gets a 1-based `seq`. IEEE
/// entries are prefixed with `[n] `. Errors: [`CiteError::NoSources`], [`CiteError::MissingTitle`].
pub fn build_citations(sources: &[SourceMeta], style: CiteStyle) -> Result<Vec<Citation>, CiteError> {
    if sources.is_empty() {
        return Err(CiteError::NoSources);
    }
    let mut out = Vec::with_capacity(sources.len());
    for (i, meta) in sources.iter().enumerate() {
        if meta.title.trim().is_empty() {
            return Err(CiteError::MissingTitle(meta.id.clone()));
        }
        let seq = (i + 1) as u32;
        let body = format_one(meta, style);
        let formatted = if style == CiteStyle::Ieee {
            format!("[{seq}] {body}")
        } else {
            body
        };
        out.push(Citation {
            id: meta.id.clone(),
            seq,
            formatted,
            style,
        });
    }
    Ok(out)
}

/// Compute inline anchors: locate each citation id's first inline-marker occurrence in `text`.
///
/// The expected inline marker is `[cite:<id>]`. Returns the UTF-16 `[start,end)` of every marker
/// found (a draft may reference a citation multiple times → multiple anchors). Markers whose id is
/// not in `valid_ids` are ignored (cannot anchor an unknown citation → no fabricated reference).
/// Deterministic, zero LLM.
pub fn find_inline_anchors(text: &str, valid_ids: &[String]) -> Vec<InlineAnchor> {
    let valid: std::collections::BTreeSet<&str> = valid_ids.iter().map(|s| s.as_str()).collect();
    let mut anchors = Vec::new();
    let bytes = text.as_bytes();
    let marker_open = b"[cite:";
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i..].starts_with(marker_open) {
            // Find the closing `]`.
            if let Some(rel) = text[i + marker_open.len()..].find(']') {
                let id = &text[i + marker_open.len()..i + marker_open.len() + rel];
                let end_byte = i + marker_open.len() + rel + 1; // include `]`
                if valid.contains(id) {
                    let start_u16 = utf16_len(&text[..i]);
                    let end_u16 = start_u16 + utf16_len(&text[i..end_byte]);
                    anchors.push(InlineAnchor {
                        offset: [start_u16, end_u16],
                        citation_id: id.to_string(),
                    });
                }
                i = end_byte;
                continue;
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        i += ch_len;
    }
    anchors
}

fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, authors: &[&str], title: &str, year: &str) -> SourceMeta {
        SourceMeta {
            id: id.into(),
            authors: authors.iter().map(|a| a.to_string()).collect(),
            title: title.into(),
            container: Some("Journal X".into()),
            year: Some(year.into()),
            url: Some("https://doi.org/10.1/x".into()),
            external: false,
        }
    }

    // ── style parsing ──

    #[test]
    fn style_roundtrip_strings() {
        for s in CiteStyle::all() {
            assert_eq!(CiteStyle::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn style_unknown_is_none() {
        assert!(CiteStyle::parse("chicago").is_none());
        assert!(CiteStyle::parse("").is_none());
    }

    // ── formatting (format-compliance, spec §9.2 = 1.00 deterministic) ──

    #[test]
    fn gbt7714_format_compliance() {
        let c = build_citations(&[meta("s1", &["张三", "李四"], "深度学习综述", "2023")], CiteStyle::Gbt7714).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].formatted.starts_with("张三, 李四. 深度学习综述. "));
        assert!(c[0].formatted.contains("2023."));
    }

    #[test]
    fn gbt7714_truncates_authors_with_deng() {
        let c = build_citations(
            &[meta("s1", &["A", "B", "C", "D", "E"], "T", "2020")],
            CiteStyle::Gbt7714,
        )
        .unwrap();
        assert!(c[0].formatted.contains("A, B, C, 等"), "got {}", c[0].formatted);
    }

    #[test]
    fn apa_format_has_year_in_parens() {
        let c = build_citations(&[meta("s1", &["Smith, J."], "A study", "2024")], CiteStyle::Apa).unwrap();
        assert!(c[0].formatted.contains("(2024)."));
        assert!(c[0].formatted.contains("A study."));
    }

    #[test]
    fn ieee_prefixes_bracket_seq_and_quotes_title() {
        let c = build_citations(
            &[meta("s1", &["A"], "Title One", "2021"), meta("s2", &["B"], "Title Two", "2022")],
            CiteStyle::Ieee,
        )
        .unwrap();
        assert!(c[0].formatted.starts_with("[1] "));
        assert!(c[1].formatted.starts_with("[2] "));
        assert!(c[0].formatted.contains("\"Title One,\""));
        assert_eq!(c[0].seq, 1);
        assert_eq!(c[1].seq, 2);
    }

    #[test]
    fn mla_format_quotes_title_with_period_inside() {
        let c = build_citations(&[meta("s1", &["Doe, John"], "On Things", "2019")], CiteStyle::Mla).unwrap();
        assert!(c[0].formatted.contains("\"On Things.\""));
    }

    #[test]
    fn missing_year_renders_nd() {
        let m = SourceMeta {
            id: "s1".into(),
            authors: vec!["A".into()],
            title: "T".into(),
            container: None,
            year: None,
            url: None,
            external: false,
        };
        let c = build_citations(&[m], CiteStyle::Apa).unwrap();
        assert!(c[0].formatted.contains("(n.d.)."));
    }

    // ── error / boundary ──

    #[test]
    fn no_sources_is_error() {
        assert_eq!(build_citations(&[], CiteStyle::Apa).unwrap_err(), CiteError::NoSources);
    }

    #[test]
    fn missing_title_is_rejected() {
        let m = SourceMeta {
            id: "bad".into(),
            title: "   ".into(),
            ..Default::default()
        };
        let err = build_citations(&[m], CiteStyle::Apa).unwrap_err();
        assert_eq!(err, CiteError::MissingTitle("bad".into()));
        assert_eq!(err.code(), "citation-missing-title");
    }

    #[test]
    fn no_authors_omits_author_segment() {
        let m = SourceMeta {
            id: "s1".into(),
            authors: vec![],
            title: "Anon Work".into(),
            year: Some("2020".into()),
            ..Default::default()
        };
        let c = build_citations(&[m], CiteStyle::Gbt7714).unwrap();
        assert!(c[0].formatted.starts_with("Anon Work."), "got {}", c[0].formatted);
    }

    // ── inline anchors ──

    #[test]
    fn inline_anchor_located_for_valid_id() {
        let text = "如所述[cite:s1]，这是结论。";
        let anchors = find_inline_anchors(text, &["s1".to_string()]);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].citation_id, "s1");
        // "如所述" = 3 CJK = 3 utf-16; marker "[cite:s1]" = 9 ascii.
        assert_eq!(anchors[0].offset, [3, 12]);
    }

    #[test]
    fn inline_anchor_ignores_unknown_id() {
        let text = "x[cite:ghost]y";
        let anchors = find_inline_anchors(text, &["s1".to_string()]);
        assert!(anchors.is_empty(), "unknown id must not anchor (no fabricated ref)");
    }

    #[test]
    fn inline_anchor_multiple_occurrences() {
        let text = "[cite:s1] ... [cite:s1]";
        let anchors = find_inline_anchors(text, &["s1".to_string()]);
        assert_eq!(anchors.len(), 2);
    }

    #[test]
    fn inline_anchor_emoji_offsets_safe() {
        let text = "😀[cite:s1]";
        let anchors = find_inline_anchors(text, &["s1".to_string()]);
        // 😀 = 2 utf-16 units → marker starts at offset 2.
        assert_eq!(anchors[0].offset[0], 2);
    }

    // ── property tests (spec §9.1 ≥3) ──
    use proptest::prelude::*;

    proptest! {
        // ① every built citation has a non-empty formatted string and 1-based contiguous seq.
        #[test]
        fn prop_seq_is_contiguous(n in 1usize..8) {
            let sources: Vec<SourceMeta> = (0..n)
                .map(|i| meta(&format!("s{i}"), &["A"], &format!("Title {i}"), "2020"))
                .collect();
            let c = build_citations(&sources, CiteStyle::Ieee).unwrap();
            for (i, cit) in c.iter().enumerate() {
                prop_assert_eq!(cit.seq, (i + 1) as u32);
                prop_assert!(!cit.formatted.is_empty());
            }
        }

        // ② inline anchor offsets are always within the text's utf-16 length and start<end.
        #[test]
        fn prop_anchor_offsets_in_bounds(prefix in "[a-z]{0,20}") {
            let text = format!("{prefix}[cite:s1]end");
            let total = utf16_len(&text);
            let anchors = find_inline_anchors(&text, &["s1".to_string()]);
            for a in anchors {
                prop_assert!(a.offset[0] < a.offset[1]);
                prop_assert!(a.offset[1] <= total);
            }
        }

        // ③ formatting is deterministic. Title must contain a non-space char (an all-space
        //    title is correctly rejected as MissingTitle — that path is covered by a unit test).
        #[test]
        fn prop_format_deterministic(title in "[a-zA-Z][a-zA-Z ]{0,29}") {
            let m = meta("s1", &["A"], &title, "2020");
            let a = build_citations(std::slice::from_ref(&m), CiteStyle::Apa).unwrap();
            let b = build_citations(std::slice::from_ref(&m), CiteStyle::Apa).unwrap();
            prop_assert_eq!(a, b);
        }
    }
}
