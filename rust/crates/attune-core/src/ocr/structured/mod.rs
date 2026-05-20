//! Office helper 结构化字段抽取框架 — 零 LLM, 全规则.
//!
//! 路径: 正则锚点 + bbox 邻近度 + 关键词字典 + 校验函数.
//! 每个 scene 一个 module, 输出 tagged union `StructuredFields::*V1`.
//!
//! Spec: docs/superpowers/specs/2026-05-20-office-helper-design.md §4

use super::{BBox, RawLine};
use serde::{Deserialize, Serialize};

pub mod normalize;
pub mod scene_card;
pub mod scene_document;
pub mod scene_id_card;
pub mod scene_receipt;
pub mod scene_table;

// ─── 公共字段类型 ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldValue {
    /// 抽出的字段值. None = 抽不出 (UI 应高亮提示手填); 绝不返编造 placeholder.
    pub value: Option<String>,
    /// 字段置信度 [0.0, 1.0]. < 0.6 → UI 高亮.
    pub confidence: f32,
    /// 来源 line 的 bbox (前端高亮用).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BBox>,
    /// 来源是第几行 (引用 lines 数组下标).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line_idx: Option<usize>,
}

impl FieldValue {
    pub fn none() -> Self {
        Self {
            value: None,
            confidence: 0.0,
            bbox: None,
            source_line_idx: None,
        }
    }

    pub fn from_line(value: String, confidence: f32, line: &RawLine, idx: usize) -> Self {
        Self {
            value: Some(value),
            confidence,
            bbox: Some(line.bbox),
            source_line_idx: Some(idx),
        }
    }
}

impl Default for FieldValue {
    fn default() -> Self {
        Self::none()
    }
}

// ─── tagged union schema (路径 Y per spec §4.4) ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "schema", rename_all = "snake_case")]
pub enum StructuredFields {
    DocumentV1 {
        fields: scene_document::DocumentFields,
        #[serde(default)]
        unrecognized_fields: Vec<String>,
        #[serde(default)]
        validation_warnings: Vec<String>,
    },
    ReceiptV1 {
        fields: scene_receipt::ReceiptFields,
        #[serde(default)]
        unrecognized_fields: Vec<String>,
        #[serde(default)]
        validation_warnings: Vec<String>,
    },
    TableV1 {
        fields: scene_table::TableFields,
        #[serde(default)]
        unrecognized_fields: Vec<String>,
        #[serde(default)]
        validation_warnings: Vec<String>,
    },
    CardV1 {
        fields: scene_card::CardFields,
        #[serde(default)]
        unrecognized_fields: Vec<String>,
        #[serde(default)]
        validation_warnings: Vec<String>,
    },
    IdCardCnV1 {
        fields: scene_id_card::IdCardCnFields,
        #[serde(default)]
        unrecognized_fields: Vec<String>,
        #[serde(default)]
        validation_warnings: Vec<String>,
    },
    BankCardV1 {
        fields: scene_id_card::BankCardFields,
        #[serde(default)]
        unrecognized_fields: Vec<String>,
        #[serde(default)]
        validation_warnings: Vec<String>,
    },
    BusinessLicenseV1 {
        fields: scene_id_card::BusinessLicenseFields,
        #[serde(default)]
        unrecognized_fields: Vec<String>,
        #[serde(default)]
        validation_warnings: Vec<String>,
    },
}

// ─── 入口路由 ───────────────────────────────────────────────────────────────

/// 按 profile id 路由到对应 scene extractor.
/// `None` = A 档场景 (screenshot / contract / ancient / form) 或未知 profile.
pub fn extract(
    profile: &str,
    lines: &[RawLine],
    id_card_subtype: Option<&str>,
) -> Option<StructuredFields> {
    match profile {
        "document" => Some(scene_document::extract(lines)),
        "receipt" => Some(scene_receipt::extract(lines)),
        "table" => Some(scene_table::extract(lines)),
        "card" => Some(scene_card::extract(lines)),
        "id_card" => scene_id_card::extract(lines, id_card_subtype?),
        _ => None,
    }
}

// ─── 公共辅助 — 锚点匹配 + bbox 邻近 ──────────────────────────────────────

/// 在锚点行之后找 value (同一行右侧 / 下一行).
///
/// 返回 `(matched_line_idx, value_text, ocr_confidence)`.
/// `max_offset` = 锚点找不到同行 value 时, 往下找几行 (典型 1-2).
pub fn find_value_after_anchor(
    lines: &[RawLine],
    anchor_re: &regex::Regex,
    max_offset: usize,
) -> Option<(usize, String, f32)> {
    for (i, l) in lines.iter().enumerate() {
        if let Some(m) = anchor_re.find(&l.text) {
            // 同行右侧 (剥 anchor 后的剩余 + 剥分隔符)
            let after = l.text[m.end()..].trim_start_matches(|c: char| {
                c == ':' || c == '：' || c == ' ' || c == '\t'
            });
            if !after.is_empty() {
                return Some((i, after.to_string(), l.confidence));
            }
            // 同行右侧空 → 找接下来 1..=max_offset 行的非空文本
            for off in 1..=max_offset {
                if let Some(next) = lines.get(i + off) {
                    let t = next.text.trim();
                    if !t.is_empty() {
                        return Some((i + off, t.to_string(), next.confidence));
                    }
                }
            }
        }
    }
    None
}

/// 在指定 line 同行右侧 (y 重叠 ≥ threshold) 找最近的下一段文本.
pub fn find_value_in_same_row(
    lines: &[RawLine],
    anchor_idx: usize,
    y_overlap_threshold: f32,
) -> Option<(usize, String, f32)> {
    let anchor = lines.get(anchor_idx)?;
    let a_y0 = anchor.bbox.y as f32;
    let a_y1 = a_y0 + anchor.bbox.h as f32;
    let a_x_end = anchor.bbox.x as f32 + anchor.bbox.w as f32;

    let mut best: Option<(usize, &RawLine, f32)> = None;
    for (i, l) in lines.iter().enumerate() {
        if i == anchor_idx {
            continue;
        }
        let y0 = l.bbox.y as f32;
        let y1 = y0 + l.bbox.h as f32;
        // y overlap ratio = intersection / min(h)
        let overlap = (a_y1.min(y1) - a_y0.max(y0)).max(0.0);
        let min_h = (a_y1 - a_y0).min(y1 - y0).max(1.0);
        let ratio = overlap / min_h;
        if ratio < y_overlap_threshold {
            continue;
        }
        let dx = (l.bbox.x as f32) - a_x_end;
        if dx < 0.0 {
            continue; // 必须在 anchor 右侧
        }
        match &best {
            None => best = Some((i, l, dx)),
            Some((_, _, cur_dx)) if dx < *cur_dx => best = Some((i, l, dx)),
            _ => {}
        }
    }
    best.map(|(i, l, _)| (i, l.text.trim().to_string(), l.confidence))
}

#[cfg(test)]
mod framework_tests {
    use super::*;
    use crate::ocr::{BBox, RawLine};

    fn rl(text: &str, x: u32, y: u32, w: u32, h: u32, conf: f32) -> RawLine {
        RawLine {
            text: text.into(),
            bbox: BBox { x, y, w, h },
            confidence: conf,
        }
    }

    #[test]
    fn field_value_none_has_zero_confidence() {
        let f = FieldValue::none();
        assert!(f.value.is_none());
        assert_eq!(f.confidence, 0.0);
    }

    #[test]
    fn anchor_finds_same_line_value() {
        let lines = vec![rl("发票号码: 12345678", 0, 0, 200, 30, 0.95)];
        let re = regex::Regex::new(r"发票号码").unwrap();
        let (idx, val, conf) = find_value_after_anchor(&lines, &re, 1).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(val, "12345678");
        assert_eq!(conf, 0.95);
    }

    #[test]
    fn anchor_falls_through_to_next_line() {
        let lines = vec![
            rl("发票号码:", 0, 0, 100, 30, 0.95),
            rl("12345678", 0, 32, 100, 30, 0.92),
        ];
        let re = regex::Regex::new(r"发票号码").unwrap();
        let (idx, val, _) = find_value_after_anchor(&lines, &re, 1).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(val, "12345678");
    }

    #[test]
    fn anchor_not_found_returns_none() {
        let lines = vec![rl("hello world", 0, 0, 100, 30, 0.9)];
        let re = regex::Regex::new(r"发票号码").unwrap();
        assert!(find_value_after_anchor(&lines, &re, 1).is_none());
    }

    #[test]
    fn same_row_picks_right_neighbor() {
        let lines = vec![
            rl("姓名", 0, 0, 50, 30, 0.95),
            rl("张三", 60, 2, 60, 28, 0.93),
            rl("身份证", 0, 50, 60, 30, 0.95),
        ];
        let (idx, val, _) = find_value_in_same_row(&lines, 0, 0.5).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(val, "张三");
    }

    #[test]
    fn extract_unknown_profile_returns_none() {
        assert!(extract("screenshot", &[], None).is_none());
        assert!(extract("totally-unknown", &[], None).is_none());
    }

    #[test]
    fn extract_id_card_without_subtype_returns_none() {
        assert!(extract("id_card", &[], None).is_none());
    }
}
