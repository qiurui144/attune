// npu-vault/crates/vault-core/src/embed.rs

use crate::error::{Result, VaultError};
use serde::Deserialize;
use std::sync::OnceLock;

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
            let keep_alive = std::env::var("ATTUNE_OLLAMA_KEEP_ALIVE")
                .unwrap_or_else(|_| "1h".to_string());
            let body = serde_json::json!({"model": model, "input": input, "keep_alive": keep_alive});
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
                out.push(non_empty_iter.next().unwrap_or_else(|| vec![0.0f32; self.dims]));
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
}
