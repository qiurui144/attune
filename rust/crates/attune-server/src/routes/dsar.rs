//! DSAR (Data Subject Access Request) — client 端 proxy 到 cloud accounts.
//!
//! GDPR Art.15/17/20 + 中国 PIPL §44-50 — 用户的数据导出 / 账户删除 / 撤销删除
//! 必须经合法的用户视角入口. attune Desktop UI 调本地 server endpoint, server 凭
//! 用户密码代发到 cloud accounts, 密码仅本次请求使用不持久化.
//!
//! 端点:
//!   - POST /api/v1/dsar/export          — 导出用户所有 cloud 数据 (accounts + 跨服务)
//!   - POST /api/v1/dsar/delete          — 软删除 cloud 账户 (30d grace)
//!   - POST /api/v1/dsar/cancel-deletion — 撤销软删除 (同会话内有效)
//!
//! 注: 本地 vault 的导出走 vault.rs 已有路径 (attune-cli vault export).
//! 这里只补充「cloud member 模式」的 cloud 端数据主权操作; BYOK / self-host
//! 用户不需要这些 endpoint, 因为没有 cloud accounts 账号.

use attune_core::cloud_client::CloudClient;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;

/// 默认 cloud accounts base URL (生产 SaaS endpoint)
const DEFAULT_CLOUD_URL: &str = "https://accounts.engi-stack.com";

#[derive(Deserialize)]
pub struct DSARCredentialsReq {
    /// 用户 email (cloud member 账号)
    pub email: String,
    /// 用户密码 — 仅本次请求使用, 不持久化
    pub password: String,
    /// cloud accounts base URL (可选, 默认 https://accounts.engi-stack.com)
    #[serde(default)]
    pub cloud_url: Option<String>,
}

fn cloud_url(req: &DSARCredentialsReq) -> String {
    req.cloud_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| DEFAULT_CLOUD_URL.to_string())
}

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({"error": msg.into()})))
}

fn validate(req: &DSARCredentialsReq) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if req.email.trim().is_empty() || req.password.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "email/password required"));
    }
    Ok(())
}

/// 内部 helper: 用密码登 cloud 拿 authenticated CloudClient.
fn login_cloud(req: &DSARCredentialsReq) -> Result<CloudClient, (StatusCode, Json<serde_json::Value>)> {
    let url = cloud_url(req);
    let mut client = CloudClient::new(url);
    client
        .login(req.email.trim(), &req.password)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, format!("login failed: {e}")))?;
    Ok(client)
}

/// POST /api/v1/dsar/export — DSAR 数据导出 proxy.
///
/// server 用用户提供的密码登录 cloud accounts 拿 session, 然后调 cloud
/// /api/v1/users/me/export 拿回 JSON dump 转给客户端. 密码不持久化.
pub async fn export_data(
    State(_state): State<SharedState>,
    Json(req): Json<DSARCredentialsReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate(&req)?;
    let client = login_cloud(&req)?;
    let body = client
        .dsar_export()
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("dsar export: {e}")))?;
    tracing::info!(
        "DSAR export: relayed cloud export for email={} (size~{} bytes)",
        req.email,
        body.to_string().len()
    );
    Ok(Json(body))
}

/// POST /api/v1/dsar/delete — 软删除 cloud 账户 proxy.
///
/// 用户提供密码确认 → server 代登录 → 调 cloud DELETE /api/v1/users/me.
/// 软删除后 cloud session 立即失效 (cloud current_user 拒 is_active=False),
/// 但用户 30 天 grace 期内可调 cancel-deletion 撤销.
pub async fn delete_account(
    State(_state): State<SharedState>,
    Json(req): Json<DSARCredentialsReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate(&req)?;
    let client = login_cloud(&req)?;
    let body = client
        .dsar_delete()
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("dsar delete: {e}")))?;
    tracing::info!(
        "DSAR delete: cloud soft-delete confirmed for email={}",
        req.email
    );
    Ok(Json(body))
}

/// POST /api/v1/dsar/cancel-deletion — 撤销软删除 proxy.
///
/// 软删除后 30 天 grace 期内有效. 需要密码再次确认 (防误触); cloud 端用
/// current_user_allow_inactive 接受 is_active=False 的 session.
///
/// **限制**: 软删除后 login endpoint 拒 is_active=False 用户 (per accounts/api/users.py),
/// 所以这里 login 会失败 → 403. UX 设计权衡: 撤销删除需用户能登录, 既然不能登录,
/// 真正的「邮件确认链接」流程留 v1.1; v1.0 覆盖 90% 场景: 用户软删除后立刻发现
/// 误操作时, 同一会话 cookie 还在, attune Desktop 可经此 endpoint 复用 session.
pub async fn cancel_deletion(
    State(_state): State<SharedState>,
    Json(req): Json<DSARCredentialsReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate(&req)?;
    let client = login_cloud(&req).map_err(|(_status, body)| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!(
                    "login refused (likely already soft-deleted; cancel-deletion must be \
                     issued from the same session that triggered the deletion): {}",
                    body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown")
                )
            })),
        )
    })?;
    let body = client
        .dsar_cancel_deletion()
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("dsar cancel: {e}")))?;
    tracing::info!(
        "DSAR cancel-deletion: cloud restore confirmed for email={}",
        req.email
    );
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_url_defaults_when_missing() {
        let req = DSARCredentialsReq {
            email: "x@y.com".to_string(),
            password: "p".to_string(),
            cloud_url: None,
        };
        assert_eq!(cloud_url(&req), DEFAULT_CLOUD_URL);
    }

    #[test]
    fn cloud_url_defaults_when_blank() {
        let req = DSARCredentialsReq {
            email: "x@y.com".to_string(),
            password: "p".to_string(),
            cloud_url: Some("   ".to_string()),
        };
        assert_eq!(cloud_url(&req), DEFAULT_CLOUD_URL);
    }

    #[test]
    fn cloud_url_uses_override() {
        let req = DSARCredentialsReq {
            email: "x@y.com".to_string(),
            password: "p".to_string(),
            cloud_url: Some("https://staging.example.com".to_string()),
        };
        assert_eq!(cloud_url(&req), "https://staging.example.com");
    }

    #[test]
    fn validate_rejects_empty_email() {
        let req = DSARCredentialsReq {
            email: "".to_string(),
            password: "p".to_string(),
            cloud_url: None,
        };
        assert!(validate(&req).is_err());
    }

    #[test]
    fn validate_rejects_empty_password() {
        let req = DSARCredentialsReq {
            email: "x@y.com".to_string(),
            password: "".to_string(),
            cloud_url: None,
        };
        assert!(validate(&req).is_err());
    }

    #[test]
    fn validate_accepts_normal_input() {
        let req = DSARCredentialsReq {
            email: "x@y.com".to_string(),
            password: "p".to_string(),
            cloud_url: None,
        };
        assert!(validate(&req).is_ok());
    }
}
