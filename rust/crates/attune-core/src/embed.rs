// npu-vault/crates/vault-core/src/embed.rs

use crate::edge_cloud::scheduler::{
    SchedulerJobState, SchedulerJobStatus, SchedulerKbTaskResponse,
};
use crate::error::{Result, VaultError};
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 共享 Runtime，复用于所有 Ollama embedding 同步调用（与 llm.rs 中 llm_rt 同理）
fn embed_rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("embed-rt")
            .enable_all()
            .build()
            .expect("embed tokio runtime init failed")
    })
}

/// 在独立线程中运行 async future，复用共享 embed Runtime，
/// 确保不在主 tokio 上下文中直接 block_on。
fn embed_block_on<F, T>(f: F) -> crate::error::Result<T>
where
    F: std::future::Future<Output = crate::error::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(move || embed_rt().block_on(f))
        .join()
        .map_err(|_| VaultError::Crypto("embed worker thread panicked".into()))?
}

/// Embedding provider trait
///
/// Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md` §11 risk 1
/// mitigation 1 — `embed` returns `(Vec<Vec<f32>>, TokenUsage)` so call sites must thread
/// usage through (or explicitly discard via `let (vecs, _usage) = ...`). Ollama's embed
/// endpoint does not expose token counts, so impls estimate via `cost::estimate_tokens`.
pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)>;
    fn dimensions(&self) -> usize;
    fn is_available(&self) -> bool;

    /// 当前 embedding 的身份键。Option A(维度键):返回 `"embed-dim<N>"`,
    /// 必须与 state.rs:1950 写入 memory_vectors.model 的串完全一致 —— 否则
    /// `list_stale_memory_ids` 会把现存行全判 stale → reindex 风暴。providers
    /// 不暴露真实模型名,维度是稳定代理(同维度换模型属语义漂移,留 v.next)。
    fn model_name(&self) -> String {
        format!("embed-dim{}", self.dimensions())
    }
}

/// 当前 active embedding 的 (model, dim) 单一来源。memory 迁移 + organize 共用,
/// 避免各处自拼维度键漂移。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingSignature {
    pub model: String,
    pub dim: usize,
}

/// 取当前 embedding provider 的签名(model + dim 的 SSOT)。
pub fn current_embedding_signature(p: &dyn EmbeddingProvider) -> EmbeddingSignature {
    EmbeddingSignature {
        model: p.model_name(),
        dim: p.dimensions(),
    }
}

/// Ollama HTTP embedding client
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dims: usize,
}

// EmbedRequest 已被 serde_json::json!() 内联构造取代（见 OllamaProvider::embed），
// 不再需要独立结构体。EmbedResponse 仍用于反序列化 Ollama 响应。
#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaProvider {
    pub fn new(base_url: &str, model: &str, dims: usize) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dims,
        }
    }

    /// 检查 Ollama 是否可用
    pub fn check_health(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        let rt = tokio::runtime::Handle::try_current();
        match rt {
            Ok(_handle) => {
                // 在 async 上下文中：在独立线程创建 Runtime 避免 runtime-in-runtime
                let client = self.client.clone();
                std::thread::spawn(move || {
                    let rt = match tokio::runtime::Runtime::new() {
                        Ok(rt) => rt,
                        Err(_) => return false,
                    };
                    rt.block_on(async { client.get(&url).send().await.is_ok() })
                })
                .join()
                .unwrap_or(false)
            }
            Err(_) => {
                // 在 sync 上下文中
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(_) => return false,
                };
                rt.block_on(async { self.client.get(&url).send().await.is_ok() })
            }
        }
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new("http://localhost:11434", "bge-m3", 1024)
    }
}

impl EmbeddingProvider for OllamaProvider {
    fn embed(&self, texts: &[&str]) -> Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)> {
        // 边界保护(per reliability audit 2026-05-24 R20):
        // empty / whitespace-only 输入会让上游 server 返 size=0 embedding 数组或报错.
        // 与 OrtEmbeddingProvider 行为一致:对 empty 输入返 zero vector (零向量在
        // cosine 相似度中会得 0 分,自然 push 出 ranking,不会污染 retrieval).
        // 避免单个 empty chunk 让整批 embed RPC 失败.
        let mut empty_indices = Vec::new();
        let mut non_empty: Vec<&str> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            if t.trim().is_empty() {
                empty_indices.push(i);
            } else {
                non_empty.push(t);
            }
        }
        // Token estimate (Ollama embed endpoint does not return usage)
        // Spec §11 risk 1 mitigation 1 — estimate via cost::estimate_tokens.
        let joined = non_empty.join("");
        let est_tokens = crate::cost::estimate_tokens(&joined, &self.model);
        let usage = crate::usage::TokenUsage {
            tokens_in: est_tokens as u32,
            tokens_out: 0,
            cached_in: 0,
            model: self.model.clone(),
            provider: "ollama".into(),
        };

        // 短路:全 empty
        if non_empty.is_empty() {
            return Ok((vec![vec![0.0f32; self.dims]; texts.len()], usage));
        }

        let url = format!("{}/api/embed", self.base_url);
        let model = self.model.clone();
        let input: Vec<String> = non_empty.iter().map(|s| s.to_string()).collect();
        let client = self.client.clone();

        let response = embed_block_on(async move {
            // F-16 Ollama 模型驻留: keep_alive=1h 让 GPU 加载的模型保留 1 小时,
            // 避免默认 5min 后卸载导致下次 chat 重新加载 (7B 模型 5-10s 重启延迟).
            // 用户可通过 ATTUNE_OLLAMA_KEEP_ALIVE env var override (e.g. "-1" 永久 / "30m" 短驻留).
            let keep_alive =
                std::env::var("ATTUNE_OLLAMA_KEEP_ALIVE").unwrap_or_else(|_| "1h".to_string());
            let body =
                serde_json::json!({"model": model, "input": input, "keep_alive": keep_alive});
            client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("ollama embed request: {e}")))?
                .json::<EmbedResponse>()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("ollama embed response: {e}")))
        })?;

        // 把 empty 占位 zero vec 插回原 index 顺序
        if empty_indices.is_empty() {
            return Ok((response.embeddings, usage));
        }
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        let mut non_empty_iter = response.embeddings.into_iter();
        for i in 0..texts.len() {
            if empty_indices.contains(&i) {
                out.push(vec![0.0f32; self.dims]);
            } else {
                out.push(
                    non_empty_iter
                        .next()
                        .unwrap_or_else(|| vec![0.0f32; self.dims]),
                );
            }
        }
        Ok((out, usage))
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn is_available(&self) -> bool {
        self.check_health()
    }
}

/// OpenAI 兼容 embedding 客户端（`POST {endpoint}/embeddings`）。
///
/// 区别于 [`OllamaProvider`]（Ollama 原生 `/api/embed`）：本 provider 走 OpenAI
/// `/v1/embeddings` 协议（`{"model","input"}` → `{"data":[{"embedding":[...]}],...}`），
/// 因此可指向**任意** OpenAI 兼容 endpoint —— OpenAI、DeepSeek、本地 vLLM / LM Studio、
/// attune Pro gateway，以及 local scheduler（G4 — 本地调度器设备把 embedding 指向本机调度器）。
///
/// `endpoint` 约定为不含 `/embeddings` 的 base（如 `https://api.openai.com/v1`），
/// 与 `llm.rs::OpenAiLlmProvider` 一致。`api_key` 为空时不发 `Authorization` 头
/// （本地 vLLM / 无鉴权 endpoint 友好）。
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    dims: usize,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingResponse {
    data: Vec<OpenAiEmbeddingDatum>,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingDatum {
    embedding: Vec<f32>,
}

impl OpenAiEmbeddingProvider {
    pub fn new(endpoint: &str, api_key: &str, model: &str, dims: usize) -> Self {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let mut client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none());
        if crate::net::destination::is_local_network_url(&endpoint) {
            client = client.no_proxy();
        }
        Self {
            client: client.build().expect("HTTP client"),
            endpoint,
            api_key: api_key.to_string(),
            model: model.to_string(),
            dims,
        }
    }
}

impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn embed(&self, texts: &[&str]) -> Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)> {
        // 与 OllamaProvider 一致的 empty/whitespace 边界保护:empty chunk 占位 zero vec,
        // 避免单个空 chunk 让整批 RPC 失败 / 污染 retrieval(零向量 cosine 得 0 分自然下沉)。
        let mut empty_indices = Vec::new();
        let mut non_empty: Vec<&str> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            if t.trim().is_empty() {
                empty_indices.push(i);
            } else {
                non_empty.push(t);
            }
        }

        let joined = non_empty.join("");
        let est_tokens = crate::cost::estimate_tokens(&joined, &self.model);
        let usage = crate::usage::TokenUsage {
            tokens_in: est_tokens as u32,
            tokens_out: 0,
            cached_in: 0,
            model: self.model.clone(),
            provider: "openai_compat".into(),
        };

        if non_empty.is_empty() {
            return Ok((vec![vec![0.0f32; self.dims]; texts.len()], usage));
        }

        let url = format!("{}/embeddings", self.endpoint);
        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let input: Vec<String> = non_empty.iter().map(|s| s.to_string()).collect();
        let client = self.client.clone();

        let response = embed_block_on(async move {
            let body = serde_json::json!({"model": model, "input": input});
            let mut req = client.post(&url).json(&body);
            // 空 api_key → 不发 Authorization(本地 vLLM / 无鉴权 endpoint)。
            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {api_key}"));
            }
            req.send()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("openai embed request: {e}")))?
                .json::<OpenAiEmbeddingResponse>()
                .await
                .map_err(|e| VaultError::LlmUnavailable(format!("openai embed response: {e}")))
        })?;

        let embeddings: Vec<Vec<f32>> = response.data.into_iter().map(|d| d.embedding).collect();

        // 把 empty 占位 zero vec 插回原 index 顺序。
        if empty_indices.is_empty() {
            return Ok((embeddings, usage));
        }
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        let mut non_empty_iter = embeddings.into_iter();
        for i in 0..texts.len() {
            if empty_indices.contains(&i) {
                out.push(vec![0.0f32; self.dims]);
            } else {
                out.push(
                    non_empty_iter
                        .next()
                        .unwrap_or_else(|| vec![0.0f32; self.dims]),
                );
            }
        }
        Ok((out, usage))
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn is_available(&self) -> bool {
        let url = format!("{}/models", self.endpoint);
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        embed_block_on(async move {
            let mut req = client.get(&url);
            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {api_key}"));
            }
            Ok(req
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false))
        })
        .unwrap_or(false)
    }
}

/// local scheduler-native embedding client.
///
/// Current local scheduler builds expose embedding as application KB tasks
/// (`POST /kb/tasks/kb.query.embed`) rather than the proposed thin
/// `/v1/embeddings` route. This provider submits the KB task, polls `/jobs/{id}`,
/// and extracts OpenAI-style `data[].embedding` from the scheduler output.
pub struct LocalSchedulerEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    task: String,
    model: String,
    dims: usize,
    max_batch_size: usize,
    max_input_chars: usize,
    max_input_tokens: usize,
    poll_timeout: Duration,
}

const DEFAULT_LOCAL_SCHEDULER_EMBED_TASK_BATCH_SIZE: usize = 512;
const MAX_LOCAL_SCHEDULER_EMBED_TASK_BATCH_SIZE: usize = 2048;

impl LocalSchedulerEmbeddingProvider {
    pub fn new(base_url: &str, task: &str, model: &str, dims: usize, poll_timeout_ms: u64) -> Self {
        let base = base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base).to_string();
        let task = task.trim();
        let model = model.trim();
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("HTTP client"),
            base_url: base,
            task: if task.is_empty() {
                "kb.query.embed".to_string()
            } else {
                task.to_string()
            },
            model: if model.is_empty() {
                "embedding-int8".to_string()
            } else {
                model.to_string()
            },
            dims,
            max_batch_size: env_usize_any(
                &[
                    "ATTUNE_SCHEDULER_EMBED_TASK_BATCH_SIZE",
                    "ATTUNE_LOCAL_SCHEDULER_EMBED_TASK_BATCH_SIZE",
                    "ATTUNE_EMBED_TASK_BATCH_SIZE",
                ],
                DEFAULT_LOCAL_SCHEDULER_EMBED_TASK_BATCH_SIZE,
            )
            .clamp(1, MAX_LOCAL_SCHEDULER_EMBED_TASK_BATCH_SIZE),
            max_input_chars: env_usize_any(
                &[
                    "ATTUNE_EMBED_MAX_INPUT_CHARS",
                    "ATTUNE_INDEX_EMBED_MAX_INPUT_CHARS",
                    "ATTUNE_SCHEDULER_EMBED_MAX_INPUT_CHARS",
                    "ATTUNE_LOCAL_EMBED_MAX_INPUT_CHARS",
                ],
                512,
            ),
            max_input_tokens: env_usize_any(
                &[
                    "ATTUNE_EMBED_MAX_INPUT_TOKENS",
                    "ATTUNE_INDEX_EMBED_MAX_INPUT_TOKENS",
                    "ATTUNE_SCHEDULER_EMBED_MAX_INPUT_TOKENS",
                    "ATTUNE_LOCAL_EMBED_MAX_INPUT_TOKENS",
                ],
                256,
            ),
            poll_timeout: Duration::from_millis(poll_timeout_ms.max(1_000)),
        }
    }

    fn truncate_input(text: &str, max_chars: usize, max_tokens: usize) -> String {
        let trimmed = text.trim();
        let token_units_budget = max_tokens.saturating_mul(2).min(max_chars.max(1)).max(2);
        if input_units(trimmed) <= token_units_budget {
            return trimmed.to_string();
        }

        const ELLIPSIS: &str = "\n...\n";
        let ellipsis_units = input_units(ELLIPSIS);
        if token_units_budget <= ellipsis_units + 2 {
            return take_prefix_units(trimmed, token_units_budget);
        }

        let body_budget = token_units_budget - ellipsis_units;
        let head_budget = body_budget / 2 + body_budget % 2;
        let tail_budget = body_budget / 2;
        let head = take_prefix_units(trimmed, head_budget);
        let tail = take_suffix_units(trimmed, tail_budget);
        format!("{}{}{}", head.trim_end(), ELLIPSIS, tail.trim_start())
    }

    fn extract_embedding_vector(v: &Value) -> Option<Vec<f32>> {
        let arr = v.as_array()?;
        let mut out = Vec::with_capacity(arr.len());
        for n in arr {
            out.push(n.as_f64()? as f32);
        }
        Some(out)
    }

    fn extract_data_embeddings(v: &Value) -> Option<Vec<Vec<f32>>> {
        let data = v.as_array()?;
        let mut out = Vec::with_capacity(data.len());
        for datum in data {
            if let Some(embedding) = datum
                .get("embedding")
                .and_then(Self::extract_embedding_vector)
            {
                out.push(embedding);
            } else if let Some(embedding) = Self::extract_embedding_vector(datum) {
                out.push(embedding);
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    fn extract_embeddings(v: &Value) -> Option<Vec<Vec<f32>>> {
        for pointer in [
            "/outputs/data",
            "/data",
            "/outputs/embeddings",
            "/embeddings",
        ] {
            if let Some(vecs) = v.pointer(pointer).and_then(Self::extract_data_embeddings) {
                return Some(vecs);
            }
        }
        for pointer in ["/outputs/embedding", "/embedding"] {
            if let Some(vec) = v.pointer(pointer).and_then(Self::extract_embedding_vector) {
                return Some(vec![vec]);
            }
        }
        v.get("outputs").and_then(Self::extract_embeddings)
    }

    fn reinsert_empty_vectors(
        embeddings: Vec<Vec<f32>>,
        empty_indices: &[usize],
        total_len: usize,
        dims: usize,
    ) -> Vec<Vec<f32>> {
        if empty_indices.is_empty() {
            return embeddings;
        }
        let mut out = Vec::with_capacity(total_len);
        let mut non_empty_iter = embeddings.into_iter();
        for i in 0..total_len {
            if empty_indices.contains(&i) {
                out.push(vec![0.0f32; dims]);
            } else {
                out.push(non_empty_iter.next().unwrap_or_else(|| vec![0.0f32; dims]));
            }
        }
        out
    }

    fn prepare_inputs(raw: &[String], max_chars: usize, max_tokens: usize) -> Vec<String> {
        raw.iter()
            .map(|text| Self::truncate_input(text, max_chars.max(1), max_tokens.max(1)))
            .collect()
    }

    fn scheduler_input_too_large(error: &VaultError) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        message.contains("too large to process")
            || message.contains("physical batch size")
            || message.contains("input too large")
            || message.contains("context length")
    }

    fn scheduler_physical_batch_too_large(error: &VaultError) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        message.contains("physical batch size")
    }

    async fn submit_embedding_task_with_limits(
        client: reqwest::Client,
        base_url: String,
        task: String,
        raw_input: Vec<String>,
        max_input_chars: usize,
        max_input_tokens: usize,
        poll_timeout: Duration,
    ) -> Result<Value> {
        let fallback_limits = [
            (max_input_chars, max_input_tokens),
            (max_input_chars.min(768), max_input_tokens.min(384)),
            (max_input_chars.min(512), max_input_tokens.min(256)),
            (max_input_chars.min(256), max_input_tokens.min(128)),
        ];
        let mut last_limits = (0usize, 0usize);
        let mut last_oversize: Option<VaultError> = None;
        for (chars, tokens) in fallback_limits {
            let limits = (chars.max(1), tokens.max(1));
            if limits == last_limits {
                continue;
            }
            last_limits = limits;
            let input = Self::prepare_inputs(&raw_input, limits.0, limits.1);
            match Self::submit_embedding_task(
                client.clone(),
                base_url.clone(),
                task.clone(),
                input,
                poll_timeout,
            )
            .await
            {
                Ok(value) => return Ok(value),
                Err(e) if Self::scheduler_physical_batch_too_large(&e) => return Err(e),
                Err(e) if Self::scheduler_input_too_large(&e) => {
                    last_oversize = Some(e);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_oversize.unwrap_or_else(|| {
            VaultError::LlmUnavailable(
                "local scheduler embed input exceeded scheduler limits".into(),
            )
        }))
    }

    async fn submit_embedding_task(
        client: reqwest::Client,
        base_url: String,
        task: String,
        input: Vec<String>,
        poll_timeout: Duration,
    ) -> Result<Value> {
        crate::edge_cloud::scheduler::validate_path_segment("task", &task)?;
        let submit_path = format!("/kb/tasks/{task}");
        let submit_url = crate::net::destination::join_local_scheduler_url(
            &base_url,
            &submit_path,
        )
        .ok_or_else(|| {
            VaultError::LlmUnavailable(
                "local scheduler embedding endpoint must use an unambiguous localhost, loopback, or private IP URL"
                    .to_string(),
            )
        })?;
        let body = serde_json::json!({"input": input});
        let resp = client
            .post(&submit_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                VaultError::LlmUnavailable(format!("local scheduler embed submit: {e}"))
            })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(VaultError::LlmUnavailable(format!(
                "local scheduler embed submit HTTP {status}: {text}"
            )));
        }
        let mut value: Value = serde_json::from_str(&text).map_err(|e| {
            VaultError::LlmUnavailable(format!(
                "local scheduler embed submit response: {e}: {text}"
            ))
        })?;
        let mut submit_response: SchedulerKbTaskResponse = serde_json::from_value(value.clone())
            .map_err(|e| {
                VaultError::LlmUnavailable(format!(
                    "local scheduler embed submit response contract: {e}: {text}"
                ))
            })?;
        submit_response.http_status = Some(status.as_u16());
        submit_response.validate_submission(false, "local scheduler embed")?;
        let submit_state = submit_response.normalized_state();
        if submit_state == SchedulerJobState::Failed {
            return Err(VaultError::LlmUnavailable(format!(
                "local scheduler embed task failed: {value}"
            )));
        }
        if submit_state != SchedulerJobState::Succeeded
            && (status.as_u16() == 202 || submit_response.job_id.is_some())
        {
            let job_id = submit_response
                .job_id
                .as_deref()
                .map(str::trim)
                .filter(|job_id| !job_id.is_empty())
                .ok_or_else(|| {
                    VaultError::LlmUnavailable("local scheduler embed missing job_id".into())
                })?
                .to_string();
            crate::edge_cloud::scheduler::validate_path_segment("job_id", &job_id)?;
            let deadline = Instant::now() + poll_timeout;
            loop {
                if Instant::now() >= deadline {
                    return Err(VaultError::LlmUnavailable(format!(
                        "local scheduler embed job {job_id} timed out after {} ms",
                        poll_timeout.as_millis()
                    )));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                let job_path = format!("/jobs/{job_id}");
                let job_url =
                    crate::net::destination::join_local_scheduler_url(&base_url, &job_path)
                        .ok_or_else(|| {
                            VaultError::LlmUnavailable(
                                "local scheduler embedding endpoint became invalid".to_string(),
                            )
                        })?;
                let resp = client.get(&job_url).send().await.map_err(|e| {
                    VaultError::LlmUnavailable(format!("local scheduler embed poll: {e}"))
                })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(VaultError::LlmUnavailable(format!(
                        "local scheduler embed poll HTTP {status}: {text}"
                    )));
                }
                value = serde_json::from_str(&text).map_err(|e| {
                    VaultError::LlmUnavailable(format!(
                        "local scheduler embed job response: {e}: {text}"
                    ))
                })?;
                let job: SchedulerJobStatus =
                    serde_json::from_value(value.clone()).map_err(|e| {
                        VaultError::LlmUnavailable(format!(
                            "local scheduler embed job response contract: {e}: {text}"
                        ))
                    })?;
                match job.normalized_state() {
                    SchedulerJobState::Succeeded => break,
                    SchedulerJobState::Failed => {
                        return Err(VaultError::LlmUnavailable(format!(
                            "local scheduler embed job failed: {}",
                            job.failure_detail().unwrap_or(&job.status)
                        )));
                    }
                    SchedulerJobState::Waiting => {}
                }
            }
        }
        Ok(value)
    }
}

fn input_units(text: &str) -> usize {
    text.chars().map(input_char_units).sum()
}

fn input_char_units(ch: char) -> usize {
    if ch.is_ascii_whitespace() {
        1
    } else if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.') {
        1
    } else {
        2
    }
}

fn take_prefix_units(text: &str, max_units: usize) -> String {
    let mut units = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let next = input_char_units(ch);
        if units + next > max_units {
            break;
        }
        units += next;
        out.push(ch);
    }
    out
}

fn take_suffix_units(text: &str, max_units: usize) -> String {
    let mut units = 0usize;
    let mut chars = Vec::new();
    for ch in text.chars().rev() {
        let next = input_char_units(ch);
        if units + next > max_units {
            break;
        }
        units += next;
        chars.push(ch);
    }
    chars.into_iter().rev().collect()
}

fn env_usize_any(keys: &[&str], default: usize) -> usize {
    keys.iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|v| *v > 0)
        })
        .unwrap_or(default)
}

impl EmbeddingProvider for LocalSchedulerEmbeddingProvider {
    fn embed(&self, texts: &[&str]) -> Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)> {
        if !crate::net::destination::is_safe_local_scheduler_url(&self.base_url) {
            return Err(VaultError::LlmUnavailable(
                "local scheduler embedding endpoint must use an unambiguous localhost, loopback, or private IP URL"
                    .to_string(),
            ));
        }
        crate::edge_cloud::scheduler::validate_path_segment("task", &self.task)?;
        let mut empty_indices = Vec::new();
        let mut raw_non_empty: Vec<String> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            if t.trim().is_empty() {
                empty_indices.push(i);
            } else {
                raw_non_empty.push(t.trim().to_string());
            }
        }

        let first_attempt =
            Self::prepare_inputs(&raw_non_empty, self.max_input_chars, self.max_input_tokens);
        let joined = first_attempt.join("");
        let est_tokens = crate::cost::estimate_tokens(&joined, &self.model);
        let usage = crate::usage::TokenUsage {
            tokens_in: est_tokens as u32,
            tokens_out: 0,
            cached_in: 0,
            model: self.model.clone(),
            provider: "local_scheduler".into(),
        };

        if raw_non_empty.is_empty() {
            return Ok((vec![vec![0.0f32; self.dims]; texts.len()], usage));
        }

        let expected = raw_non_empty.len();
        let client = self.client.clone();
        let base_url = self.base_url.clone();
        let task = self.task.clone();
        let poll_timeout = self.poll_timeout;
        let max_input_chars = self.max_input_chars;
        let max_input_tokens = self.max_input_tokens;
        let max_batch_size = self.max_batch_size.max(1);
        let dims = self.dims;

        let embeddings = embed_block_on(async move {
            let mut pending = std::collections::VecDeque::new();
            let mut start = 0usize;
            while start < expected {
                let end = (start + max_batch_size).min(expected);
                pending.push_back((start, end));
                start = end;
            }

            let mut output: Vec<Option<Vec<f32>>> = vec![None; expected];
            while let Some((start, end)) = pending.pop_front() {
                let raw_batch = raw_non_empty[start..end].to_vec();
                match Self::submit_embedding_task_with_limits(
                    client.clone(),
                    base_url.clone(),
                    task.clone(),
                    raw_batch,
                    max_input_chars,
                    max_input_tokens,
                    poll_timeout,
                )
                .await
                {
                    Ok(value) => {
                        let embeddings = Self::extract_embeddings(&value).ok_or_else(|| {
                            VaultError::LlmUnavailable(format!(
                                "local scheduler embed response missing embeddings for range {start}..{end}: {value}"
                            ))
                        })?;
                        let expected_batch = end - start;
                        if embeddings.len() != expected_batch {
                            return Err(VaultError::LlmUnavailable(format!(
                                "local scheduler embed returned {} vectors for {expected_batch} inputs in range {start}..{end}",
                                embeddings.len()
                            )));
                        }
                        for (offset, embedding) in embeddings.into_iter().enumerate() {
                            output[start + offset] = Some(embedding);
                        }
                    }
                    Err(e) if Self::scheduler_input_too_large(&e) && end - start > 1 => {
                        let mid = start + (end - start) / 2;
                        pending.push_front((mid, end));
                        pending.push_front((start, mid));
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(output
                .into_iter()
                .map(|embedding| embedding.unwrap_or_else(|| vec![0.0f32; dims]))
                .collect::<Vec<_>>())
        })?;

        Ok((
            Self::reinsert_empty_vectors(embeddings, &empty_indices, texts.len(), self.dims),
            usage,
        ))
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn is_available(&self) -> bool {
        if !crate::net::destination::is_safe_local_scheduler_url(&self.base_url)
            || crate::edge_cloud::scheduler::validate_path_segment("model", &self.model).is_err()
        {
            return false;
        }
        let model_path = format!("/models/{}", self.model);
        let Some(url) =
            crate::net::destination::join_local_scheduler_url(&self.base_url, &model_path)
        else {
            return false;
        };
        let client = self.client.clone();
        embed_block_on(async move {
            let resp = client.get(&url).send().await.map_err(|e| {
                VaultError::LlmUnavailable(format!("local scheduler embed availability: {e}"))
            })?;
            if !resp.status().is_success() {
                return Ok(false);
            }
            let value: Value = resp.json().await.map_err(|e| {
                VaultError::LlmUnavailable(format!(
                    "local scheduler embed availability response: {e}"
                ))
            })?;
            let state = value.get("state").and_then(|v| v.as_str()).unwrap_or("");
            Ok(!matches!(state, "UNAVAILABLE" | "FAILED"))
        })
        .unwrap_or(false)
    }

    fn model_name(&self) -> String {
        format!("local-scheduler:{}-dim{}", self.model, self.dims)
    }
}

/// 确定性 mock embedding provider — 仅供测试。
///
/// 把文本按 token（whitespace + 中文逐字）散列成固定维度向量：相同文本得相同向量，
/// 共享 token 的文本向量靠近。无网络、无模型，CI 友好。
#[cfg(any(test, feature = "test-utils"))]
pub struct MockEmbeddingProvider {
    dims: usize,
}

#[cfg(any(test, feature = "test-utils"))]
impl MockEmbeddingProvider {
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }

    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dims];
        // token 粒度：英文按空白切，CJK 逐字 — 让"共享词"的文本向量相近。
        let mut tokens: Vec<String> = Vec::new();
        for ws in text.to_lowercase().split_whitespace() {
            let mut latin = String::new();
            for ch in ws.chars() {
                if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                    if !latin.is_empty() {
                        tokens.push(std::mem::take(&mut latin));
                    }
                    tokens.push(ch.to_string());
                } else {
                    latin.push(ch);
                }
            }
            if !latin.is_empty() {
                tokens.push(latin);
            }
        }
        for tok in tokens {
            let mut h: u64 = 1469598103934665603;
            for b in tok.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(1099511628211);
            }
            let idx = (h % self.dims as u64) as usize;
            v[idx] += 1.0;
        }
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        } else {
            // 空文本 → 任意非零单位向量，避免 usearch cos 距离 NaN。
            v[0] = 1.0;
        }
        v
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl EmbeddingProvider for MockEmbeddingProvider {
    fn embed(&self, texts: &[&str]) -> Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)> {
        let vecs: Vec<Vec<f32>> = texts.iter().map(|t| self.embed_one(t)).collect();
        Ok((vecs, crate::usage::TokenUsage::empty("mock", "mock")))
    }
    fn dimensions(&self) -> usize {
        self.dims
    }
    fn is_available(&self) -> bool {
        true
    }
}

/// 无操作 embedding provider（降级模式）
pub struct NoopProvider;

impl EmbeddingProvider for NoopProvider {
    fn embed(&self, _texts: &[&str]) -> Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)> {
        Err(VaultError::Crypto("no embedding provider available".into()))
    }
    fn dimensions(&self) -> usize {
        0
    }
    fn is_available(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvRestore {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvRestore {
        fn new(keys: &[&'static str]) -> Self {
            Self {
                saved: keys
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut header_end = None;
        while header_end.is_none() {
            let n = stream.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            header_end = buf.windows(4).position(|w| w == b"\r\n\r\n");
        }
        let Some(header_end) = header_end.map(|idx| idx + 4) else {
            return String::from_utf8_lossy(&buf).to_string();
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        while buf.len().saturating_sub(header_end) < content_length {
            let n = stream.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    fn http_request_body(request: &str) -> &str {
        request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .unwrap_or("")
    }

    #[test]
    fn signature_is_dimension_keyed() {
        let p = MockEmbeddingProvider::new(1024);
        let sig = current_embedding_signature(&p);
        // 维度键:与 state.rs:1950 写 memory_vectors 的 "embed-dim<N>" 一致
        assert_eq!(
            sig,
            EmbeddingSignature {
                model: "embed-dim1024".into(),
                dim: 1024
            }
        );
    }

    #[test]
    fn noop_provider_not_available() {
        let provider = NoopProvider;
        assert!(!provider.is_available());
        assert!(provider.embed(&["test"]).is_err());
        assert_eq!(provider.dimensions(), 0);
    }

    #[test]
    fn ollama_provider_creation() {
        let provider = OllamaProvider::new("http://localhost:11434", "bge-m3", 1024);
        assert_eq!(provider.dimensions(), 1024);
        // 不测试实际连接（CI 环境可能无 Ollama）
    }

    #[test]
    fn openai_embedding_provider_creation_trims_endpoint() {
        let p = OpenAiEmbeddingProvider::new(
            "https://api.openai.com/v1/",
            "sk-x",
            "text-embedding-3-small",
            1536,
        );
        assert_eq!(p.dimensions(), 1536);
        assert_eq!(p.endpoint, "https://api.openai.com/v1"); // trailing slash trimmed
    }

    /// G4 — proves the OpenAI-compatible embedding provider routes its request to a
    /// **configurable** endpoint (not a hardcoded Ollama URL) and parses the
    /// `/v1/embeddings` response shape. Uses a hand-rolled single-shot TcpListener
    /// mock (no new dev-deps); the captured request line + body assert that the
    /// custom base URL + model were honoured — exactly the local-scheduler routing G4 wants.
    #[test]
    fn openai_embedding_routes_to_custom_endpoint() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let cap2 = captured.clone();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            *cap2.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_string();
            // OpenAI /v1/embeddings response shape: data[].embedding
            let body =
                r#"{"data":[{"embedding":[0.1,0.2,0.3,0.4]}],"model":"local-scheduler-embed"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        });

        let endpoint = format!("http://127.0.0.1:{port}/v1");
        let provider =
            OpenAiEmbeddingProvider::new(&endpoint, "sk-test", "local-scheduler-embed", 4);
        let (vecs, usage) = provider
            .embed(&["hello local scheduler"])
            .expect("embed ok");

        handle.join().unwrap();

        assert_eq!(vecs.len(), 1);
        assert_eq!(vecs[0], vec![0.1f32, 0.2, 0.3, 0.4]);
        assert_eq!(usage.provider, "openai_compat");
        assert_eq!(usage.model, "local-scheduler-embed");

        // reqwest lowercases header names on the wire — match case-insensitively.
        let req = captured.lock().unwrap().clone();
        let req_lc = req.to_lowercase();
        // Routed to the configured endpoint's /embeddings path, with bearer auth + the configured model.
        assert!(
            req.starts_with("POST /v1/embeddings "),
            "request line was: {req}"
        );
        assert!(
            req_lc.contains("authorization: bearer sk-test"),
            "missing bearer auth: {req}"
        );
        assert!(
            req.contains("\"model\":\"local-scheduler-embed\""),
            "model not in body: {req}"
        );
    }

    /// Empty / whitespace inputs short-circuit to a zero vector without any network
    /// call — same boundary contract as OllamaProvider / OrtEmbeddingProvider.
    #[test]
    fn openai_embedding_all_empty_short_circuits() {
        // Endpoint is bogus on purpose: if the short-circuit fails we'd get a network error.
        let provider = OpenAiEmbeddingProvider::new("http://127.0.0.1:1/v1", "", "m", 3);
        let (vecs, _usage) = provider.embed(&["", "   "]).expect("empty short-circuits");
        assert_eq!(vecs.len(), 2);
        assert_eq!(vecs[0], vec![0.0f32; 3]);
        assert_eq!(vecs[1], vec![0.0f32; 3]);
    }

    #[test]
    fn local_scheduler_embedding_polls_async_job() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let cap2 = captured.clone();

        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                cap2.lock().unwrap().push(req.clone());
                let body = if req.starts_with("POST /kb/tasks/kb.query.embed ") {
                    r#"{"job_id":"job_1","status":"queued"}"#
                } else {
                    r#"{"job_id":"job_1","status":"done","outputs":{"data":[{"embedding":[0.1,0.2,0.3,0.4]}]}}"#
                };
                let status = if req.starts_with("POST ") {
                    "202 Accepted"
                } else {
                    "200 OK"
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });

        let endpoint = format!("http://127.0.0.1:{port}");
        let provider = LocalSchedulerEmbeddingProvider::new(
            &endpoint,
            "kb.query.embed",
            "embedding-int8",
            4,
            1_000,
        );
        let (vecs, usage) = provider
            .embed(&["hello local scheduler"])
            .expect("embed ok");

        handle.join().unwrap();

        assert_eq!(vecs, vec![vec![0.1f32, 0.2, 0.3, 0.4]]);
        assert_eq!(usage.provider, "local_scheduler");
        assert_eq!(usage.model, "embedding-int8");
        let reqs = captured.lock().unwrap();
        assert!(reqs[0].starts_with("POST /kb/tasks/kb.query.embed "));
        assert!(reqs[1].starts_with("GET /jobs/job_1 "));
    }

    #[test]
    fn local_scheduler_embedding_rejects_completed_job_with_error_payload() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let (status, body) = if request_index == 0 {
                    (
                        "202 Accepted",
                        r#"{"job_id":"job_error","status":"queued"}"#,
                    )
                } else {
                    (
                        "200 OK",
                        r#"{"job_id":"job_error","status":"done","error":"OOM","outputs":{"data":[{"embedding":[0.1,0.2]}]}}"#,
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let provider = LocalSchedulerEmbeddingProvider::new(
            &format!("http://127.0.0.1:{port}"),
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        let error = provider
            .embed(&["must fail closed"])
            .expect_err("a completed job with an error payload must not return vectors");
        assert!(error.to_string().contains("OOM"), "error={error}");
        handle.join().unwrap();
    }

    #[test]
    fn local_scheduler_embedding_rejects_200_waiting_or_unknown_without_job_id() {
        use std::io::Write;
        use std::net::TcpListener;

        for pending_status in ["queued", "running", "future_scheduler_state"] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().unwrap().port();
            let handle = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept");
                let _ = read_http_request(&mut stream);
                let body = serde_json::json!({
                    "status": pending_status,
                    "outputs": {}
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            });

            let endpoint = format!("http://127.0.0.1:{port}");
            let provider = LocalSchedulerEmbeddingProvider::new(
                &endpoint,
                "kb.query.embed",
                "embedding-int8",
                4,
                1_000,
            );
            let error = provider
                .embed(&["malformed pending response"])
                .expect_err("200 waiting/unknown response without job_id must fail closed");
            assert!(
                error.to_string().contains("without job_id"),
                "status={pending_status}, error={error}"
            );
            handle.join().unwrap();
        }
    }

    #[test]
    fn local_scheduler_embedding_rejects_202_completed_body_without_job_id() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = read_http_request(&mut stream);
            let body = serde_json::json!({
                "status": "done",
                "outputs": {"data": [{"embedding": [0.1, 0.2]}]}
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let provider = LocalSchedulerEmbeddingProvider::new(
            &format!("http://127.0.0.1:{port}"),
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        let error = provider
            .embed(&["untrackable accepted job"])
            .expect_err("HTTP 202 must carry a job id even when its body says done");
        assert!(
            error.to_string().contains("without job_id"),
            "error={error}"
        );
        handle.join().unwrap();
    }

    #[test]
    fn local_scheduler_embedding_splits_physical_batch_oversize() {
        use std::io::Write;
        use std::net::TcpListener;

        let _env = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _restore = EnvRestore::new(&["ATTUNE_SCHEDULER_EMBED_TASK_BATCH_SIZE"]);
        std::env::set_var("ATTUNE_SCHEDULER_EMBED_TASK_BATCH_SIZE", "4");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let seen_lengths = std::sync::Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));
        let seen2 = seen_lengths.clone();

        let handle = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().expect("accept");
                let req = read_http_request(&mut stream);
                let body: Value = serde_json::from_str(http_request_body(&req)).unwrap();
                let input_len = body
                    .get("input")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                seen2.lock().unwrap().push(input_len);

                if input_len > 2 {
                    let body = "physical batch size exceeded";
                    let resp = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    continue;
                }

                let data = (0..input_len)
                    .map(|idx| serde_json::json!({"embedding": [input_len as f32, idx as f32]}))
                    .collect::<Vec<_>>();
                let body = serde_json::json!({
                    "status": "done",
                    "outputs": {"data": data}
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });

        let endpoint = format!("http://127.0.0.1:{port}");
        let provider = LocalSchedulerEmbeddingProvider::new(
            &endpoint,
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        let (vecs, _usage) = provider.embed(&["a", "b", "c", "d"]).expect("embed ok");

        handle.join().unwrap();
        assert_eq!(*seen_lengths.lock().unwrap(), vec![4, 2, 2]);
        assert_eq!(vecs.len(), 4);
        assert_eq!(vecs[0], vec![2.0, 0.0]);
        assert_eq!(vecs[3], vec![2.0, 1.0]);
    }

    #[test]
    fn local_scheduler_embedding_batch_size_defaults_and_clamps_for_large_hosts() {
        let _env = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _restore = EnvRestore::new(&["ATTUNE_SCHEDULER_EMBED_TASK_BATCH_SIZE"]);

        std::env::remove_var("ATTUNE_SCHEDULER_EMBED_TASK_BATCH_SIZE");
        let provider = LocalSchedulerEmbeddingProvider::new(
            "http://127.0.0.1:1",
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        assert_eq!(provider.max_batch_size, 512);

        std::env::set_var("ATTUNE_SCHEDULER_EMBED_TASK_BATCH_SIZE", "1024");
        let provider = LocalSchedulerEmbeddingProvider::new(
            "http://127.0.0.1:1",
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        assert_eq!(provider.max_batch_size, 1024);

        std::env::set_var("ATTUNE_SCHEDULER_EMBED_TASK_BATCH_SIZE", "999999");
        let provider = LocalSchedulerEmbeddingProvider::new(
            "http://127.0.0.1:1",
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        assert_eq!(provider.max_batch_size, 2048);
    }

    #[test]
    fn local_scheduler_embedding_public_endpoint_is_never_available() {
        let provider = LocalSchedulerEmbeddingProvider::new(
            "https://scheduler.example.test:8090",
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        assert!(!provider.is_available());
    }

    #[test]
    fn local_scheduler_embedding_rejects_ambiguous_base_and_path_segments() {
        for endpoint in [
            "http://user@127.0.0.1:8090",
            "http://127.0.0.1:8090/admin?target=/kb/tasks/kb.query.embed",
            "http://169.254.169.254/latest",
        ] {
            let provider = LocalSchedulerEmbeddingProvider::new(
                endpoint,
                "kb.query.embed",
                "embedding-int8",
                2,
                1_000,
            );
            let error = provider
                .embed(&["secret input"])
                .expect_err("ambiguous scheduler base must fail before transport");
            assert!(
                error.to_string().contains("must use an unambiguous"),
                "endpoint={endpoint}, error={error}"
            );
            assert!(!provider.is_available(), "endpoint={endpoint}");
        }

        let unsafe_task = LocalSchedulerEmbeddingProvider::new(
            "http://127.0.0.1:1",
            "../../admin",
            "embedding-int8",
            2,
            1_000,
        );
        let error = unsafe_task
            .embed(&["secret input"])
            .expect_err("task must remain one safe path segment");
        assert!(error.to_string().contains("invalid local scheduler task"));

        let unsafe_model = LocalSchedulerEmbeddingProvider::new(
            "http://127.0.0.1:1",
            "kb.query.embed",
            "../admin",
            2,
            1_000,
        );
        assert!(!unsafe_model.is_available());
    }

    #[test]
    fn local_scheduler_embedding_rejects_unsafe_job_id_before_poll() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = read_http_request(&mut stream);
            let body = r#"{"job_id":"../../admin","status":"queued"}"#;
            let response = format!(
                "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let provider = LocalSchedulerEmbeddingProvider::new(
            &format!("http://127.0.0.1:{port}"),
            "kb.query.embed",
            "embedding-int8",
            2,
            1_000,
        );
        let error = provider
            .embed(&["secret input"])
            .expect_err("job id must remain one safe path segment");
        assert!(error.to_string().contains("invalid local scheduler job_id"));
        handle.join().unwrap();
    }

    #[test]
    fn local_scheduler_embedding_truncates_long_inputs() {
        let text = "  abcdef  ";
        assert_eq!(
            LocalSchedulerEmbeddingProvider::truncate_input(text, 3, 480),
            "abc"
        );
        assert_eq!(
            LocalSchedulerEmbeddingProvider::truncate_input("short", 64, 480),
            "short"
        );
        let projected =
            LocalSchedulerEmbeddingProvider::truncate_input("a".repeat(2000).as_str(), 2000, 256);
        assert!(
            projected.len() <= 512,
            "projected input too long: {}",
            projected.len()
        );
        assert!(projected.contains("\n...\n"));
    }

    #[test]
    fn local_scheduler_embedding_classifies_scheduler_oversize_errors() {
        let err = VaultError::LlmUnavailable(
            "local scheduler embed poll HTTP 500: input (673 tokens) is too large to process. increase the physical batch size (current batch size: 512)"
                .into(),
        );
        assert!(
            LocalSchedulerEmbeddingProvider::scheduler_input_too_large(&err),
            "scheduler physical batch errors must trigger smaller-input retry"
        );

        let inputs = vec!["a".repeat(2000)];
        let prepared = LocalSchedulerEmbeddingProvider::prepare_inputs(&inputs, 512, 256);
        assert_eq!(prepared.len(), 1);
        assert!(
            prepared[0].len() <= 512,
            "fallback input should respect char cap"
        );
    }

    #[test]
    fn scheduler_embedding_limits_use_generic_env() {
        let _env = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let saved_generic = std::env::var("ATTUNE_EMBED_MAX_INPUT_CHARS").ok();
        std::env::set_var("ATTUNE_EMBED_MAX_INPUT_CHARS", "123");

        assert_eq!(env_usize_any(&["ATTUNE_EMBED_MAX_INPUT_CHARS"], 512), 123);

        match saved_generic {
            Some(v) => std::env::set_var("ATTUNE_EMBED_MAX_INPUT_CHARS", v),
            None => std::env::remove_var("ATTUNE_EMBED_MAX_INPUT_CHARS"),
        }
    }
}
