//! Vision-Language Model (VLM) capability。
//!
//! v0.7 scaffold：定义 `VlmProvider` trait + `MockVlmProvider` 实现 + 单测。
//! v0.8 真 VLM：Ollama `llava:7b` 或 `qwen-vl`，image base64 encode HTTP POST。
//!
//! 应用场景：
//! - 截图入库时自动生成 caption 作为 chunk 文本（让图片可被语义搜索）
//! - 用户 Chat 里贴图 + 提问，走 vqa 通道
//!
//! 与 `LlmProvider`、`OcrProvider` 的边界：
//! - OCR：图里有清晰文字时识别字符（PP-OCR）
//! - VLM caption：描述图的内容 / 场景 / 物体（"a chart showing Q1 revenue trend"）
//! - VQA：针对图回答具体问题（"图里哪条线是 2024 Q1？"）
use crate::error::Result;
use std::future::Future;
use std::path::Path;

/// Vision-Language Model 抽象。
///
/// 实现方需保证：
/// - `caption` 返回简洁描述（一般 ≤ 200 字符）
/// - `vqa` 返回针对 `question` 的自然语言回答
/// - 输入图片可以是 png / jpg / webp；实现方负责格式适配
pub trait VlmProvider: Send + Sync {
    /// 对图片生成自然语言描述。
    fn caption(&self, image_path: &Path) -> impl Future<Output = Result<String>> + Send;

    /// 视觉问答：针对 `image_path` 回答 `question`。
    fn vqa(
        &self,
        image_path: &Path,
        question: &str,
    ) -> impl Future<Output = Result<String>> + Send;
}

/// 测试 mock。返回 hardcoded 字符串，不实际读图片。
///
/// `caption` 返回 `"[mock image caption]"`；
/// `vqa` 返回 `"[mock vqa answer to: <question>]"`。
pub struct MockVlmProvider;

impl MockVlmProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MockVlmProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl VlmProvider for MockVlmProvider {
    async fn caption(&self, _image_path: &Path) -> Result<String> {
        Ok("[mock image caption]".to_string())
    }

    async fn vqa(&self, _image_path: &Path, question: &str) -> Result<String> {
        Ok(format!("[mock vqa answer to: {question}]"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn mock_caption_returns_marker() {
        let p = MockVlmProvider::new();
        let got = p.caption(&PathBuf::from("/tmp/whatever.png")).await.unwrap();
        assert_eq!(got, "[mock image caption]");
    }

    #[tokio::test]
    async fn mock_caption_does_not_touch_filesystem() {
        // 不存在的路径也应返回, 证明 mock 没读盘
        let p = MockVlmProvider::new();
        let got = p
            .caption(&PathBuf::from("/definitely/does/not/exist.png"))
            .await
            .unwrap();
        assert_eq!(got, "[mock image caption]");
    }

    #[tokio::test]
    async fn mock_vqa_echoes_question() {
        let p = MockVlmProvider::new();
        let got = p
            .vqa(
                &PathBuf::from("/tmp/chart.png"),
                "Which line represents Q1 2024?",
            )
            .await
            .unwrap();
        assert!(got.contains("Which line represents Q1 2024?"));
        assert!(got.starts_with("[mock vqa answer to:"));
    }

    #[tokio::test]
    async fn mock_vqa_supports_chinese_question() {
        let p = MockVlmProvider::new();
        let got = p
            .vqa(&PathBuf::from("/tmp/x.png"), "图里有几条折线？")
            .await
            .unwrap();
        assert!(got.contains("图里有几条折线"));
    }
}
