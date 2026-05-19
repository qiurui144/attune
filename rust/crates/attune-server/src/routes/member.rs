//! /api/v1/member — 会员状态 / settings locks endpoint.

use crate::state::SharedState;
use attune_core::cloud_client::CloudClient;
use attune_core::llm_settings::SETTINGS_META_KEY;
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
        // 付费会员：拿 cloud gateway token, 合并进 vault app_settings,
        // 桌面 chat 零配置接通云端 LLM。best-effort — 失败不阻断登录。
        let mut gateway_written = false;
        match client.me() {
            Ok(me) => match (me.gateway_url.as_deref(), me.gateway_token.as_deref()) {
                (Some(url), Some(tok)) if !url.is_empty() && !tok.is_empty() => {
                    match apply_gateway_to_vault_settings(&state, url, tok) {
                        Ok(applied) if applied => {
                            tracing::info!("member login: cloud LLM gateway written to vault settings");
                            gateway_written = true;
                        }
                        Ok(_) => {
                            tracing::info!(
                                "member login: user has own LLM config — gateway not auto-applied"
                            );
                        }
                        Err(e) => {
                            tracing::warn!("member login: gateway settings not written: {e}");
                        }
                    }
                }
                _ => {
                    tracing::info!(
                        "member login: no gateway token for {} — user keeps current LLM settings",
                        user.email
                    );
                }
            },
            Err(e) => tracing::warn!("member login: fetch /me failed: {e}"),
        }

        // Reload in-memory LLM provider so chat works immediately after login
        // without requiring a server restart. Must be called AFTER the vault lock
        // from apply_gateway_to_vault_settings has been released.
        if gateway_written {
            state.reload_llm();
        }

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

/// 把 cloud gateway endpoint + token 合并写入 vault `app_settings` meta.
///
/// **configure-if-unconfigured**: 当用户已有可用的 LLM 配置（非空 `api_key` 或 `endpoint`）时，
/// 跳过写入并返回 `Ok(false)`；仅当未配置时写入并返回 `Ok(true)`。
///
/// 读取现有 meta → 检查 [`attune_core::llm_settings::gateway_should_apply`] →
/// 若应应用则调用 `merge_gateway_into_settings` 后写回。
/// 与 `routes/settings.rs::update_settings` 使用同一 sink。
fn apply_gateway_to_vault_settings(
    state: &SharedState,
    endpoint: &str,
    token: &str,
) -> Result<bool, String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    // Parity with settings.rs: surface a clear "vault locked" error before touching meta.
    let _ = vault
        .dek_db()
        .map_err(|e| format!("vault locked: {e}"))?;
    let existing = vault
        .store()
        .get_meta(SETTINGS_META_KEY)
        .map_err(|e| format!("get_meta failed: {e}"))?;
    let current: serde_json::Value = match existing {
        Some(data) => serde_json::from_slice(&data).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };

    if !attune_core::llm_settings::gateway_should_apply(&current) {
        return Ok(false);
    }

    let merged =
        attune_core::llm_settings::merge_gateway_into_settings(current, endpoint, token);
    let data = serde_json::to_vec(&merged).map_err(|e| format!("settings ser: {e}"))?;
    vault
        .store()
        .set_meta(SETTINGS_META_KEY, &data)
        .map_err(|e| format!("set_meta failed: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use attune_core::llm_settings::{gateway_should_apply, merge_gateway_into_settings};

    // ── merge shape (kept from original, tests the pure helper) ─────────────

    #[test]
    fn login_merges_gateway_into_app_settings_meta_shape() {
        // member login must merge gateway endpoint+token into the same
        // `app_settings` JSON shape the vault meta stores (provider=openai_compat).
        let existing = serde_json::json!({"llm": {"model": "qwen2.5:3b"}});
        let merged = merge_gateway_into_settings(
            existing,
            "https://gateway.attune.ai/v1",
            "sk-newapi-abc",
        );
        let llm = merged.get("llm").and_then(|v| v.as_object()).unwrap();
        assert_eq!(llm.get("provider").and_then(|v| v.as_str()), Some("openai_compat"));
        assert_eq!(
            llm.get("endpoint").and_then(|v| v.as_str()),
            Some("https://gateway.attune.ai/v1")
        );
        assert_eq!(llm.get("api_key").and_then(|v| v.as_str()), Some("sk-newapi-abc"));
        // preexisting fields preserved
        assert_eq!(llm.get("model").and_then(|v| v.as_str()), Some("qwen2.5:3b"));
    }

    // ── configure-if-unconfigured gating ────────────────────────────────────

    #[test]
    fn gateway_skipped_when_user_has_byok_api_key() {
        // User already has their own API key — gateway must not overwrite.
        let settings = serde_json::json!({"llm": {"api_key": "sk-user", "endpoint": ""}});
        assert!(!gateway_should_apply(&settings));
    }

    #[test]
    fn gateway_skipped_when_user_has_endpoint() {
        // User has configured a local Ollama endpoint — gateway must not overwrite.
        let settings = serde_json::json!({"llm": {"api_key": "", "endpoint": "http://localhost:11434/v1"}});
        assert!(!gateway_should_apply(&settings));
    }

    #[test]
    fn gateway_applied_when_llm_unconfigured() {
        // Default factory state: no llm section → gateway should apply.
        assert!(gateway_should_apply(&serde_json::json!({})));
    }

    #[test]
    fn gateway_applied_when_llm_has_empty_key_and_endpoint() {
        // Both fields empty → treat as unconfigured → gateway applies.
        let settings =
            serde_json::json!({"llm": {"model": "qwen2.5:3b", "api_key": "", "endpoint": ""}});
        assert!(gateway_should_apply(&settings));
    }
}
