use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

/// GET /api/v1/clusters — 当前聚类快照
pub async fn list(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let snapshot = state.cluster_snapshot.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "lock poisoned"}))))?
        .clone();
    match snapshot {
        Some(s) => {
            let val = serde_json::to_value(&s)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
            Ok(Json(val))
        }
        None => Ok(Json(serde_json::json!({
            "clusters": [],
            "note": "no cluster snapshot yet, POST /clusters/rebuild to generate"
        }))),
    }
}

/// GET /api/v1/clusters/{id} — 某聚类详情
pub async fn detail(
    State(state): State<SharedState>,
    Path(id): Path<i32>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let snapshot = state.cluster_snapshot.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "lock poisoned"}))))?;
    match snapshot.as_ref() {
        Some(s) => {
            match s.clusters.iter().find(|c| c.id == id) {
                Some(c) => {
                    let val = serde_json::to_value(c)
                        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
                    Ok(Json(val))
                }
                None => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "cluster not found"})))),
            }
        }
        None => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "no snapshot"})))),
    }
}

/// POST /api/v1/clusters/rebuild — 手动触发聚类（占位实现）
pub async fn rebuild(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "note": "cluster rebuild is a heavy operation; full implementation pending in next phase"
    })))
}
