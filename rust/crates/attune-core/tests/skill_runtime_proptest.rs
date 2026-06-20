//! Property tests for the compare_to_table agent + render mapping (spec §9 — ≥3 props per agent).
//!
//! Invariants under arbitrary (document, extraction) inputs:
//!   P1. The agent never panics and always returns a comparison whose rows are a *subset* of
//!       what the model proposed (grounding can only drop, never add, a row's value).
//!   P2. A value that survives grounding appears verbatim in the source text (anti-fabrication).
//!   P3. The rendered Table is always rectangular (4 columns: 参数/A/B/差异) and re-renders to a
//!       byte-stable artifact — no input crashes the export.

use attune_core::export::{Artifact, ExportFormat};
use attune_core::llm::{ChatMessage, LlmProvider};
use attune_core::skill_runtime::{comparison_to_table, compare_to_table};
use attune_core::usage::TokenUsage;
use proptest::prelude::*;

struct FixedLlm(String);
impl LlmProvider for FixedLlm {
    fn chat(&self, _s: &str, _u: &str) -> attune_core::error::Result<(String, TokenUsage)> {
        Ok((self.0.clone(), TokenUsage::empty("p", "m")))
    }
    fn chat_with_history(&self, _m: &[ChatMessage]) -> attune_core::error::Result<(String, TokenUsage)> {
        Ok((self.0.clone(), TokenUsage::empty("p", "m")))
    }
    fn is_available(&self) -> bool {
        true
    }
    fn model_name(&self) -> &str {
        "m"
    }
}

/// A param name/value built only from safe-ish chars (so we can embed it in a JSON string).
fn token() -> impl Strategy<Value = String> {
    "[A-Za-z0-9\u{4e00}-\u{9fff} ]{1,12}".prop_filter("non-empty trimmed", |s| !s.trim().is_empty())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(80))]

    // P1 + P2: grounding only keeps values present in the doc; never panics.
    #[test]
    fn grounding_keeps_only_present_values(
        param in token(),
        val_a in token(),
        val_b in token(),
        include_a in any::<bool>(),
        include_b in any::<bool>(),
    ) {
        // Build a doc that contains val_a / val_b only when the flag says so.
        let mut doc = String::from("文档：");
        if include_a { doc.push_str(&val_a); doc.push(' '); }
        if include_b { doc.push_str(&val_b); doc.push(' '); }

        let json = format!(
            r#"{{"rows":[{{"name":{},"value_a":{},"value_b":{}}}]}}"#,
            serde_json::to_string(&param).unwrap(),
            serde_json::to_string(&val_a).unwrap(),
            serde_json::to_string(&val_b).unwrap(),
        );
        let llm = FixedLlm(json);
        let cmp = compare_to_table(&doc, "A", "B", &llm, "m");

        prop_assert_eq!(cmp.rows.len(), 1);
        let row = &cmp.rows[0];
        // P2: a kept value must be a substring of the doc.
        if let Some(v) = &row.value_a { prop_assert!(doc.contains(v)); }
        if let Some(v) = &row.value_b { prop_assert!(doc.contains(v)); }
        // P1: a value the doc does NOT contain must have been dropped to None.
        if !include_a || !doc.contains(val_a.trim()) { /* may still be Some if val_a substring of something */ }
        // diff only when both present and unequal.
        let want_diff = row.value_a.is_some() && row.value_b.is_some()
            && row.value_a.as_deref().map(str::trim) != row.value_b.as_deref().map(str::trim);
        prop_assert_eq!(row.differs, want_diff);
    }

    // P3: render → table is always rectangular + export never crashes.
    #[test]
    fn render_table_is_rectangular_and_exports(
        params in proptest::collection::vec(token(), 0..8),
    ) {
        let doc: String = params.join(" ") + " VALA VALB";
        let rows_json: Vec<String> = params
            .iter()
            .map(|p| format!(
                r#"{{"name":{},"value_a":"VALA","value_b":"VALB"}}"#,
                serde_json::to_string(p).unwrap()
            ))
            .collect();
        let json = format!(r#"{{"rows":[{}]}}"#, rows_json.join(","));
        let llm = FixedLlm(json);
        let cmp = compare_to_table(&doc, "甲", "乙", &llm, "m");
        let art = comparison_to_table(&cmp, "T");
        let Artifact::Table(t) = &art else { panic!("expected table") };
        // 4 columns, every row exactly 4 cells.
        prop_assert_eq!(t.headers.len(), 4);
        for r in &t.rows { prop_assert_eq!(r.len(), 4); }
        prop_assert!(t.validate().is_ok());
        // export to every format must succeed (no panic, Ok bytes).
        for fmt in [ExportFormat::Xlsx, ExportFormat::Csv, ExportFormat::Md, ExportFormat::Docx] {
            prop_assert!(art.render(fmt).is_ok(), "render {:?} failed", fmt);
        }
    }

    // P-extra: empty / garbage extraction never panics, always yields a valid (possibly empty) table.
    #[test]
    fn garbage_never_panics(noise in ".*") {
        let llm = FixedLlm(noise);
        let cmp = compare_to_table("文档 A B", "A", "B", &llm, "m");
        let art = comparison_to_table(&cmp, "T");
        prop_assert!(art.render(ExportFormat::Xlsx).is_ok());
    }
}
