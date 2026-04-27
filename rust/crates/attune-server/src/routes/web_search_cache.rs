//! C1 web search cache routes（W4-002，2026-04-27）。
//!
//! per W3 batch A `docs/superpowers/specs/2026-04-27-w3-batch-a-design.md` C1。
//! `Store::clear_web_search_cache` fn 在 W3 已实现，本文件 W4 挂 HTTP route 闭环 — 让
//! Settings UI "清空 Web 搜索缓存" 按钮可以走 DELETE。
//!
//! 所有操作要求 vault unlocked（与 browse_signals.rs 一致）— 缓存条目内容是 DEK 加密的，
//! count/delete 不需要解密但保持 unlocked 检查作为统一访问门，防 vault locked 时被绕过。

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::state::SharedState;

/// GET /api/v1/web_search_cache — 诊断查询缓存条目数（不返回内容）
pub async fn count(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let n = vault.store().web_search_cache_count().unwrap_or(0);
    Ok(Json(serde_json::json!({"count": n})))
}

/// DELETE /api/v1/web_search_cache — 全清 web 搜索缓存
///
/// 用户在 Settings UI 点 "清空 Web 搜索缓存" 时调用。返回删除条数。
/// per CLAUDE.md cost & trigger contract：用户显式触发，永不后台偷跑。
pub async fn delete(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let n = vault.store().clear_web_search_cache().map_err(|e| {
        tracing::warn!("C1 clear_web_search_cache failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "clear cache failed"})),
        )
    })?;
    Ok(Json(serde_json::json!({"deleted": n})))
}
