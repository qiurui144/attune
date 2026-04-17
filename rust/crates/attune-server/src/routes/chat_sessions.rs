// npu-vault/crates/vault-server/src/routes/chat_sessions.rs

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err_500(msg: &str) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": msg})))
}

#[derive(Deserialize)]
pub struct PaginationQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    20
}

/// GET /api/v1/chat/sessions?limit=20&offset=0
pub async fn list_sessions(
    State(state): State<SharedState>,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let limit = params.limit.min(200);
    let sessions = vault
        .store()
        .list_conversations(&dek, limit, params.offset)
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
) -> Result<Json<serde_json::Value>, ApiError> {
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let summary = vault
        .store()
        .get_conversation_by_id(&dek, &session_id)
        .map_err(|e| err_500(&e.to_string()))?
        .ok_or_else(|| {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "session not found"})))
        })?;
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
) -> Result<StatusCode, ApiError> {
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    // 仅校验 vault 已解锁（DEK 本身不用于 delete，不需要传给 store）
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    vault
        .store()
        .delete_conversation(&session_id)
        .map_err(|e| err_500(&e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
