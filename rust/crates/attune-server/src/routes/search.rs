use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use attune_core::search::{allocate_budget, SearchResult, INJECTION_BUDGET};

use crate::state::SharedState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    pub initial_k: Option<usize>,
    pub intermediate_k: Option<usize>,
}

fn default_top_k() -> usize {
    10
}

fn hash_query(query: &str) -> u64 {
    let mut hash: u64 = 5381;
    for b in query.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    hash
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err_500(msg: &str) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": msg})),
    )
}

pub async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // top_k = 0 会导致搜索始终返回空结果，提前拒绝
    if params.top_k == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "top_k must be > 0"})),
        ));
    }
    // OSS-S14 fix: top_k 上限 100；超过上限直接 400 拒绝避免被滥用作 DoS vector
    // (R15 实测 top_k=10000 让 search 端 5s 内全部 timeout)
    if params.top_k > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "top_k must be <= 100"})),
        ));
    }

    let cache_key = hash_query(&params.q);
    {
        let mut cache = state.search_cache.lock().map_err(|_| err_500("cache lock poisoned"))?;
        if let Some(entry) = cache.get(&cache_key) {
            // 验证原始 query 字符串防止哈希碰撞返回错误结果
            if entry.query == params.q && !entry.is_expired() {
                return Ok(Json(serde_json::json!({
                    "query": params.q,
                    "results": entry.results,
                    "total": entry.results.len(),
                    "cached": true
                })));
            }
        }
    }

    // v0.6 Phase B F-Pro Stage 4：从 query 自动 detect 领域意图，driving cross-domain penalty。
    // 命中 'legal' / 'tech' / 'medical' / 'patent' → 跨领域文档 score *= 0.4
    // 未命中（None）→ 不应用 penalty（保持向后兼容）
    let detected_domain = attune_core::search::detect_query_domain(&params.q);

    let search_params = {
        let mut p = attune_core::search::SearchParams::with_defaults(params.top_k);
        if let Some(ik) = params.initial_k { p.initial_k = ik; }
        if let Some(imk) = params.intermediate_k { p.intermediate_k = imk; }
        if let Some(d) = detected_domain.as_ref() { p.domain_hint = Some(d.clone()); }
        p
    };

    let dek = {
        let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
        vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?
    };

    let reranker = state.reranker.lock().map_err(|_| err_500("reranker lock"))?.clone();
    let emb = state.embedding.lock().map_err(|_| err_500("emb lock"))?.clone();

    let results = {
        let ft_guard = state.fulltext.lock().map_err(|_| err_500("ft lock"))?;
        let vec_guard = state.vectors.lock().map_err(|_| err_500("vec lock"))?;
        let vault_guard = state.vault.lock().map_err(|_| err_500("vault lock"))?;

        let ctx = attune_core::search::SearchContext {
            fulltext: ft_guard.as_ref(),
            vectors: vec_guard.as_ref(),
            embedding: emb,
            reranker,
            store: vault_guard.store(),
            dek: &dek,
        };
        attune_core::search::search_with_context(&ctx, &params.q, &search_params)
            .map_err(|e| err_500(&e.to_string()))?
    };

    // OSS-S17 fix: score 阈值 cutoff。当 corpus 被低质量内容污染时，BM25+vector+RRF 退化为
    // fallback default score（实测 0.000638-0.000828）让真实相关内容无法浮出。R19-R3 复现：
    // 22K 测试 garbage 主导 corpus 后，新 ingest 的 5 真实 rust md 文件 10 query 全部 top hit
    // 是 garbage 同分。
    //
    // R20 实测分数尺度：真实命中 ~0.98 / fallback noise ~0.0006-0.0008，cutoff 0.001 完美分离。
    const SCORE_CUTOFF: f32 = 0.001;
    let cutoff_filtered: Vec<_> = results.iter().filter(|r| r.score >= SCORE_CUTOFF).cloned().collect();
    let total_before_cutoff = results.len();
    let total_after_cutoff = cutoff_filtered.len();
    let results = if cutoff_filtered.is_empty() && !results.is_empty() {
        // 全部低于阈值 → 视为 no-match
        Vec::new()
    } else {
        cutoff_filtered
    };

    {
        let mut cache = state.search_cache.lock().map_err(|_| err_500("cache lock poisoned"))?;
        cache.put(cache_key, crate::state::CachedSearch {
            query: params.q.clone(),
            results: results.clone(),
            created_at: std::time::Instant::now(),
        });
    }

    Ok(Json(serde_json::json!({
        "query": params.q,
        "results": results,
        "total": results.len(),
        "cutoff_filtered": total_before_cutoff - total_after_cutoff
    })))
}

/// POST /api/v1/search/relevant -- for Chrome extension injection
pub async fn search_relevant(
    State(state): State<SharedState>,
    Json(body): Json<RelevantRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let top_k = body.top_k.unwrap_or(5);
    // OSS-S14 fix: 同样校验 search_relevant 的 top_k 上限
    if top_k == 0 || top_k > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "top_k must be in [1, 100]"})),
        ));
    }
    let budget = body.injection_budget.unwrap_or(INJECTION_BUDGET);

    let detected_domain = attune_core::search::detect_query_domain(&body.query);
    let search_params = {
        let mut p = attune_core::search::SearchParams::with_defaults(top_k);
        if let Some(ik) = body.initial_k { p.initial_k = ik; }
        if let Some(imk) = body.intermediate_k { p.intermediate_k = imk; }
        if let Some(d) = detected_domain.as_ref() { p.domain_hint = Some(d.clone()); }
        p
    };

    let dek = {
        let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
        vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?
    };

    let reranker = state.reranker.lock().map_err(|_| err_500("reranker lock"))?.clone();
    let emb = state.embedding.lock().map_err(|_| err_500("emb lock"))?.clone();

    let mut results: Vec<SearchResult> = {
        let ft_guard = state.fulltext.lock().map_err(|_| err_500("ft lock"))?;
        let vec_guard = state.vectors.lock().map_err(|_| err_500("vec lock"))?;
        let vault_guard = state.vault.lock().map_err(|_| err_500("vault lock"))?;

        let ctx = attune_core::search::SearchContext {
            fulltext: ft_guard.as_ref(),
            vectors: vec_guard.as_ref(),
            embedding: emb,
            reranker,
            store: vault_guard.store(),
            dek: &dek,
        };
        attune_core::search::search_with_context(&ctx, &body.query, &search_params)
            .map_err(|e| err_500(&e.to_string()))?
    };

    // Apply injection budget
    allocate_budget(&mut results, budget);

    Ok(Json(serde_json::json!({
        "results": results,
        "total": results.len()
    })))
}

#[derive(Deserialize)]
pub struct RelevantRequest {
    pub query: String,
    pub top_k: Option<usize>,
    pub injection_budget: Option<usize>,
    pub initial_k: Option<usize>,
    pub intermediate_k: Option<usize>,
    #[allow(dead_code)]
    pub source_types: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_query_deterministic() {
        assert_eq!(hash_query("hello"), hash_query("hello"));
        assert_ne!(hash_query("hello"), hash_query("world"));
    }

    #[test]
    fn hash_query_empty() {
        let _ = hash_query("");
    }

    /// OSS-S14 regression: top_k 必须有上限，否则 top_k=10000 触发 search 卡死
    /// (R15-R1 实测 130/130 全部 timeout)。修复后 top_k > 100 直接 400。
    /// 这里仅断言 default_top_k 与上限值一致；上限校验在 handler 路径上跑全栈测试更合适。
    #[test]
    fn search_query_top_k_bounds() {
        assert_eq!(default_top_k(), 10);
        // 上限是常量 100，handler 中检查 params.top_k > 100 即拒绝
        const TOP_K_MAX: usize = 100;
        assert!(default_top_k() < TOP_K_MAX, "default 应低于上限");
    }
}
