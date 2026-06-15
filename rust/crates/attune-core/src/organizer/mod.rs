pub mod grouping;
pub mod strategy;
pub mod types;

use crate::clusterer::{ClusterInput, Clusterer};
use crate::llm::LlmProvider;
use crate::organizer::grouping::{fallback_group_by_dir, partition_by_majority_dim};
use crate::organizer::strategy::{extractive_label, LabelCtx, OrganizationStrategy};
use crate::organizer::types::*;

/// Effective lower bound for invoking HDBSCAN. WHY: the `hdbscan` crate's
/// default `min_samples` is 5; clustering fewer than 5 points triggers an
/// index-out-of-bounds panic inside the library (observed in Task 2). So even
/// when the caller's `min_cluster_size` is smaller, we must have ≥ 5 clean
/// points before delegating to `group_only`; otherwise we use the dir fallback.
const HDBSCAN_FLOOR: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum OrganizeError {
    #[error("empty scope")]
    EmptyScope,
    #[error("cluster error: {0}")]
    Cluster(String),
}

/// 纯函数式核心:接受已 gather 的 items,产出 proposal。route 层负责 gather(vectors)。
/// llm=None → 仅 tier-2(extractive 命名);Some → tier-3 strategy 命名。
pub fn analyze_items(
    proposal_id: String,
    corpus_domain: Option<String>,
    items: Vec<ItemView>,
    strategy: &dyn OrganizationStrategy,
    llm: Option<&dyn LlmProvider>,
    min_cluster_size: usize,
) -> Result<OrganizationProposal, OrganizeError> {
    if items.is_empty() {
        return Err(OrganizeError::EmptyScope);
    }

    // 1. 维度分区(混维/无向量 → noise)
    let (clean, mut noise, mismatch) = partition_by_majority_dim(items);

    // 2. 分组:clean 达到 hdbscan 有效下限 → group_only;否则 fallback 子目录。
    let model_name = llm.map(|l| l.model_name().to_string()).unwrap_or_default();
    let id_to_item: std::collections::HashMap<String, ItemView> =
        clean.iter().cloned().map(|i| (i.item_id.clone(), i)).collect();

    let cluster_threshold = min_cluster_size.max(HDBSCAN_FLOOR);
    let raw_groups: Vec<(i32, Vec<String>)> = if clean.len() >= cluster_threshold {
        let inputs: Vec<ClusterInput> = clean
            .iter()
            .map(|i| ClusterInput {
                item_id: i.item_id.clone(),
                title: i.title.clone(),
                content_snippet: i.content_snippet.clone(),
                embedding: i.embedding.clone().unwrap_or_default(),
            })
            .collect();
        // group_only 不调 LLM,故传 noop 仅满足 Clusterer::new 签名。
        let clusterer = Clusterer::new(crate::llm::noop_llm()).with_min_items(min_cluster_size);
        clusterer
            .group_only(&inputs)
            .map_err(|e| OrganizeError::Cluster(e.to_string()))?
    } else {
        fallback_group_by_dir(&clean)
            .into_iter()
            .enumerate()
            .map(|(idx, (_dir, ids))| (idx as i32, ids))
            .collect()
    };

    // 3. 每组命名 + 角色(噪声 label=-1 → 全进 noise_items)
    let mut groups = Vec::new();
    let mut est_tokens = 0u64;
    for (gid, ids) in raw_groups {
        let members: Vec<ItemView> =
            ids.iter().filter_map(|id| id_to_item.get(id).cloned()).collect();
        if gid == -1 {
            for m in &members {
                noise.push(NoiseItem { item_id: m.item_id.clone(), title: m.title.clone() });
            }
            continue;
        }
        let view = ClusterView { group_id: gid, items: &members };
        let label = match llm {
            Some(l) => {
                est_tokens += 400;
                strategy
                    .label_cluster(&view, &LabelCtx { llm: Some(l) })
                    .unwrap_or_else(|_| extractive_label(&view))
            }
            None => extractive_label(&view),
        };
        let kind = strategy.suggest_project_kind(&label);
        let items_out: Vec<GroupItem> = members
            .iter()
            .map(|m| {
                let r = strategy.assign_role(m, &label);
                GroupItem {
                    item_id: m.item_id.clone(),
                    title: m.title.clone(),
                    role: r.as_ref().map(|x| x.role.clone()),
                    role_confidence: r.as_ref().map(|x| x.confidence),
                }
            })
            .collect();
        groups.push(ProposalGroup {
            group_id: gid,
            label: label.name,
            summary: label.summary,
            confidence: label.confidence,
            label_source: label.source,
            suggested_kind: kind,
            items: items_out,
        });
    }

    let tier = if llm.is_some() { 3 } else { 2 };
    Ok(OrganizationProposal {
        proposal_id,
        corpus_domain,
        groups,
        noise_items: noise,
        cost: CostEstimate {
            tier,
            est_tokens,
            est_usd: est_tokens as f64 * 0.0000002,
            model: model_name,
        },
        dimension_mismatch_count: mismatch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organizer::strategy::StrategyRegistry;
    use crate::organizer::types::ItemView;
    fn iv(id: &str, emb: Vec<f32>) -> ItemView {
        ItemView { item_id: id.into(), title: format!("file {id}"), content_snippet: "".into(), dir: "".into(), embedding: Some(emb) }
    }
    #[test]
    fn analyze_items_covers_all_inputs_no_llm_path() {
        let reg = StrategyRegistry::new();
        let strat = reg.resolve(None);
        // 3 个 item,min_items=2,LLM=None → group_only 聚类 + extractive 命名
        let items = vec![iv("a", vec![1.0, 0.0]), iv("b", vec![0.9, 0.1]), iv("c", vec![0.0, 1.0])];
        let p = analyze_items("p1".into(), None, items, strat.as_ref(), None, 2).unwrap();
        let mut all = p.all_item_ids();
        all.sort();
        assert_eq!(all, vec!["a", "b", "c"]);
        assert_eq!(p.cost.tier, 2); // 无 LLM → tier 2(extractive)
    }
    #[test]
    fn analyze_items_below_hdbscan_floor_falls_back_no_panic() {
        // 4 个 clean item < HDBSCAN_FLOOR(5)→ 必须走 fallback 子目录,绝不调 group_only。
        // hdbscan 库 min_samples 默认=5,点数 < 5 触发库内 index-out-of-bounds panic。
        let reg = StrategyRegistry::new();
        let strat = reg.resolve(None);
        let items = vec![
            ItemView { item_id: "a".into(), title: "a".into(), content_snippet: "".into(), dir: "d1".into(), embedding: Some(vec![1.0, 0.0]) },
            ItemView { item_id: "b".into(), title: "b".into(), content_snippet: "".into(), dir: "d1".into(), embedding: Some(vec![0.9, 0.1]) },
            ItemView { item_id: "c".into(), title: "c".into(), content_snippet: "".into(), dir: "d2".into(), embedding: Some(vec![0.0, 1.0]) },
            ItemView { item_id: "d".into(), title: "d".into(), content_snippet: "".into(), dir: "d2".into(), embedding: Some(vec![0.1, 0.9]) },
        ];
        let p = analyze_items("p2".into(), None, items, strat.as_ref(), None, 2).unwrap();
        let mut all = p.all_item_ids();
        all.sort();
        assert_eq!(all, vec!["a", "b", "c", "d"]); // 不丢不重,且没 panic
        assert_eq!(p.cost.tier, 2);
    }
    #[test]
    fn analyze_items_empty_scope_errors() {
        let reg = StrategyRegistry::new();
        let strat = reg.resolve(None);
        let r = analyze_items("p3".into(), None, vec![], strat.as_ref(), None, 2);
        assert!(matches!(r, Err(OrganizeError::EmptyScope)));
    }
}
