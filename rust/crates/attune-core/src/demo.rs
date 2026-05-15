//! v0.7 — Wizard "加载示例" sample data
//!
//! 让新用户一键加载 5 个 diverse 示例 item，避免空知识库流失。
//! 加载流程由 attune-server `routes::demo::load_demo` 调用：
//!   1. 读 `load_demo_items()` 得到 5 条 DemoItem
//!   2. 用 `vault.store().insert_item(...)` 逐条入库（source_type="demo"）
//!   3. 已加载（统计 source_type='demo' > 0）则直接 skip 以保持 idempotent
//!
//! 示例数据嵌入二进制（include_str!），不依赖运行时文件，跨平台分发友好。

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 单条示例知识。字段与 `Store::insert_item` 入参对齐。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemoItem {
    pub title: String,
    pub source_type: String,
    pub domain: String,
    pub corpus_domain: String,
    pub content: String,
}

/// 静态嵌入的 demo.json 全文。`include_str!` 路径相对当前源文件。
const DEMO_JSON: &str = include_str!("../data/demo.json");

/// 解析嵌入的 demo.json，返回 5 条 `DemoItem`。
pub fn load_demo_items() -> Result<Vec<DemoItem>> {
    let items: Vec<DemoItem> = serde_json::from_str(DEMO_JSON)?;
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_json_parses_five_items() {
        let items = load_demo_items().expect("parse demo.json");
        assert_eq!(items.len(), 5, "demo.json 应该恰好包含 5 条示例");
    }

    #[test]
    fn all_demo_fields_non_empty() {
        let items = load_demo_items().unwrap();
        for it in &items {
            assert!(!it.title.is_empty(), "title 为空: {it:?}");
            assert_eq!(it.source_type, "demo", "source_type 必须是 demo");
            assert!(!it.domain.is_empty(), "domain 为空: {it:?}");
            assert!(!it.corpus_domain.is_empty(), "corpus_domain 为空: {it:?}");
            assert!(
                it.content.len() > 100,
                "content 太短 (<100 字符): {} - {} bytes",
                it.title,
                it.content.len()
            );
        }
    }
}
