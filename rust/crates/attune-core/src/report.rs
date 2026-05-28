//! Project 报告生成（v0.7 sprint，F-Report）
//!
//! OSS 范围：仅 trait scaffold + 通用 Markdown 实现，把若干 ReportItem
//! 用 LLM 整合成一篇可读 Markdown（概要 / item 列表 / 关联分析三段）。
//!
//! attune-pro 行业版重写 `generate()`，产 legal opinion / patent claim diff /
//! sales BANT report / medical chart summary 等行业专用格式。
//!
//! 注意：本 trait 使用 Rust 1.75+ native `async fn in trait`，因此 trait
//! 本身**不**支持 `Box<dyn ReportGenerator>` 动态分发；调用方应在编译期通过
//! 泛型 `T: ReportGenerator` 注入实现。如 attune-pro 需要动态分发，可在
//! 自己的 crate 里再包一层 `async_trait` adapter。

use crate::error::Result;
use crate::llm::LlmProvider;
use std::sync::Arc;

/// 报告输入条目 — 已脱敏、已摘要、已带时间戳的项目片段
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReportItem {
    pub title: String,
    pub content_summary: String,
    /// ISO-8601 时间戳，由调用方传入
    pub created_at: String,
}

/// 报告生成器抽象。
///
/// `generate()` 拿 project_id（仅用于报告标题 / 追溯 — 实现不直接查 DB）+
/// items 列表 + LlmProvider，产一篇完整 Markdown 文本返回。
pub trait ReportGenerator: Send + Sync {
    /// 产出 Markdown 报告全文
    fn generate<'a>(
        &'a self,
        project_id: &'a str,
        items: &'a [ReportItem],
        llm: Arc<dyn LlmProvider>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>>;

    /// 模板名（attune-pro 用来在 UI 选模板时显示）
    fn template_name(&self) -> &str;
}

/// 通用 Markdown 报告生成器（OSS 默认实现）。
///
/// 流程：拼一个 system prompt 严格要求 Markdown 输出（含三段 H2：概要 /
/// 主要 items 列表 / 关联分析），把 items 序列化为 JSON 塞进 user 消息，
/// 一次性同步调 LLM 返 Markdown 文本。
pub struct MarkdownReportGenerator;

impl ReportGenerator for MarkdownReportGenerator {
    fn generate<'a>(
        &'a self,
        project_id: &'a str,
        items: &'a [ReportItem],
        llm: Arc<dyn LlmProvider>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let system = "你是一名严谨的项目分析师。请基于用户提供的 items，\
                          输出**纯 Markdown** 格式的项目报告，包含三个 H2 段落：\
                          ## 概要 / ## 主要 items 列表 / ## 关联分析。\
                          禁止额外解释、禁止使用代码块包裹整篇报告。";
            // 把 items 序列化为 JSON 给 LLM 看
            let items_json = serde_json::to_string_pretty(items).unwrap_or_else(|_| "[]".to_string());
            let user = format!(
                "项目 id: {}\n\n以下是该项目的 items（JSON 数组）：\n{}\n\n\
                 请按上述三段结构输出 Markdown 报告。",
                project_id, items_json
            );
            // LlmProvider::chat 是 sync — 在 async fn 中直接调用即可
            let (text, _usage) = llm.chat(system, &user)?;
            Ok(text)
        })
    }

    fn template_name(&self) -> &str {
        "markdown_default"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmProvider;

    /// Mock LLM provider — 返回固定字符串，不真调 API
    struct MockLlm {
        canned: String,
    }
    impl LlmProvider for MockLlm {
        fn chat(
            &self,
            _system: &str,
            _user: &str,
        ) -> Result<(String, crate::usage::TokenUsage)> {
            Ok((
                self.canned.clone(),
                crate::usage::TokenUsage::empty("mock", "mock"),
            ))
        }
        fn is_available(&self) -> bool {
            true
        }
        fn model_name(&self) -> &str {
            "mock"
        }
    }

    #[test]
    fn trait_shape_compiles() {
        // 编译期验证：MarkdownReportGenerator 实现了 ReportGenerator trait
        fn assert_impls<T: ReportGenerator>(_: &T) {}
        let g = MarkdownReportGenerator;
        assert_impls(&g);
        assert_eq!(g.template_name(), "markdown_default");
    }

    #[tokio::test]
    async fn markdown_generator_with_mock_llm() {
        let llm = Arc::new(MockLlm {
            canned: "## 概要\n这是 mock 报告\n\n## 主要 items 列表\n- item A\n\n## 关联分析\n无".to_string(),
        });
        let items = vec![
            ReportItem {
                title: "调研笔记 A".into(),
                content_summary: "关于 RVV 优化的初步结论".into(),
                created_at: "2026-05-01T10:00:00Z".into(),
            },
            ReportItem {
                title: "调研笔记 B".into(),
                content_summary: "对比 OpenBLAS 与 MLAS 的 GEMM 性能".into(),
                created_at: "2026-05-02T11:30:00Z".into(),
            },
        ];
        let generator = MarkdownReportGenerator;
        let report = generator
            .generate("proj-001", &items, llm as Arc<dyn LlmProvider>)
            .await
            .expect("generate ok");
        assert!(report.contains("## 概要"));
        assert!(report.contains("## 主要 items 列表"));
        assert!(report.contains("## 关联分析"));
    }
}
