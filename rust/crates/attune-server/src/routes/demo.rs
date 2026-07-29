//! v0.7 — Wizard "加载示例" 路由
//!
//! `POST /api/v1/demo/load` — 一键加载 attune-core 内嵌的 5 个示例 item。
//! 已经加载过（source_type='demo' 计数 > 0）则幂等 skip，不重复入库。
//!
//! 返回 JSON: `{ "loaded": N, "skipped": true|false }`

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::routes::errors::{internal, vault_locked};
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ResetDemoRequest {
    pub confirm: String,
}

pub async fn load_demo(State(state): State<SharedState>) -> AppResult<impl IntoResponse> {
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

    let items = attune_core::demo::load_demo_items().map_err(|e| internal("load_demo_items", e))?;
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

pub async fn reset_demo(
    State(state): State<SharedState>,
    Json(body): Json<ResetDemoRequest>,
) -> AppResult<impl IntoResponse> {
    if body.confirm != "CLEAR_DEMO" {
        return Err(AppError::BadRequest("confirm must be CLEAR_DEMO".into()));
    }

    let item_ids = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db().map_err(|_| vault_locked())?;
        vault
            .store()
            .list_all_item_ids()
            .map_err(|e| internal("list_all_item_ids", e))?
    };

    let mut items_deleted = 0usize;
    let mut vectors_deleted = 0usize;
    let mut queue_cleared = 0usize;
    let mut bound_dirs_cleared = 0usize;
    let mut source_tracking_cleared = 0usize;

    {
        let fulltext_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        let mut vectors_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|_| vault_locked())?;

        for id in &item_ids {
            if let (Some(vectors), Some(fulltext)) =
                (vectors_guard.as_mut(), fulltext_guard.as_ref())
            {
                let stats =
                    attune_core::reindex::purge_item_indexes(vault.store(), vectors, fulltext, id)
                        .map_err(|e| internal("purge_item_indexes", e))?;
                vectors_deleted += stats.vectors_deleted;
                queue_cleared += stats.queue_cleared;
            } else {
                queue_cleared += vault
                    .store()
                    .purge_embed_queue_for_item(id)
                    .map_err(|e| internal("purge_embed_queue_for_item", e))?;
            }

            if vault
                .store()
                .delete_item(id)
                .map_err(|e| internal("delete_item", e))?
            {
                items_deleted += 1;
            }
        }

        let (bound_dirs, indexed_files) = vault
            .store()
            .clear_all_source_tracking()
            .map_err(|e| internal("clear_all_source_tracking", e))?;
        bound_dirs_cleared += bound_dirs;
        source_tracking_cleared += indexed_files;
        queue_cleared += vault
            .store()
            .clear_demo_async_queues()
            .map_err(|e| internal("clear_demo_async_queues", e))?;

        if let Some(vectors) = vectors_guard.as_ref() {
            let vectors_path = attune_core::platform::data_dir().join("vectors.encbin");
            vectors
                .save_encrypted(&dek, &vectors_path)
                .map_err(|e| internal("save_vectors_after_demo_reset", e))?;
        }
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "items_deleted": items_deleted,
        "vectors_deleted": vectors_deleted,
        "queue_cleared": queue_cleared,
        "bound_dirs_cleared": bound_dirs_cleared,
        "source_tracking_cleared": source_tracking_cleared,
    })))
}
