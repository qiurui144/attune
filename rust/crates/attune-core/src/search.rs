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

/// 启用 cross-encoder reranker 的最小候选数。
/// 候选数 < 此阈值时，RRF 排序比 cross-encoder 重排更稳定（cross-encoder
/// 在小候选集上放大噪声 / 跨语言错配）。
pub const RERANK_MIN_CANDIDATES: usize = 5;

/// Cross-lingual 降权系数。query 与 doc 语言不匹配时，该 doc 的 score 乘以此系数。
/// 设为 0.3 而不是直接过滤：保留 cross-lingual 召回（专业术语常借用英文），
/// 但不让大篇幅异语言文档压过同语言命中。
pub const CROSS_LANG_PENALTY: f32 = 0.3;

/// 判断文本的"主导语言"：zh / en / mixed。
///
/// 启发式：计算 CJK 统一表意文字（U+4E00..U+9FFF）占比
///   - CJK >= 30% → Zh
///   - ASCII letter >= 70% → En
///   - 其他 → Mixed（不降权，因为专业术语常中英混用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang { Zh, En, Mixed }

pub fn detect_lang(s: &str) -> Lang {
    let (mut cjk, mut ascii_alpha, mut total) = (0usize, 0usize, 0usize);
    for c in s.chars() {
        if c.is_whitespace() { continue; }
        total += 1;
        if ('\u{4e00}'..='\u{9fff}').contains(&c) { cjk += 1; }
        else if c.is_ascii_alphabetic() { ascii_alpha += 1; }
    }
    if total == 0 { return Lang::Mixed; }
    let cjk_ratio = cjk as f32 / total as f32;
    let ascii_ratio = ascii_alpha as f32 / total as f32;
    if cjk_ratio >= 0.30 { Lang::Zh }
    else if ascii_ratio >= 0.70 { Lang::En }
    else { Lang::Mixed }
}

/// 对 SearchResult 列表按 query/content 语言匹配降权。
///
/// - query=Mixed 或 doc=Mixed：不降权（尊重混用场景，如中文里的英文专业术语）
/// - query.Lang != doc.Lang（Zh vs En 明确不同）：score *= CROSS_LANG_PENALTY
///
/// 仅用于为了检查 title 中的内容摘要判定。对于大文档，取 content 前 500 字作为
/// 语言样本（避免过长导致判定被尾部数据污染）
pub fn apply_cross_lang_penalty(results: &mut [SearchResult], query_lang: Lang) {
    if matches!(query_lang, Lang::Mixed) {
        return;
    }
    for r in results.iter_mut() {
        // 用 title + 前 500 字判定文档语言（避免只看 content 可能因代码块偏向 en）
        let sample: String = r.title.chars().chain(r.content.chars()).take(500).collect();
        let doc_lang = detect_lang(&sample);
        let cross = matches!(
            (query_lang, doc_lang),
            (Lang::Zh, Lang::En) | (Lang::En, Lang::Zh)
        );
        if cross {
            r.score *= CROSS_LANG_PENALTY;
        }
    }
}

/// 搜索结果
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SearchResult {
    pub item_id: String,
    pub score: f32,
    pub title: String,
    pub content: String,
    pub source_type: String,
    pub inject_content: Option<String>,
    // ── F2 (W3 batch A, 2026-04-27)：breadcrumb + offset 透传 ─────────────
    // per spec docs/superpowers/specs/2026-04-27-w3-batch-a-design.md §4
    // 关闭 W2 batch 1 的 Citation 占位状态；search 阶段 join chunk_breadcrumbs
    // sidecar 表填入数据，ChatEngine 后续映射到 Citation。
    /// 启发式：F2 v1 用 item 第一个 chunk 的 path（W5+ 切换到精确 chunk 命中）。
    /// per reviewer S2：skip_serializing_if 让空 Vec 不出现在 JSON，
    /// 保持 Chrome 扩展旧客户端契约（之前不存在此字段）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb: Vec<String>,
    /// chunk 在 item.content 的 char-level 区间。无 sidecar 数据时 None。
    /// **Known limitation (W3 batch A v1, per reviewer S1)**：当前 offset 是 sidecar
    /// 内累计 char count，不一定对齐原文 char index（行末 `\n` 处理 + `\r\n` 剥离会
    /// 引入漂移）。适合 item 顶层导航；W5+ 真正按行号映射回原文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_offset_start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_offset_end: Option<usize>,
}

/// 三阶段搜索参数
#[derive(Debug, Clone)]
pub struct SearchParams {
    pub top_k: usize,
    /// 粗召回数量（向量+全文各取此数量后 RRF 融合）
    pub initial_k: usize,
    /// Reranker 入口前的候选数量
    pub intermediate_k: usize,
    // ── J3：vector 召回 cosine 阈值（W2，2026-04-27）───────────────────────
    //
    // 设计来源（per docs/superpowers/specs/2026-04-27-w2-rag-quality-batch1-design.md §J3）：
    //   吴师兄《鹅厂面试官追问：你的 RAG 能跑通 Demo？》§2 "召回阈值：一个参数，决定生死"
    //   https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw
    //   - 0.65：召回率 0.89，top-5 含 2 个噪音
    //   - 0.72：召回率 0.84，top-5 基本有用（精度优先推荐）
    //   - 0.78：召回率 0.71，开始漏边缘 case
    //
    // attune 默认 0.65（保守端）平衡召回与精度；用户可在 Settings 调到 0.72 求精度。
    // None = 不过滤（向后兼容，初版调用方未传时不破行为）。
    /// vector 召回 cosine 阈值。Some(0.65) 默认；低于此分数的 vector 结果在 RRF 前丢弃。
    pub min_score: Option<f32>,
}

impl SearchParams {
    /// 通用 search 路径默认 — **不**应用 cosine 阈值过滤，保持 W2 之前的行为契约。
    /// 用于 `/api/v1/search` / `/api/v1/search/relevant` (Chrome 扩展) — 这些 route 的
    /// 用户期望"全部召回，自己挑"。
    /// per reviewer S2：自动启用 0.65 会让 Chrome 扩展 query 含义模糊时全无结果（cosine 0.4-0.6）。
    pub fn with_defaults(top_k: usize) -> Self {
        let initial_k = (top_k * 5).clamp(20, 100);
        let intermediate_k = (top_k * 2).clamp(top_k, 40);
        Self {
            top_k,
            initial_k,
            intermediate_k,
            min_score: None,
        }
    }

    /// **RAG / chat 专用**默认 — 启用 J3 cosine 阈值 0.65 过滤噪音
    /// per spec §J3 + 吴师兄文章曲线。chat 主流程 confidence < 3 时降到 0.55 二次检索。
    pub fn with_defaults_for_rag(top_k: usize) -> Self {
        let mut s = Self::with_defaults(top_k);
        s.min_score = Some(0.65);
        s
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
///
/// 诊断：每阶段的候选数通过 log::info!/debug! 输出，便于排查"有文档但召回 0"的问题。
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
    // J3 (per spec §J3)：拿到 vector 结果后立即按 min_score 过滤；
    // 低于阈值的进 RRF 前丢弃，避免噪音污染融合排序。
    let (vec_results, query_vec): (Vec<(String, f32)>, Option<Vec<f32>>) =
        match (&ctx.embedding, &ctx.vectors) {
            (Some(emb), Some(vecs)) => {
                match emb.embed(&[query]) {
                    Ok(e) if !e.is_empty() => {
                        let qv = e[0].clone();
                        let raw: Vec<(String, f32)> = vecs.search(&qv, params.initial_k)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(meta, score)| (meta.item_id, score))
                            .collect();
                        let filtered: Vec<(String, f32)> = match params.min_score {
                            Some(threshold) => {
                                let kept: Vec<_> = raw.into_iter()
                                    .filter(|(_, s)| *s >= threshold)
                                    .collect();
                                log::info!(
                                    "search J3: vector min_score={:.3} kept {} results",
                                    threshold, kept.len()
                                );
                                kept
                            }
                            None => raw,
                        };
                        (filtered, Some(qv))
                    }
                    _ => (vec![], None),
                }
            }
            _ => (vec![], None),
        };

    log::info!(
        "search stages: query='{}' fts={} vec={}",
        query.chars().take(50).collect::<String>(),
        ft_results.len(),
        vec_results.len(),
    );

    // 3. RRF 融合 → intermediate_k
    let fused = rrf_fuse(&vec_results, &ft_results, DEFAULT_VECTOR_WEIGHT, DEFAULT_FULLTEXT_WEIGHT, params.intermediate_k);
    log::info!("search stages: rrf_fused={}", fused.len());

    // 4. 获取并解密 items + F2 (W3 batch A) 拉 breadcrumb sidecar
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = ctx.store.get_item(ctx.dek, item_id) {
            // F2：用 item 第一个 chunk 的 breadcrumb 作启发式（W5+ 切精确 chunk）
            let (breadcrumb, off_start, off_end) = ctx
                .store
                .get_first_chunk_breadcrumb(&item.id)
                .ok()
                .flatten()
                .map(|(p, s, e)| (p, Some(s), Some(e)))
                .unwrap_or_default();
            results.push(SearchResult {
                item_id: item.id,
                score: *score,
                title: item.title,
                content: item.content,
                source_type: item.source_type,
                inject_content: None,
                breadcrumb,
                chunk_offset_start: off_start,
                chunk_offset_end: off_end,
            });
        }
    }
    log::info!("search stages: items_decrypted={}", results.len());

    // 5. Rerank 策略：
    //    a) 候选 < RERANK_MIN_CANDIDATES：跳过 cross-encoder，保留 RRF 序
    //       （小集合上 cross-encoder 放大噪声 + 跨语言错配）
    //    b) 候选够多：用 cross-encoder 重排
    //    c) 无 cross-encoder 但有 query 向量 + 候选 <= 20：用 cosine 重排
    //
    // 语言降权（反 cross-lingual 污染）：任何 rerank 方式之后，都按
    // query/doc 语言匹配对 score 做降权，防止大篇幅异语言文档排到前面。
    let query_lang = detect_lang(query);

    if let Some(reranker) = &ctx.reranker {
        if results.len() >= RERANK_MIN_CANDIDATES {
            let docs: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
            match reranker.score(query, &docs) {
                Ok(scores) => {
                    for (r, s) in results.iter_mut().zip(scores.iter()) {
                        r.score = *s;
                    }
                }
                Err(e) => {
                    log::warn!("reranker failed, keeping RRF order: {e}");
                }
            }
        } else {
            log::info!(
                "search stages: reranker skipped (candidates={} < {})",
                results.len(), RERANK_MIN_CANDIDATES
            );
        }
    } else if results.len() <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = &query_vec {
            if let Some(vecs) = ctx.vectors {
                rerank(qvec, &mut results, vecs);
            }
        }
    }

    // 语言匹配降权：任何排序策略之后统一应用，不改变同语言相对顺序
    apply_cross_lang_penalty(&mut results, query_lang);

    // 最终排序
    results.sort_by(|a, b| b.score.partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal));

    // 6. 截取 top_k（保护：如果 top_k=0，别截成空）
    let final_k = params.top_k.max(1);
    results.truncate(final_k);
    log::info!("search stages: returned={}", results.len());
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lang_pure_chinese() {
        assert_eq!(detect_lang("劳动合同法规定"), Lang::Zh);
        assert_eq!(detect_lang("民法典第五百八十四条"), Lang::Zh);
    }

    #[test]
    fn detect_lang_pure_english() {
        assert_eq!(detect_lang("What is rust ownership and borrowing"), Lang::En);
        assert_eq!(detect_lang("Box T smart pointer reference cycles"), Lang::En);
    }

    #[test]
    fn detect_lang_technical_mix() {
        // 中文为主但含英文术语 → 仍按中文处理（CJK >= 30%）
        assert_eq!(detect_lang("使用 Box<T> 处理堆内存"), Lang::Zh);
        // 少量中文的英文文档（< 30%）→ 英文
        assert_eq!(detect_lang("Rust programming language 简称 RPL"), Lang::En);
    }

    #[test]
    fn cross_lang_penalty_en_query_cn_doc_downweighted() {
        let mut results = vec![
            SearchResult {
                item_id: "1".into(), score: 0.2, title: "references-and-borrowing".into(),
                content: "In Rust, references allow you to refer to a value without taking ownership.".into(),
                source_type: "file".into(), inject_content: None, ..Default::default() },
            SearchResult {
                item_id: "2".into(), score: 0.3, title: "民法典".into(),
                content: "中华人民共和国民法典第一编 总则".into(),
                source_type: "file".into(), inject_content: None, ..Default::default() },
        ];
        apply_cross_lang_penalty(&mut results, Lang::En);
        assert_eq!(results[0].score, 0.2, "英文文档不降权");
        assert!(results[1].score < 0.1, "中文文档应被降权 (0.3 * 0.3 = 0.09): {}",
            results[1].score);
    }

    #[test]
    fn cross_lang_penalty_mixed_query_no_penalty() {
        let mut results = vec![
            SearchResult {
                item_id: "1".into(), score: 0.5, title: "rust 所有权".into(),
                content: "Rust ownership system...".into(),
                source_type: "file".into(), inject_content: None, ..Default::default() },
        ];
        apply_cross_lang_penalty(&mut results, Lang::Mixed);
        assert_eq!(results[0].score, 0.5, "Mixed query 不应降权任何结果");
    }

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
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
            SearchResult {
                item_id: "b".into(), score: 0.2, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
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
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
            SearchResult {
                item_id: "b".into(), score: 0.0, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None, ..Default::default() },
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
            SearchResult { item_id: "far".into(),   score: 0.9, title: "Far".into(),   content: "c".into(), source_type: "note".into(), inject_content: None, ..Default::default() },
            SearchResult { item_id: "close".into(), score: 0.5, title: "Close".into(), content: "c".into(), source_type: "note".into(), inject_content: None, ..Default::default() },
        ];

        rerank(&[1.0, 0.0], &mut results, &idx);
        assert_eq!(results[0].item_id, "close", "Reranker should elevate closer vector");
    }

    #[test]
    fn rerank_fallback_when_no_vector() {
        use crate::vectors::VectorIndex;

        let idx = VectorIndex::new(2).unwrap();
        let mut results = vec![
            SearchResult { item_id: "a".into(), score: 0.8, title: "A".into(), content: "c".into(), source_type: "note".into(), ..Default::default() },
            SearchResult { item_id: "b".into(), score: 0.3, title: "B".into(), content: "c".into(), source_type: "note".into(), ..Default::default() },
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
        // per reviewer S2：通用 search 默认不启用 J3 阈值，保持 W2 前行为契约
        assert_eq!(p.min_score, None);

        let p2 = SearchParams::with_defaults(1);
        assert_eq!(p2.initial_k, 20);  // min clamp
        assert_eq!(p2.intermediate_k, 2); // max(1, min(2, 40))

        let p3 = SearchParams::with_defaults(30);
        assert_eq!(p3.initial_k, 100); // max clamp
        assert_eq!(p3.intermediate_k, 40); // max clamp
    }

    // ── J3 tests（per spec §J3 + reviewer S2 路径分离）──────────────

    #[test]
    fn min_score_filter_keeps_above_threshold() {
        // 模拟 vecs.search 返回 [0.50, 0.70, 0.85]
        let raw: Vec<(String, f32)> = vec![
            ("a".into(), 0.50),
            ("b".into(), 0.70),
            ("c".into(), 0.85),
        ];
        let kept_065: Vec<_> = raw.iter().filter(|(_, s)| *s >= 0.65).cloned().collect();
        assert_eq!(kept_065.len(), 2, "0.65 阈值应保留 2 个 (0.70 + 0.85)");
        assert_eq!(kept_065[0].0, "b");
        assert_eq!(kept_065[1].0, "c");

        let kept_078: Vec<_> = raw.iter().filter(|(_, s)| *s >= 0.78).cloned().collect();
        assert_eq!(kept_078.len(), 1, "0.78 阈值应保留 1 个 (0.85)");

        let kept_055: Vec<_> = raw.iter().filter(|(_, s)| *s >= 0.55).cloned().collect();
        assert_eq!(kept_055.len(), 2, "0.55 应保留 0.70 + 0.85（不含 0.50）");
    }

    #[test]
    fn rag_defaults_enable_065_threshold() {
        // chat 路径默认走 RAG 阈值（0.65）— J3 仅对 RAG 生效，通用 search 不变
        let rag = SearchParams::with_defaults_for_rag(5);
        assert_eq!(rag.min_score, Some(0.65));
        assert_eq!(rag.top_k, 5);
        assert_eq!(rag.initial_k, 25);  // 与通用版同构
    }

    #[test]
    fn min_score_threshold_curve_documented_in_spec() {
        // 锁住吴师兄文章给出的曲线值，避免有人未读 spec 误改默认
        let rag = SearchParams::with_defaults_for_rag(5);
        assert_eq!(rag.min_score, Some(0.65), "RAG 默认 0.65（保守端，召回优先）");
        // 0.72 是吴师兄推荐的"精度优先"档，未来 Settings 提供
        // 0.78 开始漏边缘 case，仅极端精度场景用
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
