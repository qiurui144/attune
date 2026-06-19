//! 第三方账号统一管理 route(弱腿增量波 C / 能力 B)。
//!
//! 通用第三方账号凭据(WebDAV / IMAP / RSS / Git / 其它 OpenAI-compat 源)的
//! 加密登记层:
//! - `GET    /api/v1/accounts`        → 列已连账号(**脱敏**,绝不回 secret)
//! - `POST   /api/v1/accounts`        → 新增一条凭据(明文 secret 仅入参,落库即 AES-256-GCM 加密)
//! - `DELETE /api/v1/accounts/{id}`   → 删除一条凭据
//!
//! **安全契约(§1.4)**:secret 只在 POST body 单向入参;响应 / 日志 / 列表**永不回显**。
//! 凭据由 attune-core store 层用 dek 字段级加密落 `secret_enc` BLOB,明文绝不落盘。
//!
//! **建议卡解耦**:本 route **不**提供 `/suggestions` —— 那是 `routes::suggestions`
//! 的既有端点(零成本规则引擎)。本表只提供 `connected_provider_kinds()` 计数,由
//! suggestions route 读入 `SignalContext.connected_source_count` 驱动 ConnectSource 卡。
//! 整链零 LLM、零出网。
//!
//! per spec docs/superpowers/specs/2026-06-17-suggestions-and-thirdparty-accounts.md 能力 B。

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use attune_core::store::third_party_accounts::{
    is_known_provider, ThirdPartyAccountInput, ThirdPartyAccountView,
};

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct AddAccountRequest {
    pub provider: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub username: String,
    pub endpoint: String,
    /// 明文凭据 —— 仅入参,落库即加密;**绝不回显**。
    pub secret: String,
}

/// 脱敏账户视图(API 响应形态,无 secret 字段)。
#[derive(Serialize)]
pub struct AccountResponse {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub username: String,
    pub endpoint: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ThirdPartyAccountView> for AccountResponse {
    fn from(v: ThirdPartyAccountView) -> Self {
        AccountResponse {
            id: v.id,
            provider: v.provider,
            label: v.label,
            username: v.username,
            endpoint: v.endpoint,
            status: v.status,
            created_at: v.created_at,
            updated_at: v.updated_at,
        }
    }
}

fn validate(req: &AddAccountRequest) -> Result<(), AppError> {
    if !is_known_provider(&req.provider) {
        return Err(AppError::BadRequest(format!(
            "unknown provider: {}",
            req.provider
        )));
    }
    if req.endpoint.trim().is_empty() {
        return Err(AppError::BadRequest("endpoint must not be empty".into()));
    }
    if req.secret.is_empty() {
        return Err(AppError::BadRequest("secret must not be empty".into()));
    }
    Ok(())
}

/// GET /api/v1/accounts —— 列出已连接的第三方账号(**脱敏,不含 secret**)。
pub async fn list_accounts(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?; // 确保已解锁(dek 可派生)。
    let rows = vault.store().list_third_party_accounts()?;
    let accounts: Vec<AccountResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(serde_json::json!({ "accounts": accounts })))
}

/// POST /api/v1/accounts —— 新增一条第三方账号凭据(加密落库)。
/// 注意:日志 / 响应**绝不打印 secret**(§1.4)。
pub async fn add_account(
    State(state): State<SharedState>,
    Json(req): Json<AddAccountRequest>,
) -> AppResult<Json<serde_json::Value>> {
    validate(&req)?;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db()?;
    let input = ThirdPartyAccountInput {
        provider: req.provider.clone(),
        label: req.label.clone(),
        username: req.username.clone(),
        endpoint: req.endpoint.clone(),
        secret: req.secret.clone(),
    };
    let id = vault
        .store()
        .add_third_party_account(&dek, &input)
        .map_err(|e| AppError::Internal(format!("persist third-party account: {e}")))?;
    // 回脱敏结果 —— 不含 secret。
    Ok(Json(serde_json::json!({
        "id": id,
        "provider": req.provider,
        "status": "connected",
    })))
}

/// DELETE /api/v1/accounts/{id} —— 删除一条凭据。
pub async fn delete_account(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    let deleted = vault.store().delete_third_party_account(&id)?;
    if !deleted {
        return Err(AppError::NotFound(format!("account {id}")));
    }
    Ok(Json(serde_json::json!({ "deleted": id })))
}
