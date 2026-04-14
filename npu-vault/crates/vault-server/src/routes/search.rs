use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use vault_core::search::{
    allocate_budget, rerank, rrf_fuse, SearchResult, INJECTION_BUDGET, RERANK_TOP_K_THRESHOLD,
};

use crate::state::SharedState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
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

/// Embed query text and run vector search.
/// Returns (vector_search_results, query_embedding).
/// query_embedding is Some when embedding succeeded, None otherwise.
async fn embed_query(
    state: &SharedState,
    query: &str,
) -> (Vec<(String, f32)>, Option<Vec<f32>>) {
    let emb_opt = state.embedding.lock().ok().and_then(|g| g.clone());
    let vec_opt_exists = state.vectors.lock().ok().map(|g| g.is_some()).unwrap_or(false);

    let (emb, _) = match (emb_opt, vec_opt_exists) {
        (Some(emb), true) => (emb, ()),
        _ => return (vec![], None),
    };

    let query_owned = query.to_string();
    let state_clone = state.clone();

    let result = tokio::task::spawn_blocking(move || {
        let embeddings = match emb.embed(&[&query_owned]) {
            Ok(e) if !e.is_empty() => e,
            _ => return (vec![], None),
        };
        let query_vec = embeddings[0].clone();
        let vec_guard = match state_clone.vectors.lock() {
            Ok(g) => g,
            Err(_) => return (vec![], None),
        };
        let search_results = match vec_guard.as_ref() {
            Some(vecs) => vecs
                .search(&query_vec, 10)
                .unwrap_or_default()
                .into_iter()
                .map(|(meta, score)| (meta.item_id, score))
                .collect(),
            None => vec![],
        };
        (search_results, Some(query_vec))
    })
    .await;

    result.unwrap_or((vec![], None))
}

pub async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cache_key = hash_query(&params.q);
    {
        let mut cache = state.search_cache.lock().map_err(|_| err_500("cache lock poisoned"))?;
        if let Some(entry) = cache.get(&cache_key) {
            if !entry.is_expired() {
                return Ok(Json(serde_json::json!({
                    "query": params.q,
                    "results": entry.results,
                    "total": entry.results.len(),
                    "cached": true
                })));
            }
        }
    }

    let dek = {
        let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
        vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?
    };

    // Fulltext search
    let ft_results = {
        let ft_guard = state.fulltext.lock().map_err(|_| err_500("fulltext lock poisoned"))?;
        match ft_guard.as_ref() {
            Some(ft) => ft.search(&params.q, params.top_k).unwrap_or_default(),
            None => vec![],
        }
    };

    // Vector search (if embedding available)
    let (vec_results, query_vec) = embed_query(&state, &params.q).await;

    // RRF fusion
    let fused = rrf_fuse(&vec_results, &ft_results, 0.6, 0.4, params.top_k);

    // Fetch and decrypt items
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = vault.store().get_item(&dek, item_id) {
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

    // Rerank when top_k is small enough and query vector is available
    if params.top_k <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = query_vec {
            let vec_guard = state.vectors.lock().map_err(|_| err_500("vectors lock poisoned"))?;
            if let Some(vecs) = vec_guard.as_ref() {
                rerank(&qvec, &mut results, vecs);
            }
        }
    }

    {
        let mut cache = state.search_cache.lock().map_err(|_| err_500("cache lock poisoned"))?;
        cache.put(cache_key, crate::state::CachedSearch {
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

    let dek = {
        let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
        vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?
    };

    // Fulltext search
    let ft_results = {
        let ft_guard = state.fulltext.lock().map_err(|_| err_500("fulltext lock poisoned"))?;
        match ft_guard.as_ref() {
            Some(ft) => ft.search(&body.query, top_k).unwrap_or_default(),
            None => vec![],
        }
    };

    // Vector search (if embedding available)
    let (vec_results, query_vec) = embed_query(&state, &body.query).await;

    // RRF fusion
    let fused = rrf_fuse(&vec_results, &ft_results, 0.6, 0.4, top_k);

    // Fetch and decrypt items
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = vault.store().get_item(&dek, item_id) {
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

    // Rerank when top_k is small enough and query vector is available
    if top_k <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = query_vec {
            let vec_guard = state.vectors.lock().map_err(|_| err_500("vectors lock poisoned"))?;
            if let Some(vecs) = vec_guard.as_ref() {
                rerank(&qvec, &mut results, vecs);
            }
        }
    }

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
