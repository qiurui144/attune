use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;

#[derive(Deserialize)]
pub struct FeedbackRequest {
    pub item_id: String,
    pub feedback_type: String,
    pub query: Option<String>,
}

/// POST /api/v1/feedback — 提交搜索结果反馈
pub async fn submit_feedback(
    State(state): State<SharedState>,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.item_id.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "item_id too long"}))));
    }
    if body.query.as_deref().map(|q| q.len()).unwrap_or(0) > 500 {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "query too long (max 500 bytes)"}))));
    }
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let id = vault
        .store()
        .insert_feedback(&body.item_id, &body.feedback_type, body.query.as_deref())
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    Ok(Json(serde_json::json!({"id": id, "status": "ok"})))
}
