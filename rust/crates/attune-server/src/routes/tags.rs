use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

/// GET /api/v1/tags — 所有维度的聚合统计（不含 entities）
pub async fn all_dimensions(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tag_index = state.tag_index.lock().unwrap_or_else(|e| e.into_inner());
    let index = match tag_index.as_ref() {
        Some(i) => i,
        None => return Err((StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "vault locked or tag index unavailable"})))),
    };

    let dims = index.all_dimensions();
    let mut result = serde_json::Map::new();
    for dim in &dims {
        if dim == "entities" { continue; }
        let hist = index.histogram(dim);
        let values: Vec<serde_json::Value> = hist.into_iter()
            .map(|(v, c)| serde_json::json!({"value": v, "count": c}))
            .collect();
        result.insert(dim.clone(), serde_json::Value::Array(values));
    }

    Ok(Json(serde_json::json!({"dimensions": result})))
}

/// GET /api/v1/tags/{dimension} — 某维度的完整直方图
pub async fn dimension_histogram(
    State(state): State<SharedState>,
    Path(dimension): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tag_index = state.tag_index.lock().unwrap_or_else(|e| e.into_inner());
    let index = match tag_index.as_ref() {
        Some(i) => i,
        None => return Err((StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "vault locked or tag index unavailable"})))),
    };
    let hist = index.histogram(&dimension);
    let values: Vec<serde_json::Value> = hist.into_iter()
        .map(|(v, c)| serde_json::json!({"value": v, "count": c}))
        .collect();
    Ok(Json(serde_json::json!({
        "dimension": dimension,
        "values": values
    })))
}
