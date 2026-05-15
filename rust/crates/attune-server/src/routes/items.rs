use attune_core::reindex;
use attune_core::store::audit::PrivacyTier;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 20 }

pub async fn list_items(
    State(state): State<SharedState>,
    Query(params): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let limit = params.limit.min(200);
    let items = vault.store().list_items(limit, params.offset).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"items": items, "count": items.len()})))
}

pub async fn get_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    match vault.store().get_item(&dek, &id) {
        Ok(Some(item)) => Ok(Json(serde_json::json!(item))),
        Ok(None) => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))),
    }
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    pub title: Option<String>,
    pub content: Option<String>,
}

/// v0.7 记忆护城河：UI 编辑文档触发完整 reindex pipeline。
///
/// 之前 (≤ v0.6.3) 仅刷 SQL items.content → 搜索永远返回旧内容（release blocker）。
/// 现在：
/// 1. `Store::update_item` 算 content_hash 与旧值对比，返回 [`UpdateOutcome`]
/// 2. `content_changed == true` 时调 `reindex::reindex_item` 清旧向量/FTS/queue + 重切 chunk + 入队
/// 3. 同步喂 doc_update 信号到 skill_signals（自学习闭环 Phase B hook 1）
pub async fn update_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let outcome = vault.store()
        .update_item(&dek, &id, body.title.as_deref(), body.content.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    if !outcome.existed {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"}))));
    }

    let mut reindex_stats = None;
    if outcome.content_changed {
        // 重新读 item（拿 title + 新 content + source_type）走 reindex pipeline
        let item = vault.store().get_item(&dek, &id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "race: item gone"}))))?;

        let mut vectors_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let fulltext_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(vectors), Some(fulltext)) = (vectors_guard.as_mut(), fulltext_guard.as_ref()) {
            match reindex::reindex_item(vault.store(), vectors, fulltext, &id, &item.title, &item.content, &item.source_type) {
                Ok(stats) => reindex_stats = Some(stats),
                Err(e) => {
                    tracing::warn!("reindex_item failed for {id}: {e} — search 可能短暂 stale，下次 update 重试");
                }
            }
        } else {
            tracing::warn!("vectors / fulltext 未初始化，update_item skip reindex (item={id})");
        }
        drop(fulltext_guard);
        drop(vectors_guard);

        // Phase B hook 1: doc_update 信号喂 skill_evolution
        let _ = vault.store().record_signal_event("doc_update", &id, None);
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "content_changed": outcome.content_changed,
        "backfilled_hash": outcome.backfilled_hash,
        "reindex": reindex_stats.map(|s| serde_json::json!({
            "vectors_deleted": s.vectors_deleted,
            "queue_cleared": s.queue_cleared,
            "chunks_enqueued": s.chunks_enqueued,
        })),
    })))
}

/// v0.7 记忆护城河：删除路径同步清向量 + FTS + 队列。
///
/// 之前 (≤ v0.6.3) 仅 SQL 软删 item — `vectors::delete_by_item_id` 与
/// `fulltext::delete_document` 已实现但 0 处调用（死代码）→ orphan 向量
/// 永久残留，搜索时假命中已删文档。
pub async fn delete_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    // R9 P1-2 fix: vault Locked/Sealed 时拒绝删除（与 update_item / list_items 等
    // mutating handler 一致 — "锁着的 vault 不可被改"语义）。
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // 先清索引（vector + FTS + queue），再 SQL 软删 — 让 search 在删除窗口内
    // 不会读到"DB 已删但向量还在"的 partial 状态
    let mut purge_stats = None;
    {
        let mut vectors_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let fulltext_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(vectors), Some(fulltext)) = (vectors_guard.as_mut(), fulltext_guard.as_ref()) {
            match reindex::purge_item_indexes(vault.store(), vectors, fulltext, &id) {
                Ok(stats) => purge_stats = Some(stats),
                Err(e) => tracing::warn!("purge_item_indexes failed for {id}: {e}"),
            }
        }
    }

    match vault.store().delete_item(&id) {
        Ok(true) => {
            let _ = vault.store().record_signal_event("doc_delete", &id, None);
            Ok(Json(serde_json::json!({
                "status": "ok",
                "purge": purge_stats.map(|s| serde_json::json!({
                    "vectors_deleted": s.vectors_deleted,
                    "queue_cleared": s.queue_cleared,
                })),
            })))
        },
        Ok(false) => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))),
    }
}

#[derive(serde::Deserialize)]
pub struct StaleQuery {
    #[serde(default = "default_stale_days")]
    pub days: i64,
    #[serde(default = "default_stale_limit")]
    pub limit: i64,
}

fn default_stale_days() -> i64 { 30 }
fn default_stale_limit() -> i64 { 50 }

pub async fn list_stale_items(
    State(state): State<SharedState>,
    Query(params): Query<StaleQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let limit = params.limit.min(200);
    let items = vault.store().list_stale_items(params.days, limit).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let count = items.len();
    Ok(Json(serde_json::json!({"items": items, "count": count, "days": params.days})))
}

pub async fn get_item_stats(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    match vault.store().get_item_stats(&id) {
        Ok(Some(stats)) => Ok(Json(serde_json::json!(stats))),
        Ok(None) => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))),
    }
}

// ============================================================
// v0.6 Phase A.5.4 — per-file 隐私分级
// ============================================================

#[derive(Deserialize)]
pub struct PrivacyTierBody {
    /// "L0" (🔒 永不出网) | "L1" (脱敏→云，默认) | "L3" (LLM 脱敏→云)
    pub tier: String,
}

fn parse_tier(s: &str) -> Result<PrivacyTier, (StatusCode, Json<serde_json::Value>)> {
    match s.to_uppercase().as_str() {
        "L0" => Ok(PrivacyTier::L0),
        "L1" => Ok(PrivacyTier::L1),
        "L3" => Ok(PrivacyTier::L3),
        other => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("invalid tier '{other}'; expected L0|L1|L3")
            })),
        )),
    }
}

fn tier_str(t: PrivacyTier) -> &'static str {
    match t {
        PrivacyTier::L0 => "L0",
        PrivacyTier::L1 => "L1",
        PrivacyTier::L3 => "L3",
    }
}

/// PATCH /api/v1/items/{id}/privacy_tier  body: { "tier": "L0"|"L1"|"L3" }
pub async fn set_item_privacy(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<PrivacyTierBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tier = parse_tier(&body.tier)?;
    let vault = state.vault.lock().map_err(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"})))
    })?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    vault.store().set_item_privacy_tier(&id, tier).map_err(|e| {
        let code = if e.to_string().contains("not found") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (code, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"id": id, "privacy_tier": tier_str(tier)})))
}

/// GET /api/v1/items/{id}/privacy_tier
pub async fn get_item_privacy(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().map_err(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"})))
    })?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let tier = vault.store().get_item_privacy_tier(&id).map_err(|e| {
        let code = if e.to_string().contains("not found") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (code, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"id": id, "privacy_tier": tier_str(tier)})))
}

/// GET /api/v1/items/protected — 列出所有标记为 L0 的 item id（Settings UI "受保护文件"）
pub async fn list_protected_items(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().map_err(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"})))
    })?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let ids = vault.store().list_l0_item_ids().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let count = ids.len();
    Ok(Json(serde_json::json!({"items": ids, "count": count})))
}
