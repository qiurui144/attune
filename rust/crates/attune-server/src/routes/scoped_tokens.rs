//! G2 — scoped token 管理 REST (settings 面板)。
//!
//! 签发 / 列举 / 吊销外部 agent (MCP) 用的 scoped token。需 vault unlock (issue/list/revoke
//! 均要 master_key / DB 写) + (require_auth 时) vault session bearer —— 这是 admin 操作,
//! 与 MCP 自身的 scoped-token 认证不同 (MCP 走 /mcp 自有路径)。
//!
//! - `POST   /api/v1/scoped-tokens`        签发 (token 明文仅此一次返回)
//! - `GET    /api/v1/scoped-tokens`        列举 (不含 token 明文)
//! - `DELETE /api/v1/scoped-tokens/{id}`   吊销

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct IssueRequest {
    pub label: String,
    /// 权限位子集 ⊆ {search,chat,ingest}。
    pub scopes: Vec<String>,
    /// 可选有效期 (秒);缺省 30 天, 上限 90 天 (vault 层 clamp)。
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

/// POST /api/v1/scoped-tokens — 签发一个 scoped token。
pub async fn issue(
    State(state): State<SharedState>,
    Json(body): Json<IssueRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if body.label.trim().is_empty() || body.label.len() > 128 {
        return Err(AppError::BadRequest("label must be 1-128 chars".into()));
    }
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let (token, meta) = vault
        .issue_scoped_token(&body.label, &body.scopes, body.ttl_secs)
        .map_err(|e| match e {
            attune_core::error::VaultError::Locked => {
                AppError::Unauthorized("vault locked".into())
            }
            attune_core::error::VaultError::InvalidInput(m) => AppError::BadRequest(m),
            other => AppError::Internal(other.to_string()),
        })?;
    // token 明文仅此一次返回 (元数据已落库, 不可再取)。
    Ok(Json(serde_json::json!({
        "token": token,
        "token_id": meta.token_id,
        "label": meta.label,
        "scopes": meta.scopes,
        "expires_at": meta.expires_at,
    })))
}

/// GET /api/v1/scoped-tokens — 列举 (不含 token 明文)。
pub async fn list(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let items = vault.list_scoped_tokens().map_err(|e| match e {
        attune_core::error::VaultError::Locked => AppError::Unauthorized("vault locked".into()),
        other => AppError::Internal(other.to_string()),
    })?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// DELETE /api/v1/scoped-tokens/{id} — 吊销 (即时生效)。
pub async fn revoke(
    State(state): State<SharedState>,
    Path(token_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let changed = vault.revoke_scoped_token(&token_id).map_err(|e| match e {
        attune_core::error::VaultError::Locked => AppError::Unauthorized("vault locked".into()),
        other => AppError::Internal(other.to_string()),
    })?;
    if !changed {
        return Err(AppError::NotFound("token not found or already revoked".into()));
    }
    Ok(Json(serde_json::json!({ "revoked": true, "token_id": token_id })))
}
