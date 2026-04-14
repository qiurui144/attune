use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const DEFAULT_MIN_ITEMS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub id: i32,
    pub name: String,
    pub summary: String,
    pub item_count: usize,
    pub item_ids: Vec<String>,
    pub representative_item_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterSnapshot {
    pub version: u32,
    pub generated_at: String,
    pub algorithm: String,
    pub model: String,
    pub clusters: Vec<Cluster>,
    pub noise_item_ids: Vec<String>,
}

impl ClusterSnapshot {
    pub fn empty() -> Self {
        Self {
            version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            algorithm: "hdbscan".into(),
            model: String::new(),
            clusters: vec![],
            noise_item_ids: vec![],
        }
    }
}

/// 聚类输入: (item_id, title, content_snippet, embedding)
#[derive(Debug, Clone)]
pub struct ClusterInput {
    pub item_id: String,
    pub title: String,
    pub content_snippet: String,
    pub embedding: Vec<f32>,
}

pub struct Clusterer {
    llm: Arc<dyn LlmProvider>,
    min_items: usize,
}

impl Clusterer {
    pub fn new(llm: Arc<dyn LlmProvider>) -> Self {
        Self { llm, min_items: DEFAULT_MIN_ITEMS }
    }

    pub fn with_min_items(mut self, min: usize) -> Self {
        self.min_items = min;
        self
    }

    pub fn rebuild(&self, inputs: Vec<ClusterInput>) -> Result<ClusterSnapshot> {
        if inputs.len() < self.min_items {
            return Ok(ClusterSnapshot::empty());
        }

        let labels = self.run_hdbscan(&inputs)?;

        let mut groups: std::collections::BTreeMap<i32, Vec<usize>> = std::collections::BTreeMap::new();
        for (i, label) in labels.iter().enumerate() {
            groups.entry(*label).or_default().push(i);
        }

        let mut clusters: Vec<Cluster> = Vec::new();
        let mut noise_ids: Vec<String> = Vec::new();

        for (label, indices) in groups {
            if label == -1 {
                noise_ids = indices.iter().map(|&i| inputs[i].item_id.clone()).collect();
                continue;
            }

            let reps: Vec<&ClusterInput> = indices.iter().take(3).map(|&i| &inputs[i]).collect();
            let (name, summary) = self.name_cluster(&reps)
                .unwrap_or_else(|_| (format!("聚类 {label}"), "未命名".into()));

            let item_ids: Vec<String> = indices.iter().map(|&i| inputs[i].item_id.clone()).collect();
            let rep_id = item_ids.first().cloned();

            clusters.push(Cluster {
                id: label,
                name,
                summary,
                item_count: indices.len(),
                item_ids,
                representative_item_id: rep_id,
            });
        }

        Ok(ClusterSnapshot {
            version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            algorithm: "hdbscan".into(),
            model: self.llm.model_name().to_string(),
            clusters,
            noise_item_ids: noise_ids,
        })
    }

    fn run_hdbscan(&self, inputs: &[ClusterInput]) -> Result<Vec<i32>> {
        let dataset: Vec<Vec<f32>> = inputs.iter().map(|i| i.embedding.clone()).collect();
        let clusterer = hdbscan::Hdbscan::default_hyper_params(&dataset);
        let labels = clusterer
            .cluster()
            .map_err(|e| VaultError::Classification(format!("hdbscan: {e:?}")))?;
        Ok(labels)
    }

    fn name_cluster(&self, reps: &[&ClusterInput]) -> Result<(String, String)> {
        let system = "你是一个知识库聚类命名助手。给定一组相关的知识片段，生成简洁的主题名和一句话摘要。";
        let rep_texts: Vec<String> = reps.iter().map(|r| {
            let snippet: String = r.content_snippet.chars().take(300).collect();
            format!("- {}: {}", r.title, snippet)
        }).collect();
        let user = format!(
            "以下是一个聚类中的 {} 个代表样本:\n\n{}\n\n请输出 JSON:\n{{\"name\": \"主题名 (8-15 字)\", \"summary\": \"一句话摘要 (20-40 字)\"}}",
            reps.len(),
            rep_texts.join("\n")
        );
        let raw = self.llm.chat(system, &user)?;
        let trimmed = raw.trim();
        let json_str = if let Some(start) = trimmed.find('{') {
            if let Some(end) = trimmed.rfind('}') {
                &trimmed[start..=end]
            } else {
                trimmed
            }
        } else {
            trimmed
        };
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| VaultError::Classification(format!("cluster name json: {e}")))?;
        let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("未命名").to_string();
        let summary = parsed.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Ok((name, summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmProvider;

    fn make_inputs(n: usize) -> Vec<ClusterInput> {
        (0..n).map(|i| ClusterInput {
            item_id: format!("id{i}"),
            title: format!("Title {i}"),
            content_snippet: format!("content {i}"),
            embedding: vec![(i as f32) * 0.1, (i as f32) * 0.2, 0.3, 0.4],
        }).collect()
    }

    #[test]
    fn below_min_returns_empty_snapshot() {
        let mock = Arc::new(MockLlmProvider::new("m"));
        let clusterer = Clusterer::new(mock).with_min_items(20);
        let inputs = make_inputs(5);
        let snapshot = clusterer.rebuild(inputs).unwrap();
        assert!(snapshot.clusters.is_empty());
    }

    #[test]
    fn snapshot_empty_default() {
        let s = ClusterSnapshot::empty();
        assert!(s.clusters.is_empty());
        assert!(s.noise_item_ids.is_empty());
        assert_eq!(s.algorithm, "hdbscan");
    }

    #[test]
    fn cluster_serializable() {
        let c = Cluster {
            id: 0,
            name: "test".into(),
            summary: "sum".into(),
            item_count: 3,
            item_ids: vec!["a".into(), "b".into(), "c".into()],
            representative_item_id: Some("a".into()),
        };
        let json = serde_json::to_string(&c).unwrap();
        let parsed: Cluster = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
    }

    #[test]
    fn name_cluster_parses_llm_response() {
        let mock = Arc::new(MockLlmProvider::new("m"));
        mock.push_response(r#"{"name": "Rust 加密研究", "summary": "围绕 vault-core 的加密实现"}"#);
        let clusterer = Clusterer::new(mock);
        let inputs = make_inputs(3);
        let refs: Vec<&ClusterInput> = inputs.iter().collect();
        let (name, summary) = clusterer.name_cluster(&refs).unwrap();
        assert_eq!(name, "Rust 加密研究");
        assert_eq!(summary, "围绕 vault-core 的加密实现");
    }
}
