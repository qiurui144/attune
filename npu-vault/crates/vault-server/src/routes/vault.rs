use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct SetupRequest {
    pub password: String,
}

#[derive(Deserialize)]
pub struct UnlockRequest {
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

pub async fn vault_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let vault_state = vault.state();
    let item_count = if matches!(vault_state, vault_core::vault::VaultState::Unlocked) {
        vault.store().item_count().unwrap_or(0)
    } else { 0 };

    Json(serde_json::json!({
        "state": vault_state,
        "items": item_count,
    }))
}

pub async fn vault_setup(
    State(state): State<SharedState>,
    Json(body): Json<SetupRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.setup(&body.password).map_err(|e| {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
        })?;
    }
    // Initialize search engines after vault setup (vault mutex released)
    state.init_search_engines();
    crate::state::AppState::start_classify_worker(state.clone());
    crate::state::AppState::start_rescan_worker(state.clone());
    crate::state::AppState::start_queue_worker(state.clone());
    Ok(Json(serde_json::json!({"status": "ok", "state": "unlocked"})))
}

pub async fn vault_unlock(
    State(state): State<SharedState>,
    Json(body): Json<UnlockRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let token = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.unlock(&body.password).map_err(|e| {
            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e.to_string()})))
        })?
    };
    // Initialize search engines after vault unlock (vault mutex released)
    state.init_search_engines();
    crate::state::AppState::start_classify_worker(state.clone());
    crate::state::AppState::start_rescan_worker(state.clone());
    crate::state::AppState::start_queue_worker(state.clone());
    Ok(Json(serde_json::json!({"status": "ok", "token": token})))
}

pub async fn vault_lock(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Clear search engines before locking (no vault mutex held)
    state.clear_search_engines();
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.lock().map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
        })?;
    }
    Ok(Json(serde_json::json!({"status": "ok", "state": "locked"})))
}

pub async fn export_device_secret(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let secret = vault.export_device_secret().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    Ok(Json(serde_json::json!({
        "device_secret": secret,
        "warning": "Store this value securely. It's required to unlock the vault on other devices."
    })))
}

#[derive(Deserialize)]
pub struct ImportDeviceSecretRequest {
    pub device_secret: String,
}

pub async fn import_device_secret(
    State(state): State<SharedState>,
    Json(body): Json<ImportDeviceSecretRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault.import_device_secret(&body.device_secret).map_err(|e| {
        (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "message": "device secret imported. Use /vault/unlock with your master password."
    })))
}

pub async fn vault_change_password(
    State(state): State<SharedState>,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault.change_password(&body.old_password, &body.new_password).map_err(|e| {
        (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"status": "ok"})))
}
