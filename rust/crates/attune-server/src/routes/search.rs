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

    let search_params = {
        let mut p = attune_core::search::SearchParams::with_defaults(params.top_k);
        if let Some(ik) = params.initial_k { p.initial_k = ik; }
        if let Some(imk) = params.intermediate_k { p.intermediate_k = imk; }
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
        "total": results.len()
    })))
}

/// POST /api/v1/search/relevant -- for Chrome extension injection
pub async fn search_relevant(
    State(state): State<SharedState>,
    Json(body): Json<RelevantRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let top_k = body.top_k.unwrap_or(5);
    let budget = body.injection_budget.unwrap_or(INJECTION_BUDGET);

    let search_params = {
        let mut p = attune_core::search::SearchParams::with_defaults(top_k);
        if let Some(ik) = body.initial_k { p.initial_k = ik; }
        if let Some(imk) = body.intermediate_k { p.intermediate_k = imk; }
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
}
