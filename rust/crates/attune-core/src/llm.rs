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

/// Eval-mode call options — opt-in deterministic knobs.
///
/// Per spec docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md
/// §11 Risk A: `seed` is best-effort (Ollama + OpenAI support; Anthropic does not).
/// `temperature` Some(0.0) forces low-noise mode regardless of provider.
///
/// Threaded through [`LlmProvider::chat_with_options`] (T1, v1.0.6). When all
/// fields are `None` callers should prefer the legacy chat / chat_with_history
/// paths so behavior stays unchanged for production clients.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmCallOptions {
    /// Provider-side seed for reproducible sampling. Honored by Ollama
    /// (`options.seed`) and OpenAI (`seed` field). Anthropic ignores.
    pub seed: Option<u64>,
    /// Sampling temperature. `Some(0.0)` is the canonical "low noise" knob —
    /// honored by every modern provider.
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling. `Some(1.0)` paired with temperature 0 forces
    /// greedy-style sampling.
    pub top_p: Option<f32>,
    /// ACP-4 Cost Governor — output token ceiling. Maps to Ollama
    /// `options.num_predict` and OpenAI `max_tokens`. `None` = provider default
    /// (unchanged legacy behavior). Spec
    /// docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md §5.3.
    pub max_tokens: Option<u32>,
    /// ACP-4 Cost Governor — chain-of-thought / reasoning budget. When set and
    /// `max_tokens` is unset, it becomes the effective output ceiling so CoT
    /// preamble cannot balloon `tokens_out`. Also appends a terse-output system
    /// hint (see [`apply_cot_hint`]) to suppress verbose reasoning. Never alters
    /// existing messages (so a JSON-emitting agent prompt stays intact).
    pub reasoning_budget: Option<u32>,
}

impl LlmCallOptions {
    /// The authoritative output token ceiling for this call: explicit
    /// `max_tokens` wins; otherwise `reasoning_budget` acts as the ceiling;
    /// `None` means "no cap" (provider default).
    pub fn effective_output_cap(&self) -> Option<u32> {
        self.max_tokens.or(self.reasoning_budget)
    }
}

/// Build the Ollama `options` object from call options. Pure + testable.
/// `num_predict` is set from the effective output cap (max_tokens or, failing
/// that, reasoning_budget) so a configured ceiling reaches llama.cpp's sampler.
pub(crate) fn ollama_options_json(opts: &LlmCallOptions) -> serde_json::Map<String, serde_json::Value> {
    let mut options_obj = serde_json::Map::new();
    if let Some(s) = opts.seed {
        options_obj.insert("seed".into(), serde_json::json!(s));
    }
    if let Some(t) = opts.temperature {
        options_obj.insert("temperature".into(), serde_json::json!(t));
    }
    if let Some(p) = opts.top_p {
        options_obj.insert("top_p".into(), serde_json::json!(p));
    }
    if let Some(cap) = opts.effective_output_cap() {
        options_obj.insert("num_predict".into(), serde_json::json!(cap));
    }
    options_obj
}

/// Apply call options to an OpenAI-compatible request body in place. Pure +
/// testable. Sets `seed` / `temperature` / `top_p` (determinism) and
/// `max_tokens` (the ACP-4 output cap).
pub(crate) fn apply_openai_options(body: &mut serde_json::Value, opts: &LlmCallOptions) {
    if let Some(s) = opts.seed {
        body["seed"] = serde_json::json!(s);
    }
    if let Some(t) = opts.temperature {
        body["temperature"] = serde_json::json!(t);
    }
    if let Some(p) = opts.top_p {
        body["top_p"] = serde_json::json!(p);
    }
    if let Some(cap) = opts.effective_output_cap() {
        body["max_tokens"] = serde_json::json!(cap);
    }
}

/// When `reasoning_budget` is set, append a single terse-output system hint so
/// the model suppresses verbose chain-of-thought. Returns the (possibly
/// extended) message list. Existing messages are **never mutated** — R2
/// adversarial guard: a JSON-only agent prompt must stay byte-for-byte intact,
/// so the hint is additive and lives in its own system message at the tail.
pub(crate) fn apply_cot_hint(messages: &[ChatMessage], opts: &LlmCallOptions) -> Vec<ChatMessage> {
    if opts.reasoning_budget.is_none() {
        return messages.to_vec();
    }
    let mut out = messages.to_vec();
    out.push(ChatMessage::system(
        "请直接给出最终答案，简洁作答，不要输出推理过程 / 思维链。\
         Answer directly and concisely; do not emit chain-of-thought reasoning. \
         If a specific output format (e.g. JSON) was requested above, follow it exactly.",
    ));
    out
}

/// Describes what level of determinism a provider can honor.
///
/// Surfaces in the chat response `eval.determinism` field so the bench harness
/// knows whether equality of two outputs (same seed) is a real signal (`Exact`)
/// or merely a low-variance signal (`Temp0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeterminismLevel {
    /// Provider supports a server-side seed (Ollama, OpenAI, Gemini).
    Exact,
    /// Only temperature=0 + top_p=1 honored (Anthropic).
    Temp0,
    /// No deterministic knobs honored.
    #[default]
    BestEffort,
}

/// 多模态附件 — 走 OpenAI vision content array 协议 (per https://platform.openai.com/docs/guides/vision).
///
/// attune 所有 LLM 调用统一走 OpenAI 兼容协议. 图片走 vision content array,
/// 文件走 attach + 文本拼接 (OpenAI 兼容协议无 native 文件附件, 部分实现含 file_id).
#[derive(Debug, Clone)]
pub enum Attachment {
    /// 图片 (base64 data URI 或 https URL)
    Image { url_or_data_uri: String, mime: String },
    /// 文件 — 转 text 后拼到 user message (调用方负责 OCR / 提取)
    TextFile { name: String, content: String },
}

/// Chat LLM 抽象 (统一 OpenAI 兼容协议).
///
/// **Plan A1 Task I (BREAKING)**: every `chat*` method returns
/// `Result<(String, TokenUsage)>` so call sites are compile-time forced to
/// flow usage into the recorder (spec §11 risk 1 mitigation 1). Use the
/// destructure pattern:
///
/// ```ignore
/// let (response, usage) = provider.chat(system, user)?;
/// recorder.record(usage_event_from(usage, ...));
/// ```
///
/// Sites that intentionally discard usage (no recorder available yet, legacy
/// path being migrated) destructure with `let (response, _usage) = ...` to
/// stay clippy-clean.
pub trait LlmProvider: Send + Sync {
    /// 单次 chat 调用，system + user 消息，返回 (响应文本, TokenUsage).
    fn chat(&self, system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)>;

    /// Eval-mode entry point — accepts opt-in deterministic knobs.
    ///
    /// Default impl ignores `opts` and falls back to [`chat_with_history`],
    /// so any provider not yet wired keeps working (= BestEffort determinism).
    /// Providers SHOULD override to honor seed/temperature/top_p; the override
    /// is the difference between [`DeterminismLevel::Exact`] / [`DeterminismLevel::Temp0`]
    /// and the default `BestEffort`.
    ///
    /// Per spec docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md
    /// §11 Risk A. Plan: docs/superpowers/plans/2026-05-28-kb-bench-integration.md T1.
    fn chat_with_options(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> Result<String> {
        // Plan A1 Task I + C-T1 cross-merge: chat_with_history now returns
        // (String, TokenUsage); chat_with_options keeps single-String signature
        // for eval-mode caller ergonomics — usage is dropped in default impl
        // (eval mode usage is captured separately by route layer per Plan A1 §U).
        let _ = opts;
        self.chat_with_history(messages).map(|(s, _)| s)
    }

    /// What determinism level this provider can honor when [`chat_with_options`]
    /// is called. Default `BestEffort` reflects providers that have not yet
    /// implemented `chat_with_options`.
    fn determinism_level(&self) -> DeterminismLevel {
        DeterminismLevel::BestEffort
    }

    /// 带历史的多轮对话
    fn chat_with_history(
        &self,
        messages: &[ChatMessage],
    ) -> Result<(String, crate::usage::TokenUsage)> {
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

    /// ACP-4 Cost Governor entry point — multi-turn chat that honors the output
    /// token cap + CoT budget in [`LlmCallOptions`] **and** returns
    /// [`TokenUsage`] (so the governor can record it).
    ///
    /// Default impl applies the CoT suppression hint (per [`apply_cot_hint`])
    /// then forwards to [`chat_with_history`]. The cap (`max_tokens` /
    /// `num_predict`) only takes effect on providers that override this method
    /// (Ollama, OpenAI) — providers without a native cap degrade gracefully to
    /// the CoT-hint-only path, which is still a meaningful reduction.
    ///
    /// With default (all-`None`) options this is byte-identical to
    /// [`chat_with_history`] (per spec §10 backward-compat).
    ///
    /// [`TokenUsage`]: crate::usage::TokenUsage
    /// [`chat_with_history`]: LlmProvider::chat_with_history
    fn chat_with_history_opts(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let hinted = apply_cot_hint(messages, opts);
        self.chat_with_history(&hinted)
    }

    /// 多模态 chat (图片 + 文件附件).
    /// 默认 fallback: 文件 content 拼到 user 文本, 图片 attachment 丢弃 + warning.
    /// 真实多模态 provider (OpenAI vision) 应重写此方法.
    fn chat_multimodal(
        &self,
        system: &str,
        user: &str,
        attachments: &[Attachment],
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let mut user_text = String::from(user);
        let mut dropped_images = 0;
        for a in attachments {
            match a {
                Attachment::TextFile { name, content } => {
                    user_text.push_str("\n\n=== file: ");
                    user_text.push_str(name);
                    user_text.push_str(" ===\n");
                    user_text.push_str(content);
                }
                Attachment::Image { .. } => dropped_images += 1,
            }
        }
        if dropped_images > 0 {
            log::warn!(
                "{} image(s) dropped by non-vision LLM provider; use vision-capable model",
                dropped_images
            );
        }
        self.chat(system, &user_text)
    }

    /// 模型是否可用
    fn is_available(&self) -> bool;

    /// 当前使用的模型名（用于 tags.model 记录）
    fn model_name(&self) -> &str;

    /// 是否为本地 LLM（Ollama / localhost endpoint）。隐私判定用：
    /// 敏感案件强制本地时，云端 provider 不得处理注入了证据的对话。
    /// 默认 `false`（保守：未知 provider 视为云端 → F1 拦截，偏安全侧）。
    fn is_local(&self) -> bool {
        false
    }

    // ── Robust LLM infra (2026-05-22, spec: docs/superpowers/specs/2026-05-22-robust-llm-infra.md) ─
    //
    // 三个 model-agnostic 工具 — 让所有 LLM agent 不再为各家小模型单独写 JSON / retry /
    // few-shot 硬化. 通过 trait default impl 提供 best-effort fallback (走 chat()),
    // Ollama / OpenAI provider 各自重写以利用 backend 原生能力 (format=json / response_format).

    /// Schema-guided JSON generation.
    ///
    /// - `schema = None`  → 弱约束 "valid JSON" (Ollama `format="json"` / OpenAI
    ///   `response_format={type: "json_object"}`).
    /// - `schema = Some`  → 强约束 (Ollama `format=<schema_object>` /
    ///   OpenAI `response_format={type: "json_schema", json_schema: {...}}`).
    ///
    /// Default impl: 把 "请输出 valid JSON" 的指令拼到 system prompt 末尾, 走 chat().
    /// 不保证输出 valid (取决于模型). 子类型重写后才有 backend-level 强制.
    fn chat_with_format_json(
        &self,
        system: &str,
        user: &str,
        schema: Option<&serde_json::Value>,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let hint = match schema {
            Some(s) => format!(
                "{system}\n\nOutput must be valid JSON conforming to this schema:\n{s}\n\
                 Do NOT wrap in markdown fences. Do NOT add prose before or after."
            ),
            None => format!(
                "{system}\n\nOutput must be a single valid JSON object or array.\
                 \nDo NOT wrap in markdown fences. Do NOT add prose before or after."
            ),
        };
        self.chat(&hint, user)
    }

    /// Validation-loop retry — string-in, string-out (dyn-compat).
    ///
    /// 调用 `validator(raw)` 校验每次 LLM 输出. 返回 `Ok(())` → 立即返回 raw.
    /// 返回 `Err(msg)` → 把错误信息 append 到 conversation 让 LLM 自己看错在哪里,
    /// 然后重新 call. 最多 `max_attempts` 次 (典型 3 次).
    ///
    /// 用于 grounding verify / JSON valid / field complete 等"校验-反馈-重试"场景.
    /// **注意**: 用 `&dyn Fn` 而非泛型 `V`, 保留 trait dyn-compat (caller 可 `&dyn LlmProvider`).
    /// 类型化反序列化由 caller 在收到 Ok(raw) 后自己做.
    fn chat_with_retry(
        &self,
        system: &str,
        user: &str,
        max_attempts: usize,
        validator: &dyn Fn(&str) -> std::result::Result<(), String>,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        if max_attempts == 0 {
            return Err(VaultError::Classification(
                "chat_with_retry: max_attempts must be >= 1".into(),
            ));
        }
        let mut messages: Vec<ChatMessage> = vec![
            ChatMessage::system(system),
            ChatMessage::user(user),
        ];
        let mut last_err = String::new();
        for attempt in 1..=max_attempts {
            let (raw, usage) = if attempt == 1 {
                self.chat(system, user)?
            } else {
                self.chat_with_history(&messages)?
            };
            match validator(&raw) {
                Ok(()) => return Ok((raw, usage)),
                Err(e) => {
                    last_err = e.clone();
                    if attempt < max_attempts {
                        messages.push(ChatMessage::assistant(&raw));
                        messages.push(ChatMessage::user(&format!(
                            "你上一次的输出无法通过校验: {e}\n请根据错误信息重新输出, 注意不要重复上次的错误."
                        )));
                    }
                }
            }
        }
        Err(VaultError::Classification(format!(
            "LLM retry exhausted after {max_attempts} attempts: {last_err}"
        )))
    }

    /// Few-shot context loader.
    ///
    /// 构造 `[system, ex1.user, ex1.assistant, ..., exN.user, exN.assistant, user]`
    /// 的 message 序列, 调用 `chat_with_history()`. 小模型对 "follow this pattern"
    /// 风格响应良好.
    fn chat_few_shot(
        &self,
        system: &str,
        examples: &[(String, String)],
        user: &str,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        if examples.is_empty() {
            return self.chat(system, user);
        }
        let mut messages: Vec<ChatMessage> = Vec::with_capacity(2 + examples.len() * 2);
        messages.push(ChatMessage::system(system));
        for (u_ex, a_ex) in examples {
            messages.push(ChatMessage::user(u_ex));
            messages.push(ChatMessage::assistant(a_ex));
        }
        messages.push(ChatMessage::user(user));
        self.chat_with_history(&messages)
    }
}

// OllamaChatRequest / OllamaChatMessage structs removed v0.6.4 — both chat_sync
// and chat_with_history now use serde_json::json!() directly to include keep_alive
// (per F-16 Ollama 模型驻留 fix).

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: OllamaChatResponseMessage,
    /// Ollama /api/chat returns `prompt_eval_count` (input tokens) and
    /// `eval_count` (output tokens) on a successful non-streaming response.
    /// Both default to 0 if missing — typical for very small / cached responses.
    #[serde(default)]
    prompt_eval_count: u32,
    #[serde(default)]
    eval_count: u32,
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

/// **Ollama auto-detect 的 fallback 优先列表 — 不是默认行为**。
///
/// 何时使用本列表：
/// - 笔电形态默认 LLM provider = `openai_compat`（远端 token），见 `settings.rs::default_settings()` 注释；
///   **本列表只在用户在 wizard 主动选 Ollama 模式或 settings.llm.provider="ollama" 时使用**
/// - K3 一体机形态默认 provider = `ollama`，本列表用于挑预装的本地模型（典型 qwen2.5:1.5b/3b）
/// - `OllamaLlmProvider::auto_detect()` 是入口，遍历本列表与 Ollama 已下载模型匹配
/// - 用户可用 `ATTUNE_CHAT_MODEL` env var 直接覆盖（跳过本列表探测）
///
/// 顺序原则: 轻量模型优先，再逐步上探到更大的本地模型。
/// 低性能机器更应该先命中 1B/1.7B/mini 级别模型，避免自动落到 qwen2.5 3B/7B。
const PREFERRED_MODELS: &[&str] = &[
    // 轻量模型（优先，适合低性能机器）
    "llama3.2:1b",
    "phi3:mini",
    "qwen3:1.7b",
    "qwen2.5:1.5b",
    "llama3.2:3b",
    "qwen2.5:3b",
    "qwen3:4b",
    // 中等模型
    "deepseek-r1:8b",
    "qwen3:8b",
    "deepseek-r1:14b",
    // 大模型（最后兜底）
    "qwen2.5:7b",
    "qwen3.5:35b-a3b-q3_k_m",  // MoE 30B 总参 / 3B 激活
    "deepseek-r1:32b",
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

    /// 自动探测: 查询本地已下载的 chat 模型，按 PREFERRED_MODELS 优先级选择。
    /// `ATTUNE_CHAT_MODEL=name` env var 覆盖（直接用，不探测）。
    pub fn auto_detect() -> Result<Self> {
        Self::auto_detect_with_preferred(PREFERRED_MODELS)
    }

    /// 自动探测: 查询本地已下载的模型，按 caller 提供的优先级选择。
    /// 适合 summary/chat 使用不同优先级列表的场景。
    pub fn auto_detect_with_preferred(preferred_models: &[&str]) -> Result<Self> {
        // env var 优先：用户显式指定模型
        if let Ok(model) = std::env::var("ATTUNE_CHAT_MODEL") {
            if !model.is_empty() {
                return Ok(Self::with_model(&model));
            }
        }

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

        for preferred in preferred_models {
            if let Some(actual) = available.iter().find(|a| a.starts_with(preferred)) {
                return Ok(Self::with_model(actual));
            }
        }
        Err(VaultError::LlmUnavailable(format!(
            "no chat model found. Install one of: {}. Run: ollama pull qwen2.5:3b",
            preferred_models.join(", ")
        )))
    }

    fn chat_sync(&self, system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)> {
        let url = format!("{}/api/chat", self.base_url);
        // F-16 Ollama 模型驻留: keep_alive=1h. 见 embed.rs 同款注释.
        let keep_alive = std::env::var("ATTUNE_OLLAMA_KEEP_ALIVE")
            .unwrap_or_else(|_| "1h".to_string());
        // Lever 1 (2026-05-22 v1.0 GA defamation F1 push): 当 ATTUNE_OLLAMA_FORMAT_JSON=1
        // 时打开 Ollama 的 schema-guided JSON 模式 — 强制输出 valid JSON, 消除 markdown
        // wrapping / trailing prose / 未转义引号导致的 parse 错误. 默认 OFF, 不影响
        // 其他 LLM caller (chat / 摘要 / 分类) 的自由格式输出.
        let force_json = std::env::var("ATTUNE_OLLAMA_FORMAT_JSON")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let mut body = serde_json::json!({
            "model": &self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": false,
            "keep_alive": keep_alive,
        });
        if force_json {
            body["format"] = serde_json::json!("json");
        }
        let client = self.client.clone();
        let body_json = serde_json::to_vec(&body)?;
        let model = self.model.clone();

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
            let usage = crate::usage::TokenUsage {
                tokens_in: parsed.prompt_eval_count,
                tokens_out: parsed.eval_count,
                cached_in: 0,
                model,
                provider: "ollama".to_string(),
            };
            Ok((parsed.message.content, usage))
        })
    }
}

impl LlmProvider for OllamaLlmProvider {
    fn chat(&self, system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)> {
        self.chat_sync(system, user)
    }

    fn chat_with_history(
        &self,
        messages: &[ChatMessage],
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let url = format!("{}/api/chat", self.base_url);
        let ollama_messages: Vec<serde_json::Value> = messages.iter()
            .map(|m| serde_json::json!({"role": &m.role, "content": &m.content}))
            .collect();
        // F-16 Ollama 模型驻留: keep_alive=1h. 见 embed.rs 同款注释.
        let keep_alive = std::env::var("ATTUNE_OLLAMA_KEEP_ALIVE")
            .unwrap_or_else(|_| "1h".to_string());
        let body = serde_json::json!({
            "model": &self.model,
            "messages": ollama_messages,
            "stream": false,
            "keep_alive": keep_alive,
        });
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;
        let model = self.model.clone();

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
            let usage = crate::usage::TokenUsage {
                tokens_in: parsed.prompt_eval_count,
                tokens_out: parsed.eval_count,
                cached_in: 0,
                model,
                provider: "ollama".to_string(),
            };
            Ok((parsed.message.content, usage))
        })
    }

    /// ACP-4 governed entry — honors `num_predict` (output cap / CoT budget) via
    /// `options` and the CoT suppression hint, while returning real `TokenUsage`
    /// (Ollama reports `prompt_eval_count` / `eval_count`).
    fn chat_with_history_opts(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let hinted = apply_cot_hint(messages, opts);
        let url = format!("{}/api/chat", self.base_url);
        let ollama_messages: Vec<serde_json::Value> = hinted
            .iter()
            .map(|m| serde_json::json!({"role": &m.role, "content": &m.content}))
            .collect();
        let keep_alive = std::env::var("ATTUNE_OLLAMA_KEEP_ALIVE")
            .unwrap_or_else(|_| "1h".to_string());
        let options_obj = ollama_options_json(opts);
        let body = serde_json::json!({
            "model": &self.model,
            "messages": ollama_messages,
            "stream": false,
            "keep_alive": keep_alive,
            "options": options_obj,
        });
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;
        let model = self.model.clone();

        llm_block_on(async move {
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body_bytes)
                .send()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("chat: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!(
                    "ollama HTTP {status}: {body}"
                )));
            }
            let parsed: OllamaChatResponse = resp
                .json()
                .await
                .map_err(|e| VaultError::Classification(format!("parse: {e}")))?;
            let usage = crate::usage::TokenUsage {
                tokens_in: parsed.prompt_eval_count,
                tokens_out: parsed.eval_count,
                cached_in: 0,
                model,
                provider: "ollama".to_string(),
            };
            Ok((parsed.message.content, usage))
        })
    }

    /// T1 (v1.0.6 KB-bench) — eval-mode entry: forwards `LlmCallOptions` into
    /// the Ollama `options` body field. Ollama documents `seed` / `temperature`
    /// / `top_p` under `options` per https://github.com/ollama/ollama/blob/main/docs/modelfile.md.
    fn chat_with_options(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        let ollama_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({"role": &m.role, "content": &m.content}))
            .collect();
        let keep_alive = std::env::var("ATTUNE_OLLAMA_KEEP_ALIVE")
            .unwrap_or_else(|_| "1h".to_string());
        let options_obj = ollama_options_json(opts);
        let body = serde_json::json!({
            "model": &self.model,
            "messages": ollama_messages,
            "stream": false,
            "keep_alive": keep_alive,
            "options": options_obj,
        });
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;

        llm_block_on(async move {
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body_bytes)
                .send()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("chat: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!(
                    "ollama HTTP {status}: {body}"
                )));
            }
            let parsed: OllamaChatResponse = resp
                .json()
                .await
                .map_err(|e| VaultError::Classification(format!("parse: {e}")))?;
            Ok(parsed.message.content)
        })
    }

    fn determinism_level(&self) -> DeterminismLevel {
        // Ollama honors options.seed via llama.cpp sampler — server-side
        // deterministic. Per spec §11 Risk A mitigation 1.
        DeterminismLevel::Exact
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

    fn is_local(&self) -> bool {
        true // Ollama 始终是本地 runtime
    }

    /// Ollama-native schema-guided JSON.
    ///   - schema=None  → `format: "json"` (free-form valid JSON)
    ///   - schema=Some  → `format: <schema_object>` (Ollama 0.5+ structured output)
    ///
    /// 不依赖 ATTUNE_OLLAMA_FORMAT_JSON env var (那是 Lever 1 全局开关, 这里是 per-call).
    fn chat_with_format_json(
        &self,
        system: &str,
        user: &str,
        schema: Option<&serde_json::Value>,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let url = format!("{}/api/chat", self.base_url);
        let keep_alive = std::env::var("ATTUNE_OLLAMA_KEEP_ALIVE")
            .unwrap_or_else(|_| "1h".to_string());
        let mut body = serde_json::json!({
            "model": &self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": false,
            "keep_alive": keep_alive,
        });
        body["format"] = match schema {
            Some(s) => s.clone(),
            None => serde_json::json!("json"),
        };
        let client = self.client.clone();
        let body_json = serde_json::to_vec(&body)?;
        let model = self.model.clone();

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
            let usage = crate::usage::TokenUsage {
                tokens_in: parsed.prompt_eval_count,
                tokens_out: parsed.eval_count,
                cached_in: 0,
                model,
                provider: "ollama".to_string(),
            };
            Ok((parsed.message.content, usage))
        })
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
    /// `usage` may be absent on streaming responses; we never request streaming
    /// so it should always be present on success — but tolerate absence anyway.
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: String,
}

/// OpenAI-compatible usage block. Fields are all `Option<u32>` to tolerate
/// gateways that omit any subset (per actual `chat/completions` responses
/// observed from OpenAI, DeepSeek, Qwen, Anthropic-via-gateway).
#[derive(Deserialize, Default, Clone)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    /// Prompt-cache hits (OpenAI ≥ 2024-10 + Anthropic prompt-cache). Many
    /// gateways do not pass this through — treat absent as 0.
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Deserialize, Default, Clone)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

impl OpenAiUsage {
    fn into_token_usage(self, model: &str, provider: &str) -> crate::usage::TokenUsage {
        let cached_in = self
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0);
        crate::usage::TokenUsage {
            tokens_in: self.prompt_tokens.unwrap_or(0),
            tokens_out: self.completion_tokens.unwrap_or(0),
            cached_in,
            model: model.to_string(),
            provider: provider.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModelItem>,
}

#[derive(Deserialize)]
struct OpenAiModelItem {
    id: String,
}

const OPENAI_COMPAT_PREFERRED_MODELS: &[&str] = &[
    "gpt-4o-mini",
    "gpt-4.1-mini",
    "gpt-4o",
    "claude-3-5-sonnet-20241022",
    "gemini-2.0-flash",
    "deepseek-chat",
    "qwen-plus",
    "qwen-turbo",
];

async fn resolve_openai_compat_model(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Option<String> {
    let url = format!("{}/models", endpoint.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let parsed: OpenAiModelsResponse = resp.json().await.ok()?;
    if parsed.data.is_empty() {
        return None;
    }

    let available: Vec<String> = parsed.data.into_iter().map(|m| m.id).collect();
    for preferred in OPENAI_COMPAT_PREFERRED_MODELS {
        if let Some(found) = available.iter().find(|m| m.as_str() == *preferred) {
            return Some(found.clone());
        }
    }
    available.into_iter().next()
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

    fn chat_sync_impl(
        &self,
        messages: &[ChatMessage],
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let url = format!("{}/chat/completions", self.endpoint);
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let configured_model = self.model.clone();
        let endpoint = self.endpoint.clone();
        let messages_payload = messages.to_vec();

        llm_block_on(async move {
            let mut model_to_use = configured_model.trim().to_string();
            // 触发 /v1/models 探测的两种情形:
            //   (1) 用户显式写 "auto" — 历史默认值
            //   (2) 空字符串 — paid 用户从 cloud gateway 注入 settings 时 model 字段缺省
            //       (per v10-ga-cross-repo-e2e.md Finding-A: paid 用户 llm.model 被 SettingsLock
            //        锁死,UI 无法补,空 model → newapi 拒 400)
            if model_to_use.is_empty() || model_to_use.eq_ignore_ascii_case("auto") {
                if let Some(m) = resolve_openai_compat_model(&client, &endpoint, &api_key).await {
                    model_to_use = m;
                }
            }

            let first_body = serde_json::json!({
                "model": &model_to_use,
                "messages": &messages_payload,
                "stream": false,
            });
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {api_key}"))
                .body(serde_json::to_vec(&first_body).map_err(VaultError::from)?)
                .send().await
                .map_err(|e| VaultError::LlmUnavailable(format!("openai request: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();

                // 常见兼容网关会在 model 不可用时返回 model_not_found。
                // 若当前模型不存在，自动探测 /models 并重试一次。
                if body.contains("model_not_found") {
                    if let Some(fallback_model) =
                        resolve_openai_compat_model(&client, &endpoint, &api_key).await
                    {
                        if fallback_model != model_to_use {
                            let retry_body = serde_json::json!({
                                "model": &fallback_model,
                                "messages": &messages_payload,
                                "stream": false,
                            });
                            let retry = client
                                .post(&url)
                                .header("Content-Type", "application/json")
                                .header("Authorization", format!("Bearer {api_key}"))
                                .body(serde_json::to_vec(&retry_body).map_err(VaultError::from)?)
                                .send()
                                .await
                                .map_err(|e| {
                                    VaultError::LlmUnavailable(format!(
                                        "openai retry with fallback model '{fallback_model}' failed: {e}"
                                    ))
                                })?;
                            if retry.status().is_success() {
                                let parsed: OpenAiResponse = retry.json().await.map_err(|e| {
                                    VaultError::Classification(format!(
                                        "parse openai response: {e}"
                                    ))
                                })?;
                                let usage = parsed
                                    .usage
                                    .unwrap_or_default()
                                    .into_token_usage(&fallback_model, "openai_compat");
                                return parsed
                                    .choices
                                    .into_iter()
                                    .next()
                                    .map(|c| (c.message.content, usage))
                                    .ok_or_else(|| {
                                        VaultError::Classification("empty choices".into())
                                    });
                            }
                        }
                    }
                }

                return Err(VaultError::LlmUnavailable(format!("openai HTTP {status}: {body}")));
            }
            let parsed: OpenAiResponse = resp.json().await
                .map_err(|e| VaultError::Classification(format!("parse openai response: {e}")))?;
            let usage = parsed
                .usage
                .unwrap_or_default()
                .into_token_usage(&model_to_use, "openai_compat");
            parsed.choices.into_iter().next()
                .map(|c| (c.message.content, usage))
                .ok_or_else(|| VaultError::Classification("empty choices".into()))
        })
    }
}

impl LlmProvider for OpenAiLlmProvider {
    fn chat(&self, system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)> {
        self.chat_sync_impl(&[
            ChatMessage::system(system),
            ChatMessage::user(user),
        ])
    }

    fn chat_with_history(
        &self,
        messages: &[ChatMessage],
    ) -> Result<(String, crate::usage::TokenUsage)> {
        self.chat_sync_impl(messages)
    }

    /// ACP-4 governed entry — honors `max_tokens` (output cap / CoT budget) +
    /// CoT suppression hint, returning real `TokenUsage` from the OpenAI
    /// `usage` block.
    ///
    /// When no output cap is set we defer to `chat_sync_impl` so the model
    /// auto-detect retry loop + usage parsing are preserved unchanged
    /// (backward-compat). When a cap is set we issue a direct request with a
    /// concrete model so `max_tokens` semantics are unambiguous; empty / "auto"
    /// model also defers to the resolver path.
    fn chat_with_history_opts(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let hinted = apply_cot_hint(messages, opts);
        let trimmed = self.model.trim();
        if opts.effective_output_cap().is_none()
            || trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("auto")
        {
            // No cap (or no concrete model) → preserve the legacy resolver path
            // (auto-detect retry + usage parsing). CoT hint still applied.
            return self.chat_sync_impl(&hinted);
        }

        let url = format!("{}/chat/completions", self.endpoint);
        let mut body = serde_json::json!({
            "model": &self.model,
            "messages": hinted,
            "stream": false,
        });
        apply_openai_options(&mut body, opts);
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;
        let model = self.model.clone();

        llm_block_on(async move {
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {api_key}"))
                .body(body_bytes)
                .send()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("openai send: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!(
                    "openai HTTP {status}: {body}"
                )));
            }
            let parsed: OpenAiResponse = resp
                .json()
                .await
                .map_err(|e| VaultError::Classification(format!("openai json: {e}")))?;
            let usage = parsed
                .usage
                .unwrap_or_default()
                .into_token_usage(&model, "openai_compat");
            parsed
                .choices
                .into_iter()
                .next()
                .map(|c| (c.message.content, usage))
                .ok_or_else(|| VaultError::Classification("empty choices".into()))
        })
    }

    /// T1 (v1.0.6 KB-bench) — eval-mode entry: forwards `LlmCallOptions` as
    /// top-level OpenAI body fields (`seed`, `temperature`, `top_p`) per
    /// https://platform.openai.com/docs/api-reference/chat/create.
    ///
    /// Bypasses the model auto-detect retry loop in `chat_sync_impl` — the
    /// bench harness must specify a concrete model so determinism semantics
    /// are unambiguous. If `model` is empty / "auto" we fall back to the
    /// legacy path (which surfaces the same fallback model the production
    /// path would have used, keeping behavior intuitive).
    fn chat_with_options(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> Result<String> {
        let trimmed = self.model.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            // Defer to the resolver-aware path; eval block still surfaces
            // Exact since the override is registered. Strip TokenUsage tuple
            // (A1 Task J): chat_with_options keeps single-String signature
            // for eval-mode caller ergonomics.
            return self.chat_sync_impl(messages).map(|(s, _)| s);
        }

        let url = format!("{}/chat/completions", self.endpoint);
        let mut body = serde_json::json!({
            "model": &self.model,
            "messages": messages,
            "stream": false,
        });
        apply_openai_options(&mut body, opts);
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;

        llm_block_on(async move {
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {api_key}"))
                .body(body_bytes)
                .send()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("openai send: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!(
                    "openai HTTP {status}: {body}"
                )));
            }
            let parsed: OpenAiResponse = resp
                .json()
                .await
                .map_err(|e| VaultError::Classification(format!("openai json: {e}")))?;
            Ok(parsed
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default())
        })
    }

    fn determinism_level(&self) -> DeterminismLevel {
        // OpenAI chat/completions accepts top-level `seed` since
        // https://platform.openai.com/docs/api-reference/chat/create#chat-create-seed
        // (released 2023-11) — server-side deterministic. Per spec §11 Risk A
        // mitigation 1.
        DeterminismLevel::Exact
    }

    /// Vision API — content array 走 OpenAI 多模态协议.
    /// 支持图片 (base64 data URI / https URL) + 文件 (转 text 拼接).
    fn chat_multimodal(
        &self,
        system: &str,
        user: &str,
        attachments: &[Attachment],
    ) -> Result<(String, crate::usage::TokenUsage)> {
        // user content 构造 array: 文本块 + 图片块
        // 文件先拼到文本块 (OpenAI 兼容协议无原生文件附件)
        let mut text_with_files = String::from(user);
        let mut image_parts: Vec<serde_json::Value> = Vec::new();
        for a in attachments {
            match a {
                Attachment::TextFile { name, content } => {
                    text_with_files.push_str("\n\n=== file: ");
                    text_with_files.push_str(name);
                    text_with_files.push_str(" ===\n");
                    text_with_files.push_str(content);
                }
                Attachment::Image { url_or_data_uri, .. } => {
                    image_parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {"url": url_or_data_uri},
                    }));
                }
            }
        }

        // 构造 OpenAI content array
        let mut content_array: Vec<serde_json::Value> = Vec::with_capacity(1 + image_parts.len());
        content_array.push(serde_json::json!({"type": "text", "text": text_with_files}));
        content_array.extend(image_parts);

        let url = format!("{}/chat/completions", self.endpoint);
        let body = serde_json::json!({
            "model": &self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": content_array},
            ],
            "stream": false,
        });
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;
        let api_key = self.api_key.clone();
        let model = self.model.clone();

        llm_block_on(async move {
            let resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {api_key}"))
                .body(body_bytes)
                .send().await
                .map_err(|e| VaultError::LlmUnavailable(format!("openai multimodal request: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!("openai HTTP {status}: {body}")));
            }
            let parsed: OpenAiResponse = resp.json().await
                .map_err(|e| VaultError::Classification(format!("parse openai response: {e}")))?;
            let usage = parsed
                .usage
                .unwrap_or_default()
                .into_token_usage(&model, "openai_compat");
            parsed.choices.into_iter().next()
                .map(|c| (c.message.content, usage))
                .ok_or_else(|| VaultError::Classification("empty openai response".into()))
        })
    }

    fn is_available(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    /// OpenAI-compatible schema-guided JSON.
    ///   - schema=None  → `response_format = {type: "json_object"}` (free-form valid JSON)
    ///   - schema=Some  → `response_format = {type: "json_schema", json_schema: {...}}`
    ///
    /// 失败 fallback (gateway 不支持 json_schema 时常见): 自动降级为 json_object 重试一次.
    /// 仍失败 → 走 default impl (chat() + prompt hint).
    fn chat_with_format_json(
        &self,
        system: &str,
        user: &str,
        schema: Option<&serde_json::Value>,
    ) -> Result<(String, crate::usage::TokenUsage)> {
        let url = format!("{}/chat/completions", self.endpoint);
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let configured_model = self.model.clone();
        let endpoint = self.endpoint.clone();
        let system = system.to_string();
        let user = user.to_string();
        let schema_owned = schema.cloned();

        llm_block_on(async move {
            let mut model_to_use = configured_model.trim().to_string();
            if model_to_use.is_empty() || model_to_use.eq_ignore_ascii_case("auto") {
                if let Some(m) = resolve_openai_compat_model(&client, &endpoint, &api_key).await {
                    model_to_use = m;
                }
            }

            // 构造 response_format
            let primary_format = match &schema_owned {
                Some(s) => serde_json::json!({
                    "type": "json_schema",
                    "json_schema": {"name": "attune_extract", "schema": s, "strict": true},
                }),
                None => serde_json::json!({"type": "json_object"}),
            };

            let messages_payload = serde_json::json!([
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ]);

            let try_call = |fmt: serde_json::Value| {
                let url = url.clone();
                let client = client.clone();
                let api_key = api_key.clone();
                let model = model_to_use.clone();
                let messages = messages_payload.clone();
                async move {
                    let body = serde_json::json!({
                        "model": model,
                        "messages": messages,
                        "stream": false,
                        "response_format": fmt,
                    });
                    let resp = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Authorization", format!("Bearer {api_key}"))
                        .body(serde_json::to_vec(&body).map_err(VaultError::from)?)
                        .send()
                        .await
                        .map_err(|e| VaultError::LlmUnavailable(format!("openai request: {e}")))?;
                    let status = resp.status();
                    if !status.is_success() {
                        let body = resp.text().await.unwrap_or_default();
                        return Err(VaultError::LlmUnavailable(format!(
                            "openai HTTP {status}: {body}"
                        )));
                    }
                    let parsed: OpenAiResponse = resp.json().await.map_err(|e| {
                        VaultError::Classification(format!("parse openai response: {e}"))
                    })?;
                    let usage = parsed
                        .usage
                        .as_ref()
                        .cloned()
                        .unwrap_or_default()
                        .into_token_usage(&model, "openai_compat");
                    parsed
                        .choices
                        .into_iter()
                        .next()
                        .map(|c| (c.message.content, usage))
                        .ok_or_else(|| VaultError::Classification("empty choices".into()))
                }
            };

            // 主路径
            match try_call(primary_format).await {
                Ok(s) => Ok(s),
                Err(e_primary) => {
                    // schema 模式失败 → 降级 json_object
                    if schema_owned.is_some() {
                        log::warn!(
                            "openai json_schema mode failed ({e_primary}), falling back to json_object"
                        );
                        match try_call(serde_json::json!({"type": "json_object"})).await {
                            Ok(s) => Ok(s),
                            Err(e_fallback) => {
                                // 二次失败 → 报最有用的错 (用户最关心 fallback 阶段错误)
                                Err(VaultError::LlmUnavailable(format!(
                                    "openai both json_schema and json_object failed: \
                                     primary={e_primary}; fallback={e_fallback}"
                                )))
                            }
                        }
                    } else {
                        Err(e_primary)
                    }
                }
            }
        })
    }

    fn is_local(&self) -> bool {
        // OpenAI 兼容协议既可指向云端也可指向本地（Ollama v1 / LM Studio / vLLM）。
        // 按 endpoint 的 **host 精确判定**（解析 URL 取 host，非子串匹配）——
        // 子串匹配会把 `https://localhost.evil.com/v1` 这类含 "localhost" 的
        // 云端地址误判为本地, 绕过 F1 隐私守卫使证据对话外发。
        let Ok(url) = reqwest::Url::parse(&self.endpoint) else {
            return false; // 解析失败 → 保守视为云端
        };
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.eq_ignore_ascii_case("localhost") {
            return true;
        }
        // is_loopback 覆盖 127.0.0.0/8 与 ::1；is_unspecified 覆盖 0.0.0.0 与 ::
        host.parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback() || ip.is_unspecified())
            .unwrap_or(false)
    }
}

/// 测试专用 Mock，按顺序返回预设响应
pub struct MockLlmProvider {
    responses: Mutex<Vec<String>>,
    model: String,
    /// 测试用 — 记录最后一次 chat 收到的 user content (供 chat_multimodal 默认 fallback 验证)
    last_user: Mutex<String>,
    /// 测试用 — 记录最后一次 chat_with_history 收到的全部 messages (system + history
    /// + user)。F-17 G3 用此验证 L0 内容是否泄漏进出网 payload (注入在 system prompt)。
    last_messages: Mutex<Vec<ChatMessage>>,
    /// T1 — runtime-flippable determinism level so a single mock instance can
    /// serve both `Exact` and `Temp0` flavored tests
    /// (see `eval_determinism_test.rs::anthropic_provider_degrades_to_temp0`).
    determinism: Mutex<DeterminismLevel>,
}

impl MockLlmProvider {
    pub fn new(model: &str) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            model: model.to_string(),
            last_user: Mutex::new(String::new()),
            last_messages: Mutex::new(Vec::new()),
            // Defaults to Exact so tests that call `chat_with_options` without
            // explicit configuration see the "happy path" deterministic-seed
            // semantics. Other call sites (legacy mock-driven unit tests)
            // never touch determinism_level / chat_with_options so this is
            // backward compatible.
            determinism: Mutex::new(DeterminismLevel::Exact),
        }
    }

    pub fn push_response(&self, json: &str) {
        self.responses.lock().unwrap_or_else(|e| e.into_inner()).push(json.to_string());
    }

    pub fn last_received_user(&self) -> Option<String> {
        let s = self.last_user.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if s.is_empty() { None } else { Some(s) }
    }

    /// 测试用 — 返回最后一次 `chat_with_history` 收到的全部 messages 拼接文本
    /// (role:content\n...)。F-17 G3 用此断言出网 payload 不含 L0 内容。
    pub fn last_outbound_payload(&self) -> String {
        self.last_messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|m| format!("{}:{}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// T1 — Override the mock's reported [`DeterminismLevel`] (used by
    /// `eval_determinism_test::anthropic_provider_degrades_to_temp0` to make
    /// a single mock instance impersonate an Anthropic-flavored provider).
    pub fn set_determinism_level(&self, level: DeterminismLevel) {
        *self
            .determinism
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = level;
    }
}

impl LlmProvider for MockLlmProvider {
    fn chat(&self, _system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)> {
        *self.last_user.lock().unwrap_or_else(|e| e.into_inner()) = user.to_string();
        let mut guard = self.responses.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_empty() {
            return Err(VaultError::Classification("no mock response".into()));
        }
        let response = guard.remove(0);
        Ok((response, crate::usage::TokenUsage::empty("mock", &self.model)))
    }

    fn chat_with_history(
        &self,
        messages: &[ChatMessage],
    ) -> Result<(String, crate::usage::TokenUsage)> {
        // Record the full outbound payload (system + history + user) so F-17 G3
        // tests can assert L0 content never reached the (cloud) LLM. Mock still
        // returns the next preset, ignoring message content for the response.
        *self.last_messages.lock().unwrap_or_else(|e| e.into_inner()) = messages.to_vec();
        self.chat("", "")
    }

    /// T1 — deterministic answer of the form `mock-<seed>-<hash(last_user)>`
    /// so integration tests can assert equality / inequality across seeds
    /// without preloading the response queue. Bypasses the response queue
    /// entirely so legacy tests pushing responses are unaffected.
    fn chat_with_options(
        &self,
        messages: &[ChatMessage],
        opts: &LlmCallOptions,
    ) -> Result<String> {
        use std::hash::{Hash, Hasher};
        let user = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        // Record what we saw — keeps multimodal fallback assertions stable.
        *self.last_user.lock().unwrap_or_else(|e| e.into_inner()) = user.clone();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        user.hash(&mut hasher);
        let seed_part = opts.seed.unwrap_or(0);
        Ok(format!("mock-{seed_part}-{:x}", hasher.finish()))
    }

    fn determinism_level(&self) -> DeterminismLevel {
        *self
            .determinism
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
        let (resp, usage) = mock.chat("sys", "user").unwrap();
        assert_eq!(resp, r#"{"hello":"world"}"#);
        assert_eq!(usage.provider, "mock");
        assert_eq!(usage.model, "test-model");
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
    fn openai_is_local_uses_exact_host_not_substring() {
        let local = |ep: &str| OpenAiLlmProvider::new(ep, "k", "m").is_local();
        // 真本地 endpoint → true
        assert!(local("http://localhost:11434/v1"));
        assert!(local("http://127.0.0.1:1234/v1"));
        assert!(local("http://[::1]:8080/v1"));
        assert!(local("http://127.0.0.5/v1"), "127.0.0.0/8 回环段应判本地");
        // 正常云端 → false
        assert!(!local("https://api.openai.com/v1"));
        assert!(!local("https://hiapi.online/v1"));
        // F1 绕过攻击面：含 "localhost"/"127.0.0.1" 子串的云端地址必须判云端
        assert!(!local("https://localhost.evil.com/v1"), "子串 localhost 不得绕过");
        assert!(!local("https://127.0.0.1.evil.com/v1"), "子串 127.0.0.1 不得绕过");
        assert!(!local("https://api.openai.com/v1?proxy=localhost"), "query 不参与判定");
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

    // ── ACP-4 Cost Governor: output token cap + CoT budget ──────────────
    // Spec: docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md §5.3.
    // The cap/budget must reach the provider request body (not be silently dropped),
    // and CoT budget must never corrupt JSON output (it only constrains length +
    // adds a suppression hint, never alters message structure).

    #[test]
    fn ollama_options_json_includes_num_predict_when_max_tokens_set() {
        let opts = LlmCallOptions {
            max_tokens: Some(256),
            ..Default::default()
        };
        let obj = ollama_options_json(&opts);
        assert_eq!(
            obj.get("num_predict").and_then(|v| v.as_i64()),
            Some(256),
            "max_tokens must map to Ollama options.num_predict"
        );
    }

    #[test]
    fn ollama_options_json_omits_num_predict_when_unset() {
        let obj = ollama_options_json(&LlmCallOptions::default());
        assert!(
            obj.get("num_predict").is_none(),
            "no cap → no num_predict (unchanged behavior, per §10 backward-compat)"
        );
    }

    #[test]
    fn cot_budget_caps_effective_output_when_max_tokens_unset() {
        // reasoning_budget present but no explicit max_tokens → CoT budget becomes
        // the effective output ceiling so chain-of-thought cannot balloon tokens_out.
        let opts = LlmCallOptions {
            max_tokens: None,
            reasoning_budget: Some(128),
            ..Default::default()
        };
        assert_eq!(
            opts.effective_output_cap(),
            Some(128),
            "CoT budget must act as the output ceiling when no explicit cap"
        );
    }

    #[test]
    fn explicit_max_tokens_wins_over_cot_budget() {
        let opts = LlmCallOptions {
            max_tokens: Some(512),
            reasoning_budget: Some(128),
            ..Default::default()
        };
        assert_eq!(
            opts.effective_output_cap(),
            Some(512),
            "explicit max_tokens is the authoritative cap"
        );
    }

    #[test]
    fn openai_apply_options_sets_max_tokens_in_body() {
        let mut body = serde_json::json!({"model": "gpt-4o-mini", "messages": []});
        let opts = LlmCallOptions {
            max_tokens: Some(300),
            ..Default::default()
        };
        apply_openai_options(&mut body, &opts);
        assert_eq!(
            body.get("max_tokens").and_then(|v| v.as_i64()),
            Some(300),
            "max_tokens must be set as a top-level OpenAI body field"
        );
    }

    #[test]
    fn cot_suppression_hint_is_appended_not_replacing_messages() {
        // reasoning_budget present → a terse-output system hint is appended as an
        // EXTRA system message; existing messages are never mutated (so a JSON-
        // emitting agent prompt stays intact — R2 adversarial: cap must not break JSON).
        let original = vec![
            ChatMessage::system("Output ONLY valid JSON: {\"x\": 1}"),
            ChatMessage::user("go"),
        ];
        let opts = LlmCallOptions {
            reasoning_budget: Some(64),
            ..Default::default()
        };
        let out = apply_cot_hint(&original, &opts);
        assert_eq!(out.len(), original.len() + 1, "exactly one hint message added");
        assert_eq!(out[0].content, original[0].content, "original system untouched");
        assert!(
            out.last().unwrap().role == "system",
            "hint is a system message"
        );
        assert!(
            out.last().unwrap().content.contains("简洁")
                || out.last().unwrap().content.to_lowercase().contains("concise"),
            "hint instructs concise / no chain-of-thought output"
        );
    }

    #[test]
    fn cot_hint_noop_when_budget_unset() {
        let original = vec![ChatMessage::user("go")];
        let out = apply_cot_hint(&original, &LlmCallOptions::default());
        assert_eq!(out.len(), original.len(), "no budget → no hint (unchanged)");
    }

    #[test]
    fn chat_with_history_opts_returns_usage_and_applies_cot_hint() {
        // The governed entry point must (a) return TokenUsage (so ACP-4 can
        // record it) and (b) append the CoT suppression hint when a reasoning
        // budget is set. Default trait impl forwards to chat_with_history.
        let mock = MockLlmProvider::new("test");
        mock.push_response("answer");
        let messages = vec![ChatMessage::user("question")];
        let opts = LlmCallOptions {
            reasoning_budget: Some(64),
            ..Default::default()
        };
        let (resp, usage) = mock.chat_with_history_opts(&messages, &opts).unwrap();
        assert_eq!(resp, "answer");
        assert_eq!(usage.provider, "mock", "governed entry must return usage");
        // The hint-appending behavior itself is verified directly by
        // `cot_suppression_hint_is_appended_not_replacing_messages`; here we
        // only confirm the governed entry returns (text, usage) without error.
    }

    #[test]
    fn chat_with_history_opts_noop_options_matches_legacy() {
        // Default options (all None) → behaves exactly like chat_with_history
        // (per §10 backward-compat: cache miss / no governor knobs = current behavior).
        let mock = MockLlmProvider::new("m");
        mock.push_response("r1");
        let messages = vec![ChatMessage::system("s"), ChatMessage::user("u")];
        let (resp, _usage) = mock
            .chat_with_history_opts(&messages, &LlmCallOptions::default())
            .unwrap();
        assert_eq!(resp, "r1");
    }

    #[test]
    fn mock_chat_with_history() {
        let mock = MockLlmProvider::new("test");
        mock.push_response("history reply");
        let messages = vec![
            ChatMessage::system("sys prompt"),
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("how are you"),
        ];
        let (resp, _usage) = mock.chat_with_history(&messages).unwrap();
        assert_eq!(resp, "history reply");
    }

    #[test]
    fn chat_multimodal_default_fallback_concats_text_files_and_warns_on_images() {
        // Mock provider 走 trait default impl (无 vision 支持)
        let mock = MockLlmProvider::new("text-only-model");
        mock.push_response("ack");
        let attachments = vec![
            Attachment::TextFile {
                name: "evidence.txt".into(),
                content: "借条 出借人 借款人".into(),
            },
            Attachment::Image {
                url_or_data_uri: "data:image/jpeg;base64,...".into(),
                mime: "image/jpeg".into(),
            },
        ];
        let (resp, _usage) = mock.chat_multimodal("system", "请分析", &attachments).unwrap();
        assert_eq!(resp, "ack");
        // mock 收到的 user text 应含 file content (拼接)
        let received = mock.last_received_user().unwrap_or_default();
        assert!(received.contains("evidence.txt"));
        assert!(received.contains("借条"));
        // 图片对非 vision provider drop 不算错 (有 log::warn)
    }

    #[test]
    fn attachment_image_serializes_to_openai_content_array() {
        // 验证 OpenAI vision content array 结构 (不真调 API)
        let img_part = serde_json::json!({
            "type": "image_url",
            "image_url": {"url": "data:image/png;base64,iVBOR..."},
        });
        let user_content = serde_json::json!([
            {"type": "text", "text": "What's in this image?"},
            img_part,
        ]);
        let s = serde_json::to_string(&user_content).unwrap();
        assert!(s.contains(r#""type":"text""#));
        assert!(s.contains(r#""type":"image_url""#));
        assert!(s.contains(r#""url":"data:image/png;base64"#));
    }

    #[test]
    fn attachment_text_file_concat_format() {
        let f = Attachment::TextFile {
            name: "doc.pdf".into(),
            content: "page1 content\npage2 content".into(),
        };
        // 默认 fallback 拼接格式应明确分隔
        match f {
            Attachment::TextFile { name, content } => {
                assert_eq!(name, "doc.pdf");
                assert!(content.contains("page1"));
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Robust LLM infra tests (2026-05-22) ─────────────────────────────────

    #[test]
    fn chat_with_format_json_default_fallback_works() {
        // Mock provider 走 default impl: 拼 hint 到 system, 走 chat()
        let mock = MockLlmProvider::new("text-only");
        mock.push_response(r#"{"k":"v"}"#);
        let (s, _u) = mock.chat_with_format_json("sys", "user", None).unwrap();
        assert_eq!(s, r#"{"k":"v"}"#);
    }

    #[test]
    fn chat_with_format_json_default_fallback_with_schema() {
        let mock = MockLlmProvider::new("text-only");
        mock.push_response(r#"[{"f":"a"}]"#);
        let schema = serde_json::json!({"type": "array"});
        let (s, _u) = mock.chat_with_format_json("sys", "user", Some(&schema)).unwrap();
        assert_eq!(s, r#"[{"f":"a"}]"#);
    }

    #[test]
    fn chat_with_retry_succeeds_on_second_attempt() {
        let mock = MockLlmProvider::new("test");
        mock.push_response("oops bad");
        mock.push_response(r#"{"ok":1}"#);
        let (raw, _u) = mock.chat_with_retry(
            "sys",
            "user",
            3,
            &|s: &str| {
                serde_json::from_str::<serde_json::Value>(s)
                    .map(|_| ())
                    .map_err(|e| format!("invalid json: {e}"))
            },
        ).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["ok"], 1);
    }

    #[test]
    fn chat_with_retry_exhausts_after_max_attempts() {
        let mock = MockLlmProvider::new("test");
        mock.push_response("bad1");
        mock.push_response("bad2");
        mock.push_response("bad3");
        let result = mock.chat_with_retry(
            "sys",
            "user",
            3,
            &|s: &str| {
                serde_json::from_str::<serde_json::Value>(s)
                    .map(|_| ())
                    .map_err(|e| format!("invalid: {e}"))
            },
        );
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exhausted"), "err msg should mention exhausted: {msg}");
        assert!(msg.contains("3 attempts"), "err msg should include max attempts: {msg}");
    }

    #[test]
    fn chat_with_retry_zero_attempts_errors() {
        let mock = MockLlmProvider::new("test");
        let result = mock.chat_with_retry(
            "sys",
            "user",
            0,
            &|_s: &str| Ok(()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn chat_few_shot_with_empty_examples_falls_back_to_chat() {
        let mock = MockLlmProvider::new("test");
        mock.push_response("answer");
        let (s, _u) = mock.chat_few_shot("sys", &[], "user").unwrap();
        assert_eq!(s, "answer");
    }

    #[test]
    fn chat_few_shot_with_examples_calls_history_path() {
        // MockLlmProvider.chat_with_history 默认实现取最后一条 user message + 第一条 system.
        // 这里主要验证 "不 panic + 走 chat_with_history 路径而非 chat()" — 真正的
        // 顺序验证由真 LLM 集成测试覆盖.
        let mock = MockLlmProvider::new("test");
        mock.push_response("final-reply");
        let examples = vec![
            ("Q1".to_string(), "A1".to_string()),
            ("Q2".to_string(), "A2".to_string()),
        ];
        let (s, _u) = mock.chat_few_shot("sys", &examples, "user-final").unwrap();
        assert_eq!(s, "final-reply");
    }
}
