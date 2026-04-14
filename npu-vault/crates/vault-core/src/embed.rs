// npu-vault/crates/vault-core/src/embed.rs

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};

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
                // 在 async 上下文中
                let client = self.client.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async { client.get(&url).send().await.is_ok() })
                })
                .join()
                .unwrap_or(false)
            }
            Err(_) => {
                // 在 sync 上下文中
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async { self.client.get(&url).send().await.is_ok() })
            }
        }
    }
}

impl EmbeddingProvider for OllamaProvider {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.base_url);
        let body = EmbedRequest {
            model: &self.model,
            input: texts.to_vec(),
        };

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| VaultError::Crypto(format!("tokio runtime: {e}")))?;

        let response = rt.block_on(async {
            self.client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| VaultError::Crypto(format!("ollama request: {e}")))?
                .json::<EmbedResponse>()
                .await
                .map_err(|e| VaultError::Crypto(format!("ollama response: {e}")))
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
