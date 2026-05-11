//! /api/v1/member — 会员状态 / settings locks endpoint.

use crate::state::SharedState;
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
    pub tier: String,
    #[serde(default)]
    pub license_id: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub llm_quota_remaining: u64,
}

pub async fn login_token(
    State(state): State<SharedState>,
    Json(req): Json<LoginTokenReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let new_state = match req.tier.as_str() {
        "free" => MemberState::Free { account_id: req.account_id.clone() },
        "paid" | "trial" => {
            let lic = req.license_id.unwrap_or_default();
            if lic.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "paid tier requires license_id"})),
                ));
            }
            MemberState::Member {
                account_id: req.account_id,
                tier: req.tier,
                license_id: lic,
                llm_quota_remaining: req.llm_quota_remaining,
            }
        }
        "enterprise" => {
            let lic = req.license_id.unwrap_or_default();
            let team = req.team_id.unwrap_or_default();
            if lic.is_empty() || team.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "enterprise tier requires license_id + team_id"})),
                ));
            }
            MemberState::Enterprise {
                account_id: req.account_id,
                team_id: team,
                license_id: lic,
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

/// POST /api/v1/member/logout — 重置会员状态为 LoggedOut
pub async fn logout(State(state): State<SharedState>) -> Json<serde_json::Value> {
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
    Json(serde_json::json!({"status": "ok", "state": "logged_out"}))
}
