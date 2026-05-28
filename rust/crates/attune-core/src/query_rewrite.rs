//! Query rewriting — LLM 把用户口语 query 改写为检索关键词，提升 RAG hit rate。
//!
//! 设计：
//! - LLM 输出严格关键词序列（空格分隔），剥离 "请问"/"我想知道"/"帮我看看" 等口语
//! - 失败时 fallback 返回原 query（不影响主流程）
//! - 输出后做一次 sanitize（去标点 / 截断），防止 LLM 偶尔输出解释性文字

use crate::error::Result;
use crate::llm::LlmProvider;
use std::sync::Arc;

const REWRITE_SYSTEM_PROMPT: &str = r#"你是一个搜索查询关键词提取器。把用户的自然语言查询改写为最适合全文检索 + 向量检索的关键词序列。

严格要求：
1. 输出格式：仅关键词，空格分隔，例如 "keyword1 keyword2 keyword3"
2. 去除口语化前缀：请问、我想知道、帮我看看、能不能、可以吗、麻烦你、谢谢等
3. 去除停用词：的、了、是、在、有、和、与
4. 保留专有名词、技术术语、人名地名
5. 中英文混合时保留原文（不要翻译）
6. 不要输出任何解释、引号、标点、换行
7. 关键词数量控制在 2-6 个

示例：
用户："请问最近一周谁在群里说了关于 Rust async runtime 的事？"
输出：Rust async runtime 群

用户："How do I configure SQLite WAL mode?"
输出：SQLite WAL mode configure

用户："tantivy 中文分词"
输出：tantivy 中文 分词
"#;

/// 改写用户 query 为检索关键词。
///
/// LLM 失败时返回原 query（fallback 不丢失功能）。
pub async fn rewrite_query(query: &str, llm: Arc<dyn LlmProvider>) -> Result<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    // LlmProvider::chat 是同步签名（内部用 llm_block_on），spawn_blocking 包裹避免阻塞 async runtime
    let q = trimmed.to_string();
    let llm_clone = llm.clone();
    let rewrite_result = tokio::task::spawn_blocking(move || {
        llm_clone.chat(REWRITE_SYSTEM_PROMPT, &q)
    })
    .await;

    match rewrite_result {
        Ok(Ok((raw, _usage))) => Ok(sanitize_keywords(&raw, trimmed)),
        // LLM 调用失败 / join error 都走 fallback
        Ok(Err(_)) | Err(_) => Ok(trimmed.to_string()),
    }
}

/// 清理 LLM 输出：
/// - 取第一行（防止 LLM 多行解释）
/// - 去常见标点 / 引号
/// - 截断超长（>200 字符视为 LLM 跑飞，fallback 原 query）
/// - 空输出 fallback
fn sanitize_keywords(raw: &str, fallback: &str) -> String {
    let first_line = raw.lines().next().unwrap_or("").trim();

    // LLM 跑飞（输出整段解释）— fallback
    if first_line.len() > 200 {
        return fallback.to_string();
    }

    let cleaned: String = first_line
        .chars()
        .map(|c| match c {
            '"' | '\'' | '“' | '”' | '‘' | '’' | '`' => ' ',
            ',' | '，' | '。' | '!' | '！' | '?' | '？' | ';' | '；' | ':' | '：' => ' ',
            '(' | ')' | '（' | '）' | '[' | ']' | '【' | '】' => ' ',
            _ => c,
        })
        .collect();

    let collapsed: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    if collapsed.is_empty() {
        fallback.to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::VaultError;
    use crate::llm::{ChatMessage, LlmProvider};

    /// 测试用 mock — 返回固定字符串
    struct StubLlm {
        response: String,
    }

    impl LlmProvider for StubLlm {
        fn chat(
            &self,
            _system: &str,
            _user: &str,
        ) -> Result<(String, crate::usage::TokenUsage)> {
            Ok((
                self.response.clone(),
                crate::usage::TokenUsage::empty("stub", "stub"),
            ))
        }

        fn chat_with_history(
            &self,
            _messages: &[ChatMessage],
        ) -> Result<(String, crate::usage::TokenUsage)> {
            Ok((
                self.response.clone(),
                crate::usage::TokenUsage::empty("stub", "stub"),
            ))
        }

        fn is_available(&self) -> bool { true }
        fn model_name(&self) -> &str { "stub" }
    }

    /// 失败用 mock — 总是返回 LlmUnavailable
    struct FailingLlm;

    impl LlmProvider for FailingLlm {
        fn chat(
            &self,
            _system: &str,
            _user: &str,
        ) -> Result<(String, crate::usage::TokenUsage)> {
            Err(VaultError::LlmUnavailable("test fail".into()))
        }

        fn is_available(&self) -> bool { false }
        fn model_name(&self) -> &str { "failing" }
    }

    #[tokio::test]
    async fn rewrite_english_query() {
        let llm: Arc<dyn LlmProvider> = Arc::new(StubLlm {
            response: "SQLite WAL mode configure".into(),
        });
        let out = rewrite_query("How do I configure SQLite WAL mode?", llm)
            .await
            .unwrap();
        assert_eq!(out, "SQLite WAL mode configure");
    }

    #[tokio::test]
    async fn rewrite_chinese_colloquial_query() {
        let llm: Arc<dyn LlmProvider> = Arc::new(StubLlm {
            response: "Rust async runtime 群".into(),
        });
        let out = rewrite_query(
            "请问最近一周谁在群里说了关于 Rust async runtime 的事？",
            llm,
        )
        .await
        .unwrap();
        assert_eq!(out, "Rust async runtime 群");
    }

    #[tokio::test]
    async fn rewrite_already_keywords_passthrough() {
        // LLM 通常对已经是关键词的输入也会输出关键词
        let llm: Arc<dyn LlmProvider> = Arc::new(StubLlm {
            response: "tantivy 中文 分词".into(),
        });
        let out = rewrite_query("tantivy 中文分词", llm).await.unwrap();
        assert_eq!(out, "tantivy 中文 分词");
    }

    #[tokio::test]
    async fn rewrite_fallback_on_llm_failure() {
        let llm: Arc<dyn LlmProvider> = Arc::new(FailingLlm);
        let out = rewrite_query("请问 Rust 怎么用", llm).await.unwrap();
        // LLM 失败 — 返回原 query 而非 panic
        assert_eq!(out, "请问 Rust 怎么用");
    }

    #[tokio::test]
    async fn rewrite_sanitizes_punctuation_and_quotes() {
        let llm: Arc<dyn LlmProvider> = Arc::new(StubLlm {
            response: "\"Rust\", async, runtime!".into(),
        });
        let out = rewrite_query("test", llm).await.unwrap();
        assert_eq!(out, "Rust async runtime");
    }

    #[tokio::test]
    async fn rewrite_empty_query() {
        let llm: Arc<dyn LlmProvider> = Arc::new(StubLlm {
            response: "anything".into(),
        });
        let out = rewrite_query("   ", llm).await.unwrap();
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn rewrite_fallback_on_runaway_llm_output() {
        // LLM 跑飞 — 输出超长解释
        let long_response = "我认为用户想问的是关于 ".repeat(20);
        let llm: Arc<dyn LlmProvider> = Arc::new(StubLlm {
            response: long_response,
        });
        let out = rewrite_query("原始 query", llm).await.unwrap();
        assert_eq!(out, "原始 query");
    }

    #[test]
    fn sanitize_takes_first_line_only() {
        let raw = "Rust async\n这是我的解释...";
        let out = sanitize_keywords(raw, "fallback");
        assert_eq!(out, "Rust async");
    }
}
