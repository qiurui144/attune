//! `table_v1` — 通用表格还原 (cells 2D + 表头检测).
//!
//! Spec §4.2: headers / rows / row_count / column_count.
//! 算法: y 聚类成逻辑行 + x k-means 列对齐. D2.3 实施.

use super::{FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TableFields {
    pub headers: FieldValue,     // JSON array string e.g. r#"["col1","col2"]"#
    pub rows: FieldValue,        // JSON 2D array string e.g. r#"[["a","b"],["c","d"]]"#
    pub row_count: FieldValue,   // integer-as-string
    pub column_count: FieldValue,
}

/// D2.1 stub — 返空表格. D2.3 实施真正的 cells 还原.
pub fn extract(_lines: &[RawLine]) -> StructuredFields {
    StructuredFields::TableV1 {
        fields: TableFields::default(),
        unrecognized_fields: vec!["table_structure".into()],
        validation_warnings: vec![],
    }
}
