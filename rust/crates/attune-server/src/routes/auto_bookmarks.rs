//! W4 G2 — 自动 bookmark 候选 routes（2026-04-27）。
//!
//! per W4 plan G2 + spec §3.G2 修订。
//! POST 不暴露 — 候选行只能由 routes::browse_signals::record_batch 内 high_engagement
//! 路径写入（保证 G1/G2 同源 + 同 dwell/scroll/copy 阈值）。
//!
//! W4-005: 错误响应走 routes::errors helper。

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::routes::errors::{internal, vault_locked};
use crate::error::AppResult;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub only_pending: bool,
}
fn default_limit() -> usize {
    20
}

/// GET /api/v1/auto_bookmarks?limit=20&only_pending=true — 列表诊断 + G3 worker 输入
pub async fn list(
    State(state): State<SharedState>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let limit = q.limit.min(200);
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let total = vault.store().auto_bookmarks_count().unwrap_or(0);
    let pending = vault.store().pending_auto_bookmarks_count().unwrap_or(0);
    let rows = vault
        .store()
        .list_recent_auto_bookmarks(&dek, limit, q.only_pending)
        .map_err(|e| internal("list_recent_auto_bookmarks", e))?;
    // 出 JSON 时不直接 serialize struct (AutoBookmarkRow 没有 Serialize) — 手动构造
    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "url": r.url,
                "title": r.title,
                "domain_hash": r.domain_hash,
                "dwell_ms": r.dwell_ms,
                "scroll_pct": r.scroll_pct,
                "copy_count": r.copy_count,
                "visit_count": r.visit_count,
                "created_at_secs": r.created_at_secs,
                "promoted": r.promoted,
                "promoted_item_id": r.promoted_item_id,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "total": total,
        "pending": pending,
        "items": items,
    })))
}

/// DELETE /api/v1/auto_bookmarks — 全清候选（含已 promote 的历史记录）
/// 注意：不清 items 表 — 用户应另走 items.delete 删 promote 后的真实条目
pub async fn delete(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;
    let n = vault
        .store()
        .clear_all_auto_bookmarks()
        .map_err(|e| internal("clear_all_auto_bookmarks", e))?;
    Ok(Json(serde_json::json!({"deleted": n})))
}
