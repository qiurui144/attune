use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;

/// Trust-chain T8: hydrate the in-memory [`attune_core::entitlement::EntitlementCache`]
/// from the `plugin_entitlements` vault table at unlock. Reads rows under a SHORT vault
/// lock (get dek + `list_entitlements` → owned Vec), then populates the cache (cache's
/// own independent lock) AFTER the vault lock drops — never nested (lock-ordering 铁律).
/// Best-effort: a locked / empty / unparseable vault leaves the cache empty (free users
/// or no entitlements → no dispatch gate).
fn hydrate_entitlement_cache(state: &SharedState) {
    let rows = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(dek) = vault.dek_db() else { return };
        vault.store().list_entitlements(&dek).unwrap_or_default()
        // vault lock drops here
    };
    state.entitlement_cache.hydrate_from_rows(rows);
}

fn spawn_post_unlock_services(state: SharedState) {
    // Keep unlock/setup responsive: crypto unlock returns the bearer token immediately,
    // while local indexes, model bootstrap, EP stack probing, and workers warm up in the
    // background. LLM and entitlement cache hydration stay synchronous because they are
    // short local reads and make post-login/chat routes usable right away.
    state.reload_llm();
    hydrate_entitlement_cache(&state);
    let member_restore_state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _ =
            crate::routes::member::restore_member_state_from_cloud_session(&member_restore_state);
    });

    tokio::task::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.init_search_engines();
            // #2 #5: 底座模型(embedding/reranker/ocr/asr)后台拉取,解锁不阻塞在 ~330MB 下载上。
            crate::state::AppState::spawn_model_bootstrap(state.clone());
            // EP 运行时软件栈(cuda/openvino/rocm/directml/vitisai userspace)按需安装,
            // 缺则像底座模型一样后台拉取(内核驱动除外,走 #6 consent)。
            crate::state::AppState::spawn_stack_bootstrap(state.clone());
            crate::state::AppState::start_classify_worker(state.clone());
            crate::state::AppState::start_rescan_worker(state.clone());
            crate::state::AppState::start_reindex_worker(state.clone());
            crate::state::AppState::start_webdav_sync_worker(state.clone());
            crate::state::AppState::start_email_sync_worker(state.clone());
            crate::state::AppState::start_rss_sync_worker(state.clone());
            crate::state::AppState::start_monitoring_worker(state.clone());
            crate::state::AppState::start_queue_worker(state.clone());
            crate::state::AppState::start_skill_evolver(state.clone());
            crate::state::AppState::start_entitlement_worker(state.clone());
            // G3①: drain any locked-mode staged ingests now that the DEK is available.
            crate::state::AppState::start_staging_drain_worker(state);
        }));
        if result.is_err() {
            tracing::error!("vault post-unlock service bootstrap panicked");
        }
    });
}

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
    } else {
        0
    };

    Json(serde_json::json!({
        "state": vault_state,
        "items": item_count,
    }))
}

pub async fn vault_setup(
    State(state): State<SharedState>,
    Json(body): Json<SetupRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // setup 成功后内部走一次 lock+unlock，复用 unlock 的 token 颁发路径，
    // 让首次安装直接拿到可用 token（避免客户端必须 restart server 再 unlock）。
    let (token, recovery_key) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let recovery_key = vault
            .setup_with_recovery_key(&body.password)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        // setup 自动 Unlocked；先 lock 再 unlock，复用 unlock token 颁发路径。
        // 首次安装一次性操作，多一次 Argon2id 派生可接受。
        vault
            .lock()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let token = vault
            .unlock(&body.password)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        (token, recovery_key)
    };
    // Bug-C: vault unlock 后立即触发 reload_llm,确保 settings 中已有的 llm config
    // 在 server restart 后第一次 chat 即可工作(不再依赖 member-login gateway_should_apply
    // 走 reload_llm 分支)。
    spawn_post_unlock_services(state.clone());
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
) -> AppResult<Json<serde_json::Value>> {
    let token = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .unlock(&body.password)
            .map_err(|e| AppError::Unauthorized(e.to_string()))?
    };
    // Bug-C: per setup 同步注释,unlock 后强制 reload_llm,杜绝
    // "server restart → unlock → chat 503" 的 P3。
    spawn_post_unlock_services(state.clone());
    Ok(Json(serde_json::json!({"status": "ok", "token": token})))
}

pub async fn vault_lock(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    // Clear search engines before locking (no vault mutex held)
    state.clear_search_engines();
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .lock()
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }
    Ok(Json(serde_json::json!({"status": "ok", "state": "locked"})))
}

pub async fn export_device_secret(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let secret = vault
        .export_device_secret()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

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
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault
        .import_device_secret(&body.device_secret)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "message": "device secret imported. Use /vault/unlock with your master password."
    })))
}

pub async fn vault_change_password(
    State(state): State<SharedState>,
    Json(body): Json<ChangePasswordRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault
        .change_password(&body.old_password, &body.new_password)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({"status": "ok"})))
}

pub async fn vault_forgot_password_reset(
    State(state): State<SharedState>,
    Json(body): Json<ForgotPasswordResetRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // reset 前先清理内存索引，避免残留状态继续服务。
    state.clear_search_engines();
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .forgot_password_reset(&body.confirmation)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
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
) -> AppResult<Json<serde_json::Value>> {
    let token = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .reset_password_with_recovery_key(&body.recovery_key, &body.new_password)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        vault
            .unlock(&body.new_password)
            .map_err(|e| AppError::Unauthorized(e.to_string()))?
    };

    // Bug-C: reset 后也走 unlock 同样路径,显式 reload_llm。
    spawn_post_unlock_services(state.clone());

    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "unlocked",
        "token": token,
    })))
}

/// G3① observability: pending locked-mode staged ingest count. Readable in ANY vault
/// state (no DEK needed) so a UI / monitor can show "N files queued, waiting for unlock".
pub async fn vault_staging_status(State(_state): State<SharedState>) -> Json<serde_json::Value> {
    let pending = attune_core::staging::IngestStaging::open_default().count();
    Json(serde_json::json!({ "pending": pending }))
}

/// G3② auto-unlock threat-model copy. The real key-sealing mechanism (TPM / OS keyring /
/// secure enclave) is PENDING a dedicated security review; enabling auto-unlock changes
/// the threat model (a host-key-sealed vault is readable by anyone with physical access to
/// the device). This text is surfaced in the UI next to the toggle.
const AUTO_UNLOCK_THREAT_MODEL: &str = "Enabling auto-unlock seals the vault key on this \
device so it unlocks without your password after a reboot. This changes the threat model: \
anyone with physical access to the device can then read your vault. The key-sealing \
mechanism is pending a security review and is NOT yet implemented — turning this on only \
records your intent and shows this warning; no key is written to disk.";

#[derive(Deserialize)]
pub struct AutoUnlockRequest {
    pub enabled: bool,
}

/// Path to the auto-unlock intent flag. Stored as a tiny non-secret file under config_dir
/// (a single byte `0`/`1`) so it is readable/settable regardless of vault lock state.
/// Deliberately holds NO key material — the real sealing is PENDING-security-review.
fn auto_unlock_flag_path() -> std::path::PathBuf {
    attune_core::platform::config_dir().join("auto_unlock.flag")
}

fn read_auto_unlock_flag() -> bool {
    std::fs::read(auto_unlock_flag_path())
        .ok()
        .and_then(|b| b.first().copied())
        .map(|b| b == b'1')
        .unwrap_or(false)
}

/// GET auto-unlock state. `implemented:false` signals the real sealing is not yet built.
pub async fn vault_get_auto_unlock(State(_state): State<SharedState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "enabled": read_auto_unlock_flag(),
        "implemented": false,
        "threat_model": AUTO_UNLOCK_THREAT_MODEL,
    }))
}

/// PUT auto-unlock intent. Records the flag and returns the threat-model warning. Because
/// the real key-sealing is PENDING-security-review, enabling does NOT actually seal a key
/// or auto-unlock anything — it only persists the intent + warns. This avoids shipping a
/// half-built, insecure key store.
pub async fn vault_set_auto_unlock(
    State(_state): State<SharedState>,
    Json(body): Json<AutoUnlockRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let path = auto_unlock_flag_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, if body.enabled { b"1" } else { b"0" })
        .map_err(|e| AppError::Internal(format!("persist auto-unlock flag: {e}")))?;
    Ok(Json(serde_json::json!({
        "enabled": body.enabled,
        "implemented": false,
        "warning": if body.enabled { AUTO_UNLOCK_THREAT_MODEL } else { "" },
        "code": "auto-unlock-pending-security-review",
    })))
}
