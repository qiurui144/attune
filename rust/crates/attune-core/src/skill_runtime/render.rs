//! Render step impl — map a step's structured output into the export [`Artifact`] IR.
//!
//! The render step is the bridge between "what an agent produced" and "what the export engine
//! writes": it is **zero-cost** (pure CPU, no LLM). For `compare_to_table` it turns a
//! [`ParamComparison`] into a [`Table`] with columns 参数 / A / B / 差异.

use crate::export::{Artifact, Table};
use crate::skill_runtime::compare_to_table::ParamComparison;

/// The header label for the "differs?" column.
const DIFF_YES: &str = "✓";
const DIFF_NO: &str = "";
const ABSENT: &str = "（未提供）";

/// Build a comparison [`Table`] Artifact from a [`ParamComparison`].
///
/// Columns: `参数` | `<entity_a>` | `<entity_b>` | `差异`. One row per parameter. An absent
/// value renders as "（未提供）" (never a fabricated number — grounding guard already nulled
/// ungrounded values). `title` becomes the sheet name / caption.
pub fn comparison_to_table(cmp: &ParamComparison, title: &str) -> Artifact {
    let headers = vec![
        "参数".to_string(),
        cmp.entity_a.clone(),
        cmp.entity_b.clone(),
        "差异".to_string(),
    ];
    let rows: Vec<Vec<String>> = cmp
        .rows
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                r.value_a.clone().unwrap_or_else(|| ABSENT.to_string()),
                r.value_b.clone().unwrap_or_else(|| ABSENT.to_string()),
                if r.differs { DIFF_YES } else { DIFF_NO }.to_string(),
            ]
        })
        .collect();
    Artifact::table(Table {
        title: Some(title.to_string()),
        headers,
        rows,
        aligns: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_intelligence::token_bill::TokenBill;
    use crate::skill_runtime::compare_to_table::ParamRow;

    fn cmp() -> ParamComparison {
        ParamComparison {
            entity_a: "设备 A".into(),
            entity_b: "设备 B".into(),
            rows: vec![
                ParamRow {
                    name: "分辨率".into(),
                    value_a: Some("1080p".into()),
                    value_b: Some("4K".into()),
                    differs: true,
                    ungrounded: false,
                },
                ParamRow {
                    name: "重量".into(),
                    value_a: Some("2kg".into()),
                    value_b: Some("2kg".into()),
                    differs: false,
                    ungrounded: false,
                },
                ParamRow {
                    name: "价格".into(),
                    value_a: None,
                    value_b: Some("999".into()),
                    differs: false,
                    ungrounded: false,
                },
            ],
            token_bill: TokenBill::default(),
            warnings: vec![],
        }
    }

    #[test]
    fn maps_columns_and_rows() {
        let art = comparison_to_table(&cmp(), "设备参数比对");
        let Artifact::Table(t) = art else {
            panic!("expected table")
        };
        assert_eq!(t.title.as_deref(), Some("设备参数比对"));
        assert_eq!(t.headers, vec!["参数", "设备 A", "设备 B", "差异"]);
        assert_eq!(t.rows.len(), 3);
        // diff row: 分辨率 1080p / 4K / ✓
        assert_eq!(t.rows[0], vec!["分辨率", "1080p", "4K", "✓"]);
        // same row: 重量 → no diff mark
        assert_eq!(t.rows[1][3], "");
        // absent A value → （未提供）, never invented
        assert_eq!(t.rows[2][1], "（未提供）");
        // the table validates (rectangular)
        assert!(t.validate().is_ok());
    }

    #[test]
    fn empty_comparison_is_headers_only_table() {
        let mut c = cmp();
        c.rows.clear();
        let art = comparison_to_table(&c, "空表");
        let Artifact::Table(t) = art else { panic!() };
        assert!(t.rows.is_empty());
        assert!(t.validate().is_ok(), "headers-only table is valid");
    }
}
