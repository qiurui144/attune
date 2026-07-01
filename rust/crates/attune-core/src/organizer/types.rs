use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LabelSource {
    Llm,
    Extractive,
}

/// gather 阶段每个待整理 item 的视图(引擎内部用)。
#[derive(Debug, Clone)]
pub struct ItemView {
    pub item_id: String,
    pub title: String,
    pub content_snippet: String,
    /// 文件所在目录(用于 fallback 子目录分组);无则空串。
    pub dir: String,
    /// 来自 vectors.get_vector;None = 无向量(入 noise + 回填)。
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleAssignment {
    pub role: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupItem {
    pub item_id: String,
    pub title: String,
    pub role: Option<String>,
    pub role_confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalGroup {
    pub group_id: i32,
    pub label: String,
    pub summary: String,
    pub confidence: f32,
    pub label_source: LabelSource,
    pub suggested_kind: String,
    pub items: Vec<GroupItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseItem {
    pub item_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub tier: u8,
    pub est_tokens: u64,
    pub est_usd: f64,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationProposal {
    pub proposal_id: String,
    pub corpus_domain: Option<String>,
    pub groups: Vec<ProposalGroup>,
    pub noise_items: Vec<NoiseItem>,
    pub cost: CostEstimate,
    pub dimension_mismatch_count: usize,
}

impl OrganizationProposal {
    pub fn all_item_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .groups
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.item_id.clone()))
            .collect();
        v.extend(self.noise_items.iter().map(|n| n.item_id.clone()));
        v
    }
}

/// 传给 strategy.label_cluster 的只读簇视图。
pub struct ClusterView<'a> {
    pub group_id: i32,
    pub items: &'a [ItemView],
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proposal_item_union_equals_inputs() {
        // proposal 的所有 group items + noise = 输入 item 全集(不丢/不重)
        let p = OrganizationProposal {
            proposal_id: "p1".into(),
            corpus_domain: None,
            groups: vec![ProposalGroup {
                group_id: 0,
                label: "A".into(),
                summary: "".into(),
                confidence: 0.9,
                label_source: LabelSource::Llm,
                suggested_kind: "collection".into(),
                items: vec![GroupItem {
                    item_id: "i1".into(),
                    title: "t".into(),
                    role: None,
                    role_confidence: None,
                }],
            }],
            noise_items: vec![NoiseItem {
                item_id: "i2".into(),
                title: "t2".into(),
            }],
            cost: CostEstimate {
                tier: 3,
                est_tokens: 100,
                est_usd: 0.0001,
                model: "m".into(),
            },
            dimension_mismatch_count: 0,
        };
        let mut got: Vec<String> = p.all_item_ids();
        got.sort();
        assert_eq!(got, vec!["i1".to_string(), "i2".to_string()]);
    }
}
