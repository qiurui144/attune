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

#[derive(Deserialize)]
pub struct ForgotPasswordResetRequest {
    pub confirmation: String,
}

#[derive(Deserialize)]
pub struct ResetWithRecoveryKeyRequest {
    pub recovery_key: String,
    pub new_password: String,
}

pub async fn vault_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let vault_state = vault.state();
    let item_count = if matches!(vault_state, attune_core::vault::VaultState::Unlocked) {
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
    // setup 成功后内部走一次 lock+unlock，复用 unlock 的 token 颁发路径，
    // 让首次安装直接拿到可用 token（避免客户端必须 restart server 再 unlock）。
    let (token, recovery_key) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let recovery_key = vault.setup_with_recovery_key(&body.password).map_err(|e| {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        // setup 自动 Unlocked；先 lock 再 unlock，复用 unlock token 颁发路径。
        // 首次安装一次性操作，多一次 Argon2id 派生可接受。
        vault.lock().map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        let token = vault.unlock(&body.password).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        (token, recovery_key)
    };
    // Initialize search engines after vault setup (vault mutex released)
    state.init_search_engines();
    // Bug-C: vault unlock 后立即触发 reload_llm,确保 settings 中已有的 llm config
    // 在 server restart 后第一次 chat 即可工作(不再依赖 member-login gateway_should_apply
    // 走 reload_llm 分支)。init_search_engines 内部 compare_exchange 保证 LLM 也只 init 一次;
    // 但 server 跨次重启后第二次 unlock 不会重跑 init_search_engines 的 LLM 块,故此处显式 reload。
    state.reload_llm();
    crate::state::AppState::start_classify_worker(state.clone());
    crate::state::AppState::start_rescan_worker(state.clone());
    crate::state::AppState::start_reindex_worker(state.clone());
    crate::state::AppState::start_webdav_sync_worker(state.clone());
    crate::state::AppState::start_email_sync_worker(state.clone());
    crate::state::AppState::start_rss_sync_worker(state.clone());
    crate::state::AppState::start_queue_worker(state.clone());
    crate::state::AppState::start_skill_evolver(state.clone());
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "unlocked",
        "token": token,
        "recovery_key": recovery_key,
    })))
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
    // Bug-C: per setup 同步注释,unlock 后强制 reload_llm,杜绝
    // "server restart → unlock → chat 503" 的 P3。
    state.reload_llm();
    crate::state::AppState::start_classify_worker(state.clone());
    crate::state::AppState::start_rescan_worker(state.clone());
    crate::state::AppState::start_reindex_worker(state.clone());
    crate::state::AppState::start_webdav_sync_worker(state.clone());
    crate::state::AppState::start_email_sync_worker(state.clone());
    crate::state::AppState::start_rss_sync_worker(state.clone());
    crate::state::AppState::start_queue_worker(state.clone());
    crate::state::AppState::start_skill_evolver(state.clone());
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

pub async fn vault_forgot_password_reset(
    State(state): State<SharedState>,
    Json(body): Json<ForgotPasswordResetRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // reset 前先清理内存索引，避免残留状态继续服务。
    state.clear_search_engines();
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.forgot_password_reset(&body.confirmation).map_err(|e| {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
        })?;
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "sealed",
        "message": "vault reset complete, run setup again"
    })))
}

pub async fn vault_reset_with_recovery_key(
    State(state): State<SharedState>,
    Json(body): Json<ResetWithRecoveryKeyRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let token = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .reset_password_with_recovery_key(&body.recovery_key, &body.new_password)
            .map_err(|e| {
                (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
            })?;
        vault.unlock(&body.new_password).map_err(|e| {
            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e.to_string()})))
        })?
    };

    state.init_search_engines();
    // Bug-C: reset 后也走 unlock 同样路径,显式 reload_llm。
    state.reload_llm();
    crate::state::AppState::start_classify_worker(state.clone());
    crate::state::AppState::start_rescan_worker(state.clone());
    crate::state::AppState::start_reindex_worker(state.clone());
    crate::state::AppState::start_webdav_sync_worker(state.clone());
    crate::state::AppState::start_email_sync_worker(state.clone());
    crate::state::AppState::start_rss_sync_worker(state.clone());
    crate::state::AppState::start_queue_worker(state.clone());
    crate::state::AppState::start_skill_evolver(state.clone());

    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "unlocked",
        "token": token,
    })))
}
