//! v0.7 — Wizard "加载示例" 路由
//!
//! `POST /api/v1/demo/load` — 一键加载 attune-core 内嵌的 5 个示例 item。
//! 已经加载过（source_type='demo' 计数 > 0）则幂等 skip，不重复入库。
//!
//! 返回 JSON: `{ "loaded": N, "skipped": true|false }`

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use crate::routes::errors::{internal, vault_locked};
use crate::error::AppResult;
use crate::state::SharedState;

pub async fn load_demo(
    State(state): State<SharedState>,
) -> AppResult<impl IntoResponse> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let store = vault.store();

    // 幂等检查：source_type='demo' 已有任何条目就直接返回。
    let agg = store
        .aggregate_items_by_source_type()
        .map_err(|e| internal("aggregate_items_by_source_type", e))?;
    let already_loaded = agg.iter().any(|(s, n)| s == "demo" && *n > 0);
    if already_loaded {
        let existing: i64 = agg
            .iter()
            .find(|(s, _)| s == "demo")
            .map(|(_, n)| *n)
            .unwrap_or(0);
        return Ok(Json(serde_json::json!({
            "loaded": 0,
            "skipped": true,
            "existing": existing,
        })));
    }

    let items = attune_core::demo::load_demo_items()
        .map_err(|e| internal("load_demo_items", e))?;
    let mut loaded = 0usize;
    for it in &items {
        // domain 字段透传；corpus_domain 当前 schema 没有独立列，
        // 但 Wizard 后续会把它作为 tag 写入或扩列。这里只保证 5 条 demo item 入库。
        store
            .insert_item(
                &dek,
                &it.title,
                &it.content,
                None,
                &it.source_type,
                Some(it.domain.as_str()),
                None,
            )
            .map_err(|e| internal("insert_item(demo)", e))?;
        loaded += 1;
    }

    Ok(Json(serde_json::json!({
        "loaded": loaded,
        "skipped": false,
    })))
}
