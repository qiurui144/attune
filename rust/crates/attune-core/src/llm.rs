use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

/// 共享 tokio Runtime，供所有 LLM 同步 HTTP 调用复用。
/// 使用独立 Runtime 而非主 Runtime，避免在 spawn_blocking / 测试上下文中
/// 调用 block_on 时触发 "Cannot start a runtime from within a runtime" panic。
fn llm_rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("llm-rt")
            .enable_all()
            .build()
            .expect("llm tokio runtime init failed")
    })
}

/// 在独立线程中运行 async future，复用共享 LLM Runtime。
/// 线程逃逸确保不在主 tokio 上下文中直接 block_on（避免 runtime-within-runtime）。
fn llm_block_on<F, T>(f: F) -> crate::error::Result<T>
where
    F: std::future::Future<Output = crate::error::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(move || llm_rt().block_on(f))
        .join()
        .map_err(|_| VaultError::LlmUnavailable("llm worker thread panicked".into()))?
}

/// 对话消息（公开，用于多轮对话）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,    // "system" / "user" / "assistant"
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: &str) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: &str) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

/// Chat LLM 抽象
pub trait LlmProvider: Send + Sync {
    /// 单次 chat 调用，system + user 消息，返回完整响应文本
    fn chat(&self, system: &str, user: &str) -> Result<String>;

    /// 带历史的多轮对话
    fn chat_with_history(&self, messages: &[ChatMessage]) -> Result<String> {
        // 默认实现：取最后一条 user 消息，用第一条 system 消息
        let system = messages.iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let user = messages.iter().rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        self.chat(system, user)
    }

    /// 模型是否可用
    fn is_available(&self) -> bool;

    /// 当前使用的模型名（用于 tags.model 记录）
    fn model_name(&self) -> &str;
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaChatMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a str>,
}

#[derive(Serialize)]
struct OllamaChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: OllamaChatResponseMessage,
}

#[derive(Deserialize)]
struct OllamaChatResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagsModel>,
}

#[derive(Deserialize)]
struct TagsModel {
    name: String,
}

/// Ollama chat client
pub struct OllamaLlmProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

/// 按优先级排列的默认 chat 模型候选
const PREFERRED_MODELS: &[&str] = &[
    "qwen2.5:7b",
    "qwen2.5:3b",
    "qwen2.5:1.5b",
    "llama3.2:3b",
    "llama3.2:1b",
    "phi3:mini",
];

impl OllamaLlmProvider {
    /// 显式指定模型
    pub fn with_model(model: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            base_url: "http://localhost:11434".to_string(),
            model: model.to_string(),
        }
    }

    /// 自动探测: 查询本地已下载的 chat 模型，按 PREFERRED_MODELS 优先级选择
    pub fn auto_detect() -> Result<Self> {
        let provider = Self::with_model("placeholder");
        let client = provider.client.clone();
        let url = format!("{}/api/tags", provider.base_url);

        let available: Vec<String> = llm_block_on(async move {
            let resp = client.get(&url).send().await
                .map_err(|e| VaultError::LlmUnavailable(format!("ollama unreachable: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!("ollama HTTP {status}: {body}")));
            }
            let tags: TagsResponse = resp.json().await
                .map_err(|e| VaultError::LlmUnavailable(format!("parse tags: {e}")))?;
            Ok(tags.models.into_iter().map(|m| m.name).collect())
        })?;

        for preferred in PREFERRED_MODELS {
            if available.iter().any(|a| a.starts_with(preferred)) {
                return Ok(Self::with_model(preferred));
            }
        }
        Err(VaultError::LlmUnavailable(format!(
            "no chat model found. Install one of: {}. Run: ollama pull qwen2.5:3b",
            PREFERRED_MODELS.join(", ")
        )))
    }

    fn chat_sync(&self, system: &str, user: &str) -> Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        let body = OllamaChatRequest {
            model: &self.model,
            messages: vec![
                OllamaChatMessage { role: "system", content: system },
                OllamaChatMessage { role: "user", content: user },
            ],
            stream: false,
            // format 不强制为 json：分类场景通过 system prompt 要求 JSON 输出，
            // 避免 format:"json" 破坏通用对话响应
            format: None,
        };
        let client = self.client.clone();
        let body_json = serde_json::to_vec(&body)?;

        llm_block_on(async move {
            let resp = client.post(&url)
                .header("Content-Type", "application/json")
                .body(body_json)
                .send().await
                .map_err(|e| VaultError::LlmUnavailable(format!("chat request: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!("ollama HTTP {status}: {body}")));
            }
            let parsed: OllamaChatResponse = resp.json().await
                .map_err(|e| VaultError::Classification(format!("parse chat response: {e}")))?;
            Ok(parsed.message.content)
        })
    }
}

impl LlmProvider for OllamaLlmProvider {
    fn chat(&self, system: &str, user: &str) -> Result<String> {
        self.chat_sync(system, user)
    }

    fn chat_with_history(&self, messages: &[ChatMessage]) -> Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        let ollama_messages: Vec<serde_json::Value> = messages.iter()
            .map(|m| serde_json::json!({"role": &m.role, "content": &m.content}))
            .collect();
        let body = serde_json::json!({
            "model": &self.model,
            "messages": ollama_messages,
            "stream": false,
        });
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;

        llm_block_on(async move {
            let resp = client.post(&url)
                .header("Content-Type", "application/json")
                .body(body_bytes).send().await
                .map_err(|e| VaultError::LlmUnavailable(format!("chat: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!("ollama HTTP {status}: {body}")));
            }
            let parsed: OllamaChatResponse = resp.json().await
                .map_err(|e| VaultError::Classification(format!("parse: {e}")))?;
            Ok(parsed.message.content)
        })
    }

    fn is_available(&self) -> bool {
        let client = self.client.clone();
        let url = format!("{}/api/tags", self.base_url);
        llm_block_on(async move {
            client.get(&url).send().await
                .map(|_| ())
                .map_err(|e| VaultError::LlmUnavailable(e.to_string()))
        }).is_ok()
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// OpenAI-compatible LLM client
///
/// Works with any OpenAI Chat Completions API compatible backend:
///   - OpenAI:     endpoint = "https://api.openai.com/v1"
///   - Ollama v1:  endpoint = "http://localhost:11434/v1"
///   - LM Studio:  endpoint = "http://localhost:1234/v1"
///   - vLLM:       endpoint = "http://localhost:8000/v1"
pub struct OpenAiLlmProvider {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: String,
}

impl OpenAiLlmProvider {
    pub fn new(endpoint: &str, api_key: &str, model: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    fn chat_sync_impl(&self, messages: &[ChatMessage]) -> Result<String> {
        let url = format!("{}/chat/completions", self.endpoint);
        let body = serde_json::json!({
            "model": &self.model,
            "messages": messages,
            "stream": false,
        });
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;
        let api_key = self.api_key.clone();

        llm_block_on(async move {
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {api_key}"))
                .body(body_bytes)
                .send().await
                .map_err(|e| VaultError::LlmUnavailable(format!("openai request: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!("openai HTTP {status}: {body}")));
            }
            let parsed: OpenAiResponse = resp.json().await
                .map_err(|e| VaultError::Classification(format!("parse openai response: {e}")))?;
            parsed.choices.into_iter().next()
                .map(|c| c.message.content)
                .ok_or_else(|| VaultError::Classification("empty choices".into()))
        })
    }
}

impl LlmProvider for OpenAiLlmProvider {
    fn chat(&self, system: &str, user: &str) -> Result<String> {
        self.chat_sync_impl(&[
            ChatMessage::system(system),
            ChatMessage::user(user),
        ])
    }

    fn chat_with_history(&self, messages: &[ChatMessage]) -> Result<String> {
        self.chat_sync_impl(messages)
    }

    fn is_available(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// 测试专用 Mock，按顺序返回预设响应
pub struct MockLlmProvider {
    responses: Mutex<Vec<String>>,
    model: String,
}

impl MockLlmProvider {
    pub fn new(model: &str) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            model: model.to_string(),
        }
    }

    pub fn push_response(&self, json: &str) {
        self.responses.lock().unwrap().push(json.to_string());
    }
}

impl LlmProvider for MockLlmProvider {
    fn chat(&self, _system: &str, _user: &str) -> Result<String> {
        let mut guard = self.responses.lock().unwrap();
        if guard.is_empty() {
            return Err(VaultError::Classification("no mock response".into()));
        }
        Ok(guard.remove(0))
    }

    fn chat_with_history(&self, _messages: &[ChatMessage]) -> Result<String> {
        // Mock ignores history, returns next preset
        self.chat("", "")
    }

    fn is_available(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_provider_creation() {
        let p = OllamaLlmProvider::with_model("qwen2.5:3b");
        assert_eq!(p.model_name(), "qwen2.5:3b");
    }

    #[test]
    fn mock_provider_returns_preset() {
        let mock = MockLlmProvider::new("test-model");
        mock.push_response(r#"{"hello":"world"}"#);
        let resp = mock.chat("sys", "user").unwrap();
        assert_eq!(resp, r#"{"hello":"world"}"#);
        assert_eq!(mock.model_name(), "test-model");
        assert!(mock.is_available());
    }

    #[test]
    fn mock_provider_errors_when_empty() {
        let mock = MockLlmProvider::new("test");
        let result = mock.chat("sys", "user");
        assert!(result.is_err());
    }

    #[test]
    fn openai_provider_creation() {
        let p = OpenAiLlmProvider::new("https://api.openai.com/v1", "sk-test", "gpt-4o-mini");
        assert_eq!(p.model_name(), "gpt-4o-mini");
        assert!(p.is_available());
    }

    #[test]
    fn chat_message_constructors() {
        let s = ChatMessage::system("sys");
        assert_eq!(s.role, "system");
        assert_eq!(s.content, "sys");

        let u = ChatMessage::user("hi");
        assert_eq!(u.role, "user");

        let a = ChatMessage::assistant("reply");
        assert_eq!(a.role, "assistant");
    }

    #[test]
    fn mock_chat_with_history() {
        let mock = MockLlmProvider::new("test");
        mock.push_response("history reply".into());
        let messages = vec![
            ChatMessage::system("sys prompt"),
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("how are you"),
        ];
        let resp = mock.chat_with_history(&messages).unwrap();
        assert_eq!(resp, "history reply");
    }
}
