// npu-vault/crates/vault-core/src/infer/mod.rs

pub mod embedding;
pub mod model_store;
pub mod provider;
pub mod reranker;

use crate::error::Result;

/// Cross-encoder reranker：对每个 (query, document) 对输出相关性分数
pub trait RerankProvider: Send + Sync {
    /// 返回分数列表 [0.0, 1.0]，顺序与 `documents` 一致
    fn score(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>>;
}

/// 测试用 mock，返回预设分数
pub struct MockRerankProvider {
    scores: std::sync::Mutex<Vec<f32>>,
}

impl MockRerankProvider {
    pub fn new(scores: Vec<f32>) -> Self {
        Self { scores: std::sync::Mutex::new(scores) }
    }
}

impl RerankProvider for MockRerankProvider {
    fn score(&self, _query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        let preset = self.scores.lock().unwrap();
        let result = (0..documents.len())
            .map(|i| *preset.get(i % preset.len().max(1)).unwrap_or(&0.5))
            .collect();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerank_provider_exists() {
        // Verify trait object dispatch works end-to-end
        let mock: Box<dyn RerankProvider> = Box::new(MockRerankProvider::new(vec![0.5]));
        let scores = mock.score("q", &["doc"]).unwrap();
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn mock_reranker_returns_preset_scores() {
        let mock = MockRerankProvider::new(vec![0.9, 0.3, 0.7]);
        let docs = ["doc1", "doc2", "doc3"];
        let scores = mock.score("query", &docs).unwrap();
        assert_eq!(scores, vec![0.9, 0.3, 0.7]);
    }

    #[test]
    fn mock_reranker_cycles_when_fewer_presets_than_docs() {
        let mock = MockRerankProvider::new(vec![0.8]);
        let docs = ["a", "b", "c"];
        let scores = mock.score("q", &docs).unwrap();
        assert_eq!(scores.len(), 3);
        assert!((scores[0] - 0.8).abs() < 1e-5);
        assert!((scores[1] - 0.8).abs() < 1e-5);
        assert!((scores[2] - 0.8).abs() < 1e-5);
    }
}
