//! Vision-Language Model (VLM) capability。
//!
//! 与 `LlmProvider`、`OcrProvider` 的边界：
//! - OCR：图里有清晰文字时识别字符（PP-OCR）
//! - VLM caption：描述图的内容 / 场景 / 物体（"a chart showing Q1 revenue trend"）
//! - VQA：针对图回答具体问题（"图里哪条线是 2024 Q1？"）
//!
//! `LlmVlmProvider` 是薄适配器：持有任意 `LlmProvider`，
//! 读图片 → base64 data URI → 调 `chat_multimodal` 带 `Attachment::Image`，
//! 不自己实现 OpenAI vision 协议。
//!
//! 接口使用同步签名（与 `LlmProvider` 一致），方便 `Arc<dyn VlmProvider>` dyn 调用。
use crate::error::Result;
use crate::llm::{Attachment, LlmProvider};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::path::Path;
use std::sync::Arc;

/// Vision-Language Model 抽象（同步接口，dyn-compatible）。
///
/// 实现方需保证：
/// - `caption` 返回简洁描述（一般 ≤ 200 字符）
/// - `vqa` 返回针对 `question` 的自然语言回答
/// - 输入图片可以是 png / jpg / webp；实现方负责格式适配
pub trait VlmProvider: Send + Sync {
    /// 对图片生成自然语言描述。
    fn caption(&self, image_path: &Path) -> Result<String>;

    /// 视觉问答：针对 `image_path` 回答 `question`。
    fn vqa(&self, image_path: &Path, question: &str) -> Result<String>;
}

/// 基于 `LlmProvider` 的 VLM 薄适配器。
///
/// 读图片文件 → base64 data URI → 调 `LlmProvider::chat_multimodal`（带 `Attachment::Image`）。
/// 底层 provider 需支持 vision（如 OpenAiLlmProvider with GPT-4o / gpt-4-vision-preview）；
/// 不支持 vision 的 provider 会 drop 图片并仅用文本 prompt，返回退化结果。
pub struct LlmVlmProvider {
    llm: Arc<dyn LlmProvider>,
}

impl LlmVlmProvider {
    pub fn new(llm: Arc<dyn LlmProvider>) -> Self {
        Self { llm }
    }

    /// 读图片为 base64 data URI，格式：`data:<mime>;base64,<data>`。
    fn read_image_as_data_uri(image_path: &Path) -> Result<(String, String)> {
        let bytes = std::fs::read(image_path)?;
        let mime = mime_from_path(image_path);
        let b64 = STANDARD.encode(&bytes);
        Ok((format!("data:{mime};base64,{b64}"), mime.to_string()))
    }
}

impl VlmProvider for LlmVlmProvider {
    fn caption(&self, image_path: &Path) -> Result<String> {
        let (data_uri, mime) = Self::read_image_as_data_uri(image_path)?;
        let attachment = Attachment::Image { url_or_data_uri: data_uri, mime };
        // chat_multimodal 是同步调用，直接转发；LLM 内部走独立 llm_rt Runtime.
        // Plan A1 Task I: discard TokenUsage here — VLM caption is invoked from
        // batch ingest paths without a recorder context. When Task U wires the
        // chat route, plumb usage through this layer too.
        self.llm
            .chat_multimodal(
                "You are a concise image description assistant.",
                "用一句话（≤200字）描述这张图的内容、场景和主要物体。",
                &[attachment],
            )
            .map(|(s, _u)| s)
    }

    fn vqa(&self, image_path: &Path, question: &str) -> Result<String> {
        let (data_uri, mime) = Self::read_image_as_data_uri(image_path)?;
        let attachment = Attachment::Image { url_or_data_uri: data_uri, mime };
        self.llm
            .chat_multimodal(
                "You are a precise visual question answering assistant.",
                question,
                &[attachment],
            )
            .map(|(s, _u)| s)
    }
}

/// 从文件扩展名推断图片 MIME type。
fn mime_from_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        _ => "image/png",
    }
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
    fn caption(&self, _image_path: &Path) -> Result<String> {
        Ok("[mock image caption]".to_string())
    }

    fn vqa(&self, _image_path: &Path, question: &str) -> Result<String> {
        Ok(format!("[mock vqa answer to: {question}]"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn mock_caption_returns_marker() {
        let p = MockVlmProvider::new();
        let got = p.caption(&PathBuf::from("/tmp/whatever.png")).unwrap();
        assert_eq!(got, "[mock image caption]");
    }

    #[test]
    fn mock_caption_does_not_touch_filesystem() {
        // 不存在的路径也应返回, 证明 mock 没读盘
        let p = MockVlmProvider::new();
        let got = p.caption(&PathBuf::from("/definitely/does/not/exist.png")).unwrap();
        assert_eq!(got, "[mock image caption]");
    }

    #[test]
    fn mock_vqa_echoes_question() {
        let p = MockVlmProvider::new();
        let got = p.vqa(&PathBuf::from("/tmp/chart.png"), "Which line represents Q1 2024?").unwrap();
        assert!(got.contains("Which line represents Q1 2024?"));
        assert!(got.starts_with("[mock vqa answer to:"));
    }

    #[test]
    fn mock_vqa_supports_chinese_question() {
        let p = MockVlmProvider::new();
        let got = p.vqa(&PathBuf::from("/tmp/x.png"), "图里有几条折线？").unwrap();
        assert!(got.contains("图里有几条折线"));
    }

    // ── LlmVlmProvider 单测 ──────────────────────────────────────────────────

    #[test]
    fn mime_from_path_variants() {
        assert_eq!(mime_from_path(Path::new("a.png")), "image/png");
        assert_eq!(mime_from_path(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(mime_from_path(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(mime_from_path(Path::new("a.webp")), "image/webp");
        assert_eq!(mime_from_path(Path::new("a.gif")), "image/gif");
        assert_eq!(mime_from_path(Path::new("a.PNG")), "image/png"); // 大小写不敏感
        assert_eq!(mime_from_path(Path::new("a.unknown")), "image/png"); // fallback
    }

    #[test]
    fn llm_vlm_caption_returns_llm_response() {
        use crate::llm::MockLlmProvider;
        use std::io::Write;

        // 写最小有效 PNG 文件（1×1 像素）
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let tiny_png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
            0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
            0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41,
            0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
            0x00, 0x00, 0x02, 0x00, 0x01, 0xe2, 0x21, 0xbc,
            0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
            0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        tmp.write_all(tiny_png).unwrap();
        // NamedTempFile 无 .png 扩展名，复制一份带扩展名的
        let png_path = tmp.path().with_extension("png");
        std::fs::copy(tmp.path(), &png_path).unwrap();

        let mock_llm = Arc::new(MockLlmProvider::new("gpt-4o"));
        mock_llm.push_response("a tiny red square");

        let vlm = LlmVlmProvider::new(mock_llm);
        let caption = vlm.caption(&png_path).unwrap();
        assert_eq!(caption, "a tiny red square");

        let _ = std::fs::remove_file(&png_path);
    }

    #[test]
    fn llm_vlm_vqa_passes_question_to_llm() {
        use crate::llm::MockLlmProvider;
        use std::io::Write;

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        let png_path = tmp.path().with_extension("png");
        std::fs::copy(tmp.path(), &png_path).unwrap();

        let mock_llm = Arc::new(MockLlmProvider::new("gpt-4o"));
        mock_llm.push_response("3 lines");

        let vlm = LlmVlmProvider::new(mock_llm);
        let answer = vlm.vqa(&png_path, "图里有几条折线？").unwrap();
        assert_eq!(answer, "3 lines");

        let _ = std::fs::remove_file(&png_path);
    }

    #[test]
    fn llm_vlm_caption_errors_on_missing_file() {
        use crate::llm::MockLlmProvider;

        let mock_llm = Arc::new(MockLlmProvider::new("gpt-4o"));
        let vlm = LlmVlmProvider::new(mock_llm);
        let result = vlm.caption(Path::new("/nonexistent/path/x.png"));
        assert!(result.is_err(), "missing file should return Err");
    }

    /// 记录 chat_multimodal 收到的附件里是否含 Image —— 验证 caption 真把图传进 vision 路径
    struct RecordingLlm {
        image_seen: std::sync::Mutex<bool>,
    }

    impl LlmProvider for RecordingLlm {
        fn chat(
            &self,
            _system: &str,
            _user: &str,
        ) -> Result<(String, crate::usage::TokenUsage)> {
            Ok((
                "text-only".to_string(),
                crate::usage::TokenUsage::empty("mock", "recording-mock"),
            ))
        }
        fn chat_multimodal(
            &self,
            _system: &str,
            _user: &str,
            attachments: &[Attachment],
        ) -> Result<(String, crate::usage::TokenUsage)> {
            let has_image = attachments.iter().any(|a| matches!(a, Attachment::Image { .. }));
            *self.image_seen.lock().unwrap_or_else(|e| e.into_inner()) = has_image;
            Ok((
                "recorded".to_string(),
                crate::usage::TokenUsage::empty("mock", "recording-mock"),
            ))
        }
        fn is_available(&self) -> bool {
            true
        }
        fn model_name(&self) -> &str {
            "recording-mock"
        }
    }

    #[test]
    fn llm_vlm_caption_passes_image_attachment_to_chat_multimodal() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        let png = tmp.path().with_extension("png");
        std::fs::copy(tmp.path(), &png).unwrap();

        let rec = Arc::new(RecordingLlm { image_seen: std::sync::Mutex::new(false) });
        let vlm = LlmVlmProvider::new(rec.clone());
        let out = vlm.caption(&png).unwrap();
        assert_eq!(out, "recorded");
        assert!(
            *rec.image_seen.lock().unwrap_or_else(|e| e.into_inner()),
            "caption 必须把 Image attachment 传入 chat_multimodal"
        );
        let _ = std::fs::remove_file(&png);
    }
}
