use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ClickRequest {
    pub query: String,
    pub item_id: String,
}

/// POST /api/v1/behavior/click — 记录一次点击
pub async fn log_click(
    State(state): State<SharedState>,
    Json(body): Json<ClickRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    vault.store().log_click(&dek, &body.query, &body.item_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize { 20 }

/// GET /api/v1/behavior/history — 最近搜索历史
pub async fn history(
    State(state): State<SharedState>,
    Query(params): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let limit = params.limit.min(200);
    let history = vault.store().recent_searches(&dek, limit)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    Ok(Json(serde_json::json!({"history": history})))
}

/// GET /api/v1/behavior/popular — 最常点击的条目
pub async fn popular(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let popular = vault.store().popular_items(20)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let list: Vec<serde_json::Value> = popular.into_iter()
        .map(|(id, count)| serde_json::json!({"item_id": id, "click_count": count}))
        .collect();

    Ok(Json(serde_json::json!({"popular": list})))
}
