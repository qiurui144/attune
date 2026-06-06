//! /api/v1/member — 会员状态 / settings locks endpoint.

use crate::state::SharedState;
use attune_core::cloud_client::{CloudClient, License, UserInfo};
use attune_core::llm_settings::SETTINGS_META_KEY;
use attune_core::member_session::{MemberState, SettingsLocks};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

/// Result of the blocking CloudClient interaction (B4): carried back from
/// `spawn_blocking` into the async tail. `license`/`me` are `None` for free users
/// or when the best-effort `/me` fetch failed.
struct CloudLoginData {
    user: UserInfo,
    license: Option<License>,
    me: Option<UserInfo>,
    /// B5 (2026-06-06): best-effort plugin auto-install report. Computed inside
    /// the SAME blocking thread as the login (sync_plugins does blocking network
    /// I/O — must NOT run on the async worker, same constraint as B4). `None` for
    /// free users (no entitlements to sync).
    plugin_sync: Option<attune_core::plugin_sync::SyncReport>,
}

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
    let is_paid = req.tier.as_str() == "paid";
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

    // B5 (2026-06-06): mirror login_password — a paid member-login must auto-install
    // entitled pro plugins. This endpoint carries no credentials (the desktop client
    // already authenticated to cloud), so we can only sync when a persisted cloud
    // session exists. Runs on a blocking thread (sync_plugins = blocking network
    // I/O, same B4 constraint). Best-effort: any failure (no session / unreachable)
    // is logged and never fails the login (§4.5); signature verification inside
    // sync is NOT bypassed.
    let plugin_sync = if is_paid {
        tokio::task::spawn_blocking(member_session_sync_plugins)
            .await
            .unwrap_or(None)
    } else {
        None
    };
    let plugins_json = plugin_sync.as_ref().map(sync_report_to_json);

    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": new_state,
        "plugin_sync": plugins_json,
    })))
}

/// Build a `CloudClient` from the persisted CLI cloud session (`config_dir/
/// cloud-session.json`) and run best-effort plugin sync. Returns `None` when no
/// session is available (so the `login_token` paid path simply skips). Used only
/// by `login_token`, which carries no live credentials of its own.
fn member_session_sync_plugins() -> Option<attune_core::plugin_sync::SyncReport> {
    let path = attune_core::platform::config_dir().join("cloud-session.json");
    let json = std::fs::read_to_string(&path).ok()?;
    let sess: serde_json::Value = serde_json::from_str(&json).ok()?;
    let cloud_url = sess.get("cloud_url").and_then(|v| v.as_str())?;
    let session = sess.get("session").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
    let client = CloudClient::with_session(cloud_url, session);
    Some(attune_core::plugin_sync::best_effort_sync_plugins(&client))
}

/// Serialize a [`SyncReport`] into the stable UI JSON shape (shared by both
/// member-login endpoints).
fn sync_report_to_json(r: &attune_core::plugin_sync::SyncReport) -> serde_json::Value {
    serde_json::json!({
        "installed": r.installed,
        "skipped_already_installed": r.skipped_already_installed,
        "failed": r.failed
            .iter()
            .map(|(id, reason)| serde_json::json!({"plugin_id": id, "reason": reason}))
            .collect::<Vec<_>>(),
    })
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
/// - 默认 cloud_url 为 https://accounts.engi-stack.com，可由请求覆盖。
pub async fn login_password(
    State(state): State<SharedState>,
    Json(mut req): Json<LoginPasswordReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if req.email.trim().is_empty() || req.password.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "email/password required"})),
        ));
    }

    let cloud_url = req
        .cloud_url
        .unwrap_or_else(|| "https://accounts.engi-stack.com".to_string());

    // B4 (2026-06-06): CloudClient wraps `reqwest::blocking`, which spins up (and on
    // drop tears down) a current-thread Tokio runtime. Calling it directly inside this
    // async handler panicked the worker with "Cannot drop a runtime in a context where
    // blocking is not allowed", resetting the connection — membership login was 100%
    // broken on the real server (mock/unit tests never hit the live blocking path).
    // Move the whole blocking CloudClient interaction (login → list_licenses → me) onto
    // a blocking thread; the async tail (vault write + state mutation) stays here.
    let email = req.email.trim().to_string();
    let password = std::mem::take(&mut req.password);
    let license_code = req.license_code.clone();
    let blocking = tokio::task::spawn_blocking(move || -> Result<CloudLoginData, (StatusCode, String)> {
        let mut client = CloudClient::new(cloud_url);
        let user = client
            .login(&email, &password)
            .map_err(|e| (StatusCode::UNAUTHORIZED, format!("login failed: {e}")))?;
        let is_paid = matches!(user.plan.as_str(), "pro" | "pro_plus" | "enterprise");
        if !is_paid {
            return Ok(CloudLoginData { user, license: None, me: None, plugin_sync: None });
        }
        let licenses = client
            .list_licenses()
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("list licenses failed: {e}")))?;
        let selected = if let Some(code) = license_code.as_deref() {
            let code = code.trim();
            if code.is_empty() {
                licenses.into_iter().next()
            } else {
                licenses
                    .into_iter()
                    .find(|lic| lic.license_key == code || lic.id.to_string() == code)
            }
        } else {
            licenses.into_iter().next()
        }
        .ok_or((StatusCode::BAD_REQUEST, "paid user has no matching license".to_string()))?;
        // best-effort gateway token fetch — a failure here must not block login.
        let me = client.me().ok();
        // B5 (2026-06-06): auto-install entitled pro plugins (e.g. law-pro) so
        // domain-specific agents work right after login, no manual `attune
        // sync-plugins`. Runs on THIS blocking thread (reusing the authenticated
        // client + its session cookie). best_effort_* never returns Err — a sync
        // failure logs + yields an empty report; the login still succeeds (§4.5).
        // Signature verification (verify_with_key) inside sync is NOT bypassed:
        // an unverified package fails closed and is reported in `failed`.
        let plugin_sync = Some(attune_core::plugin_sync::best_effort_sync_plugins(&client));
        Ok(CloudLoginData { user, license: Some(selected), me, plugin_sync })
    });
    let CloudLoginData { user, license, me, plugin_sync } = blocking
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("login task join error: {e}")})),
            )
        })?
        .map_err(|(code, msg)| (code, Json(serde_json::json!({"error": msg}))))?;

    let new_state = if let Some(selected) = license {
        // 付费会员：拿 cloud gateway token, 合并进 vault app_settings,
        // 桌面 chat 零配置接通云端 LLM。best-effort — 失败不阻断登录。
        let mut gateway_written = false;
        match me {
            Some(me) => match (me.gateway_url.as_deref(), me.gateway_token.as_deref()) {
                (Some(url), Some(tok)) if !url.is_empty() && !tok.is_empty() => {
                    // Bug-1 fix (spec 2026-05-24): cloud 下发的默认 model 一并写入,
                    // 避免 fresh vault paid 用户 chat 因 model=null → 404。
                    let default_model = me.gateway_default_model.as_deref();
                    match apply_gateway_to_vault_settings(&state, url, tok, default_model) {
                        Ok(applied) if applied => {
                            tracing::info!(
                                "member login: cloud LLM gateway written to vault settings (default_model={:?})",
                                default_model,
                            );
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
            None => tracing::warn!("member login: fetch /me failed — user keeps current LLM settings"),
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
    // B5: surface the best-effort plugin auto-install outcome to the UI (non-fatal;
    // the login already succeeded regardless of plugin sync).
    let plugins_json = plugin_sync.as_ref().map(sync_report_to_json);
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": new_state,
        "email": user.email,
        "tier": user.plan,
        "plugin_sync": plugins_json,
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
    default_model: Option<&str>,
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

    let merged = attune_core::llm_settings::merge_gateway_into_settings(
        current,
        endpoint,
        token,
        default_model,
    );
    let data = serde_json::to_vec(&merged).map_err(|e| format!("settings ser: {e}"))?;
    vault
        .store()
        .set_meta(SETTINGS_META_KEY, &data)
        .map_err(|e| format!("set_meta failed: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use attune_core::cloud_client::CloudClient;
    use attune_core::llm_settings::{gateway_should_apply, merge_gateway_into_settings};

    // ── B4 regression: blocking CloudClient must not panic the async worker ──
    //
    // Before B4, login_password() called CloudClient::login() (reqwest::blocking,
    // which owns a current-thread Tokio runtime) directly inside the async handler.
    // Dropping that runtime inside an async context panicked the tokio-rt-worker
    // with "Cannot drop a runtime in a context where blocking is not allowed",
    // resetting the connection — membership login was 100% broken on the real
    // server. The fix moves the blocking call onto spawn_blocking. This test drives
    // the exact pattern on a multi-thread runtime against an unreachable address: it
    // must return Err (connection refused), NEVER panic.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_cloud_client_via_spawn_blocking_does_not_panic() {
        let result = tokio::task::spawn_blocking(|| {
            // port 1 is unreachable → login returns Err; the point is that creating
            // and dropping the embedded blocking runtime here does not panic.
            let mut client = CloudClient::new("http://127.0.0.1:1");
            client.login("user@example.com", "pw-not-real")
        })
        .await
        .expect("spawn_blocking join must succeed (no worker panic)");
        assert!(result.is_err(), "login against an unreachable host must be Err, not panic/Ok");
    }

    // Guards the anti-pattern the fix removed: doing the same blocking call WITHOUT
    // spawn_blocking, directly on the async worker, is what panicked. We cannot
    // assert the panic here without aborting the test process, so this test documents
    // (via the passing spawn_blocking variant above) that spawn_blocking is required.

    // ── merge shape (kept from original, tests the pure helper) ─────────────

    #[test]
    fn login_merges_gateway_into_app_settings_meta_shape() {
        // member login must merge gateway endpoint+token into the same
        // `app_settings` JSON shape the vault meta stores (provider=openai_compat).
        let existing = serde_json::json!({"llm": {"model": "qwen2.5:3b"}});
        let merged = merge_gateway_into_settings(
            existing,
            "https://gateway.engi-stack.com/v1",
            "sk-newapi-abc",
            None,
        );
        let llm = merged.get("llm").and_then(|v| v.as_object()).unwrap();
        assert_eq!(llm.get("provider").and_then(|v| v.as_str()), Some("openai_compat"));
        assert_eq!(
            llm.get("endpoint").and_then(|v| v.as_str()),
            Some("https://gateway.engi-stack.com/v1")
        );
        assert_eq!(llm.get("api_key").and_then(|v| v.as_str()), Some("sk-newapi-abc"));
        // preexisting fields preserved
        assert_eq!(llm.get("model").and_then(|v| v.as_str()), Some("qwen2.5:3b"));
    }

    /// Bug-1 regression (spec 2026-05-24): fresh vault paid 用户 login,gateway 写入
    /// endpoint+token+**model** 三件套,避免 chat 因 model=null → newapi 404。
    #[test]
    fn login_writes_default_model_into_fresh_vault_settings() {
        // 模拟 fresh vault — 完全没有 llm 字段
        let merged = merge_gateway_into_settings(
            serde_json::json!({}),
            "https://gateway.engi-stack.com/v1",
            "sk-newapi-fresh",
            Some("deepseek-v4-flash"),
        );
        let llm = merged.get("llm").and_then(|v| v.as_object()).unwrap();
        assert_eq!(llm.get("provider").and_then(|v| v.as_str()), Some("openai_compat"));
        assert_eq!(
            llm.get("model").and_then(|v| v.as_str()),
            Some("deepseek-v4-flash"),
            "fresh vault paid 用户 login 应自动写入 cloud 下发的 default model"
        );
        assert_eq!(llm.get("api_key").and_then(|v| v.as_str()), Some("sk-newapi-fresh"));
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
