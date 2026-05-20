//! `document_v1` — 标准文档结构化抽取 (段落聚类 + 双栏检测 + block 类型).
//!
//! Spec §4.2 document_v1:
//!   - 输出: title + blocks[{type, text, bbox, order}]
//!   - block 类型: title / paragraph / list / figure_caption / footer
//!   - 双栏检测: x 直方图两个显著峰值 → 左/右列, 按 left top→bottom → right top→bottom 排序
//!   - title: 字号最大的前 1 个 block (h > median × 1.4)
//!   - footer: y > page * 90% + 含"第 X 页"/页码
//!   - figure_caption: 含 "图 N"/"Figure N"/"Table N"
//!   - 准确度红线: 字符级 ≥ 92% (OCR 引擎本身, 不计 block order)

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
    /// median per-line height inside this block — font-size signal for title detection.
    #[serde(default)]
    pub font_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<FieldValue>,
    /// reading-order-sorted blocks
    pub blocks: Vec<BlockItem>,
}

/// 主入口.
pub fn extract(lines: &[RawLine]) -> StructuredFields {
    if lines.is_empty() {
        return StructuredFields::DocumentV1 {
            fields: DocumentFields::default(),
            unrecognized_fields: vec!["title".into(), "blocks".into()],
            validation_warnings: vec![],
        };
    }

    // ─── Step 1: 段落聚类 (y 邻近合并行间 < 1.5 × 字高) ────────────
    let paragraphs = cluster_paragraphs(lines);

    // ─── Step 2: 双栏检测 ─────────────────────────────────────────
    let column_split = detect_two_column_split(lines);

    // ─── Step 3: 给段落分配 order (双栏走左列→右列) ─────────────
    let mut blocks: Vec<BlockItem> = paragraphs
        .into_iter()
        .map(|p| {
            let kind = classify_block(&p, lines);
            BlockItem {
                kind,
                text: p.text,
                bbox: p.bbox,
                order: 0, // 临时, 后面 reorder 时填
                font_size: p.median_line_h,
            }
        })
        .collect();

    if let Some(split_x) = column_split {
        // 双栏: 先左列 top→bottom, 再右列 top→bottom
        blocks.sort_by_key(|b| {
            let center_x = b.bbox.x + b.bbox.w / 2;
            let col = if center_x < split_x { 0 } else { 1 };
            (col, b.bbox.y)
        });
    } else {
        // 单栏: top→bottom
        blocks.sort_by_key(|b| b.bbox.y);
    }
    for (i, b) in blocks.iter_mut().enumerate() {
        b.order = i as u32;
    }

    // ─── Step 4: title 选择 (字号最大 + 在前 3 个 block 内) ───────
    let title_field = detect_title(&blocks);
    let title_unrecognized = title_field.is_none();

    let fields = DocumentFields {
        title: title_field,
        blocks,
    };

    let mut unrecognized: Vec<String> = Vec::new();
    if title_unrecognized {
        unrecognized.push("title".into());
    }

    StructuredFields::DocumentV1 {
        fields,
        unrecognized_fields: unrecognized,
        validation_warnings: vec![],
    }
}

#[derive(Clone)]
struct Paragraph {
    text: String,
    bbox: BBox,
    /// median individual-line height (font size signal — independent of line count)
    median_line_h: u32,
}

/// y 邻近段落聚类. 行间距 < 1.5 × 平均字高 → 同段.
fn cluster_paragraphs(lines: &[RawLine]) -> Vec<Paragraph> {
    if lines.is_empty() {
        return vec![];
    }
    let mut sorted: Vec<&RawLine> = lines.iter().collect();
    sorted.sort_by_key(|l| (l.bbox.y, l.bbox.x));

    let mut paragraphs: Vec<Vec<&RawLine>> = Vec::new();
    let mut current: Vec<&RawLine> = Vec::new();

    for l in sorted.iter() {
        if let Some(last) = current.last() {
            let last_y_end = last.bbox.y + last.bbox.h;
            // 用较小的字高做 gap 阈值 baseline (避免大字号 title 跨过本应分段的小 gap)
            let line_height = last.bbox.h.min(l.bbox.h);
            let gap = l.bbox.y.saturating_sub(last_y_end);
            let same_para = (gap as f32) < 1.5 * line_height as f32;
            // 字号差异显著 (max/min > 1.4) → 强制分段 (title vs body 边界)
            let font_size_jump = {
                let max_h = last.bbox.h.max(l.bbox.h) as f32;
                let min_h = last.bbox.h.min(l.bbox.h) as f32;
                max_h / min_h.max(1.0) > 1.4
            };
            // 段落必须在 x 范围内大致重叠 (避免双栏的两列被合并成一段)
            let x_overlap_ok = {
                let l0 = last.bbox.x;
                let l1 = last.bbox.x + last.bbox.w;
                let c0 = l.bbox.x;
                let c1 = l.bbox.x + l.bbox.w;
                let inter = l1.min(c1).saturating_sub(l0.max(c0));
                let min_w = (l1 - l0).min(c1 - c0).max(1);
                (inter as f32 / min_w as f32) > 0.3
            };
            if same_para && x_overlap_ok && !font_size_jump {
                current.push(*l);
                continue;
            } else {
                if !current.is_empty() {
                    paragraphs.push(current.clone());
                }
                current.clear();
            }
        }
        current.push(*l);
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }

    paragraphs
        .into_iter()
        .map(|p| {
            let text = p
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let min_x = p.iter().map(|l| l.bbox.x).min().unwrap_or(0);
            let min_y = p.iter().map(|l| l.bbox.y).min().unwrap_or(0);
            let max_x = p
                .iter()
                .map(|l| l.bbox.x + l.bbox.w)
                .max()
                .unwrap_or(0);
            let max_y = p
                .iter()
                .map(|l| l.bbox.y + l.bbox.h)
                .max()
                .unwrap_or(0);
            let bbox = BBox {
                x: min_x,
                y: min_y,
                w: max_x - min_x,
                h: max_y - min_y,
            };
            let mut hs: Vec<u32> = p.iter().map(|l| l.bbox.h).collect();
            hs.sort_unstable();
            let median_line_h = hs[hs.len() / 2];
            Paragraph {
                text,
                bbox,
                median_line_h,
            }
        })
        .collect()
}

/// 双栏检测. 收集所有行 x_center 直方图, 若两峰显著 → 返回分界 x.
/// 简化版: 用 [min_x, max_x] 中点 m, 统计 < m 和 ≥ m 的行数; 两侧都 ≥ 30% 总数 → 双栏.
fn detect_two_column_split(lines: &[RawLine]) -> Option<u32> {
    if lines.len() < 6 {
        return None;
    }
    let min_x = lines.iter().map(|l| l.bbox.x).min().unwrap_or(0);
    let max_x = lines
        .iter()
        .map(|l| l.bbox.x + l.bbox.w)
        .max()
        .unwrap_or(0);
    if max_x <= min_x {
        return None;
    }
    let mid_x = (min_x + max_x) / 2;
    let mut left = 0;
    let mut right = 0;
    for l in lines {
        let cx = l.bbox.x + l.bbox.w / 2;
        if cx < mid_x {
            left += 1;
        } else {
            right += 1;
        }
    }
    let total = lines.len();
    let left_ratio = left as f32 / total as f32;
    let right_ratio = right as f32 / total as f32;
    // 两侧都 ≥ 30% + 左侧 max_x_end 不跨越右侧 min_x → 视为双栏
    if left_ratio >= 0.3 && right_ratio >= 0.3 {
        // 进一步验证: 左侧 lines 的 max(x+w) 应 ≤ 右侧 lines 的 min(x) + 小容忍
        let left_max_end = lines
            .iter()
            .filter(|l| (l.bbox.x + l.bbox.w / 2) < mid_x)
            .map(|l| l.bbox.x + l.bbox.w)
            .max()
            .unwrap_or(0);

        let right_min_start = lines
            .iter()
            .filter(|l| (l.bbox.x + l.bbox.w / 2) >= mid_x)
            .map(|l| l.bbox.x)
            .min()
            .unwrap_or(0);
        if left_max_end <= right_min_start + 20 {
            return Some(mid_x);
        }
    }
    None
}

/// 单段落分类启发式.
fn classify_block(p: &Paragraph, lines: &[RawLine]) -> BlockKind {
    let t = p.text.trim();
    if t.is_empty() {
        return BlockKind::Paragraph;
    }
    // figure_caption: 含 "图 N"/"Figure N"/"Table N"
    let cap_re = regex::Regex::new(r"^(图|Figure|Table|表)\s*\d").unwrap();
    if cap_re.is_match(t) {
        return BlockKind::FigureCaption;
    }
    // list: 以 1./•/-/* 开头
    let list_re = regex::Regex::new(r"^\s*([\d]+[.\)）]|[•●·*\-])\s").unwrap();
    if list_re.is_match(t) {
        return BlockKind::List;
    }
    // footer: y 在页面下 10% + 含页码模式 ("第 X 页" / 单纯数字)
    let max_y = lines
        .iter()
        .map(|l| l.bbox.y + l.bbox.h)
        .max()
        .unwrap_or(1);
    let in_footer_zone = (p.bbox.y as f32) > (max_y as f32 * 0.9);
    if in_footer_zone {
        let footer_re =
            regex::Regex::new(r"^\s*(第\s*\d+\s*页|Page\s*\d+|\d{1,3})\s*$").unwrap();
        if footer_re.is_match(t) {
            return BlockKind::Footer;
        }
    }
    BlockKind::Paragraph
}

/// 找最大字号块作 title.
/// 用 BlockItem.font_size (来自原始行级 median 高度) 作 font signal,
/// 避免被多行段块 h 误导. title = font_size > median(non-title font_sizes) × 1.4 +
/// 内容 ≤ 80 字符 + 在前 5 个 block 内.
fn detect_title(blocks: &[BlockItem]) -> Option<FieldValue> {
    if blocks.is_empty() {
        return None;
    }
    // Baseline = median of all font sizes EXCLUDING the top candidate; with N=1 block
    // there's no title to detect (need contrast). With N=2 we use the smaller as
    // baseline. With N≥3 use median of all-but-largest.
    let mut sizes: Vec<u32> = blocks.iter().map(|b| b.font_size).collect();
    sizes.sort_unstable(); // ascending
    if sizes.len() < 2 {
        return None;
    }
    // Drop the largest, take median of the rest
    let mut without_largest = sizes.clone();
    without_largest.pop();
    let baseline = without_largest[without_largest.len() / 2] as f32;

    let candidates: Vec<&BlockItem> = blocks.iter().take(5).collect();
    let mut best: Option<(&BlockItem, u32)> = None;
    for b in candidates {
        if (b.font_size as f32) > baseline * 1.4 && b.text.chars().count() <= 80 {
            match best {
                None => best = Some((b, b.font_size)),
                Some((_, cur)) if b.font_size > cur => best = Some((b, b.font_size)),
                _ => {}
            }
        }
    }
    best.map(|(b, _)| FieldValue {
        value: Some(b.text.clone()),
        confidence: 0.85,
        bbox: Some(b.bbox),
        source_line_idx: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::BBox;

    fn rl(text: &str, x: u32, y: u32, w: u32, h: u32) -> RawLine {
        RawLine {
            text: text.into(),
            bbox: BBox { x, y, w, h },
            confidence: 0.95,
        }
    }

    #[test]
    fn empty_input_returns_no_blocks() {
        let StructuredFields::DocumentV1 { fields, unrecognized_fields, .. } = extract(&[]) else {
            unreachable!()
        };
        assert!(fields.blocks.is_empty());
        assert!(unrecognized_fields.contains(&"blocks".to_string()));
    }

    #[test]
    fn single_paragraph_one_block() {
        let lines = vec![
            rl("First line of paragraph.", 10, 10, 200, 25),
            rl("Second line continues.", 10, 40, 200, 25),
            rl("Third line ends it.", 10, 70, 200, 25),
        ];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        // All 3 lines should merge into one paragraph (continuous y, same x)
        assert_eq!(fields.blocks.len(), 1);
        assert_eq!(fields.blocks[0].kind, BlockKind::Paragraph);
    }

    #[test]
    fn gap_breaks_into_multiple_paragraphs() {
        let lines = vec![
            rl("Paragraph 1 first line.", 10, 10, 200, 25),
            rl("Paragraph 1 second line.", 10, 40, 200, 25),
            // big gap (Δy = 100) — exceeds 1.5 × line_height
            rl("Paragraph 2 starts here.", 10, 170, 200, 25),
        ];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        assert_eq!(fields.blocks.len(), 2);
    }

    #[test]
    fn figure_caption_classified() {
        let lines = vec![rl("图 3 系统架构", 10, 10, 200, 25)];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        assert_eq!(fields.blocks.len(), 1);
        assert_eq!(fields.blocks[0].kind, BlockKind::FigureCaption);
    }

    #[test]
    fn list_classified() {
        let lines = vec![rl("1. First item", 10, 10, 200, 25)];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        assert_eq!(fields.blocks[0].kind, BlockKind::List);
    }

    #[test]
    fn footer_classified_when_in_bottom_zone_and_matches_page_pattern() {
        let lines = vec![
            rl("Body text", 10, 10, 200, 25),
            rl("More body", 10, 50, 200, 25),
            // y=900 / max_y=925 → ratio 0.97 (in bottom 10%)
            rl("第 3 页", 10, 900, 100, 25),
        ];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        let footer_block = fields.blocks.iter().find(|b| b.kind == BlockKind::Footer);
        assert!(footer_block.is_some(), "should detect footer; got blocks: {:?}",
            fields.blocks.iter().map(|b| (&b.kind, &b.text)).collect::<Vec<_>>());
    }

    #[test]
    fn two_column_layout_reorders_left_then_right() {
        // 左列 3 行 (x=10-200), 右列 3 行 (x=300-490)
        let lines = vec![
            rl("L1", 10, 10, 200, 25),
            rl("R1", 300, 10, 200, 25),
            rl("L2", 10, 50, 200, 25),
            rl("R2", 300, 50, 200, 25),
            rl("L3", 10, 90, 200, 25),
            rl("R3", 300, 90, 200, 25),
        ];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        // 阅读顺序应是 L1 → L2 → L3 → R1 → R2 → R3
        let texts: Vec<&str> = fields.blocks.iter().map(|b| b.text.as_str()).collect();
        let l1_pos = texts.iter().position(|t| *t == "L1").unwrap();
        let l3_pos = texts.iter().position(|t| *t == "L3").unwrap();
        let r1_pos = texts.iter().position(|t| *t == "R1").unwrap();
        assert!(l1_pos < l3_pos, "L1 before L3");
        assert!(l3_pos < r1_pos, "all left column before all right column");
    }

    #[test]
    fn title_detected_for_large_font_first_block() {
        // 大字号标题 + 普通段落
        let lines = vec![
            rl("Big Title", 10, 10, 400, 60), // h=60 (大)
            rl("Body paragraph one.", 10, 90, 400, 25), // h=25
            rl("Body paragraph two.", 10, 150, 400, 25),
        ];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        assert!(fields.title.is_some(), "should detect title");
        let t = fields.title.unwrap();
        assert!(t.value.as_deref().unwrap().contains("Title"));
    }

    #[test]
    fn order_field_assigned_sequentially() {
        let lines = vec![
            rl("Block A", 10, 10, 200, 25),
            rl("Block B", 10, 100, 200, 25),
            rl("Block C", 10, 200, 200, 25),
        ];
        let StructuredFields::DocumentV1 { fields, .. } = extract(&lines) else {
            unreachable!()
        };
        for (i, b) in fields.blocks.iter().enumerate() {
            assert_eq!(b.order, i as u32);
        }
    }
}
