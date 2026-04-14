use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;
use vault_core::taxonomy::Taxonomy;

/// GET /api/v1/plugins — 列出所有可用插件（内置 + 用户）
pub async fn list(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // First try from AppState's loaded taxonomy
    if let Some(tax) = state.taxonomy.lock().unwrap().as_ref() {
        let list: Vec<serde_json::Value> = tax.plugins.iter().map(|p| serde_json::json!({
            "id": p.id,
            "name": p.name,
            "version": p.version,
            "description": p.description,
            "source": if ["tech", "law"].contains(&p.id.as_str()) { "builtin" } else { "user" },
            "dimensions": p.dimensions.iter().map(|d| serde_json::json!({
                "name": d.name,
                "label": d.label,
                "description": d.description,
            })).collect::<Vec<_>>(),
        })).collect();
        return Ok(Json(serde_json::json!({"plugins": list})));
    }

    // Fallback: vault locked, only return builtins
    let plugins = Taxonomy::load_builtin_plugins()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let list: Vec<serde_json::Value> = plugins.iter().map(|p| serde_json::json!({
        "id": p.id,
        "name": p.name,
        "version": p.version,
        "description": p.description,
        "source": "builtin",
        "dimensions": p.dimensions.iter().map(|d| serde_json::json!({
            "name": d.name,
            "label": d.label,
            "description": d.description,
        })).collect::<Vec<_>>(),
    })).collect();

    Ok(Json(serde_json::json!({"plugins": list})))
}
