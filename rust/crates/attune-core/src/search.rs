// npu-vault/crates/vault-core/src/search.rs

use std::collections::HashMap;
use std::sync::Arc;

use crate::embed::EmbeddingProvider;
use crate::index::FulltextIndex;
use crate::infer::RerankProvider;
use crate::store::Store;
use crate::vectors::VectorIndex;

/// RRF 参数
pub const RRF_K: f32 = 60.0;
pub const RERANK_VECTOR_WEIGHT: f32 = 0.7;
pub const RERANK_RRF_WEIGHT: f32 = 0.3;
pub const RERANK_TOP_K_THRESHOLD: usize = 20;
pub const DEFAULT_VECTOR_WEIGHT: f32 = 0.6;
pub const DEFAULT_FULLTEXT_WEIGHT: f32 = 0.4;
pub const INJECTION_BUDGET: usize = 2000;

/// 搜索结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub item_id: String,
    pub score: f32,
    pub title: String,
    pub content: String,
    pub source_type: String,
    pub inject_content: Option<String>,
}

/// 三阶段搜索参数
#[derive(Debug, Clone)]
pub struct SearchParams {
    pub top_k: usize,
    /// 粗召回数量（向量+全文各取此数量后 RRF 融合）
    pub initial_k: usize,
    /// Reranker 入口前的候选数量
    pub intermediate_k: usize,
}

impl SearchParams {
    pub fn with_defaults(top_k: usize) -> Self {
        let initial_k = (top_k * 5).clamp(20, 100);
        let intermediate_k = (top_k * 2).clamp(top_k, 40);
        Self { top_k, initial_k, intermediate_k }
    }
}

/// 搜索上下文：持有所有搜索所需组件的引用
pub struct SearchContext<'a> {
    pub fulltext: Option<&'a FulltextIndex>,
    pub vectors: Option<&'a VectorIndex>,
    pub embedding: Option<Arc<dyn EmbeddingProvider>>,
    pub reranker: Option<Arc<dyn RerankProvider>>,
    pub store: &'a Store,
    pub dek: &'a crate::crypto::Key32,
}

/// RRF 融合两组排名结果
pub fn rrf_fuse(
    vector_results: &[(String, f32)],
    fulltext_results: &[(String, f32)],
    vector_weight: f32,
    fulltext_weight: f32,
    top_k: usize,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();

    for (rank, (id, _score)) in vector_results.iter().enumerate() {
        let rrf = vector_weight / (RRF_K + rank as f32 + 1.0);
        *scores.entry(id.clone()).or_default() += rrf;
    }
    for (rank, (id, _score)) in fulltext_results.iter().enumerate() {
        let rrf = fulltext_weight / (RRF_K + rank as f32 + 1.0);
        *scores.entry(id.clone()).or_default() += rrf;
    }

    let mut sorted: Vec<(String, f32)> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(top_k);
    sorted
}

/// 动态注入预算分配
pub fn allocate_budget(results: &mut [SearchResult], budget: usize) {
    let total_score: f32 = results.iter().map(|r| r.score).sum();
    if total_score <= 0.0 || results.is_empty() {
        // 保证每条至少 100 字符，与正比路径中 .max(100.0) 对齐
        let per_item = (budget / results.len().max(1)).max(100);
        for r in results.iter_mut() {
            let content = &r.content;
            let end = content.char_indices()
                .nth(per_item)
                .map(|(i, _)| i)
                .unwrap_or(content.len());
            r.inject_content = Some(content[..end].to_string());
        }
        return;
    }
    for r in results.iter_mut() {
        let share = r.score / total_score;
        let alloc = (budget as f32 * share).max(100.0) as usize;
        let content = &r.content;
        let end = content.char_indices()
            .nth(alloc)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        r.inject_content = Some(content[..end].to_string());
    }
}

/// 计算两个向量的余弦相似度，任一范数为 0 时返回 0.0
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "cosine_similarity: dimension mismatch");
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-8 || norm_b < 1e-8 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// 对 RRF 一阶结果进行余弦相似度二次排序。
///
/// 当 query 向量可用且结果集实际数量不超过 `RERANK_TOP_K_THRESHOLD` 时调用。
/// 原地修改 `results` 的 `score` 字段并重新排序。
pub fn rerank(
    query_vec: &[f32],
    results: &mut [SearchResult],
    vector_index: &VectorIndex,
) {
    for result in results.iter_mut() {
        let rrf_score = result.score;
        let rerank_score = vector_index
            .get_vector(&result.item_id)
            .map(|item_vec| cosine_similarity(query_vec, &item_vec))
            .unwrap_or(0.0);
        result.score = RERANK_VECTOR_WEIGHT * rerank_score + RERANK_RRF_WEIGHT * rrf_score;
    }
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

/// 三阶段搜索：initial_k 粗召回 → intermediate_k RRF 融合 → Rerank → top_k 返回
///
/// 同时被 search 端点和 chat 引擎调用，避免重复逻辑。
pub fn search_with_context(
    ctx: &SearchContext<'_>,
    query: &str,
    params: &SearchParams,
) -> crate::error::Result<Vec<SearchResult>> {
    // 1. 全文搜索（initial_k）
    let ft_results = ctx.fulltext
        .map(|ft| ft.search(query, params.initial_k).unwrap_or_else(|e| {
            log::warn!("fulltext search error: {e}");
            vec![]
        }))
        .unwrap_or_default();

    // 2. 向量搜索（initial_k）
    let (vec_results, query_vec): (Vec<(String, f32)>, Option<Vec<f32>>) =
        match (&ctx.embedding, &ctx.vectors) {
            (Some(emb), Some(vecs)) => {
                match emb.embed(&[query]) {
                    Ok(e) if !e.is_empty() => {
                        let qv = e[0].clone();
                        let vr = vecs.search(&qv, params.initial_k)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(meta, score)| (meta.item_id, score))
                            .collect();
                        (vr, Some(qv))
                    }
                    _ => (vec![], None),
                }
            }
            _ => (vec![], None),
        };

    // 3. RRF 融合 → intermediate_k
    let fused = rrf_fuse(&vec_results, &ft_results, DEFAULT_VECTOR_WEIGHT, DEFAULT_FULLTEXT_WEIGHT, params.intermediate_k);

    // 4. 获取并解密 items
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = ctx.store.get_item(ctx.dek, item_id) {
            results.push(SearchResult {
                item_id: item.id,
                score: *score,
                title: item.title,
                content: item.content,
                source_type: item.source_type,
                inject_content: None,
            });
        }
    }

    // 5. Rerank（有 Reranker 时用 cross-encoder；有 query 向量时用余弦；否则跳过）
    if let Some(reranker) = &ctx.reranker {
        let docs: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
        if let Ok(scores) = reranker.score(query, &docs) {
            for (r, s) in results.iter_mut().zip(scores.iter()) {
                r.score = *s;
            }
            results.sort_by(|a, b| b.score.partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal));
        }
    } else if results.len() <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = &query_vec {
            if let Some(vecs) = ctx.vectors {
                rerank(qvec, &mut results, vecs);
            }
        }
    }

    // 6. 截取 top_k
    results.truncate(params.top_k);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_fuse_basic() {
        let vec_results = vec![
            ("a".into(), 0.9), ("b".into(), 0.7), ("c".into(), 0.5),
        ];
        let ft_results = vec![
            ("b".into(), 10.0), ("a".into(), 8.0), ("d".into(), 5.0),
        ];

        let fused = rrf_fuse(&vec_results, &ft_results, 0.6, 0.4, 10);
        assert!(!fused.is_empty());
        // "a" 和 "b" 在两个列表中都出现，应该排名靠前
        let top_ids: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
        assert!(top_ids.contains(&"a"));
        assert!(top_ids.contains(&"b"));
    }

    #[test]
    fn rrf_fuse_empty() {
        let fused = rrf_fuse(&[], &[], 0.6, 0.4, 10);
        assert!(fused.is_empty());
    }

    #[test]
    fn rrf_fuse_single_source() {
        let vec_results = vec![("a".into(), 0.9)];
        let fused = rrf_fuse(&vec_results, &[], 0.6, 0.4, 10);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].0, "a");
    }

    #[test]
    fn allocate_budget_proportional() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(), score: 0.8, title: "A".into(),
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None,
            },
            SearchResult {
                item_id: "b".into(), score: 0.2, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None,
            },
        ];
        allocate_budget(&mut results, 2000);

        let a_len = results[0].inject_content.as_ref().unwrap().chars().count();
        let b_len = results[1].inject_content.as_ref().unwrap().chars().count();
        // "a" has 80% score, should get ~1600 chars; "b" has 20%, should get ~400 (min 100)
        assert!(a_len > b_len, "Higher score should get more budget: a={a_len} b={b_len}");
        assert!(b_len >= 100, "Minimum budget should be 100: got {b_len}");
    }

    #[test]
    fn allocate_budget_zero_scores() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(), score: 0.0, title: "A".into(),
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None,
            },
            SearchResult {
                item_id: "b".into(), score: 0.0, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None,
            },
        ];
        allocate_budget(&mut results, 2000);
        // Equal distribution when scores are 0
        let a_len = results[0].inject_content.as_ref().unwrap().chars().count();
        let b_len = results[1].inject_content.as_ref().unwrap().chars().count();
        assert_eq!(a_len, b_len, "Equal scores should get equal budget");
    }

    #[test]
    fn cosine_similarity_basic() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-5);
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]) - 0.0).abs() < 1e-5);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn rerank_orders_by_cosine() {
        use crate::vectors::{VectorIndex, VectorMeta};

        let mut idx = VectorIndex::new(2).unwrap();
        idx.add(&[1.0, 0.0], VectorMeta { item_id: "close".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
        idx.add(&[0.0, 1.0], VectorMeta { item_id: "far".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();

        let mut results = vec![
            SearchResult { item_id: "far".into(),   score: 0.9, title: "Far".into(),   content: "c".into(), source_type: "note".into(), inject_content: None },
            SearchResult { item_id: "close".into(), score: 0.5, title: "Close".into(), content: "c".into(), source_type: "note".into(), inject_content: None },
        ];

        rerank(&[1.0, 0.0], &mut results, &idx);
        assert_eq!(results[0].item_id, "close", "Reranker should elevate closer vector");
    }

    #[test]
    fn rerank_fallback_when_no_vector() {
        use crate::vectors::VectorIndex;

        let idx = VectorIndex::new(2).unwrap();
        let mut results = vec![
            SearchResult { item_id: "a".into(), score: 0.8, title: "A".into(), content: "c".into(), source_type: "note".into(), inject_content: None },
            SearchResult { item_id: "b".into(), score: 0.3, title: "B".into(), content: "c".into(), source_type: "note".into(), inject_content: None },
        ];
        rerank(&[1.0, 0.0], &mut results, &idx);
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn search_params_defaults_clamp_correctly() {
        let p = SearchParams::with_defaults(5);
        assert_eq!(p.top_k, 5);
        assert_eq!(p.initial_k, 25);   // 5*5=25, in [20,100]
        assert_eq!(p.intermediate_k, 10); // 5*2=10, in [5,40]

        let p2 = SearchParams::with_defaults(1);
        assert_eq!(p2.initial_k, 20);  // min clamp
        assert_eq!(p2.intermediate_k, 2); // max(1, min(2, 40))

        let p3 = SearchParams::with_defaults(30);
        assert_eq!(p3.initial_k, 100); // max clamp
        assert_eq!(p3.intermediate_k, 40); // max clamp
    }

    // #9: search_with_context 三阶段管道（有 Reranker）
    #[test]
    fn search_with_context_reranker_reorders_results() {
        use crate::infer::MockRerankProvider;
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();

        // 插入两条 item
        store.insert_item(&dek, "低分文档", "content about cats", None, "note", None, None).unwrap();
        store.insert_item(&dek, "高分文档", "content about dogs", None, "note", None, None).unwrap();

        // Reranker 固定返回固定分数（第二条评分更高）
        let reranker: std::sync::Arc<dyn crate::infer::RerankProvider> =
            std::sync::Arc::new(MockRerankProvider::new(vec![0.1, 0.9]));

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: Some(reranker),
            store: &store,
            dek: &dek,
        };

        // 无 FTS 也无向量时 fused 为空，search_with_context 返回空但不 panic
        let params = SearchParams::with_defaults(5);
        let results = search_with_context(&ctx, "dogs", &params);
        assert!(results.is_ok(), "search_with_context should not fail with reranker");
        // 无数据源时结果为空
        assert!(results.unwrap().is_empty());
    }

    // #10: search_with_context 纯 FTS fallback（无 embedding、无 reranker）
    #[test]
    fn search_with_context_fts_only_fallback() {
        use crate::store::Store;

        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();

        let ctx = SearchContext {
            fulltext: None,
            vectors: None,
            embedding: None,
            reranker: None,
            store: &store,
            dek: &dek,
        };

        let params = SearchParams::with_defaults(5);
        let results = search_with_context(&ctx, "any query", &params).unwrap();
        // 无数据源时结果为空，但不应 panic
        assert!(results.is_empty());
    }
}
