// npu-vault/crates/vault-server/src/routes/chat_sessions.rs

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

fn err_500(msg: &str) -> AppError {
    AppError::Internal(msg.to_string())
}

/// Sentinel `project_id` value selecting only loose (project-less) conversations.
/// chat-centric IA (2026-06-26).
pub const LOOSE_SENTINEL: &str = "__loose__";

#[derive(Deserialize)]
pub struct PaginationQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    /// chat-centric IA (2026-06-26): optional project filter.
    ///   - absent          → all conversations (legacy behavior)
    ///   - `__loose__`     → only loose (project_id IS NULL)
    ///   - `<project id>`  → only that project's branch conversations
    #[serde(default)]
    pub project_id: Option<String>,
}

fn default_limit() -> usize {
    20
}

/// Map the `project_id` query param to the store's three-state filter.
///   None → None (all); Some("__loose__") → Some(None); Some(pid) → Some(Some(pid)).
fn project_filter(project_id: Option<&str>) -> Option<Option<&str>> {
    match project_id {
        None => None,
        Some(LOOSE_SENTINEL) => Some(None),
        Some(pid) => Some(Some(pid)),
    }
}

/// GET /api/v1/chat/sessions?limit=20&offset=0&project_id=<id|__loose__>
pub async fn list_sessions(
    State(state): State<SharedState>,
    Query(params): Query<PaginationQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let dek = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let limit = params.limit.min(200);
    let filter = project_filter(params.project_id.as_deref());
    let sessions = vault
        .store()
        .list_conversations_scoped(&dek, filter, limit, params.offset)
        .map_err(|e| err_500(&e.to_string()))?;
    let total = sessions.len();
    Ok(Json(serde_json::json!({
        "sessions": sessions,
        "total": total,
    })))
}

/// GET /api/v1/chat/sessions/:id
pub async fn get_session(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let dek = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let summary = vault
        .store()
        .get_conversation_by_id(&dek, &session_id)
        .map_err(|e| err_500(&e.to_string()))?
        .ok_or_else(|| AppError::NotFound("session not found".into()))?;
    let messages = vault
        .store()
        .get_conversation_messages(&dek, &session_id)
        .map_err(|e| err_500(&e.to_string()))?;
    Ok(Json(serde_json::json!({
        "session": summary,
        "messages": messages,
    })))
}

/// DELETE /api/v1/chat/sessions/:id
pub async fn delete_session(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> AppResult<StatusCode> {
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    // 仅校验 vault 已解锁（DEK 本身不用于 delete，不需要传给 store）
    let _ = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    vault
        .store()
        .delete_conversation(&session_id)
        .map_err(|e| err_500(&e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
