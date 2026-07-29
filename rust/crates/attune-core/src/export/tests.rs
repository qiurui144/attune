//! Export engine tests — §6.1 six-category matrix. The **round-trip accuracy**
//! suite (the highest-value, spec §9.1) lives in `tests/export_roundtrip.rs`
//! (integration) because it pulls in `calamine` / `pdf-extract` re-parsers; this
//! inline module covers IR invariants, boundary, error, and the cost-contract
//! anti-escalation proptest.

use super::*;

fn sample_table() -> Table {
    Table {
        title: Some("设备参数差异".into()),
        headers: vec!["参数".into(), "设备A".into(), "设备B".into()],
        rows: vec![
            vec!["电压".into(), "220V".into(), "110V".into()],
            vec!["功率".into(), "1000W".into(), "1500W".into()],
        ],
        aligns: vec![Align::Left, Align::Right, Align::Right],
    }
}

// ───────────────────────── boundary (≥5 #[test]) ────────────────────────────

#[test]
fn empty_rows_table_renders_all_formats() {
    let t = Table {
        title: None,
        headers: vec!["a".into(), "b".into()],
        rows: vec![],
        aligns: vec![],
    };
    let art = Artifact::table(t);
    for f in [
        ExportFormat::Md,
        ExportFormat::Csv,
        ExportFormat::Xlsx,
        ExportFormat::Docx,
        ExportFormat::Pdf,
    ] {
        let bytes = art.render(f).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        assert!(!bytes.is_empty(), "{f:?} produced empty output");
    }
}

#[test]
fn unicode_emoji_cells() {
    let t = Table {
        title: Some("emoji 😀 表".into()),
        headers: vec!["名字".into(), "符号".into()],
        rows: vec![
            vec!["心".into(), "❤️".into()],
            vec!["火".into(), "🔥".into()],
        ],
        aligns: vec![],
    };
    let md = Artifact::table(t).render(ExportFormat::Md).unwrap();
    let s = String::from_utf8(md).unwrap();
    assert!(s.contains('心') && s.contains("🔥"));
}

#[test]
fn special_chars_md_escaped() {
    let t = Table {
        title: None,
        headers: vec!["pipe|col".into()],
        rows: vec![vec!["a|b".into()]],
        aligns: vec![],
    };
    let s = String::from_utf8(Artifact::table(t).render(ExportFormat::Md).unwrap()).unwrap();
    // the literal pipe inside a cell must be escaped so it doesn't break the grid
    assert!(s.contains("\\|"));
}

#[test]
fn very_wide_and_tall_table() {
    let headers: Vec<String> = (0..40).map(|i| format!("col{i}")).collect();
    let rows: Vec<Vec<String>> = (0..200)
        .map(|r| (0..40).map(|c| format!("{r}-{c}")).collect())
        .collect();
    let t = Table {
        title: Some("big".into()),
        headers,
        rows,
        aligns: vec![],
    };
    let bytes = Artifact::table(t).render(ExportFormat::Xlsx).unwrap();
    assert!(bytes.len() > 1000);
}

#[test]
fn deep_heading_levels_clamped() {
    let d = Document {
        title: Some("t".into()),
        blocks: vec![
            Block::Heading {
                level: 9,
                text: "deep".into(),
            },
            Block::Heading {
                level: 0,
                text: "zero".into(),
            },
        ],
    };
    // must not panic and must render
    let s = String::from_utf8(Artifact::document(d).render(ExportFormat::Md).unwrap()).unwrap();
    assert!(s.contains("deep") && s.contains("zero"));
}

// ───────────────────────── error / exception (≥3) ───────────────────────────

#[test]
fn ragged_rows_rejected() {
    let t = Table {
        title: None,
        headers: vec!["a".into(), "b".into()],
        rows: vec![vec!["only-one".into()]],
        aligns: vec![],
    };
    let err = Artifact::table(t).render(ExportFormat::Csv).unwrap_err();
    assert_eq!(err.code(), "malformed-ir");
}

#[test]
fn zero_column_table_rejected() {
    let t = Table {
        title: None,
        headers: vec![],
        rows: vec![],
        aligns: vec![],
    };
    let err = Artifact::table(t).validate().unwrap_err();
    assert_eq!(err.code(), "malformed-ir");
}

#[test]
fn document_multi_table_to_csv_rejected() {
    let d = Document {
        title: None,
        blocks: vec![Block::Table(sample_table()), Block::Table(sample_table())],
    };
    let err = Artifact::document(d).render(ExportFormat::Csv).unwrap_err();
    assert_eq!(err.code(), "unsupported-artifact");
}

#[test]
fn document_no_table_to_xlsx_rejected() {
    let d = Document {
        title: None,
        blocks: vec![Block::Paragraph { text: "x".into() }],
    };
    let err = Artifact::document(d)
        .render(ExportFormat::Xlsx)
        .unwrap_err();
    assert_eq!(err.code(), "unsupported-artifact");
}

#[test]
fn bad_aligns_length_rejected() {
    let t = Table {
        title: None,
        headers: vec!["a".into(), "b".into()],
        rows: vec![],
        aligns: vec![Align::Left], // wrong length
    };
    assert_eq!(t.validate().unwrap_err().code(), "malformed-ir");
}

// ───────────────────────── format / serde basics ────────────────────────────

#[test]
fn format_parse_and_meta() {
    assert_eq!(ExportFormat::parse("XLSX"), Some(ExportFormat::Xlsx));
    assert_eq!(ExportFormat::parse("markdown"), Some(ExportFormat::Md));
    assert_eq!(ExportFormat::parse("word"), Some(ExportFormat::Docx));
    assert_eq!(ExportFormat::parse("nope"), None);
    assert_eq!(ExportFormat::Pdf.extension(), "pdf");
    assert_eq!(ExportFormat::Csv.mime(), "text/csv; charset=utf-8");
}

#[test]
fn ir_json_round_trips() {
    let art = Artifact::table(sample_table());
    let json = serde_json::to_string(&art).unwrap();
    let back: Artifact = serde_json::from_str(&json).unwrap();
    assert_eq!(art, back);
}

// ───────────────────────── proptest (≥3) ────────────────────────────────────

mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn arb_cell() -> impl Strategy<Value = String> {
        // include formula-injection prefixes + CJK + control-ish chars
        prop_oneof![
            "[\\PC]{0,12}",
            Just("=1+1".to_string()),
            Just("@cmd".to_string()),
            Just("-x".to_string()),
            Just("中文单元格".to_string()),
            Just("a|b".to_string()),
        ]
    }

    fn arb_table() -> impl Strategy<Value = Table> {
        (1usize..5, 0usize..6).prop_flat_map(|(ncols, nrows)| {
            let headers = prop::collection::vec(arb_cell(), ncols..=ncols);
            let rows = prop::collection::vec(
                prop::collection::vec(arb_cell(), ncols..=ncols),
                nrows..=nrows,
            );
            (headers, rows).prop_map(|(headers, rows)| Table {
                title: None,
                headers,
                rows,
                aligns: vec![],
            })
        })
    }

    proptest! {
        /// Any valid table renders to every format without panicking and yields
        /// non-empty bytes.
        #[test]
        fn any_table_renders_all_formats(t in arb_table()) {
            let art = Artifact::table(t);
            for f in [ExportFormat::Md, ExportFormat::Csv, ExportFormat::Xlsx, ExportFormat::Docx, ExportFormat::Pdf] {
                let bytes = art.render(f).expect("render");
                prop_assert!(!bytes.is_empty());
            }
        }

        /// IR survives JSON serialization round-trip for arbitrary tables.
        #[test]
        fn ir_json_proptest(t in arb_table()) {
            let art = Artifact::table(t);
            let json = serde_json::to_string(&art).unwrap();
            let back: Artifact = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(art, back);
        }

        /// ⭐ Cost-contract anti-escalation guard (CLAUDE.md §成本契约): every
        /// export format is and stays Free for any artifact. Combined with the
        /// fact that this module never imports `LlmProvider` (compile-time guard),
        /// export can never silently become a paid operation.
        #[test]
        fn export_is_always_free(t in arb_table()) {
            let _art = Artifact::table(t);
            for f in [ExportFormat::Md, ExportFormat::Csv, ExportFormat::Xlsx, ExportFormat::Docx, ExportFormat::Pdf] {
                prop_assert_eq!(f.cost_tier(), CostTier::Free);
            }
        }
    }
}
