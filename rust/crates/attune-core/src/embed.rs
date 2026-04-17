// npu-vault/crates/vault-core/src/embed.rs

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};
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
pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
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

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

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

    pub fn default() -> Self {
        Self::new("http://localhost:11434", "bge-m3", 1024)
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

impl EmbeddingProvider for OllamaProvider {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.base_url);
        let model = self.model.clone();
        let input: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let client = self.client.clone();

        let response = embed_block_on(async move {
            let body = serde_json::json!({"model": model, "input": input});
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

        Ok(response.embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn is_available(&self) -> bool {
        self.check_health()
    }
}

/// 无操作 embedding provider（降级模式）
pub struct NoopProvider;

impl EmbeddingProvider for NoopProvider {
    fn embed(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>> {
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
