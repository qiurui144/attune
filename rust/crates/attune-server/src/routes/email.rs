//! Email IMAP 采集账户 route —— 账户 CRUD + 手动同步触发。

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use attune_core::ingest::EmailConfig;
use attune_core::store::email_accounts::EmailAccountInput;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// 默认同步文件夹 —— body 未给 folders 时用 INBOX + Sent。
fn default_folders() -> Vec<String> {
    vec!["INBOX".to_string(), "Sent".to_string()]
}

fn default_imap_port() -> u16 {
    993
}

#[derive(Deserialize)]
pub struct BindEmailRequest {
    pub host: String,
    #[serde(default = "default_imap_port")]
    pub port: u16,
    pub username: String,
    pub password: String,
    #[serde(default = "default_folders")]
    pub folders: Vec<String>,
    #[serde(default)]
    pub corpus_domain: Option<String>,
}

#[derive(Serialize)]
pub struct EmailAccountView {
    pub dir_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub folders: Vec<String>,
    pub corpus_domain: String,
    pub last_sync: Option<String>,
}

/// 校验账户输入：host / username / password 不能为空，port 必须非 0。
fn validate(req: &BindEmailRequest) -> Result<(), AppError> {
    if req.host.trim().is_empty() {
        return Err(AppError::BadRequest("host must not be empty".into()));
    }
    if req.username.trim().is_empty() {
        return Err(AppError::BadRequest("username must not be empty".into()));
    }
    if req.password.is_empty() {
        return Err(AppError::BadRequest("password must not be empty".into()));
    }
    if req.port == 0 {
        return Err(AppError::BadRequest("port must not be zero".into()));
    }
    Ok(())
}

/// GET /api/v1/index/email-accounts —— 列出已配置账户（不含密码）。
pub async fn list_email_accounts(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db()?;
    let rows = vault.store().list_email_accounts(&dek)?;
    let accounts: Vec<EmailAccountView> = rows
        .into_iter()
        .map(|r| EmailAccountView {
            dir_id: r.dir_id,
            host: r.host,
            port: r.port,
            username: r.username,
            folders: r.folders,
            corpus_domain: r.corpus_domain,
            last_sync: r.last_sync,
        })
        .collect();
    Ok(Json(serde_json::json!({ "accounts": accounts })))
}

/// POST /api/v1/index/bind-email —— 新增 / 更新账户并立即跑首轮同步。
pub async fn bind_email(
    State(state): State<SharedState>,
    Json(req): Json<BindEmailRequest>,
) -> AppResult<Json<serde_json::Value>> {
    validate(&req)?;
    let corpus_domain = req
        .corpus_domain
        .clone()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| "general".to_string());

    // 创建 / 复用 bound_dirs 记录（email: 前缀标记邮箱源）+ 落库加密账户配置。
    let dir_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let store = vault.store();
        let path = format!("email:{}@{}", req.username, req.host);
        let dir_id = store
            .bind_directory(&path, false, &["eml"])
            .map_err(|e| AppError::Internal(format!("bind email dir: {e}")))?;
        let input = EmailAccountInput {
            dir_id: dir_id.clone(),
            host: req.host.clone(),
            port: req.port,
            username: req.username.clone(),
            password: req.password.clone(),
            folders: req.folders.clone(),
            corpus_domain: corpus_domain.clone(),
        };
        store
            .upsert_email_account(&dek, &input)
            .map_err(|e| AppError::Internal(format!("persist email account: {e}")))?;
        dir_id
    };

    // 首轮同步在阻塞线程跑（IMAP 网络 I/O + DB 写）—— 不阻塞 axum worker。
    let config = EmailConfig {
        host: req.host.clone(),
        port: req.port,
        username: req.username.clone(),
        password: req.password.clone(),
        folders: req.folders.clone(),
    };
    let state_cloned = state.clone();
    let dir_cloned = dir_id.clone();
    let domain_cloned = corpus_domain.clone();
    let stats = tokio::task::spawn_blocking(move || {
        crate::ingest_email::sync_email_account(
            &state_cloned,
            &dir_cloned,
            config,
            &domain_cloned,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("email sync task join: {e}")))?
    .map_err(AppError::BadGateway)?;

    Ok(Json(serde_json::json!({
        "dir_id": dir_id,
        "sync": stats,
    })))
}

/// DELETE /api/v1/index/email-accounts/{dir_id} —— 删除账户（已入库内容保留）。
pub async fn delete_email_account(
    State(state): State<SharedState>,
    Path(dir_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    let store = vault.store();
    store
        .delete_email_account(&dir_id)
        .map_err(|e| AppError::Internal(format!("delete email account: {e}")))?;
    // bound_dirs 记录一并解绑（email_folder_uids 经 ON DELETE CASCADE 已清）。
    let _ = store.unbind_directory(&dir_id);
    Ok(Json(serde_json::json!({ "deleted": dir_id })))
}

/// POST /api/v1/index/email-accounts/{dir_id}/sync —— 手动触发一次增量同步。
pub async fn sync_email_account_now(
    State(state): State<SharedState>,
    Path(dir_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let (config, corpus_domain) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let row = vault
            .store()
            .get_email_account(&dek, &dir_id)?
            .ok_or_else(|| AppError::NotFound(format!("email account {dir_id}")))?;
        let config = EmailConfig {
            host: row.host,
            port: row.port,
            username: row.username,
            password: row.password,
            folders: row.folders,
        };
        (config, row.corpus_domain)
    };

    let state_cloned = state.clone();
    let dir_cloned = dir_id.clone();
    let stats = tokio::task::spawn_blocking(move || {
        crate::ingest_email::sync_email_account(
            &state_cloned,
            &dir_cloned,
            config,
            &corpus_domain,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("email sync task join: {e}")))?
    .map_err(AppError::BadGateway)?;

    Ok(Json(serde_json::json!({
        "dir_id": dir_id,
        "sync": stats,
    })))
}
