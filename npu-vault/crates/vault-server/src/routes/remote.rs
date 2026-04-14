use crate::state::SharedState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use vault_core::scanner_webdav::{scan_remote, WebDavConfig};

#[derive(Deserialize)]
pub struct BindRemoteRequest {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: u32,
}
fn default_depth() -> u32 {
    1
}

/// POST /api/v1/index/bind-remote — 绑定远程 WebDAV 目录并扫描
pub async fn bind_remote(
    State(state): State<SharedState>,
    Json(body): Json<BindRemoteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let config = WebDavConfig {
        url: body.url.clone(),
        username: body.username.clone(),
        password: body.password.clone(),
        depth: body.depth,
    };

    // Create bound_dirs record with special prefix to mark as remote
    let dir_id = {
        let vault = state.vault.lock().unwrap();
        let _ = vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

        vault
            .store()
            .bind_directory(&format!("webdav:{}", body.url), false, &["md", "txt"])
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?
    };

    // Run scan in blocking task (webdav uses blocking reqwest)
    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let scan_result = tokio::task::spawn_blocking(move || {
        let vault = state_clone.vault.lock().unwrap();
        let dek = vault.dek_db()?;
        scan_remote(&config, vault.store(), &dek, &dir_id_clone)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "dir_id": dir_id,
        "scan": scan_result
    })))
}
