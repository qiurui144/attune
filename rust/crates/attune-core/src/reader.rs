//! Reader 高亮 / 划词数据结构（v0.7 sprint，F-Reader scaffold；大头 v0.8）
//!
//! v0.7 本会话仅落数据结构。v0.8 实际 UI:
//! pdf.js + react-pdf 渲染 PDF → 用户划词捕获 bbox → 持久化为 PdfHighlight
//! → 转 Annotation 入 store。WebHighlight 用 DOM xpath + text offset 复位。

use crate::store::Annotation;
use serde::{Deserialize, Serialize};

/// PDF 高亮 — bbox 为 PDF user space 坐标 (x0, y0, x1, y1)，单位 pt。
///
/// `annotation_id` 关联到 attune 通用 Annotation（store::Annotation）。
/// 若高亮尚未升级为带文字内容的 Annotation，则为 None（纯划线高亮）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PdfHighlight {
    /// 来源 item id（PDF 文件的 item）
    pub item_id: String,
    /// 页码（1-indexed）
    pub page: u32,
    /// PDF user space bbox：(x0, y0, x1, y1)
    pub bbox: (f32, f32, f32, f32),
    /// 划取的文字
    pub text: String,
    /// 高亮颜色（CSS 颜色字符串，与 Annotation.color 对齐）
    pub color: String,
    /// 关联的 Annotation id（可为 None 表示纯高亮无附注）
    pub annotation_id: Option<String>,
}

impl PdfHighlight {
    /// 转为通用 Annotation —— offset_start/end 暂用 page * 10_000 + bbox x0/x1
    /// 作为占位（不影响入库），v0.8 应改为对接 PDF text extractor 给出的真实字节 offset。
    ///
    /// `source` 默认 "user"；如外层是 AI 自动划重点产生的，调用方应改成 "ai"。
    pub fn to_annotation(&self) -> Annotation {
        // 占位 offset 方案：每页给 10_000 间隔避免跨页冲突；bbox x 当行内偏移
        let base = (self.page as i64) * 10_000;
        let offset_start = base + self.bbox.0 as i64;
        let offset_end = base + self.bbox.2 as i64;
        let now = chrono::Utc::now().to_rfc3339();
        Annotation {
            id: self
                .annotation_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            item_id: self.item_id.clone(),
            offset_start,
            offset_end,
            text_snippet: self.text.clone(),
            label: None,
            color: self.color.clone(),
            content: String::new(),
            source: "user".to_string(),
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// Web 页面高亮 — 用 xpath + text offset 复位，因为 HTML DOM 没有稳定的字节 offset。
///
/// v0.8 实现：用户在浏览器扩展中划词，扩展把 (xpath, start, end) 发回 attune，
/// attune 持久化；再访问该 URL 时扩展按 xpath 查节点 → 计算 text offset → 高亮。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebHighlight {
    /// 来源 URL（不含 fragment / query 视实现而定）
    pub url: String,
    /// CSS xpath 定位锚节点（如 `/html/body/div[2]/article/p[5]`）
    pub xpath: String,
    /// 在该节点 textContent 中的起始字符 offset
    pub text_offset_start: usize,
    /// 在该节点 textContent 中的结束字符 offset
    pub text_offset_end: usize,
    /// 高亮颜色（CSS 颜色字符串）
    pub color: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_highlight_serde_roundtrip() {
        let h = PdfHighlight {
            item_id: "item-abc".into(),
            page: 3,
            bbox: (72.0, 100.5, 540.0, 120.0),
            text: "重要观点".into(),
            color: "#ffeb3b".into(),
            annotation_id: Some("ann-xyz".into()),
        };
        let json = serde_json::to_string(&h).expect("serialize");
        let parsed: PdfHighlight = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, h);
        // 转 Annotation 不 panic, 字段对齐
        let ann = h.to_annotation();
        assert_eq!(ann.item_id, "item-abc");
        assert_eq!(ann.id, "ann-xyz");
        assert_eq!(ann.text_snippet, "重要观点");
        assert_eq!(ann.color, "#ffeb3b");
        // offset 占位方案：page=3 → base=30000；x0=72 → start=30072；x1=540 → end=30540
        assert_eq!(ann.offset_start, 30072);
        assert_eq!(ann.offset_end, 30540);
    }

    #[test]
    fn web_highlight_bbox_offsets() {
        let h = WebHighlight {
            url: "https://example.com/article".into(),
            xpath: "/html/body/div[2]/article/p[5]".into(),
            text_offset_start: 100,
            text_offset_end: 160,
            color: "#03a9f4".into(),
        };
        assert_eq!(h.text_offset_end - h.text_offset_start, 60);
        // serde roundtrip
        let json = serde_json::to_string(&h).expect("serialize");
        let parsed: WebHighlight = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, h);
    }
}
