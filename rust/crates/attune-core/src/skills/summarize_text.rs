//! summarize_text — 单文档摘要 (调 LLM 一次).
//!
//! 默认启用. 适合 < 5K tokens 单文档. 大文档集汇总走 summarize_document_set (默认禁用).

use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;

const DEFAULT_SYSTEM_PROMPT: &str = "你是文档摘要助手. 请用 100-200 字总结输入文本的核心内容, \
不要添加原文之外的信息, 不做主观判断. 输出纯文本, 不要 markdown.";

/// 输入: 待摘要文本
/// 输出: 摘要字符串
pub fn summarize(llm: &dyn LlmProvider, text: &str) -> Result<String> {
    summarize_with_prompt(llm, text, DEFAULT_SYSTEM_PROMPT)
}

/// 输入超长截断阈值 (字符数, 不是 tokens). 经验值: 20K 字符 ≈ 5K-10K tokens.
const MAX_INPUT_CHARS: usize = 20_000;

pub fn summarize_with_prompt(llm: &dyn LlmProvider, text: &str, system: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err(VaultError::Crypto("empty input to summarize".into()));
    }
    let input = if text.chars().count() > MAX_INPUT_CHARS {
        text.chars().take(MAX_INPUT_CHARS).collect::<String>()
    } else {
        text.to_string()
    };
    // Plan A1 Task I: drop the TokenUsage since this entry point predates
    // the recorder (summarize is invoked from non-route paths). When the
    // recorder is plumbed through (Task U / future), surface `TokenUsage`
    // from this function instead of swallowing it.
    llm.chat(system, &input).map(|(s, _u)| s)
}

/// 多文档汇总. 默认禁用 (大量 token), 由调用方显式启用.
pub fn summarize_document_set(
    llm: &dyn LlmProvider,
    docs: &[(&str, &str)],
    enabled: bool,
) -> Result<String> {
    if !enabled {
        return Err(VaultError::Crypto(
            "summarize_document_set disabled by default (high token cost); pass enabled=true to opt in".into(),
        ));
    }
    if docs.is_empty() {
        return Err(VaultError::Crypto("empty docs to summarize".into()));
    }
    let mut joined = String::with_capacity(docs.len() * 1000);
    for (filename, text) in docs {
        joined.push_str("=== ");
        joined.push_str(filename);
        joined.push_str(" ===\n");
        joined.push_str(text);
        joined.push_str("\n\n");
    }
    let system = "你是多文档汇总助手. 请综合所有输入文档, 用 300-500 字总结核心要点 + 文档间的关联. \
不要添加原文之外的信息, 不做主观判断.";
    summarize_with_prompt(llm, &joined, system)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmProvider;

    /// Mock LLM: 直接拼接 system + user 返回, 用于测试 summarize 不依赖真 LLM.
    struct MockLlm;
    impl LlmProvider for MockLlm {
        fn chat(
            &self,
            system: &str,
            user: &str,
        ) -> Result<(String, crate::usage::TokenUsage)> {
            Ok((
                format!(
                    "[MOCK] sys.len={}, user.len={}, head={:?}",
                    system.len(),
                    user.len(),
                    user.chars().take(20).collect::<String>(),
                ),
                crate::usage::TokenUsage::empty("mock", "mock"),
            ))
        }
        fn is_available(&self) -> bool { true }
        fn model_name(&self) -> &str { "mock" }
    }

    #[test]
    fn summarize_empty_text_errors() {
        let llm = MockLlm;
        let r = summarize(&llm, "");
        assert!(r.is_err());
        let r2 = summarize(&llm, "   \n\t  ");
        assert!(r2.is_err());
    }

    #[test]
    fn summarize_truncates_long_input() {
        let llm = MockLlm;
        let huge: String = "a".repeat(50_000);
        let r = summarize(&llm, &huge).expect("should summarize");
        // mock 返回 user.len, 应被截到 20000
        assert!(r.contains("user.len=20000"));
    }

    #[test]
    fn summarize_normal_input_invokes_llm() {
        let llm = MockLlm;
        let r = summarize(&llm, "这是一份测试文档").expect("ok");
        assert!(r.starts_with("[MOCK]"));
    }

    #[test]
    fn summarize_document_set_disabled_by_default() {
        let llm = MockLlm;
        let docs = [("a.pdf", "doc A"), ("b.pdf", "doc B")];
        let r = summarize_document_set(&llm, &docs, false);
        assert!(r.is_err());
        let msg = format!("{:?}", r.unwrap_err());
        assert!(msg.contains("disabled by default"));
    }

    #[test]
    fn summarize_document_set_runs_when_enabled() {
        let llm = MockLlm;
        let docs = [("a.pdf", "doc A 内容"), ("b.pdf", "doc B 内容")];
        let r = summarize_document_set(&llm, &docs, true).expect("ok");
        assert!(r.starts_with("[MOCK]"));
    }

    #[test]
    fn summarize_document_set_empty_errors() {
        let llm = MockLlm;
        let r = summarize_document_set(&llm, &[], true);
        assert!(r.is_err());
    }
}
