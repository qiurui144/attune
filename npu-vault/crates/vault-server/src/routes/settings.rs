use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

const SETTINGS_KEY: &str = "app_settings";

pub async fn get_settings(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let settings = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    match settings {
        Some(data) => {
            let json: serde_json::Value = serde_json::from_slice(&data).unwrap_or(default_settings());
            Ok(Json(json))
        }
        None => Ok(Json(default_settings())),
    }
}

pub async fn update_settings(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // Merge with existing settings
    let existing = vault.store().get_meta(SETTINGS_KEY)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let mut current: serde_json::Value = match existing {
        Some(data) => serde_json::from_slice(&data).unwrap_or(default_settings()),
        None => default_settings(),
    };

    // 白名单校验：只允许写入已知配置键，防止任意键污染 vault_meta
    const ALLOWED_KEYS: &[&str] = &[
        "injection_mode", "injection_budget", "excluded_domains", "search", "embedding", "web_search",
    ];
    if let (Some(current_obj), Some(body_obj)) = (current.as_object_mut(), body.as_object()) {
        for (k, v) in body_obj {
            if ALLOWED_KEYS.contains(&k.as_str()) {
                current_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let data = serde_json::to_vec(&current)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
    vault.store().set_meta(SETTINGS_KEY, &data)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    Ok(Json(current))
}

fn default_settings() -> serde_json::Value {
    serde_json::json!({
        "injection_mode": "auto",
        "injection_budget": 2000,
        "excluded_domains": ["mail.google.com", "web.whatsapp.com"],
        "search": {
            "default_top_k": 10,
            "vector_weight": 0.6,
            "fulltext_weight": 0.4
        },
        "embedding": {
            "model": "bge-m3",
            "ollama_url": "http://localhost:11434"
        },
        "web_search": {
            "enabled": false,
            "provider": "brave",
            "api_key": "",
            "base_url": ""
        }
    })
}
