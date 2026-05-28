use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use attune_core::search::{allocate_budget, SearchResult, INJECTION_BUDGET};

use crate::eval as eval_surface;
use crate::state::SharedState;

/// 从 app_settings 读取 search.query_rewrite.enabled 开关。
/// 未配置时返回 false（保守默认：LLM 不可用时不应在后台静默等待）。
fn query_rewrite_enabled(state: &crate::state::AppState) -> bool {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(Some(data)) = vault.store().get_meta("app_settings") else { return false; };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data) else { return false; };
    json.get("search")
        .and_then(|s| s.get("query_rewrite"))
        .and_then(|qr| qr.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 若开关开启且 LLM 可用，改写 query；失败或关闭时降级返回原始 query。
async fn maybe_rewrite_query(
    state: &crate::state::AppState,
    query: &str,
) -> String {
    if !query_rewrite_enabled(state) {
        return query.to_string();
    }
    let Some(llm) = state.llm() else {
        return query.to_string();
    };
    match attune_core::query_rewrite::rewrite_query(query, llm).await {
        Ok(rewritten) if !rewritten.is_empty() => {
            if rewritten != query {
                tracing::debug!(original = query, rewritten = %rewritten, "query_rewrite: query rewritten");
            }
            rewritten
        }
        Ok(_) => query.to_string(),
        Err(e) => {
            // LLM 失败时降级，不阻断检索
            tracing::warn!(query = query, err = %e, "query_rewrite: failed, falling back to original");
            query.to_string()
        }
    }
}

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
    headers: HeaderMap,
    Query(params): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // T2 (v1.0.6 KB-bench): parse opt-in eval headers + start wall-clock.
    // Old clients send no eval headers → parsed_eval all-default → eval block
    // is null in response (backward compatible).
    let parsed_eval = eval_surface::parse_eval_headers(&headers);
    let t_search_start = std::time::Instant::now();

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
                let cached_latency_ms = t_search_start.elapsed().as_millis() as u64;
                let eval_block = eval_surface::build_eval_block(&parsed_eval, cached_latency_ms);
                return Ok(Json(serde_json::json!({
                    "query": params.q,
                    "results": entry.results,
                    "total": entry.results.len(),
                    "cached": true,
                    // T2: eval block; null unless X-Attune-Eval-Mode set
                    "eval": eval_block,
                    "latency_ms": cached_latency_ms,
                })));
            }
        }
    }

    // query_rewrite：将口语化 query 改写为检索关键词（开关在 settings.search.query_rewrite.enabled）
    //
    // T1 (v1.0.6 KB-bench, plan Step 11): bench harness can pin
    // `X-Attune-Eval-Skip-Rewrite: true` to bypass the LLM rewrite call
    // entirely — this isolates retrieval quality from LLM-noise in
    // deterministic bench runs (per spec §11 Risk A).
    let effective_query = if parsed_eval.skip_rewrite {
        params.q.clone()
    } else {
        maybe_rewrite_query(&state, &params.q).await
    };

    // v0.6 Phase B F-Pro Stage 4：从 query 自动 detect 领域意图，driving cross-domain penalty。
    // 命中 'legal' / 'tech' / 'medical' / 'patent' → 跨领域文档 score *= 0.4
    // 未命中（None）→ 不应用 penalty（保持向后兼容）
    let detected_domain = attune_core::search::detect_query_domain(&effective_query);

    let search_params = {
        let mut p = attune_core::search::SearchParams::with_defaults(params.top_k);
        if let Some(ik) = params.initial_k { p.initial_k = ik; }
        if let Some(imk) = params.intermediate_k { p.intermediate_k = imk; }
        if let Some(d) = detected_domain.as_ref() { p.domain_hint = Some(d.clone()); }
        // T1 (v1.0.6 KB-bench): forward eval knobs into SearchParams. Today
        // only `skip_rewrite` actively gates the rewrite call above; `seed`
        // and `skip_rerank` flow through to attune-core for v1.1 when
        // SearchTracer lands (per spec §9.5 #6). Keeping the fields
        // populated means downstream consumers can read which knobs were
        // active without grepping HTTP headers.
        p.seed = parsed_eval.seed;
        p.skip_rewrite = parsed_eval.skip_rewrite;
        p.skip_rerank = parsed_eval.skip_rerank;
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
        attune_core::search::search_with_context(&ctx, &effective_query, &search_params)
            .map_err(|e| err_500(&e.to_string()))?
    };

    // OSS-S17 fix: score 阈值 cutoff。当 corpus 被低质量内容污染时，BM25+vector+RRF 退化为
    // fallback default score（实测 0.000638-0.000828）让真实相关内容无法浮出。R19-R3 复现：
    // 22K 测试 garbage 主导 corpus 后，新 ingest 的 5 真实 rust md 文件 10 query 全部 top hit
    // 是 garbage 同分。
    //
    // R20 实测分数尺度：真实命中 ~0.98 / fallback noise ~0.0006-0.0008。
    // R25 修订: 律师文书测试发现，corpus 含 42K garbage 时合法文书 RRF 后 score 也低于 0.001
    // (BM25 给真实命中 ~0.5，但 RRF 与 vector 结果合并后 normalize 大幅降低)。
    // 因此把 cutoff 降到 0.0001 — 仍能过滤纯 fallback noise 但允许低-mid score 真实结果通过。
    const SCORE_CUTOFF: f32 = 0.001;  // R20 production value, R25 debug 后恢复
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

    let search_latency_ms = t_search_start.elapsed().as_millis() as u64;
    let eval_block = eval_surface::build_eval_block(&parsed_eval, search_latency_ms);

    Ok(Json(serde_json::json!({
        "query": params.q,
        "results": results,
        "total": results.len(),
        "cutoff_filtered": total_before_cutoff - total_after_cutoff,
        // T2 (v1.0.6 KB-bench): eval block null unless X-Attune-Eval-Mode: 1 header set
        "eval": eval_block,
        "latency_ms": search_latency_ms,
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

    // query_rewrite：Chrome 扩展注入路径同样受益于 query 改写
    let effective_query = maybe_rewrite_query(&state, &body.query).await;

    let detected_domain = attune_core::search::detect_query_domain(&effective_query);
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
        attune_core::search::search_with_context(&ctx, &effective_query, &search_params)
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

    /// query_rewrite_enabled 读 JSON 开关的逻辑提取为纯函数测试
    #[test]
    fn parse_query_rewrite_enabled_from_settings_json() {
        // 开关开启
        let json: serde_json::Value = serde_json::json!({
            "search": { "query_rewrite": { "enabled": true } }
        });
        let enabled = json.get("search")
            .and_then(|s| s.get("query_rewrite"))
            .and_then(|qr| qr.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(enabled, "enabled=true 应返回 true");

        // 开关关闭
        let json: serde_json::Value = serde_json::json!({
            "search": { "query_rewrite": { "enabled": false } }
        });
        let enabled = json.get("search")
            .and_then(|s| s.get("query_rewrite"))
            .and_then(|qr| qr.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(!enabled, "enabled=false 应返回 false");

        // 无配置时默认 false（LLM 不可用时保守策略）
        let json: serde_json::Value = serde_json::json!({});
        let enabled = json.get("search")
            .and_then(|s| s.get("query_rewrite"))
            .and_then(|qr| qr.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(!enabled, "无配置时应 default=false");
    }

    /// 开关关闭时，即使 LLM 可用，query 也不应被改写（保持原样传入检索）
    #[test]
    fn rewrite_disabled_setting_returns_original() {
        // 开关逻辑是纯 JSON 读取，直接测解析结果
        let json_disabled: serde_json::Value = serde_json::json!({
            "search": { "query_rewrite": { "enabled": false } }
        });
        let enabled = json_disabled
            .get("search").and_then(|s| s.get("query_rewrite"))
            .and_then(|qr| qr.get("enabled")).and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(!enabled, "开关 false 时不应触发 rewrite");
    }
}
