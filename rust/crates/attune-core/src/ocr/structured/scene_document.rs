//! `document_v1` — 标准文档结构化抽取 (段落聚类 + 双栏检测 + block 类型).
//!
//! Spec §4.2 document_v1: D2.6 实施细节; D2.1 桩.

use super::{FieldValue, StructuredFields};
use crate::ocr::{BBox, RawLine};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Title,
    Paragraph,
    List,
    FigureCaption,
    Footer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockItem {
    #[serde(rename = "type")]
    pub kind: BlockKind,
    pub text: String,
    pub bbox: BBox,
    /// reading order (0-based)
    pub order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<FieldValue>,
    /// reading-order-sorted blocks
    pub blocks: Vec<BlockItem>,
}

/// D2.1 stub — A-档兜底 (lines → 单一 paragraph block, 无 title 推断).
/// D2.6 替换成真正的段落聚类 + 双栏检测 + 启发式分类.
pub fn extract(lines: &[RawLine]) -> StructuredFields {
    let blocks: Vec<BlockItem> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| BlockItem {
            kind: BlockKind::Paragraph,
            text: l.text.clone(),
            bbox: l.bbox,
            order: i as u32,
        })
        .collect();

    StructuredFields::DocumentV1 {
        fields: DocumentFields {
            title: None,
            blocks,
        },
        unrecognized_fields: if lines.is_empty() {
            vec!["title".into(), "blocks".into()]
        } else {
            vec!["title".into()] // D2.1 stub never推断 title
        },
        validation_warnings: vec![],
    }
}
