//! /api/v1/member — 会员状态 / settings locks endpoint.

use crate::state::SharedState;
use attune_core::cloud_client::CloudClient;
use attune_core::member_session::{MemberState, SettingsLocks};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

/// GET /api/v1/member/state — 当前会员状态 (UI 展示)
pub async fn get_state(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Json(serde_json::json!({
        "state": m,
        "is_logged_in": m.is_logged_in(),
        "is_paid": m.is_paid(),
        "account_id": m.account_id(),
    }))
}

/// GET /api/v1/member/locks — 当前 SettingsLocks (UI 灰显字段决策)
pub async fn get_locks(State(state): State<SharedState>) -> Json<SettingsLocks> {
    let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Json(SettingsLocks::for_state(&m))
}

/// POST /api/v1/member/login-token — 用 cloud login 后拿到的 user info 设置 member_state
/// 此 endpoint 不直接调云端 (避免 server 持密码), 由客户端 cloud_client login 后回传结果
#[derive(serde::Deserialize)]
pub struct LoginTokenReq {
    pub account_id: String,
    /// "free" | "paid"
    pub tier: String,
    #[serde(default)]
    pub license_id: Option<String>,
    #[serde(default)]
    pub llm_quota_remaining: u64,
}

pub async fn login_token(
    State(state): State<SharedState>,
    Json(req): Json<LoginTokenReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let new_state = match req.tier.as_str() {
        "free" => MemberState::Free { account_id: req.account_id },
        "paid" => {
            let lic = req.license_id.unwrap_or_default();
            if lic.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "paid tier requires license_id"})),
                ));
            }
            MemberState::Paid {
                account_id: req.account_id,
                license_id: lic,
                llm_quota_remaining: req.llm_quota_remaining,
            }
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("unknown tier '{other}'")})),
            ));
        }
    };
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = new_state.clone();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": new_state,
    })))
}

#[derive(serde::Deserialize)]
pub struct LoginPasswordReq {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub cloud_url: Option<String>,
    #[serde(default)]
    pub license_code: Option<String>,
}

/// POST /api/v1/member/login-password — 账号密码登录 cloud accounts，回填 member_state。
///
/// 说明：
/// - 密码只用于本次请求，不持久化到磁盘。
/// - 默认 cloud_url 为 https://accounts.attune.ai，可由请求覆盖。
pub async fn login_password(
    State(state): State<SharedState>,
    Json(req): Json<LoginPasswordReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if req.email.trim().is_empty() || req.password.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "email/password required"})),
        ));
    }

    let cloud_url = req
        .cloud_url
        .unwrap_or_else(|| "https://accounts.attune.ai".to_string());
    let mut client = CloudClient::new(cloud_url);

    let user = client.login(req.email.trim(), &req.password).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": format!("login failed: {e}")})),
        )
    })?;

    // accounts plan → member tier：pro / pro_plus / enterprise 视为 paid，其余 free。
    let is_paid = matches!(user.plan.as_str(), "pro" | "pro_plus" | "enterprise");
    let new_state = if is_paid {
        let licenses = client.list_licenses().map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("list licenses failed: {e}")})),
            )
        })?;
        let selected = if let Some(code) = req.license_code.as_deref() {
            let code = code.trim();
            if code.is_empty() {
                licenses.first()
            } else {
                licenses
                    .iter()
                    .find(|lic| lic.license_key == code || lic.id.to_string() == code)
            }
        } else {
            licenses.first()
        }
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "paid user has no matching license"})),
            )
        })?;
        MemberState::Paid {
            account_id: user.id.to_string(),
            license_id: selected.id.to_string(),
            // 新 License 不再携带 per-license LLM 配额 —— 配额由 cloud gateway 侧统计。
            llm_quota_remaining: 0,
        }
    } else {
        MemberState::Free {
            account_id: user.id.to_string(),
        }
    };

    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = new_state.clone();
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": new_state,
        "email": user.email,
        "tier": user.plan,
    })))
}

/// POST /api/v1/member/logout — 重置会员状态为 LoggedOut
pub async fn logout(State(state): State<SharedState>) -> Json<serde_json::Value> {
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
    Json(serde_json::json!({"status": "ok", "state": "logged_out"}))
}
