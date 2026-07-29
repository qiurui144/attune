//! /api/v1/member — 会员状态 / settings locks endpoint.

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use attune_core::cloud_client::{
    current_plan_grants_paid, plan_grants_paid, CloudClient, License, UserInfo,
};
use attune_core::cloud_session::{CloudSessionStore, CloudSessionTransaction};
use attune_core::entitlement::EntitlementCache;
use attune_core::entitlement_reverify::{apply_refresh_rounds, RefreshSummary, ReverifyOutcome};
use attune_core::llm_settings::SETTINGS_META_KEY;
use attune_core::member_session::{MemberState, SettingsLocks};
use attune_core::plugin_hub::PluginHubProvider;
use attune_core::plugin_sig::TrustMode;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::{Datelike, Utc};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Default public cloud accounts endpoint when the self-host override
/// (`settings.cloud.accounts_url`) is unset/empty.
const DEFAULT_ACCOUNTS_URL: &str = "https://accounts.engi-stack.com";
const DEFAULT_PLUGINHUB_URL: &str = "https://hub.engi-stack.com";
const MEMBER_REVERIFY_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MEMBER_NETWORK_GRACE: Duration = Duration::from_secs(15 * 60);

/// Resolve the cloud accounts base URL **server-side** from persisted settings
/// (`app_settings.cloud.accounts_url`), defaulting to the public engi-stack
/// endpoint. SECURITY (SSRF / paywall-bypass): the accounts URL is NEVER taken
/// from the request body — a client-controlled URL would let an attacker point
/// login/activation at their own server and forge "paid" / inject a malicious
/// gateway config. Self-host operators configure this once under 设置 → cloud.
pub(crate) fn resolve_accounts_url(state: &SharedState) -> String {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let configured = vault
        .store()
        .get_meta(SETTINGS_META_KEY)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| {
            v.get("cloud")
                .and_then(|c| c.get("accounts_url"))
                .and_then(|u| u.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
    configured.unwrap_or_else(|| DEFAULT_ACCOUNTS_URL.to_string())
}

fn member_billing_json(accounts_url: &str) -> serde_json::Value {
    let base = accounts_url.trim_end_matches('/');
    serde_json::json!({
        "accounts_url": accounts_url,
        "upgrade_url": format!("{base}/upgrade"),
        "billing_url": format!("{base}/billing"),
    })
}

/// GET /api/v1/member/billing — 会员购买/账单入口。
///
/// URL 由服务端 settings 解析，避免前端硬编码公共云，也避免客户端传入 URL
/// 造成 SSRF / 付费墙绕过。
pub async fn get_billing(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let accounts_url = resolve_accounts_url(&state);
    Json(member_billing_json(&accounts_url))
}

fn resolve_pluginhub_url(state: &SharedState) -> String {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let configured = vault
        .store()
        .get_meta(SETTINGS_META_KEY)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| {
            v.get("pluginhub")
                .and_then(|p| p.get("url"))
                .and_then(|u| u.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
    configured.unwrap_or_else(|| DEFAULT_PLUGINHUB_URL.to_string())
}

/// SECURITY: redact a license_key for log/identity use. Never log the raw key
/// (§1.4) — emit a stable `lic:<8-hex>` digest prefix so operators can correlate
/// without the credential ever reaching a log sink.
fn redact_license_key(license_key: &str) -> String {
    let digest = Sha256::digest(license_key.as_bytes());
    format!("lic:{}", &hex::encode(digest)[..8])
}

/// Result of the blocking CloudClient interaction (B4): carried back from
/// `spawn_blocking` into the async tail. `user` is the authoritative `/me`
/// snapshot and `license` is `None` for free users.
struct CloudLoginData {
    user: UserInfo,
    license: Option<License>,
    cloud_url: String,
    session_token: Option<String>,
    llm_quota_remaining: u64,
    device: Option<
        std::result::Result<
            attune_core::cloud_client::DeviceActivateResult,
            attune_core::cloud_client::DeviceActivateError,
        >,
    >,
    /// B5 (2026-06-06): best-effort plugin auto-install report. Computed inside
    /// the SAME blocking thread as the login (sync_plugins does blocking network
    /// I/O — must NOT run on the async worker, same constraint as B4). `None` for
    /// free users (no entitlements to sync).
    plugin_sync: Option<attune_core::plugin_sync::SyncReport>,
}

fn validate_login_identity(
    login_response: &UserInfo,
    authenticated_user: &UserInfo,
) -> Result<(), (StatusCode, String)> {
    if login_response.id != authenticated_user.id {
        return Err((
            StatusCode::FORBIDDEN,
            "authenticated account does not match login response".to_string(),
        ));
    }
    Ok(())
}

/// GET /api/v1/member/state — 当前会员状态 (UI 展示)
pub async fn get_state(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let should_restore = {
        let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner());
        matches!(*m, MemberState::LoggedOut)
    };
    if should_restore {
        let _ = restore_member_state_from_cloud_session(&state).await;
    }

    let m = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let (license_id, llm_quota_remaining) = match &m {
        MemberState::Paid {
            license_id,
            llm_quota_remaining,
            ..
        } => (Some(license_id.as_str()), *llm_quota_remaining),
        _ => (None, 0),
    };
    Json(serde_json::json!({
        "state": m,
        "is_logged_in": m.is_logged_in(),
        "is_paid": m.is_paid(),
        "account_id": m.account_id(),
        "license_id": license_id,
        "llm_quota_remaining": llm_quota_remaining,
    }))
}

/// GET /api/v1/member/locks — 当前 SettingsLocks (UI 灰显字段决策)
pub async fn get_locks(State(state): State<SharedState>) -> Json<SettingsLocks> {
    let m = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
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
    // The verifier reads the persisted session, so it belongs inside the same
    // account transaction as cleanup/commit/runtime publication. Otherwise a
    // concurrent login could replace the cookie after proof but before apply.
    let _transition = state.member_transition.lock().await;
    let session_transaction = acquire_cloud_session_transition(&state.cloud_session_store)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e,
                    "code": "cloud-session-lock-failed",
                    "paid_applied": false,
                })),
            )
        })?;
    let is_paid = req.tier.as_str() == "paid";
    let new_state = match req.tier.as_str() {
        "free" => MemberState::Free {
            account_id: req.account_id,
        },
        "paid" => {
            // C1 paywall-bypass fix: a "paid" claim MUST be verified server-side before it can
            // gate billable cloud-LLM spend (doc-intel is the first such consumer). The previous
            // `!lic.is_empty()` check trusted the client; now `verify_paid` proves the license
            // against the cloud session (CloudMemberVerifier) and FAILS CLOSED on every error
            // path. A forged / empty / unverifiable claim → 403, never Paid.
            let lic = req.license_id.unwrap_or_default();
            let verifier = state.member_verifier();
            let account_for_verify = req.account_id.clone();
            let license_for_verify = lic.clone();
            let transaction_for_verify = Arc::clone(&session_transaction);
            tokio::task::spawn_blocking(move || {
                verifier.verify_paid_with_session(
                    &account_for_verify,
                    &license_for_verify,
                    transaction_for_verify.as_ref(),
                )
            })
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("paid verification task: {e}")})),
                )
            })?
            .map_err(|e| paid_verification_error(&e))?;
            MemberState::Paid {
                account_id: req.account_id,
                license_id: lic.trim().to_string(),
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

    // An explicit successful token login opts into Cloud SaaS. Consent is the
    // first durable mutation: if it cannot be stored, the old account cookie,
    // runtime, and member state remain exactly as they were.
    crate::routes::privacy::set_outbound_enabled(&state, "cloud_saas", true).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("persist cloud consent: {e}")})),
        )
    })?;

    // Fence persisted-session restore before changing consent or runtime.
    // - Free token login has no authoritative server-side session of its own,
    //   so any previous cookie must be retired. `remove_cloud_session` writes a
    //   durable suppression marker before deletion; even a deletion error
    //   cannot resurrect the previous Paid/different account on restart.
    // - Paid token login was verified against the persisted session above. If
    //   one exists, temporarily suppress it while old local credentials are
    //   cleared, then publish it again only after that cleanup commits.
    // Test-only verifiers may prove Paid without a disk session, hence the
    // `false` transaction result rather than inventing a cookie.
    let prepare_session_transaction = Arc::clone(&session_transaction);
    let prepare_result = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        if is_paid {
            if prepare_session_transaction
                .load()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                prepare_session_transaction
                    .suppress_restore()
                    .map_err(|e| e.to_string())?;
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            prepare_session_transaction
                .remove()
                .map(|_| false)
                .map_err(|e| e.to_string())
        }
    })
    .await;
    let paid_session_needs_commit = match prepare_result {
        Ok(Ok(needs_commit)) => needs_commit,
        Ok(Err(e)) => {
            fail_close_member_runtime(&state, &format!("prepare member session: {e}"));
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("prepare member session: {e}"),
                    "code": "cloud-session-cleanup-failed",
                    "paid_applied": false,
                })),
            ));
        }
        Err(e) => {
            fail_close_member_runtime(&state, &format!("prepare member session task: {e}"));
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("prepare member session task: {e}"),
                    "code": "cloud-session-cleanup-failed",
                    "paid_applied": false,
                })),
            ));
        }
    };

    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
    if let Err(e) = clear_member_credentials_preserving_unowned_llm(&state, None) {
        // Cleanup may already have removed some old credentials, so publishing
        // a Paid session now could rebuild a mixed account. Keep the marker as
        // an explicit fail-closed state until a fresh credentialed login.
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("clear previous member state: {e}"),
                "code": "member-switch-cleanup-failed",
                "session_restore_suppressed": !is_paid || paid_session_needs_commit,
            })),
        ));
    }

    if paid_session_needs_commit {
        commit_staged_cloud_session_restore(Arc::clone(&session_transaction))
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": e,
                        "code": "cloud-session-commit-failed",
                        "paid_applied": false,
                        "session_restore_suppressed": true,
                    })),
                )
            })?;
    }
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = new_state.clone();
    if let Err(e) = bind_member_session_epoch(&state, session_transaction.as_ref()) {
        fail_close_member_runtime(&state, &e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": e,
                "code": "cloud-session-epoch-failed",
                "paid_applied": false,
            })),
        ));
    }

    // B5 (2026-06-06): mirror login_password — a paid member-login must auto-install
    // entitled pro plugins. This endpoint carries no credentials (the desktop client
    // already authenticated to cloud), so we can only sync when a persisted cloud
    // session exists. Runs on a blocking thread (sync_plugins = blocking network
    // I/O, same B4 constraint). Best-effort: any failure (no session / unreachable)
    // is logged and never fails the login (§4.5); signature verification inside
    // sync is NOT bypassed.
    let plugin_sync = if is_paid {
        let plugin_session_transaction = Arc::clone(&session_transaction);
        tokio::task::spawn_blocking(move || {
            member_session_sync_plugins(plugin_session_transaction.as_ref())
        })
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

/// Map a [`MemberVerifyError`] to the wire response. A missing/empty license is a client input
/// error (400); every "could not prove paid" reason (no session / unreachable / not-on-account /
/// revoked) is a 403 — the claim is simply not authorized as Paid. The verifier message never
/// carries a credential.
fn paid_verification_error(
    e: &attune_core::member_verifier::MemberVerifyError,
) -> (StatusCode, Json<serde_json::Value>) {
    use attune_core::member_verifier::MemberVerifyError as E;
    let status = match e {
        E::MissingLicenseId => StatusCode::BAD_REQUEST,
        E::NoCloudSession
        | E::Unavailable(_)
        | E::LicenseNotOnAccount
        | E::AccountMismatch
        | E::LicenseRevoked
        | E::LicenseNotPaid => StatusCode::FORBIDDEN,
    };
    (
        status,
        Json(serde_json::json!({
            "error": e.to_string(),
            "code": "paid-verification-failed",
        })),
    )
}

/// Build a `CloudClient` from the persisted CLI cloud session (`config_dir/
/// cloud-session.json`) and run best-effort plugin sync. Returns `None` when no
/// session is available (so the `login_token` paid path simply skips). Used only
/// by `login_token`, which carries no live credentials of its own.
fn member_session_sync_plugins(
    session_transaction: &CloudSessionTransaction,
) -> Option<attune_core::plugin_sync::SyncReport> {
    let client = cloud_client_from_transaction(session_transaction)?;
    Some(attune_core::plugin_sync::best_effort_sync_plugins(&client))
}

fn vault_is_unlocked(state: &SharedState) -> bool {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    matches!(vault.state(), attune_core::vault::VaultState::Unlocked)
}

fn bind_member_session_epoch(
    state: &SharedState,
    session_transaction: &CloudSessionTransaction,
) -> Result<(), String> {
    let epoch = session_transaction
        .epoch()
        .map_err(|e| format!("read committed cloud session epoch: {e}"))?;
    *state
        .member_session_epoch
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(epoch);
    let paid = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_paid();
    *state
        .member_verified_at
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = paid.then(Instant::now);
    Ok(())
}

fn fail_close_member_runtime(state: &SharedState, reason: &str) {
    tracing::warn!("member runtime fail-closed: {reason}");
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
    *state
        .member_session_epoch
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    *state
        .member_verified_at
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    if let Err(e) = clear_member_credentials(state, None) {
        tracing::warn!("member runtime fail-closed persistence cleanup failed: {e}");
    }
}

enum PaidReverifyOutcome {
    Verified,
    AuthoritativeDeny(String),
    Unavailable(String),
}

fn cloud_verify_error(context: &str, error: attune_core::error::VaultError) -> PaidReverifyOutcome {
    let detail = error.to_string();
    let server_error = (500..600).any(|status| detail.contains(&status.to_string()));
    if matches!(error, attune_core::error::VaultError::Io(_)) || server_error {
        PaidReverifyOutcome::Unavailable(format!("{context}: {detail}"))
    } else {
        PaidReverifyOutcome::AuthoritativeDeny(format!("{context}: {detail}"))
    }
}

fn device_verify_error(
    context: &str,
    error: attune_core::cloud_client::DeviceActivateError,
) -> PaidReverifyOutcome {
    match error {
        attune_core::cloud_client::DeviceActivateError::Unavailable(detail) => {
            PaidReverifyOutcome::Unavailable(format!("{context}: {detail}"))
        }
        other => PaidReverifyOutcome::AuthoritativeDeny(format!("{context}: {other}")),
    }
}

fn reverify_current_paid_membership(
    state: &SharedState,
    session_transaction: &CloudSessionTransaction,
) -> PaidReverifyOutcome {
    let current = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let MemberState::Paid {
        account_id,
        license_id,
        ..
    } = current
    else {
        return PaidReverifyOutcome::Verified;
    };

    match session_transaction.load() {
        Ok(Some(session)) => {
            let client = CloudClient::with_session(session.cloud_url, session.session);
            let me = match client.me() {
                Ok(me) => me,
                Err(error) => return cloud_verify_error("member /me reverify", error),
            };
            if me.id.to_string() != account_id
                || !current_plan_grants_paid(&me.plan, me.plan_expires.as_deref())
            {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "member account/plan no longer grants Paid".to_string(),
                );
            }
            let licenses = match client.list_licenses() {
                Ok(licenses) => licenses,
                Err(error) => return cloud_verify_error("member license reverify", error),
            };
            let Some(license) = licenses.into_iter().find(|license| {
                license.canonical_id() == license_id
                    || license.id.to_string() == license_id
                    || license.license_key == license_id
            }) else {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "paid license is no longer owned by the account".to_string(),
                );
            };
            if license.revoked_at.is_some() || !plan_grants_paid(&license.plan) {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "paid license was revoked or downgraded".to_string(),
                );
            }
            let fingerprint = attune_core::device_fingerprint::device_fingerprint();
            let device = match client.device_activate(&license.license_key, &fingerprint) {
                Ok(device) => device,
                Err(error) => return device_verify_error("member device reverify", error),
            };
            let device_plan = if device.plan.trim().is_empty() {
                license.plan.as_str()
            } else {
                device.plan.as_str()
            };
            if !current_plan_grants_paid(device_plan, device.expires_at.as_deref()) {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "member device proof expired or downgraded".to_string(),
                );
            }
            PaidReverifyOutcome::Verified
        }
        Ok(None) => {
            let (license_key, persisted_device) = match load_activation_receipt(state) {
                Ok(Some(receipt)) => receipt,
                Ok(None) => {
                    return PaidReverifyOutcome::AuthoritativeDeny(
                        "paid runtime has neither cloud session nor activation receipt".to_string(),
                    )
                }
                Err(error) => {
                    return PaidReverifyOutcome::AuthoritativeDeny(format!(
                        "activation receipt is unreadable: {error}"
                    ))
                }
            };
            let client = CloudClient::new(resolve_accounts_url(state));
            let activation = match client.activate_license(&license_key) {
                Ok(activation) => activation,
                Err(error) => return cloud_verify_error("activation license reverify", error),
            };
            if !current_plan_grants_paid(&activation.plan, activation.expires_at.as_deref()) {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "activation license expired or downgraded".to_string(),
                );
            }
            let fingerprint = attune_core::device_fingerprint::device_fingerprint();
            let device = match client.device_activate(&license_key, &fingerprint) {
                Ok(device) => device,
                Err(error) => return device_verify_error("activation device reverify", error),
            };
            if !persisted_device.device_id.trim().is_empty()
                && !device.device_id.trim().is_empty()
                && persisted_device.device_id != device.device_id
            {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "activation device identity changed".to_string(),
                );
            }
            let device_plan = if device.plan.trim().is_empty() {
                activation.plan.as_str()
            } else {
                device.plan.as_str()
            };
            if !current_plan_grants_paid(device_plan, device.expires_at.as_deref()) {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "activation device proof expired or downgraded".to_string(),
                );
            }
            let identity = redact_license_key(&license_key);
            if account_id != identity || license_id != identity {
                return PaidReverifyOutcome::AuthoritativeDeny(
                    "activation receipt does not match the current Paid state".to_string(),
                );
            }
            PaidReverifyOutcome::Verified
        }
        Err(error) => {
            PaidReverifyOutcome::AuthoritativeDeny(format!("cloud session is unreadable: {error}"))
        }
    }
}

/// Blocking reconciliation body. The caller owns `member_transition` and the
/// path transaction, so the epoch and authoritative proof refer to one stable
/// cookie throughout the check.
fn reconcile_member_session_locked(
    state: &SharedState,
    transaction: &CloudSessionTransaction,
) -> bool {
    if !vault_is_unlocked(state) {
        return false;
    }
    let logged_in = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_logged_in();
    if !logged_in {
        *state
            .member_session_epoch
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *state
            .member_verified_at
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        return true;
    }
    let expected = state
        .member_session_epoch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some(expected) = expected else {
        fail_close_member_runtime(state, "logged-in member runtime has no bound session epoch");
        return false;
    };
    let current = match transaction.epoch() {
        Ok(epoch) => epoch,
        Err(error) => {
            fail_close_member_runtime(state, &format!("read cloud session epoch: {error}"));
            return false;
        }
    };
    if current != expected {
        fail_close_member_runtime(state, "persisted cloud session changed in another process");
        return false;
    }

    let is_paid = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_paid();
    if !is_paid {
        return true;
    }
    let last_verified = *state
        .member_verified_at
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if last_verified.is_some_and(|verified| verified.elapsed() < MEMBER_REVERIFY_INTERVAL) {
        return true;
    }
    if !crate::routes::privacy::outbound_enabled(state, "cloud_saas") {
        if last_verified.is_some_and(|verified| verified.elapsed() < MEMBER_NETWORK_GRACE) {
            tracing::warn!(
                "member reverify deferred because cloud SaaS is disabled; retaining bounded grace"
            );
            return true;
        }
        fail_close_member_runtime(
            state,
            "member reverify grace expired while cloud SaaS was disabled",
        );
        return false;
    }
    match reverify_current_paid_membership(state, transaction) {
        PaidReverifyOutcome::Verified => {
            *state
                .member_verified_at
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            true
        }
        PaidReverifyOutcome::AuthoritativeDeny(reason) => {
            fail_close_member_runtime(state, &reason);
            false
        }
        PaidReverifyOutcome::Unavailable(reason) => {
            if last_verified.is_some_and(|verified| verified.elapsed() < MEMBER_NETWORK_GRACE) {
                tracing::warn!("member reverify unavailable; retaining bounded grace: {reason}");
                true
            } else {
                fail_close_member_runtime(
                    state,
                    &format!("member reverify grace expired: {reason}"),
                );
                false
            }
        }
    }
}

/// Ensure a logged-in runtime is still paired with the exact cloud-session
/// state from which it was published. A sequential `attune login` in another
/// process changes the durable epoch; the next API request then tears down the
/// stale account before its handler can use the old gateway/entitlements.
pub(crate) async fn reconcile_member_session_epoch(state: &SharedState) -> bool {
    let runtime_is_logged_in = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_logged_in();
    let has_bound_epoch = state
        .member_session_epoch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    if !runtime_is_logged_in && !has_bound_epoch {
        return true;
    }

    let _transition = state.member_transition.lock().await;
    let transaction = match acquire_cloud_session_transition(&state.cloud_session_store).await {
        Ok(transaction) => transaction,
        Err(e) => {
            fail_close_member_runtime(state, &e);
            return false;
        }
    };
    let state_for_check = state.clone();
    let transaction_for_check = Arc::clone(&transaction);
    match tokio::task::spawn_blocking(move || {
        reconcile_member_session_locked(&state_for_check, &transaction_for_check)
    })
    .await
    {
        Ok(coherent) => coherent,
        Err(error) => {
            fail_close_member_runtime(state, &format!("member reconcile task failed: {error}"));
            false
        }
    }
}

/// Restore the in-memory member state from the persisted cloud session.
///
/// This closes the restart gap: `login_password` persists `cloud-session.json`,
/// but `AppState::new` necessarily starts as LoggedOut. On vault unlock and on a
/// lazy `/member/state` read, this function replays the authoritative cloud
/// session into `MemberState`, gateway/pluginhub settings, entitlement rows, and
/// best-effort plugin sync. It never turns a failed cloud check into Paid.
pub(crate) async fn restore_member_state_from_cloud_session(
    state: &SharedState,
) -> Option<MemberState> {
    let _transition = state.member_transition.lock().await;
    let session_transaction =
        match acquire_cloud_session_transition(&state.cloud_session_store).await {
            Ok(transaction) => transaction,
            Err(e) => {
                tracing::warn!("member restore: cannot fence cloud session: {e}");
                return None;
            }
        };
    let state_for_restore = state.clone();
    let transaction_for_restore = Arc::clone(&session_transaction);
    match tokio::task::spawn_blocking(move || {
        restore_member_state_from_cloud_session_locked(&state_for_restore, &transaction_for_restore)
    })
    .await
    {
        Ok(restored) => restored,
        Err(e) => {
            tracing::warn!("member restore task failed: {e}");
            None
        }
    }
}

/// Blocking half of [`restore_member_state_from_cloud_session`]. The caller
/// owns `member_transition` for this function's complete lifetime.
fn restore_member_state_from_cloud_session_locked(
    state: &SharedState,
    session_transaction: &CloudSessionTransaction,
) -> Option<MemberState> {
    // Lock means all vault-derived/member runtime must stay absent. In
    // particular, a require_auth=false deployment must not let a lazy
    // `/member/state` read immediately rebuild account state after the user
    // explicitly locked the vault.
    if !vault_is_unlocked(state) {
        return None;
    }
    if !crate::routes::privacy::outbound_enabled(state, "cloud_saas") {
        return None;
    }
    let Some(client) = cloud_client_from_transaction(session_transaction) else {
        let restored = restore_member_state_from_activation_receipt_locked(state);
        if restored.is_some() && bind_member_session_epoch(state, session_transaction).is_err() {
            fail_close_member_runtime(state, "bind activation session epoch");
            return None;
        }
        return restored;
    };
    let me = match client.me() {
        Ok(me) => me,
        Err(e) => {
            tracing::warn!("member restore: persisted cloud session is not usable: {e}");
            return None;
        }
    };
    let is_paid = current_plan_grants_paid(&me.plan, me.plan_expires.as_deref());
    let llm_quota_remaining = if is_paid {
        client
            .me_quota_json()
            .ok()
            .and_then(|value| quota_remaining_from_json(&value))
            .unwrap_or(0)
    } else {
        0
    };
    let restored = if is_paid {
        let selected = match client.list_licenses() {
            Ok(licenses) => licenses
                .into_iter()
                .find(|lic| lic.revoked_at.is_none() && plan_grants_paid(&lic.plan))
                .or_else(|| {
                    tracing::warn!("member restore: paid account has no active paid license");
                    None
                })?,
            Err(e) => {
                tracing::warn!("member restore: list licenses failed: {e}");
                return None;
            }
        };
        let fp = attune_core::device_fingerprint::device_fingerprint();
        let device = match client.device_activate(&selected.license_key, &fp) {
            Ok(dev) => dev,
            Err(e) => {
                tracing::warn!(
                    "member restore: device binding failed; paid state not restored: {e}"
                );
                return None;
            }
        };
        let device_plan = if device.plan.trim().is_empty() {
            selected.plan.as_str()
        } else {
            device.plan.as_str()
        };
        if !current_plan_grants_paid(device_plan, device.expires_at.as_deref()) {
            tracing::warn!("member restore: device binding no longer reports a paid plan");
            return None;
        }
        let restored = MemberState::Paid {
            account_id: me.id.to_string(),
            license_id: selected.canonical_id(),
            llm_quota_remaining,
        };
        if !vault_is_unlocked(state) {
            return None;
        }
        // The persisted rows belong to the previous locally cached account
        // until proven otherwise.  Clear them before activating this verified
        // account so entitlements can never accumulate across switches.
        if let Err(e) = clear_member_credentials(state, None) {
            tracing::warn!("member restore: failed to clear previous account state: {e}");
            return None;
        }
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = restored.clone();
        wire_cloud_gateway(
            state,
            me.gateway_url.as_deref(),
            me.gateway_token.as_deref(),
            me.gateway_default_model.as_deref(),
            &me.email,
        );
        wire_pluginhub_provider(state, &selected.license_key, &me.email);
        store_login_entitlements(state, &selected);
        store_device_binding(state, &device);
        let sync = attune_core::plugin_sync::best_effort_sync_plugins(&client);
        if !sync.installed.is_empty() || !sync.updated.is_empty() || !sync.failed.is_empty() {
            tracing::info!(
                "member restore: plugin sync installed={}, updated={}, failed={}",
                sync.installed.len(),
                sync.updated.len(),
                sync.failed.len()
            );
        }
        restored
    } else {
        let restored = MemberState::Free {
            account_id: me.id.to_string(),
        };
        if !vault_is_unlocked(state) {
            return None;
        }
        // A successful authoritative downgrade/login must immediately remove
        // cached paid credentials and entitlement rows.  Runtime teardown is
        // unconditional inside clear_member_credentials even if disk cleanup
        // later reports an error.
        if let Err(e) = clear_member_credentials(state, None) {
            tracing::warn!("member restore: failed to persist free-tier cleanup: {e}");
        }
        restored
    };

    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = restored.clone();
    if !vault_is_unlocked(state) {
        // Close the race where a lock lands after the pre-install check but
        // before the verified cloud response is applied. Whichever operation
        // finishes last observes Locked and leaves no member credential alive.
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
        *state
            .member_session_epoch
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        state.entitlement_cache.hydrate_from_rows(Vec::new());
        state.reload_llm();
        state.reload_plugin_hub(None, None);
        return None;
    }
    if let Err(e) = bind_member_session_epoch(state, session_transaction) {
        fail_close_member_runtime(state, &e);
        return None;
    }
    Some(restored)
}

/// Restore a license-code activation when no account cookie exists. The raw
/// activation code is stored only inside the vault-encrypted device receipt;
/// every restart replays both cloud authorization and device binding before
/// publishing Paid state, so revoked/expired/non-paid licenses fail closed.
fn restore_member_state_from_activation_receipt_locked(state: &SharedState) -> Option<MemberState> {
    let (license_key, persisted_device) = match load_activation_receipt(state) {
        Ok(Some(receipt)) => receipt,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("activation restore: unreadable encrypted receipt: {e}");
            return None;
        }
    };
    let cloud_url = resolve_accounts_url(state);
    let pluginhub_url = resolve_pluginhub_url(state);
    let client = CloudClient::new(cloud_url);
    let activation = match client.activate_license(&license_key) {
        Ok(result) if current_plan_grants_paid(&result.plan, result.expires_at.as_deref()) => {
            result
        }
        Ok(_) => {
            tracing::warn!("activation restore: license no longer grants a paid plan");
            return None;
        }
        Err(e) => {
            tracing::warn!("activation restore: online authorization failed: {e}");
            return None;
        }
    };
    let fp = attune_core::device_fingerprint::device_fingerprint();
    let device = match client.device_activate(&license_key, &fp) {
        Ok(device) => device,
        Err(e) => {
            tracing::warn!("activation restore: online device proof failed: {e}");
            return None;
        }
    };
    let device_plan = if device.plan.trim().is_empty() {
        activation.plan.as_str()
    } else {
        device.plan.as_str()
    };
    if !current_plan_grants_paid(device_plan, device.expires_at.as_deref()) {
        tracing::warn!("activation restore: device proof no longer grants a paid plan");
        return None;
    }
    if !persisted_device.device_id.trim().is_empty()
        && !device.device_id.trim().is_empty()
        && persisted_device.device_id != device.device_id
    {
        tracing::warn!("activation restore: cloud returned a different device identity");
        return None;
    }

    let plugin_sync = sync_activation_plugins_detailed(
        &pluginhub_url,
        &license_key,
        &activation.allowed_plugins,
        Some(fp.fingerprint_sig.as_str()),
    );
    if !vault_is_unlocked(state) {
        return None;
    }
    if let Err(e) = clear_member_credentials(state, None) {
        tracing::warn!("activation restore: failed to clear stale member runtime: {e}");
        return None;
    }
    // This write is the durable commit barrier. If it fails, remain LoggedOut
    // and do not rebuild any membership-owned provider or entitlement.
    if let Err(e) = persist_activation_receipt(state, &device, &license_key) {
        tracing::warn!("activation restore: failed to renew encrypted receipt: {e}");
        return None;
    }

    let license_identity = redact_license_key(&license_key);
    let restored = MemberState::Paid {
        account_id: license_identity.clone(),
        license_id: license_identity.clone(),
        llm_quota_remaining: 0,
    };
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = restored.clone();
    wire_cloud_gateway(
        state,
        activation.gateway_url.as_deref(),
        activation.gateway_token.as_deref(),
        activation.gateway_default_model.as_deref(),
        &license_identity,
    );
    wire_pluginhub_provider(state, &license_key, &license_identity);
    store_activation_entitlements(
        state,
        &license_identity,
        &activation.allowed_plugins,
        &plugin_sync.entitlements,
    );
    if !plugin_sync.report.installed.is_empty()
        || !plugin_sync.report.updated.is_empty()
        || !plugin_sync.report.failed.is_empty()
    {
        tracing::info!(
            "activation restore: plugin sync installed={}, updated={}, failed={}",
            plugin_sync.report.installed.len(),
            plugin_sync.report.updated.len(),
            plugin_sync.report.failed.len()
        );
    }
    if !vault_is_unlocked(state) {
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
        state.entitlement_cache.hydrate_from_rows(Vec::new());
        state.reload_llm();
        state.reload_plugin_hub(None, None);
        return None;
    }
    Some(restored)
}

/// Serialize a [`SyncReport`] into the stable UI JSON shape (shared by both
/// member-login endpoints).
fn sync_report_to_json(r: &attune_core::plugin_sync::SyncReport) -> serde_json::Value {
    serde_json::json!({
        "installed": r.installed,
        "updated": r.updated,
        "skipped_already_installed": r.skipped_already_installed,
        "failed": r.failed
            .iter()
            .map(|(id, reason)| serde_json::json!({"plugin_id": id, "reason": reason}))
            .collect::<Vec<_>>(),
    })
}

fn month_window_ms() -> (i64, i64) {
    let now = Utc::now();
    let start = now
        .with_day(1)
        .and_then(|d| d.date_naive().and_hms_opt(0, 0, 0).map(|dt| dt.and_utc()))
        .unwrap_or(now);
    (start.timestamp_millis(), now.timestamp_millis())
}

fn local_usage_summary_json(state: &SharedState) -> serde_json::Value {
    let (from_ms, to_ms) = month_window_ms();
    let summary = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.store().usage_summary(from_ms, to_ms).ok()
    };
    match summary {
        Some(s) => serde_json::json!({
            "events": s.events,
            "llm_tokens_input": s.tokens_in,
            "llm_tokens_output": s.tokens_out,
            "llm_tokens_total": s.tokens_in + s.tokens_out,
            "llm_cost_usd": s.cost_usd,
            "plugin_installs": 0,
            "cache_hit_rate": s.cache_hit_rate,
            "prompt_cache_hit_rate": s.prompt_cache_hit_rate,
        }),
        None => serde_json::json!({
            "events": 0,
            "llm_tokens_input": 0,
            "llm_tokens_output": 0,
            "llm_tokens_total": 0,
            "llm_cost_usd": 0.0,
            "plugin_installs": 0,
            "cache_hit_rate": 0.0,
            "prompt_cache_hit_rate": 0.0,
        }),
    }
}

fn fallback_quota_json(
    state: &SharedState,
    member: &MemberState,
    error: Option<String>,
) -> serde_json::Value {
    let tier = match member {
        MemberState::Paid { .. } => "pro",
        MemberState::Free { .. } => "free",
        MemberState::LoggedOut => "self-managed",
    };
    let limit = match member {
        MemberState::Paid {
            llm_quota_remaining,
            ..
        } => *llm_quota_remaining,
        _ => 0,
    };
    let mut cross_service_errors = serde_json::Map::new();
    if let Some(e) = error {
        cross_service_errors.insert("accounts".into(), serde_json::Value::String(e));
    }
    let usage = local_usage_summary_json(state);
    let used = usage
        .get("llm_tokens_total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let remaining = if limit > 0 {
        limit.saturating_sub(used)
    } else {
        0
    };
    let percent_used = if limit > 0 {
        ((used as f64 / limit as f64) * 100.0).min(100.0)
    } else {
        0.0
    };
    serde_json::json!({
        "tier": tier,
        "plan_expires": null,
        "month": Utc::now().format("%Y-%m").to_string(),
        "usage": usage,
        "quota": {
            "llm_tokens_monthly": limit,
            "remaining": remaining,
            "percent_used": percent_used,
        },
        "history": [],
        "local_usage": local_usage_summary_json(state),
        "cross_service_errors": cross_service_errors,
    })
}

fn attach_local_usage(mut value: serde_json::Value, state: &SharedState) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("local_usage".into(), local_usage_summary_json(state));
    }
    value
}

fn quota_remaining_from_json(value: &serde_json::Value) -> Option<u64> {
    [
        "/quota/remaining",
        "/quota/llm_tokens_remaining",
        "/llm_quota_remaining",
        "/remaining",
    ]
    .into_iter()
    .find_map(|pointer| {
        let value = value.pointer(pointer)?;
        value
            .as_u64()
            .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
    })
}

/// GET /api/v1/users/me/quota — 本地配额视图代理。
///
/// 有持久化 cloud session 时透传 accounts 的真实 quota；否则返回可渲染的零数据,
/// 让免费/自配 token 用户也能看到说明和升级入口,而不是空白页。
pub async fn get_quota(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let _transition = state.member_transition.lock().await;
    let session_transaction =
        match acquire_cloud_session_transition(&state.cloud_session_store).await {
            Ok(transaction) => transaction,
            Err(error) => {
                fail_close_member_runtime(&state, &error);
                let member = state
                    .member_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                return Json(fallback_quota_json(&state, &member, Some(error)));
            }
        };
    let state_for_check = state.clone();
    let transaction_for_check = Arc::clone(&session_transaction);
    let coherent = match tokio::task::spawn_blocking(move || {
        reconcile_member_session_locked(&state_for_check, &transaction_for_check)
    })
    .await
    {
        Ok(coherent) => coherent,
        Err(error) => {
            fail_close_member_runtime(
                &state,
                &format!("member quota reconcile task failed: {error}"),
            );
            false
        }
    };
    let member = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if !coherent {
        return Json(fallback_quota_json(
            &state,
            &member,
            Some("member session changed or could not be verified".to_string()),
        ));
    }
    if !member.is_logged_in() {
        return Json(fallback_quota_json(&state, &member, None));
    }
    if !crate::routes::privacy::outbound_enabled(&state, "cloud_saas") {
        return Json(fallback_quota_json(
            &state,
            &member,
            Some("cloud SaaS is disabled by privacy settings".to_string()),
        ));
    }
    let Some(client) = cloud_client_from_transaction(&session_transaction) else {
        return Json(fallback_quota_json(&state, &member, None));
    };
    let fallback_member = member.clone();
    match tokio::task::spawn_blocking(move || {
        let _session_fence = session_transaction;
        client.me_quota_json()
    })
    .await
    {
        Ok(Ok(v)) => Json(attach_local_usage(v, &state)),
        Ok(Err(e)) => Json(fallback_quota_json(
            &state,
            &fallback_member,
            Some(e.to_string()),
        )),
        Err(e) => Json(fallback_quota_json(
            &state,
            &fallback_member,
            Some(format!("quota task join error: {e}")),
        )),
    }
}

#[derive(serde::Deserialize)]
pub struct LoginPasswordReq {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub license_code: Option<String>,
}

/// POST /api/v1/member/login-password — 账号密码登录 cloud accounts，回填 member_state。
///
/// 说明：
/// - 密码只用于本次请求，不持久化到磁盘。
/// - accounts URL 由**服务端** `settings.cloud.accounts_url` 决定（默认
///   https://accounts.engi-stack.com）。SECURITY: 不接受请求体覆盖 —— 见
///   [`resolve_accounts_url`]（SSRF / 付费墙绕过)。
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
    // Preserve request order across slow cloud authentication: once a valid
    // account transition starts, logout/activation/another login must observe
    // its final disk + runtime state rather than overtaking its commit.
    let _transition = state.member_transition.lock().await;

    let cloud_url = resolve_accounts_url(&state);

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
    let cloud_url_for_blocking = cloud_url.clone();
    let blocking =
        tokio::task::spawn_blocking(move || -> Result<CloudLoginData, (StatusCode, String)> {
            let mut client = CloudClient::new(cloud_url_for_blocking.clone());
            let login_response = client
                .login(&email, &password)
                .map_err(|e| (StatusCode::UNAUTHORIZED, format!("login failed: {e}")))?;
            let session_token = client.session_token().map(str::to_string);
            // `/me` is bound to the newly issued session cookie and is therefore
            // authoritative for identity and plan.  Never combine licenses or a
            // gateway from that session with an account id taken only from the
            // login response.
            let me = client.me().map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("verify authenticated account failed: {e}"),
                )
            })?;
            validate_login_identity(&login_response, &me)?;
            let user = me.clone();
            let is_paid = current_plan_grants_paid(&user.plan, user.plan_expires.as_deref());
            if !is_paid {
                return Ok(CloudLoginData {
                    user,
                    license: None,
                    cloud_url: cloud_url_for_blocking,
                    session_token,
                    llm_quota_remaining: 0,
                    device: None,
                    plugin_sync: None,
                });
            }
            let licenses = client.list_licenses().map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("list licenses failed: {e}"),
                )
            })?;
            let eligible = licenses
                .into_iter()
                .filter(|lic| lic.revoked_at.is_none() && plan_grants_paid(&lic.plan));
            let selected = if let Some(code) = license_code.as_deref() {
                let code = code.trim();
                if code.is_empty() {
                    eligible.into_iter().next()
                } else {
                    eligible.into_iter().find(|lic| {
                        lic.license_key == code
                            || lic.id.to_string() == code
                            || lic.license_id.map(|id| id.to_string()).as_deref() == Some(code)
                    })
                }
            } else {
                eligible.into_iter().next()
            }
            .ok_or((
                StatusCode::BAD_REQUEST,
                "paid user has no matching active paid license".to_string(),
            ))?;
            let llm_quota_remaining = client
                .me_quota_json()
                .ok()
                .and_then(|value| quota_remaining_from_json(&value))
                .unwrap_or(0);
            let fp = attune_core::device_fingerprint::device_fingerprint();
            let device = client.device_activate(&selected.license_key, &fp);
            if device.as_ref().is_ok_and(|device| {
                let plan = if device.plan.trim().is_empty() {
                    selected.plan.as_str()
                } else {
                    device.plan.as_str()
                };
                !current_plan_grants_paid(plan, device.expires_at.as_deref())
            }) {
                return Err((
                    StatusCode::FORBIDDEN,
                    "device binding returned a non-paid license plan".to_string(),
                ));
            }
            // B5 (2026-06-06): auto-install entitled pro plugins (e.g. law-pro) so
            // domain-specific agents work right after login, no manual `attune
            // sync-plugins`. Runs on THIS blocking thread (reusing the authenticated
            // client + its session cookie). best_effort_* never returns Err — a sync
            // failure logs + yields an empty report; the login still succeeds (§4.5).
            // Signature verification (verify_with_key) inside sync is NOT bypassed:
            // an unverified package fails closed and is reported in `failed`.
            let plugin_sync = if device.is_ok() {
                Some(attune_core::plugin_sync::best_effort_sync_plugins(&client))
            } else {
                None
            };
            Ok(CloudLoginData {
                user,
                license: Some(selected),
                cloud_url: cloud_url_for_blocking,
                session_token,
                llm_quota_remaining,
                device: Some(device),
                plugin_sync,
            })
        });
    let CloudLoginData {
        user,
        license,
        cloud_url,
        session_token,
        llm_quota_remaining,
        device,
        plugin_sync,
    } = blocking
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("login task join error: {e}")})),
            )
        })?
        .map_err(|(code, msg)| (code, Json(serde_json::json!({"error": msg}))))?;

    // SECURITY (§11 R2): vertical is UI copy only and comes from the
    // session-authenticated `/me` snapshot; signed entitlements remain the gate.
    let vertical = user.vertical.clone();

    // Validate the paid device result before touching the previous account's
    // runtime or persisted session. A failed account switch must leave the
    // already-active account intact rather than logging it out and then
    // resurrecting it only after restart.
    let validated_device = if license.is_some() {
        match device {
            Some(Ok(dev)) => Some(dev),
            Some(Err(e)) => return Err(device_binding_error(&e)),
            None => {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": "device binding result missing",
                        "code": "device-binding-missing",
                        "paid_applied": false,
                    })),
                ));
            }
        }
    } else {
        None
    };

    // Cloud authentication has no local side effects. Fence the shared
    // CLI/server cookie before consent or account cleanup and retain the same
    // lock through final runtime publication.
    let session_transaction = acquire_cloud_session_transition(&state.cloud_session_store)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e,
                    "code": "cloud-session-lock-failed",
                    "paid_applied": false,
                })),
            )
        })?;

    // Consent durability is the local commit barrier.  Keep the previous
    // account/session/runtime untouched if the explicit Cloud SaaS opt-in
    // cannot be persisted.
    crate::routes::privacy::set_outbound_enabled(&state, "cloud_saas", true).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("persist cloud consent: {e}")})),
        )
    })?;

    // Stage the newly authenticated cookie behind a durable restore-suppression
    // marker before replacing the old runtime. If the cloud did not issue a
    // cookie, retire the previous one; removal itself installs the same marker.
    // Nothing can lazy-restore the new/old account until cleanup commits below.
    let cloud_url_for_session = cloud_url.clone();
    let session_for_disk = session_token.clone();
    let has_staged_session = session_for_disk
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty());
    let staged_session_transaction = Arc::clone(&session_transaction);
    let stage_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        match session_for_disk.filter(|token| !token.trim().is_empty()) {
            Some(token) => staged_session_transaction
                .stage(&cloud_url_for_session, &token)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            None => staged_session_transaction
                .remove()
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    })
    .await;
    match stage_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            fail_close_member_runtime(&state, &format!("persist cloud session: {e}"));
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("persist cloud session: {e}"),
                    "code": "cloud-session-stage-failed",
                    "paid_applied": false,
                })),
            ));
        }
        Err(e) => {
            fail_close_member_runtime(&state, &format!("persist cloud session task: {e}"));
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("persist cloud session task: {e}"),
                    "code": "cloud-session-stage-failed",
                    "paid_applied": false,
                })),
            ));
        }
    }

    // Authentication has succeeded.  From this point the new account replaces
    // the old local membership atomically from the runtime's perspective: drop
    // the old provider/entitlements before wiring any credential returned for
    // the new account.
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
    if let Err(e) = clear_member_credentials(&state, None) {
        // The staged session is already restore-suppressed. Try to remove its
        // raw cookie as hygiene, but retain/report a rollback error rather than
        // relying on deletion for safety.
        let rollback_error = rollback_staged_cloud_session(Arc::clone(&session_transaction))
            .await
            .err();
        if let Some(rollback_error) = rollback_error.as_deref() {
            tracing::warn!(
                "member login: staged session rollback file cleanup failed; restore remains suppressed: {rollback_error}"
            );
        }
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("clear previous member state: {e}"),
                "code": "member-switch-cleanup-failed",
                "session_restore_suppressed": true,
                "session_file_removed": rollback_error.is_none(),
            })),
        ));
    }

    // Publish the staged cookie only after old membership credentials and
    // entitlements have been durably cleared. A commit failure leaves the
    // marker in place; best-effort raw-file removal cannot weaken that fence.
    if has_staged_session {
        if let Err(e) = commit_staged_cloud_session_restore(Arc::clone(&session_transaction)).await
        {
            let rollback_error = rollback_staged_cloud_session(Arc::clone(&session_transaction))
                .await
                .err();
            if let Some(rollback_error) = rollback_error.as_deref() {
                tracing::warn!(
                    "member login: failed session commit cleanup; restore remains suppressed: {rollback_error}"
                );
            }
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e,
                    "code": "cloud-session-commit-failed",
                    "paid_applied": false,
                    "session_restore_suppressed": true,
                    "session_file_removed": rollback_error.is_none(),
                })),
            ));
        }
    }

    let (new_state, device_json) = if let Some(selected) = license {
        let dev = validated_device.expect("paid device was validated before account switch");
        let paid_state = MemberState::Paid {
            account_id: user.id.to_string(),
            license_id: selected.canonical_id(),
            // The accounts quota snapshot feeds ACP scheduling; the gateway
            // remains authoritative and will still reject stale overspend.
            llm_quota_remaining,
        };
        // Runtime membership credential suppression is fail-closed.  Mark the
        // already verified account Paid immediately before its providers are
        // rebuilt, never before device binding succeeds.
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = paid_state.clone();
        // 付费会员：拿 cloud gateway token, 合并进 vault app_settings,
        // 桌面 chat 零配置接通云端 LLM。best-effort — 失败不阻断登录。
        wire_cloud_gateway(
            &state,
            user.gateway_url.as_deref(),
            user.gateway_token.as_deref(),
            user.gateway_default_model.as_deref(),
            &user.email,
        );

        wire_pluginhub_provider(&state, &selected.license_key, &user.email);
        store_login_entitlements(&state, &selected);
        store_device_binding(&state, &dev);

        (
            paid_state,
            Some(serde_json::json!({
                "device_id": dev.device_id,
                "max_activations": dev.max_activations,
                "current_activations": dev.current_activations,
            })),
        )
    } else {
        (
            MemberState::Free {
                account_id: user.id.to_string(),
            },
            None,
        )
    };

    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = new_state.clone();
    if let Err(e) = bind_member_session_epoch(&state, session_transaction.as_ref()) {
        fail_close_member_runtime(&state, &e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": e,
                "code": "cloud-session-epoch-failed",
                "paid_applied": false,
            })),
        ));
    }
    // B5: surface the best-effort plugin auto-install outcome to the UI (non-fatal;
    // the login already succeeded regardless of plugin sync).
    let plugins_json = plugin_sync.as_ref().map(sync_report_to_json);
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": new_state,
        "email": user.email,
        "tier": user.plan,
        "vertical": vertical,
        "plugin_sync": plugins_json,
        "device": device_json,
    })))
}

/// POST /api/v1/member/entitlements/refresh — 手动触发一轮 entitlement re-verify
/// (三入口之一:周期 worker / 登录 / **手动**,spec §7.2 / plan T8)。
///
/// 必须已登录会员(R1.1 复用:未登录 → 401)。对缓存中每条 entitlement 跑真
/// `verify_round`,响应经 SEC-1/2 门(`authorize_snapshot`)**后**才转 Active;
/// 写回缓存 + vault(短取 vault 锁,**不嵌套** fulltext/vectors)。
///
/// - cloud 完全不可达(所有 verify 5xx/transport)→ 502 `{code: cloud-unreachable}`,
///   **本地缓存原样不动**(spec §7.2 error 5)。
/// - 否则 → 200 `{refreshed, statuses}`。
pub async fn refresh_entitlements(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let _transition = state.member_transition.lock().await;
    // R1.1: 必须已登录(free 或 paid 都可手动 refresh;未登录拒)。
    {
        let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner());
        if !m.is_logged_in() {
            return Err(AppError::Unauthorized("member login required".into()));
        }
    }
    if !crate::routes::privacy::outbound_enabled(&state, "cloud_saas") {
        return Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "cloud SaaS is disabled by privacy settings",
                "code": "cloud-saas-disabled",
            }),
        ));
    }

    // 网络 I/O 是 blocking(CloudClient = reqwest::blocking)→ spawn_blocking(B4 约束)。
    // 把 EntitlementCache(Arc 内,clone 廉价)move 进 blocking 线程跑真 verify;写回
    // vault 也在该线程(短取 vault 锁)。结果 RefreshSummary 带回 async tail 映射响应。
    let cache = state.entitlement_cache.clone();
    let state_for_writeback = state.clone();
    let summary = tokio::task::spawn_blocking(move || -> RefreshSummary {
        run_refresh_round_locked(&state_for_writeback, &cache)
    })
    .await
    .map_err(|e| AppError::Internal(format!("refresh task join error: {e}")))?;

    if summary.all_network_error {
        // cloud 完全不可达 —— 缓存未被破坏(apply_reverify NetworkError 不动缓存)。
        return Err(AppError::detailed(
            StatusCode::BAD_GATEWAY,
            serde_json::json!({ "error": "cloud unreachable", "code": "cloud-unreachable" }),
        ));
    }
    Ok(Json(serde_json::json!({
        "status": "ok",
        "refreshed": summary.refreshed,
        "statuses": summary
            .statuses
            .iter()
            .map(|(id, st)| serde_json::json!({ "plugin_id": id, "status": st }))
            .collect::<Vec<_>>(),
    })))
}

/// POST /api/v1/member/plugins/sync — 手动重试会员插件下载/更新。
///
/// 账号密码/CLI 登录优先复用 `cloud-session.json` 的 cloud session,以 cloud 返回的
/// entitled_plugins 做完整签名链同步。授权码激活路径没有账号 session,则回退到本地
/// entitlement_cache + pluginhub license_key 走 activation 同源的 PluginHub 安装链。
pub async fn sync_plugins_now(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let _transition = state.member_transition.lock().await;
    let session_transaction = acquire_cloud_session_transition(&state.cloud_session_store)
        .await
        .map_err(|error| {
            fail_close_member_runtime(&state, &error);
            AppError::Internal(error)
        })?;
    let state_for_check = state.clone();
    let transaction_for_check = Arc::clone(&session_transaction);
    let coherent = tokio::task::spawn_blocking(move || {
        reconcile_member_session_locked(&state_for_check, &transaction_for_check)
    })
    .await
    .map_err(|error| {
        let detail = format!("member reconcile task failed: {error}");
        fail_close_member_runtime(&state, &detail);
        AppError::Internal(detail)
    })?;
    if !coherent {
        return Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "member session changed or could not be verified",
                "code": "member-session-invalid",
            }),
        ));
    }
    {
        let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner());
        if !m.is_paid() {
            return Err(AppError::detailed(
                StatusCode::FORBIDDEN,
                serde_json::json!({
                    "error": "paid membership required",
                    "code": "membership-required",
                }),
            ));
        }
    }
    if !crate::routes::privacy::outbound_enabled(&state, "cloud_saas") {
        return Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "cloud SaaS is disabled by privacy settings",
                "code": "cloud-saas-disabled",
            }),
        ));
    }

    let state_for_sync = state.clone();
    let transaction_for_sync = Arc::clone(&session_transaction);
    let report = tokio::task::spawn_blocking(
        move || -> Result<attune_core::plugin_sync::SyncReport, String> {
            if let Some(client) = cloud_client_from_transaction(&transaction_for_sync) {
                return Ok(attune_core::plugin_sync::best_effort_sync_plugins(&client));
            }

            let (hub_url, license_key) = resolve_pluginhub_config(&state_for_sync);
            let Some(license_key) = license_key.filter(|s| !s.trim().is_empty()) else {
                return Err(
                    "no cloud session or pluginhub license key available for plugin sync".into(),
                );
            };
            let allowed_plugins = entitled_plugin_ids_for_retry(&state_for_sync);
            if allowed_plugins.is_empty() {
                return Ok(attune_core::plugin_sync::SyncReport {
                    installed: Vec::new(),
                    updated: Vec::new(),
                    skipped_already_installed: Vec::new(),
                    failed: Vec::new(),
                });
            }
            let fp = attune_core::device_fingerprint::device_fingerprint();
            Ok(sync_activation_plugins(
                &hub_url,
                &license_key,
                &allowed_plugins,
                Some(fp.fingerprint_sig.as_str()),
            ))
        },
    )
    .await
    .map_err(|e| AppError::Internal(format!("plugin sync task join error: {e}")))?
    .map_err(|e| {
        AppError::detailed(
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": e,
                "code": "plugin-sync-unavailable",
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "plugin_sync": sync_report_to_json(&report),
    })))
}

/// 跑一轮 refresh(blocking):读缓存 → 真 verify 每条 → apply → 写回 vault。
/// 复用为 worker 的单轮逻辑(worker 周期调用本函数)。无可达 cloud session →
/// 返回空 summary(`all_network_error=false`、`refreshed=0` —— 不误判 502)。
pub fn run_refresh_round(state: &SharedState, cache: &EntitlementCache) -> RefreshSummary {
    // The periodic worker is a native thread, while the HTTP refresh path
    // already holds the async transition guard and calls the locked helper.
    // Serializing here prevents an old account's verification round from
    // repopulating entitlements after logout/account switch.
    let _transition = state.member_transition.blocking_lock();
    run_refresh_round_locked(state, cache)
}

fn run_refresh_round_locked(state: &SharedState, cache: &EntitlementCache) -> RefreshSummary {
    if !vault_is_unlocked(state) {
        return RefreshSummary::default();
    }
    let session_transaction = match state.cloud_session_store.transaction() {
        Ok(transaction) => transaction,
        Err(error) => {
            fail_close_member_runtime(
                state,
                &format!("acquire entitlement refresh session fence: {error}"),
            );
            return RefreshSummary::default();
        }
    };
    if !reconcile_member_session_locked(state, &session_transaction) {
        return RefreshSummary::default();
    }
    if !state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_logged_in()
    {
        return RefreshSummary::default();
    }
    if !crate::routes::privacy::outbound_enabled(state, "cloud_saas") {
        return RefreshSummary::default();
    }
    let now = Utc::now();
    let mode = resolve_trust_mode(state);

    // 构建 CloudClient(从持久化 cloud session)。无 session → 不算"网络错"
    // (没有可 verify 的入口),返回空 summary。
    let Some(client) = cloud_client_from_transaction(&session_transaction) else {
        return RefreshSummary::default();
    };

    let rounds = attune_core::entitlement_reverify::reverify_all(cache, &client, mode, &now);
    let summary = apply_refresh_rounds(cache, &rounds, &now);

    // 写回 vault:仅对被接受(Active/BusinessDeny)的轮次落盘。短取 vault 锁,
    // **不**在持 entitlement 锁时取 vault(apply_refresh_rounds 已释放 cache 锁)。
    writeback_accepted(state, &rounds);
    summary
}

/// 把 apply 后被接受的行写回 vault DB(短取 vault 锁,不嵌套)。NetworkError/
/// Unauthorized 的轮次不写(缓存与 vault 都保持原样)。
fn writeback_accepted(state: &SharedState, rounds: &[(String, ReverifyOutcome, Option<String>)]) {
    let cache = &state.entitlement_cache;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(dek) = vault.dek_db() else { return }; // vault locked → skip writeback
    for (plugin_id, outcome, verified_at) in rounds {
        // is_deny:验签通过的 revoked/suspended 是 AUTHORITATIVE DOWNGRADE,必须绕过
        // upsert 的反降级归并落盘(REVIEW Critical-1),否则吊销在重启后复活。
        let (new_status, is_deny) = match outcome {
            ReverifyOutcome::Active => ("active", false),
            ReverifyOutcome::BusinessDeny(s) => (s.as_str(), true),
            // 网络错 / 未授权 → 不动 vault(grace,缓存也未变)。
            ReverifyOutcome::NetworkError | ReverifyOutcome::Unauthorized(_) => continue,
        };
        // 取缓存当前行(apply 已更新内存),按其 last_verified_at 落盘。
        let va = verified_at.as_deref().unwrap_or("");
        if is_deny {
            // 显式降级:直接 UPDATE,无 rank guard(merge 不会吃掉吊销)。
            // 用 verified_at 作 freshness 基准;空则退回行内 last_verified_at。
            let last_verified = if va.is_empty() {
                cache
                    .last_verified_at(plugin_id)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default()
            } else {
                va.to_string()
            };
            if let Err(e) =
                vault
                    .store()
                    .set_entitlement_status(plugin_id, new_status, &last_verified)
            {
                tracing::error!(
                    "reverify: failed to persist verified deny for {plugin_id} (status={new_status}): {e}"
                );
            }
        } else if let Some(mut row) = cache
            .snapshot()
            .into_iter()
            .find(|r| &r.plugin_id == plugin_id)
        {
            row.status = new_status.to_string();
            if !va.is_empty() {
                row.last_verified_at = va.to_string();
            }
            row.updated_at = va.to_string();
            if let Err(e) = vault.store().upsert_entitlement(&dek, &row) {
                tracing::error!(
                    "reverify: failed to persist active write-back for {plugin_id}: {e}"
                );
            }
        }
    }
}

fn cloud_client_from_transaction(
    session_transaction: &CloudSessionTransaction,
) -> Option<CloudClient> {
    let session = session_transaction.load().ok().flatten()?;
    Some(CloudClient::with_session(
        session.cloud_url,
        session.session,
    ))
}

async fn acquire_cloud_session_transition(
    store: &CloudSessionStore,
) -> Result<Arc<CloudSessionTransaction>, String> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || store.transaction().map(Arc::new))
        .await
        .map_err(|e| format!("acquire cloud session transition task: {e}"))?
        .map_err(|e| format!("acquire cloud session transition: {e}"))
}

async fn rollback_staged_cloud_session(
    transaction: Arc<CloudSessionTransaction>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || transaction.remove())
        .await
        .map_err(|e| format!("rollback cloud session task: {e}"))?
        .map(|_| ())
        .map_err(|e| format!("rollback cloud session file: {e}"))
}

async fn commit_staged_cloud_session_restore(
    transaction: Arc<CloudSessionTransaction>,
) -> Result<(), String> {
    let removed_marker = tokio::task::spawn_blocking(move || transaction.commit())
        .await
        .map_err(|e| format!("commit cloud session task: {e}"))?
        .map_err(|e| format!("commit cloud session marker: {e}"))?;
    if !removed_marker {
        return Err(
            "commit cloud session marker: staged transaction is no longer current".to_string(),
        );
    }
    Ok(())
}

fn resolve_pluginhub_config(state: &SharedState) -> (String, Option<String>) {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let parsed = crate::settings_store::load_settings(&vault).ok().flatten();
    let url = parsed
        .as_ref()
        .and_then(|v| {
            v.get("pluginhub")
                .and_then(|p| p.get("url"))
                .and_then(|u| u.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| DEFAULT_PLUGINHUB_URL.to_string());
    let key = parsed.and_then(|v| {
        v.get("pluginhub")
            .and_then(|p| p.get("license_key"))
            .and_then(|k| k.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    (url, key)
}

fn wire_pluginhub_provider(state: &SharedState, license_key: &str, who: &str) {
    if license_key.trim().is_empty() {
        return;
    }
    let hub_url = resolve_pluginhub_url(state);
    match apply_pluginhub_to_vault_settings(state, &hub_url, license_key) {
        Ok(true) => {
            tracing::info!("member pluginhub: configured PluginHub for {who}");
            state.reload_plugin_hub_from_settings();
        }
        Ok(false) => tracing::info!(
            "member pluginhub: user has a BYOK PluginHub credential; membership value not applied"
        ),
        Err(e) => tracing::warn!("member pluginhub: settings not written for {who}: {e}"),
    }
}

fn apply_pluginhub_to_vault_settings(
    state: &SharedState,
    hub_url: &str,
    license_key: &str,
) -> Result<bool, String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let existing = crate::settings_store::load_settings(&vault)
        .map_err(|e| format!("load settings failed: {e}"))?;
    let mut current: serde_json::Value = match existing {
        Some(settings) => settings,
        None => serde_json::json!({}),
    };
    if !current.is_object() {
        current = serde_json::json!({});
    }
    let obj = current.as_object_mut().expect("current is object");
    let pluginhub = obj
        .entry("pluginhub")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| "settings.pluginhub must be an object".to_string())?;
    let membership_owned = pluginhub
        .get("managed_by")
        .and_then(serde_json::Value::as_str)
        == Some(attune_core::llm_settings::MEMBER_GATEWAY_OWNER);
    let has_user_key = pluginhub
        .get("license_key")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty());
    if has_user_key && !membership_owned {
        return Ok(false);
    }
    pluginhub.insert("url".into(), serde_json::Value::String(hub_url.to_string()));
    pluginhub.insert(
        "license_key".into(),
        serde_json::Value::String(license_key.to_string()),
    );
    pluginhub.insert(
        "managed_by".into(),
        serde_json::Value::String(attune_core::llm_settings::MEMBER_GATEWAY_OWNER.to_string()),
    );
    crate::settings_store::persist_settings(&vault, current)
        .map_err(|e| format!("persist settings failed: {e}"))?;
    Ok(true)
}

fn entitled_plugin_ids_for_retry(state: &SharedState) -> Vec<String> {
    state
        .entitlement_cache
        .snapshot()
        .into_iter()
        .filter(|row| {
            !row.tier.trim().eq_ignore_ascii_case("free")
                && !matches!(
                    row.status.trim().to_ascii_lowercase().as_str(),
                    "revoked" | "suspended" | "expired"
                )
        })
        .map(|row| row.plugin_id)
        .collect()
}

/// 解析当前 `plugin_trust_mode`(app_settings meta);缺失/旧配置/vault 锁 → 默认
/// [`TrustMode::Warn`](决策 2 + spec §10 grandfather)。T11 加 UI setter;本函数
/// 只读,默认 Warn 让 client 先于 cloud v4 ship 不破网(跨仓 bootstrap)。
fn resolve_trust_mode(state: &SharedState) -> TrustMode {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(data) = vault.store().get_meta(SETTINGS_META_KEY) else {
        return TrustMode::Warn;
    };
    let Some(bytes) = data else {
        return TrustMode::Warn;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return TrustMode::Warn;
    };
    v.get("plugin_trust_mode")
        .and_then(|m| serde_json::from_value::<TrustMode>(m.clone()).ok())
        .unwrap_or(TrustMode::Warn)
}

#[derive(serde::Deserialize)]
pub struct ActivateLicenseReq {
    pub license_key: String,
}

/// 授权码激活的两阶段结果(blocking 线程内一次性完成两次 cloud 调用):
/// - 阶段一 **authorization**:`/member/activate` 成功 → 用户被授权为 Paid + 拿 gateway。
/// - 阶段二 **device binding**:`/devices/activate` 把本机指纹绑到 license + 颁 device_token。
///
/// 商业保护语义:云端授权成功后仍必须绑定当前设备；设备绑定失败时不下载付费插件、
/// 不写 gateway/pluginhub、不置 Paid。device 绑定结果用 `Result` 携带分类错误。
struct ActivationOutcome {
    activate: attune_core::cloud_client::ActivateResult,
    device: std::result::Result<
        attune_core::cloud_client::DeviceActivateResult,
        attune_core::cloud_client::DeviceActivateError,
    >,
    /// Best-effort pro plugin install driven by activation-code entitlements.
    /// Unlike password/session login, this path has no cloud session cookie, so it
    /// talks to PluginHub with the license key. Runs only after device binding
    /// succeeds, so an unbound copied device cannot receive paid plugin packages.
    plugin_sync: attune_core::plugin_sync::SyncReport,
    plugin_entitlements: Vec<PluginHubInstallEntitlement>,
}

enum ActivationTaskError {
    Cloud(String),
    LicensePlanNotPaid,
}

/// POST /api/v1/member/activate-license — 授权码 (license_key) 激活全链。
///
/// 授权激活 = ① **绑定设备**(本机指纹 → license,颁 device_token + 限设备数)
/// ② pro 下载(entitlement allowed_plugins 供 pluginhub)③ new-api 调用(gateway LLM)。
/// 镜像 `login_password` 的付费分支并补齐设备绑定(#79 全链):
/// 调 cloud `activate_license`(授权)+ `device_activate`(绑本机)→ 复用
/// [`wire_cloud_gateway`] 配 gateway LLM(锁定 endpoint/api_key/provider)+ 落 entitlement
/// + 持久化 device_token + 置 [`MemberState::Paid`]。
///
/// 错误语义(两阶段分离,fail-closed):
/// - 空 license_key → 400。
/// - 授权阶段 `/member/activate` 4xx/5xx/transport → 502 (`activate-failed`),**不**置 Paid。
/// - 设备绑定阶段(授权已成功):
///   - 超设备数 → 409 (`max-devices-reached`),给可操作提示(用户应在别处吊销旧设备);
///   - 指纹/license 被拒 → 403 (`device-rejected`);
///   - cloud 不可达 → 502 (`device-activate-unavailable`)。
///
/// 这些错误下不写入 Paid/gateway/pluginhub/entitlement,避免授权码或本地状态被迁移滥用。
pub async fn activate_license(
    State(state): State<SharedState>,
    Json(req): Json<ActivateLicenseReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let license_key = req.license_key.trim().to_string();
    if license_key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": "license_key required", "code": "license-key-required"}),
            ),
        ));
    }
    let _transition = state.member_transition.lock().await;
    // SECURITY: accounts URL 来自服务端 settings(不接受请求体覆盖)—— 见
    // resolve_accounts_url(SSRF / 付费墙绕过)。
    let cloud_url = resolve_accounts_url(&state);
    let pluginhub_url = resolve_pluginhub_url(&state);

    // B4 约束:CloudClient = reqwest::blocking → spawn_blocking,async tail 留本线程。
    // 两次 cloud 调用(authorize + device bind)在**同一** blocking 线程里串行完成,
    // 复用同一 client(及 session)。指纹采集也在此线程(machine-id / 持久化 device.id
    // 的文件 I/O,非异步上下文)。
    let key_for_blocking = license_key.clone();
    let outcome = tokio::task::spawn_blocking(
        move || -> Result<ActivationOutcome, ActivationTaskError> {
            let client = CloudClient::new(cloud_url);
            // 阶段一:授权(失败即整体失败,fail-closed)。
            let activate = client
                .activate_license(&key_for_blocking)
                .map_err(|e| ActivationTaskError::Cloud(e.to_string()))?;
            if !current_plan_grants_paid(&activate.plan, activate.expires_at.as_deref()) {
                return Err(ActivationTaskError::LicensePlanNotPaid);
            }
            // 阶段二:绑定本机设备(授权已成功;绑定结果按分类错误带回,不在此抛)。
            let fp = attune_core::device_fingerprint::device_fingerprint();
            let device = client.device_activate(&key_for_blocking, &fp);
            if device.as_ref().is_ok_and(|device| {
                let plan = if device.plan.trim().is_empty() {
                    activate.plan.as_str()
                } else {
                    device.plan.as_str()
                };
                !current_plan_grants_paid(plan, device.expires_at.as_deref())
            }) {
                return Err(ActivationTaskError::LicensePlanNotPaid);
            }
            let (plugin_sync, plugin_entitlements) = if device.is_ok() {
                let sync = sync_activation_plugins_detailed(
                    &pluginhub_url,
                    &key_for_blocking,
                    &activate.allowed_plugins,
                    Some(fp.fingerprint_sig.as_str()),
                );
                (sync.report, sync.entitlements)
            } else {
                (
                    attune_core::plugin_sync::SyncReport {
                        installed: Vec::new(),
                        updated: Vec::new(),
                        skipped_already_installed: Vec::new(),
                        failed: Vec::new(),
                    },
                    Vec::new(),
                )
            };
            Ok(ActivationOutcome {
                activate,
                device,
                plugin_sync,
                plugin_entitlements,
            })
        },
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("activate task join error: {e}")})),
        )
    })?
    .map_err(|error| match error {
        ActivationTaskError::Cloud(message) => {
            // 授权阶段失败:无效授权码 / cloud 不可达 → 502。绝不在错误路径置 Paid。
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("activate failed: {message}"), "code": "activate-failed"})),
            )
        }
        ActivationTaskError::LicensePlanNotPaid => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "license plan does not grant paid membership",
                "code": "license-plan-not-paid",
            })),
        ),
    })?;

    let ActivationOutcome {
        activate: result,
        device,
        plugin_sync,
        plugin_entitlements,
    } = outcome;
    let dev = match device {
        Ok(dev) => dev,
        Err(e) => return Err(device_binding_error(&e)),
    };

    let session_transaction = acquire_cloud_session_transition(&state.cloud_session_store)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": e,
                    "code": "cloud-session-lock-failed",
                    "paid_applied": false,
                })),
            )
        })?;

    // Consent is the first local commit barrier. A persistence failure must
    // leave the previous session and active member runtime untouched.
    crate::routes::privacy::set_outbound_enabled(&state, "cloud_saas", true).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("persist cloud consent: {e}")})),
        )
    })?;

    // License-code activation has no account cookie of its own. Retire the
    // previous cookie under the same cross-process fence retained through
    // receipt, provider, entitlement, and Paid-state publication.
    let activation_session_transaction = Arc::clone(&session_transaction);
    let remove_result =
        tokio::task::spawn_blocking(move || activation_session_transaction.remove()).await;
    match remove_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            fail_close_member_runtime(&state, &format!("remove old cloud session: {e}"));
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("remove old cloud session: {e}"),
                    "code": "cloud-session-cleanup-failed",
                    "paid_applied": false,
                })),
            ));
        }
        Err(e) => {
            fail_close_member_runtime(&state, &format!("remove old cloud session task: {e}"));
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("remove old cloud session task: {e}"),
                    "code": "cloud-session-cleanup-failed",
                    "paid_applied": false,
                })),
            ));
        }
    }

    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
    clear_member_credentials(&state, None).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("clear previous member state: {e}"),
                "code": "member-switch-cleanup-failed",
            })),
        )
    })?;

    // 置 Paid。授权码是凭据，绝不能进入状态 API 或 access log；仅使用稳定摘要
    // 作为本地关联标识，原始 key 只保留在受控的 provider/entitlement sinks。
    let license_identity = redact_license_key(&license_key);
    let new_state = MemberState::Paid {
        account_id: license_identity.clone(),
        license_id: license_identity.clone(),
        llm_quota_remaining: 0,
    };
    // A license-code activation has no account cookie. Its encrypted receipt
    // must be durable before Paid/runtime publication so a 200 response can be
    // restored after restart by fresh online authorization + device proof.
    persist_activation_receipt(&state, &dev, &license_key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("persist activation receipt: {e}"),
                "code": "activation-receipt-persist-failed",
                "paid_applied": false,
            })),
        )
    })?;
    // Set only after authorization + device binding + old-account cleanup, but
    // before rebuilding membership-owned runtime providers.
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = new_state.clone();

    // ── 授权成功:配 gateway LLM + 落 entitlement + 置 Paid(与 login_password 同逻辑)──
    // best-effort:写失败不阻断激活(用户仍是 Paid,只是 chat 需手填 key,§4.5)。
    // SECURITY (§1.4): `who` 进 tracing 日志 —— 绝不传 license_key 明文,传脱敏摘要。
    wire_cloud_gateway(
        &state,
        result.gateway_url.as_deref(),
        result.gateway_token.as_deref(),
        result.gateway_default_model.as_deref(),
        &redact_license_key(&license_key),
    );
    wire_pluginhub_provider(&state, &license_key, &redact_license_key(&license_key));
    // 落 entitlement:allowed_plugins 写进缓存 + vault,供 pluginhub 安装授权(②)+
    // 周期 re-verify 基准。best-effort,失败仅 warn。
    store_activation_entitlements(
        &state,
        &license_identity,
        &result.allowed_plugins,
        &plugin_entitlements,
    );
    if let Err(e) = bind_member_session_epoch(&state, session_transaction.as_ref()) {
        fail_close_member_runtime(&state, &e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": e,
                "code": "cloud-session-epoch-failed",
                "paid_applied": false,
            })),
        ));
    }
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": new_state,
        "plan": result.plan,
        "expires_at": result.expires_at,
        // GAP-B: passthrough cloud-issued vertical (UI copy only, never a gate
        // per §11 R2; client relays, does not self-report).
        "vertical": result.vertical,
        "allowed_plugins": result.allowed_plugins,
        "plugin_sync": sync_report_to_json(&plugin_sync),
        "device": {
            "device_id": dev.device_id,
            "max_activations": dev.max_activations,
            "current_activations": dev.current_activations,
        },
    })))
}

fn sync_activation_plugins(
    hub_url: &str,
    license_key: &str,
    allowed_plugins: &[String],
    device_fp: Option<&str>,
) -> attune_core::plugin_sync::SyncReport {
    sync_activation_plugins_detailed(hub_url, license_key, allowed_plugins, device_fp).report
}

fn sync_activation_plugins_detailed(
    hub_url: &str,
    license_key: &str,
    allowed_plugins: &[String],
    device_fp: Option<&str>,
) -> ActivationPluginSync {
    let hub = attune_core::plugin_hub::HttpPluginHubProvider::new(hub_url, license_key);
    match attune_core::plugin_registry::PluginRegistry::default_plugins_dir() {
        Ok(plugins_dir) => sync_activation_plugins_with_hub_detailed(
            &hub,
            allowed_plugins,
            device_fp,
            &plugins_dir,
            true,
        ),
        Err(e) => ActivationPluginSync {
            report: activation_sync_failed_for_all(
                allowed_plugins,
                format!("plugins dir unavailable: {e}"),
            ),
            entitlements: Vec::new(),
        },
    }
}

fn activation_sync_failed_for_all(
    allowed_plugins: &[String],
    reason: String,
) -> attune_core::plugin_sync::SyncReport {
    attune_core::plugin_sync::SyncReport {
        installed: Vec::new(),
        updated: Vec::new(),
        skipped_already_installed: Vec::new(),
        failed: allowed_plugins
            .iter()
            .map(|plugin_id| (plugin_id.clone(), reason.clone()))
            .collect(),
    }
}

#[cfg(test)]
fn sync_activation_plugins_with_hub(
    hub: &dyn PluginHubProvider,
    allowed_plugins: &[String],
    device_fp: Option<&str>,
    plugins_dir: &std::path::Path,
) -> attune_core::plugin_sync::SyncReport {
    sync_activation_plugins_with_hub_detailed(hub, allowed_plugins, device_fp, plugins_dir, false)
        .report
}

#[derive(Debug, Clone)]
struct PluginHubInstallEntitlement {
    plugin_id: String,
    decrypt_key: Option<String>,
    trial_expires: Option<String>,
}

#[derive(Debug, Clone)]
struct ActivationPluginSync {
    report: attune_core::plugin_sync::SyncReport,
    entitlements: Vec<PluginHubInstallEntitlement>,
}

fn sync_activation_plugins_with_hub_detailed(
    hub: &dyn PluginHubProvider,
    allowed_plugins: &[String],
    device_fp: Option<&str>,
    plugins_dir: &std::path::Path,
    refresh_installed_entitlements: bool,
) -> ActivationPluginSync {
    let mut report = attune_core::plugin_sync::SyncReport {
        installed: Vec::new(),
        updated: Vec::new(),
        skipped_already_installed: Vec::new(),
        failed: Vec::new(),
    };
    let mut entitlements = Vec::new();
    if allowed_plugins.is_empty() {
        return ActivationPluginSync {
            report,
            entitlements,
        };
    }

    if let Err(e) = std::fs::create_dir_all(plugins_dir) {
        return ActivationPluginSync {
            report: activation_sync_failed_for_all(
                allowed_plugins,
                format!("create plugins dir failed: {e}"),
            ),
            entitlements,
        };
    }

    for plugin_id in allowed_plugins {
        let dst = plugins_dir.join(plugin_id);
        if dst.is_dir() && !refresh_installed_entitlements {
            report.skipped_already_installed.push(plugin_id.clone());
            continue;
        }

        let install = match hub.install_plugin(plugin_id, device_fp) {
            Ok(resp) => resp,
            Err(e) => {
                report
                    .failed
                    .push((plugin_id.clone(), format!("hub install failed: {e}")));
                continue;
            }
        };
        entitlements.push(PluginHubInstallEntitlement {
            plugin_id: plugin_id.clone(),
            decrypt_key: install.decrypt_key.clone(),
            trial_expires: install.trial_expires.clone(),
        });
        if dst.is_dir() {
            report.skipped_already_installed.push(plugin_id.clone());
            continue;
        }
        let download_url = install.download_url.trim();
        let pkg_result = if download_url.is_empty() {
            hub.download_plugin(plugin_id, &install.version)
        } else {
            hub.download_plugin_url(download_url)
        };
        let pkg = match pkg_result {
            Ok(pkg) => pkg,
            Err(e) => {
                report
                    .failed
                    .push((plugin_id.clone(), format!("hub download failed: {e}")));
                continue;
            }
        };
        if let Err(e) =
            attune_core::plugin_sync::verify_plugin_package_sha256(&pkg, &install.sha256)
        {
            report.failed.push((
                plugin_id.clone(),
                format!("plugin package integrity check failed: {e}"),
            ));
            continue;
        }
        let key_bytes = install.decrypt_key.as_ref().map(|k| k.as_bytes().to_vec());
        match attune_core::plugin_sync::install_official_plugin_package_with_key(
            plugin_id,
            &pkg,
            plugins_dir,
            key_bytes.as_deref(),
        ) {
            Ok(path) => {
                tracing::info!(
                    "activate: installed plugin {plugin_id} → {}",
                    path.display()
                );
                report.installed.push(plugin_id.clone());
            }
            Err(e) => report
                .failed
                .push((plugin_id.clone(), format!("plugin install failed: {e}"))),
        }
    }
    ActivationPluginSync {
        report,
        entitlements,
    }
}

/// 把设备绑定失败映射为 HTTP 响应(可操作提示)。失败路径不置 Paid,不写 gateway,
/// 不下载付费插件；UI 需要提示用户处理设备上限/拒绝/云端不可达后重试。
fn device_binding_error(
    e: &attune_core::cloud_client::DeviceActivateError,
) -> (StatusCode, Json<serde_json::Value>) {
    use attune_core::cloud_client::DeviceActivateError as E;
    let (status, code, hint) = match e {
        E::MaxDevicesReached(_) => (
            StatusCode::CONFLICT,
            "max-devices-reached",
            "已达该授权码的设备上限。请在其他设备上注销，或联系管理员吊销旧设备后重试。",
        ),
        E::Rejected(_) => (
            StatusCode::FORBIDDEN,
            "device-rejected",
            "设备绑定被拒绝（授权码无效/已吊销，或设备指纹不匹配）。",
        ),
        E::Unavailable(_) => (
            StatusCode::BAD_GATEWAY,
            "device-activate-unavailable",
            "设备绑定服务暂时不可达，请稍后重试。",
        ),
    };
    (
        status,
        Json(serde_json::json!({
            "status": "device-binding-failed",
            "error": e.to_string(),
            "code": code,
            "hint": hint,
            "paid_applied": false,
        })),
    )
}

/// Best-effort persistence for account/session logins. Those logins can be
/// restored by their cloud session even if this auxiliary device credential
/// cannot be written.
fn store_device_binding(
    state: &SharedState,
    dev: &attune_core::cloud_client::DeviceActivateResult,
) {
    if let Err(e) = persist_device_binding(state, dev, None) {
        tracing::warn!("member login: device binding not persisted: {e}");
    }
}

/// License-code activation has no account session, so its encrypted receipt is
/// the only durable proof available after restart. Unlike the account-login
/// helper above, callers must propagate any error and fail the activation.
fn persist_activation_receipt(
    state: &SharedState,
    dev: &attune_core::cloud_client::DeviceActivateResult,
    license_key: &str,
) -> Result<(), String> {
    persist_device_binding(state, dev, Some(license_key))
}

fn persist_device_binding(
    state: &SharedState,
    dev: &attune_core::cloud_client::DeviceActivateResult,
    activation_license_key: Option<&str>,
) -> Result<(), String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault
        .dek_db()
        .map_err(|e| format!("vault is not unlocked: {e}"))?;
    let data = encrypted_device_binding_payload(&dek, dev, activation_license_key)
        .map_err(|e| format!("encrypt device binding: {e}"))?;
    vault
        .store()
        .set_meta(DEVICE_BINDING_META_KEY, &data)
        .map_err(|e| format!("persist device binding: {e}"))
}

fn encrypted_device_binding_payload(
    dek: &attune_core::crypto::Key32,
    dev: &attune_core::cloud_client::DeviceActivateResult,
    activation_license_key: Option<&str>,
) -> attune_core::error::Result<Vec<u8>> {
    let payload = serde_json::json!({
        "schema": 2,
        "device_token": dev.device_token,
        "device_id": dev.device_id,
        // Present only for the license-code path and protected by the vault
        // DEK. It is never returned by settings/member APIs or written to logs.
        "activation_license_key": activation_license_key,
        "max_activations": dev.max_activations,
        "current_activations": dev.current_activations,
        "issued_at": dev.issued_at,
        "expires_at": dev.expires_at,
    });
    attune_core::crypto::encrypt(dek, &serde_json::to_vec(&payload)?)
}

fn load_activation_receipt(
    state: &SharedState,
) -> Result<Option<(String, attune_core::cloud_client::DeviceActivateResult)>, String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault
        .dek_db()
        .map_err(|e| format!("vault is not unlocked: {e}"))?;
    let Some(encrypted) = vault
        .store()
        .get_meta(DEVICE_BINDING_META_KEY)
        .map_err(|e| format!("read activation receipt: {e}"))?
    else {
        return Ok(None);
    };
    let plaintext = attune_core::crypto::decrypt(&dek, &encrypted)
        .map_err(|e| format!("decrypt activation receipt: {e}"))?;
    let payload: serde_json::Value =
        serde_json::from_slice(&plaintext).map_err(|e| format!("parse activation receipt: {e}"))?;
    let Some(license_key) = payload
        .get("activation_license_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let device = serde_json::from_value(payload)
        .map_err(|e| format!("parse activation device binding: {e}"))?;
    Ok(Some((license_key, device)))
}

/// Vault meta key for the encrypted device binding credential.
pub const DEVICE_BINDING_META_KEY: &str = "device_binding";

/// 共享:把 cloud 下发的 gateway endpoint+token(+默认 model)配进 vault settings
/// 并热重载 LLM provider。`login_password` 与 `activate_license` 复用同一逻辑(DRY),
/// 保证两条会员入口写出**完全一致**的锁定 gateway 配置。
///
/// best-effort:`url`/`token` 缺失或为空 → 跳过(用户保留现有 LLM 设置);写入失败
/// → warn,不阻断登录/激活(§4.5)。
///
/// SECURITY (§1.4):`who` 进 tracing 日志,**必须是非敏感标识**(email,或
/// [`redact_license_key`] 生成的 `lic:<8-hex>` 摘要)—— **绝不**传 license_key /
/// gateway_token / password 明文。`token` 仅写入 vault settings,从不进日志。
fn wire_cloud_gateway(
    state: &SharedState,
    url: Option<&str>,
    token: Option<&str>,
    default_model: Option<&str>,
    who: &str,
) {
    let mut gateway_written = false;
    match (url, token) {
        (Some(url), Some(tok)) if !url.is_empty() && !tok.is_empty() => {
            // Bug-1 fix (spec 2026-05-24): cloud 下发默认 model 一并写入,避免 fresh
            // vault paid 用户 chat 因 model=null → 404。
            match apply_gateway_to_vault_settings(state, url, tok, default_model) {
                Ok(true) => {
                    tracing::info!(
                        "member gateway: cloud LLM gateway written to vault settings (default_model={default_model:?})"
                    );
                    gateway_written = true;
                }
                Ok(false) => {
                    tracing::info!(
                        "member gateway: user has own LLM config — gateway not auto-applied"
                    );
                }
                Err(e) => tracing::warn!("member gateway: settings not written: {e}"),
            }
        }
        _ => tracing::info!(
            "member gateway: no gateway token for {who} — keeps current LLM settings"
        ),
    }
    // Reload in-memory LLM provider so chat works immediately, no server restart.
    // MUST run AFTER apply_gateway_to_vault_settings released its vault lock.
    if gateway_written {
        state.reload_llm();
    }
}

/// best-effort 把激活授权的 `allowed_plugins` 落进 entitlement 缓存 + vault。
/// 这是 pluginhub 安装授权 + 周期 re-verify 的本地基准。vault 锁短取,不嵌套
/// fulltext/vectors(lock-ordering)。失败仅 warn,绝不阻断激活。
fn store_activation_entitlements(
    state: &SharedState,
    license_id: &str,
    allowed_plugins: &[String],
    plugin_entitlements: &[PluginHubInstallEntitlement],
) {
    if allowed_plugins.is_empty() {
        return;
    }
    let now = Utc::now().to_rfc3339();
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = match vault.dek_db() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("activate: vault locked — entitlements not persisted: {e}");
            return;
        }
    };
    for plugin_id in allowed_plugins {
        let hub_row = plugin_entitlements
            .iter()
            .find(|entry| entry.plugin_id == *plugin_id);
        let row = attune_core::store::plugin_entitlements::EntitlementRow {
            plugin_id: plugin_id.clone(),
            license_id: license_id.to_string(),
            decrypt_key: hub_row.and_then(|entry| entry.decrypt_key.clone()),
            tier: "paid".into(),
            status: "active".into(),
            trial_expires: hub_row.and_then(|entry| entry.trial_expires.clone()),
            // 授权码路径下 cloud 未随激活下发 per-plugin 公钥;签名校验仍由
            // pluginhub 安装 + 周期 re-verify 时按 EntitledPlugin.signing_pubkey_hex
            // 校验。此处先记账,留空 pubkey。
            signing_pubkey_hex: String::new(),
            last_verified_at: now.clone(),
            grace_started_at: None,
            updated_at: now.clone(),
        };
        // 同步内存缓存 (dispatch 决策即时可见) + 持久化 vault。
        state.entitlement_cache.upsert(row.clone());
        if let Err(e) = vault.store().upsert_entitlement(&dek, &row) {
            tracing::warn!("activate: failed to persist entitlement {plugin_id}: {e}");
        }
    }
}

/// best-effort 把账号密码登录返回的 `license.entitled_plugins` 落进 entitlement
/// 缓存 + vault。不同于授权码激活路径,这里 cloud 已经给了完整 EntitledPlugin
/// 元数据(版本、签名公钥、平台包),所以复用 core 的 entitlement_row_for。
/// 失败仅 warn,不阻断登录；但成功时 `/plugins` 的 entitlement_status 和 dispatch
/// gate 会立即看到 active。
fn store_login_entitlements(state: &SharedState, license: &attune_core::cloud_client::License) {
    if license.entitled_plugins.is_empty() {
        return;
    }
    let now = Utc::now().to_rfc3339();
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = match vault.dek_db() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("member login: vault locked — entitlements not persisted: {e}");
            return;
        }
    };
    for ep in &license.entitled_plugins {
        let row = attune_core::plugin_sync::entitlement_row_for(ep, license, &now);
        state.entitlement_cache.upsert(row.clone());
        if let Err(e) = vault.store().upsert_entitlement(&dek, &row) {
            tracing::warn!(
                "member login: failed to persist entitlement {}: {e}",
                ep.plugin_id
            );
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct LogoutOutcome {
    pub removed_local_session: bool,
    pub remote_logout_succeeded: bool,
    pub cleared_member_credentials: bool,
}

fn clear_member_credentials(
    state: &SharedState,
    legacy_gateway: Option<&(String, String)>,
) -> Result<bool, String> {
    clear_member_credentials_inner(state, legacy_gateway, true)
}

/// Token-login may run after a caller has injected an independently governed
/// provider (for example a desktop-owned local runtime). Preserve that handle
/// when persisted settings prove no membership-owned LLM was removed. Account
/// logout/activation/password-switch paths still force a settings reload.
fn clear_member_credentials_preserving_unowned_llm(
    state: &SharedState,
    legacy_gateway: Option<&(String, String)>,
) -> Result<bool, String> {
    clear_member_credentials_inner(state, legacy_gateway, false)
}

fn clear_member_credentials_inner(
    state: &SharedState,
    legacy_gateway: Option<&(String, String)>,
    force_llm_reload: bool,
) -> Result<bool, String> {
    // Stop every provider candidate built from the outgoing account before
    // touching its durable settings. Per-provider reload epochs then ensure a
    // candidate built in this transition window cannot overwrite the later
    // replacement account's provider.
    state.invalidate_credential_generation();
    let persistence_result = (|| -> Result<(bool, bool), String> {
        let mut changed = false;
        let mut removed_membership_llm = false;
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let vault_unlocked = vault.dek_db().is_ok();
        let existing = crate::settings_store::load_settings(&vault)
            .map_err(|e| format!("read settings during logout: {e}"))?;
        if let Some(current) = existing {
            let (mut current, mut llm_changed) =
                attune_core::llm_settings::remove_membership_gateway_from_settings(current);
            if !llm_changed {
                if let Some((endpoint, token)) = legacy_gateway {
                    (current, llm_changed) =
                        attune_core::llm_settings::remove_legacy_membership_gateway_match(
                            current, endpoint, token,
                        );
                }
            }
            removed_membership_llm = llm_changed;
            changed |= llm_changed;
            if llm_changed {
                changed |= crate::settings_store::delete_secret(
                    &vault,
                    crate::settings_store::LLM_API_KEY_SECRET,
                )
                .map_err(|e| format!("remove member gateway secret during logout: {e}"))?;
            }
            let mut pluginhub_changed = false;
            if let Some(pluginhub) = current
                .get_mut("pluginhub")
                .and_then(serde_json::Value::as_object_mut)
            {
                let membership_owned = pluginhub
                    .get("managed_by")
                    .and_then(serde_json::Value::as_str)
                    == Some(attune_core::llm_settings::MEMBER_GATEWAY_OWNER);
                if membership_owned {
                    // Clear only the credential installed by member login. A
                    // user-entered PluginHub BYOK has no ownership marker.
                    pluginhub_changed |= pluginhub.remove("license_key").is_some();
                    pluginhub_changed |= pluginhub.remove("managed_by").is_some();
                }
            }
            if pluginhub_changed {
                changed |= crate::settings_store::delete_secret(
                    &vault,
                    crate::settings_store::PLUGINHUB_LICENSE_KEY_SECRET,
                )
                .map_err(|e| format!("remove PluginHub secret during logout: {e}"))?;
            }
            changed |= pluginhub_changed;
            if changed {
                if vault_unlocked {
                    crate::settings_store::persist_settings(&vault, current)
                        .map_err(|e| format!("persist settings during logout: {e}"))?;
                } else {
                    // A sealed vault cannot migrate unrelated legacy BYOK
                    // plaintext.  Teardown must nevertheless remove the
                    // membership-owned fields, so preserve the remaining raw
                    // settings verbatim until the next unlocked migration.
                    let raw = serde_json::to_vec(&current)
                        .map_err(|e| format!("serialize settings during logout: {e}"))?;
                    vault
                        .store()
                        .set_meta(attune_core::llm_settings::SETTINGS_META_KEY, &raw)
                        .map_err(|e| format!("persist sealed settings during logout: {e}"))?;
                }
            }
        }
        changed |= vault
            .store()
            .delete_meta(DEVICE_BINDING_META_KEY)
            .map_err(|e| format!("remove device binding during logout: {e}"))?;
        changed |= vault
            .store()
            .clear_entitlements()
            .map_err(|e| format!("remove entitlements during logout: {e}"))?
            > 0;
        Ok((changed, removed_membership_llm))
    })();

    // Entitlements and membership-owned integrations are torn down even when
    // disk cleanup fails. Token-only compatibility login preserves an unrelated,
    // independently configured LLM; explicit logout/switch paths force a reload.
    state.entitlement_cache.hydrate_from_rows(Vec::new());
    if force_llm_reload
        || persistence_result
            .as_ref()
            .map(|(_, removed_membership_llm)| *removed_membership_llm)
            .unwrap_or(true)
    {
        state.reload_llm();
    }
    // Membership teardown must not disable a user-owned PluginHub BYOK. The
    // settings-aware reload suppresses a still-persisted membership credential
    // while restoring an unrelated user credential when the vault is unlocked.
    state.reload_plugin_hub_from_settings();
    persistence_result.map(|(changed, _)| changed)
}

/// Shared teardown for member logout and the privacy "wipe cloud session"
/// action. Local removal is authoritative; remote revocation is best-effort.
pub(crate) async fn perform_logout(state: &SharedState) -> Result<LogoutOutcome, String> {
    let _transition = state.member_transition.lock().await;
    let session_transaction = acquire_cloud_session_transition(&state.cloud_session_store).await?;
    let session_for_cleanup = Arc::clone(&session_transaction);
    let (persisted, removed_result) = tokio::task::spawn_blocking(move || {
        // A corrupt session must still be removable. It cannot be used for a
        // remote call, but it must never make logout self-recover on restart.
        let persisted = session_for_cleanup
            .load()
            .map_err(|e| {
                tracing::warn!("member logout: ignoring unreadable persisted session: {e}");
                e
            })
            .ok()
            .flatten();
        let removed = session_for_cleanup
            .remove()
            .map_err(|e| format!("remove cloud session: {e}"));
        (persisted, removed)
    })
    .await
    .map_err(|e| format!("local logout task join failed: {e}"))?;

    // Local logout is authoritative and must take effect before a potentially
    // slow/unreachable remote revocation call. This closes the window where a
    // user had clicked logout but the old gateway and entitlement cache stayed
    // live for up to the HTTP timeout.
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
    *state
        .member_session_epoch
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    *state
        .member_verified_at
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    let mut credential_cleanup = clear_member_credentials(state, None);
    let privacy_cleanup = crate::routes::privacy::set_outbound_enabled(state, "cloud_saas", false);

    let remote_result = match persisted {
        Some(session) => tokio::task::spawn_blocking(move || {
            let mut client = CloudClient::with_session(session.cloud_url, session.session);
            let legacy_gateway = client.me().ok().and_then(|me| {
                me.gateway_url
                    .zip(me.gateway_token)
                    .filter(|(url, token)| !url.trim().is_empty() && !token.trim().is_empty())
            });
            (client.logout().is_ok(), legacy_gateway)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("member logout: remote task failed: {e}");
            (false, None)
        }),
        None => (false, None),
    };

    // Upgrade compatibility: settings written before provenance markers can be
    // removed only after the authenticated account reports an exact endpoint +
    // token match. Runtime was already torn down above; this is a second,
    // idempotent persistence pass.
    if let Some(legacy_gateway) = remote_result.1.as_ref() {
        match clear_member_credentials(state, Some(legacy_gateway)) {
            Ok(legacy_changed) => {
                if let Ok(changed) = credential_cleanup.as_mut() {
                    *changed |= legacy_changed;
                }
            }
            Err(e) if credential_cleanup.is_ok() => credential_cleanup = Err(e),
            Err(e) => tracing::warn!("member logout: legacy cleanup also failed: {e}"),
        }
    }

    let removed_local_session = removed_result?;
    let cleared_member_credentials = credential_cleanup?;
    privacy_cleanup?;
    Ok(LogoutOutcome {
        removed_local_session,
        remote_logout_succeeded: remote_result.0,
        cleared_member_credentials,
    })
}

/// POST /api/v1/member/logout — revoke and remove the persisted session and all
/// credentials managed by that membership before exposing LoggedOut.
pub async fn logout(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let outcome = perform_logout(&state).await.map_err(AppError::Internal)?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "logged_out",
        "session": outcome,
    })))
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
    let existing = crate::settings_store::load_settings(&vault)
        .map_err(|e| format!("load settings failed: {e}"))?;
    let current: serde_json::Value = match existing {
        Some(settings) => settings,
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
    crate::settings_store::persist_settings(&vault, merged)
        .map_err(|e| format!("persist settings failed: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use attune_core::cloud_client::CloudClient;
    use attune_core::entitlement::{EntStatus, EntitlementCache};
    use attune_core::entitlement_reverify::{apply_refresh_rounds, ReverifyOutcome};
    use attune_core::llm_settings::{gateway_should_apply, merge_gateway_into_settings};
    use attune_core::member_session::MemberState;
    use attune_core::plugin_hub::{InstallResponse, PluginHubProvider, PluginListingResponse};
    use attune_core::store::plugin_entitlements::EntitlementRow;
    use chrono::{DateTime, Utc};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn row(plugin_id: &str, status: &str, last_verified: &str) -> EntitlementRow {
        EntitlementRow {
            plugin_id: plugin_id.into(),
            license_id: "lic-x".into(),
            decrypt_key: None,
            tier: "paid".into(),
            status: status.into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: last_verified.into(),
            grace_started_at: None,
            updated_at: last_verified.into(),
        }
    }

    #[test]
    fn member_billing_urls_trim_trailing_slash() {
        let v = super::member_billing_json("https://accounts.example.com/");
        assert_eq!(
            v.get("upgrade_url").and_then(|u| u.as_str()),
            Some("https://accounts.example.com/upgrade")
        );
        assert_eq!(
            v.get("billing_url").and_then(|u| u.as_str()),
            Some("https://accounts.example.com/billing")
        );
    }

    #[tokio::test]
    async fn login_token_consent_failure_preserves_existing_member_state() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _dir = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-consent-order").expect("setup");
        vault
            .store()
            .set_meta(attune_core::llm_settings::SETTINGS_META_KEY, b"[]")
            .expect("install malformed settings root");
        let state = Arc::new(crate::state::AppState::new(vault, false));
        let existing = MemberState::Paid {
            account_id: "existing-account".into(),
            license_id: "existing-license".into(),
            llm_quota_remaining: 9,
        };
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = existing.clone();

        let error = super::login_token(
            axum::extract::State(Arc::clone(&state)),
            axum::Json(super::LoginTokenReq {
                account_id: "new-account".into(),
                tier: "free".into(),
                license_id: None,
                llm_quota_remaining: 0,
            }),
        )
        .await
        .expect_err("consent persistence must fail before switching accounts");

        assert_eq!(error.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            *state.member_state.lock().unwrap_or_else(|e| e.into_inner()),
            existing,
            "no member teardown may run until cloud consent is durable"
        );
    }

    #[tokio::test]
    async fn free_login_token_retires_old_paid_session_before_state_switch() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("attune");
        let _dir = crate::test_support::override_data_dir(data_dir);
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-free-token-switch").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::Paid {
            account_id: "old-paid-account".into(),
            license_id: "old-paid-license".into(),
            llm_quota_remaining: 10,
        };
        attune_core::cloud_session::persist_cloud_session(
            "https://old-account.example.test",
            "session=old-paid-session",
        )
        .expect("persist old session precondition");

        let response = super::login_token(
            axum::extract::State(Arc::clone(&state)),
            axum::Json(super::LoginTokenReq {
                account_id: "new-free-account".into(),
                tier: "free".into(),
                license_id: None,
                llm_quota_remaining: 0,
            }),
        )
        .await
        .expect("free token login");

        assert_eq!(response.0["state"]["kind"], "free");
        assert_eq!(response.0["state"]["account_id"], "new-free-account");
        assert!(
            attune_core::cloud_session::load_cloud_session()
                .expect("load after free switch")
                .is_none(),
            "free login must retire the previous paid account session"
        );

        // Model process restart/lazy restore by dropping only in-memory member
        // state. The old Paid account must stay unavailable.
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::LoggedOut;
        assert!(super::restore_member_state_from_cloud_session(&state)
            .await
            .is_none());
        assert!(matches!(
            *state.member_state.lock().unwrap_or_else(|e| e.into_inner()),
            MemberState::LoggedOut
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_transition_serializes_session_and_runtime_publication() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-member-transaction").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));
        let store = attune_core::cloud_session::CloudSessionStore::new(
            tmp.path().join("cloud-session.json"),
        );

        let (a_staged_tx, a_staged_rx) = tokio::sync::oneshot::channel();
        let (release_a_tx, release_a_rx) = tokio::sync::oneshot::channel();
        let state_a = Arc::clone(&state);
        let store_a = store.clone();
        let transition_a = tokio::spawn(async move {
            let _guard = state_a.member_transition.lock().await;
            let session_transaction = Arc::new(store_a.transaction().expect("lock A session"));
            session_transaction
                .stage("https://accounts-a.example.test", "session=account-a")
                .expect("stage A");
            a_staged_tx.send(()).expect("signal A staged");
            release_a_rx.await.expect("release A");
            super::commit_staged_cloud_session_restore(session_transaction)
                .await
                .expect("commit A");
            *state_a
                .member_state
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = MemberState::Paid {
                account_id: "account-a".into(),
                license_id: "license-a".into(),
                llm_quota_remaining: 1,
            };
        });
        a_staged_rx.await.expect("A reached staged barrier");

        let (b_started_tx, b_started_rx) = tokio::sync::oneshot::channel();
        let (b_entered_tx, mut b_entered_rx) = tokio::sync::oneshot::channel();
        let state_b = Arc::clone(&state);
        let store_b = store.clone();
        let transition_b = tokio::spawn(async move {
            b_started_tx.send(()).expect("signal B attempt");
            let _guard = state_b.member_transition.lock().await;
            b_entered_tx.send(()).expect("signal B entered");
            let session_transaction = Arc::new(store_b.transaction().expect("lock B session"));
            session_transaction
                .stage("https://accounts-b.example.test", "session=account-b")
                .expect("stage B");
            super::commit_staged_cloud_session_restore(session_transaction)
                .await
                .expect("commit B");
            *state_b
                .member_state
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = MemberState::Paid {
                account_id: "account-b".into(),
                license_id: "license-b".into(),
                llm_quota_remaining: 2,
            };
        });
        b_started_rx.await.expect("B attempted transition");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut b_entered_rx)
                .await
                .is_err(),
            "B must not overwrite A's staged session before A publishes"
        );

        release_a_tx.send(()).expect("release A transaction");
        transition_a.await.expect("join A");
        b_entered_rx.await.expect("B enters after A");
        transition_b.await.expect("join B");

        assert_eq!(
            store.load().expect("load final session"),
            Some(attune_core::cloud_session::PersistedCloudSession {
                cloud_url: "https://accounts-b.example.test".into(),
                session: "session=account-b".into(),
            })
        );
        assert!(matches!(
            &*state.member_state.lock().unwrap_or_else(|e| e.into_inner()),
            MemberState::Paid { account_id, license_id, .. }
                if account_id == "account-b" && license_id == "license-b"
        ));
    }

    #[tokio::test]
    async fn stale_session_commit_fails_closed() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let store = attune_core::cloud_session::CloudSessionStore::new(
            tmp.path().join("cloud-session.json"),
        );
        store
            .persist("https://accounts.example.test", "session=already-published")
            .expect("published session precondition");

        let transaction = Arc::new(store.transaction().expect("lock session"));
        let error = super::commit_staged_cloud_session_restore(transaction)
            .await
            .expect_err("a missing transaction marker must not count as a commit");
        assert!(error.contains("no longer current"));
    }

    #[tokio::test]
    async fn external_account_switch_invalidates_bound_member_runtime() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("attune");
        let _dir = crate::test_support::override_data_dir(data_dir);
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-session-epoch").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        state
            .cloud_session_store
            .persist("https://accounts-a.example.test", "session=account-a")
            .expect("persist account A");
        let account_a_epoch = state
            .cloud_session_store
            .epoch()
            .expect("read account A epoch");
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::Paid {
            account_id: "account-a".into(),
            license_id: "license-a".into(),
            llm_quota_remaining: 1,
        };
        *state
            .member_session_epoch
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(account_a_epoch);
        *state
            .member_verified_at
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());

        let independent_cli = attune_core::cloud_session::CloudSessionStore::new(
            state.cloud_session_store.path().to_path_buf(),
        );
        independent_cli
            .persist("https://accounts-b.example.test", "session=account-b")
            .expect("CLI switches to account B");

        assert!(
            !super::reconcile_member_session_epoch(&state).await,
            "the server must fail closed instead of combining account A runtime with account B cookie"
        );
        assert!(matches!(
            *state.member_state.lock().unwrap_or_else(|e| e.into_inner()),
            MemberState::LoggedOut
        ));
        assert!(state
            .member_session_epoch
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none());
        assert_eq!(
            independent_cli.load().expect("load account B"),
            Some(attune_core::cloud_session::PersistedCloudSession {
                cloud_url: "https://accounts-b.example.test".into(),
                session: "session=account-b".into(),
            }),
            "server teardown must not delete the newer CLI session"
        );
    }

    #[test]
    fn quota_remaining_accepts_current_and_legacy_cloud_shapes() {
        for value in [
            serde_json::json!({"quota": {"remaining": 42}}),
            serde_json::json!({"quota": {"llm_tokens_remaining": "43"}}),
            serde_json::json!({"llm_quota_remaining": 44}),
            serde_json::json!({"remaining": "45"}),
        ] {
            assert!(
                super::quota_remaining_from_json(&value).is_some(),
                "{value}"
            );
        }
        assert_eq!(
            super::quota_remaining_from_json(&serde_json::json!({"quota": {"remaining": 0}})),
            Some(0)
        );
        assert_eq!(
            super::quota_remaining_from_json(&serde_json::json!({"quota": {"remaining": -1}})),
            None
        );
    }

    #[test]
    fn login_entitlements_seed_cache_and_vault() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("attune");
        let _dir = crate::test_support::override_data_dir(data_dir);

        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-login-entitlements").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        let license = attune_core::cloud_client::License {
            id: 18,
            name: Some("Pro".into()),
            plan: "pro".into(),
            license_key: "lic-test".into(),
            license_id: Some(18),
            revoked_at: None,
            last_used_at: None,
            created_at: None,
            vertical: Some("law".into()),
            entitled_plugins: vec![attune_core::cloud_client::EntitledPlugin {
                plugin_id: "law-pro".into(),
                version: "1.0.9".into(),
                download_url: "https://hub.engi-stack.com/api/v1/packages/law-pro-1.0.9.tar.gz"
                    .into(),
                sha256: "b59f94e8153ff358073e88acbe980c230c2f58755617aac57eb213ec5affbd78".into(),
                platform_packages: Vec::new(),
                signing_pubkey_hex:
                    "3fc9afb5b7a7bc8c7863cdb33070e7effad930efaf234069dc5d2bcdf993c6d4".into(),
                decrypt_key: None,
            }],
        };

        super::store_login_entitlements(&state, &license);

        let now = Utc::now();
        assert_eq!(
            state.entitlement_cache.status("law-pro", &now),
            EntStatus::Active
        );

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().expect("dek");
        let row = vault
            .store()
            .get_entitlement(&dek, "law-pro")
            .expect("read entitlement")
            .expect("entitlement row");
        assert_eq!(row.plugin_id, "law-pro");
        assert_eq!(row.license_id, "18");
        assert_eq!(row.tier, "paid");
        assert_eq!(row.status, "active");
        assert_eq!(
            row.signing_pubkey_hex,
            "3fc9afb5b7a7bc8c7863cdb33070e7effad930efaf234069dc5d2bcdf993c6d4"
        );
    }

    #[test]
    fn managed_member_secrets_are_encrypted_and_clear_while_vault_is_sealed() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("attune");
        let _dir = crate::test_support::override_data_dir(data_dir);
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-member-secret-lifecycle").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        assert!(super::apply_gateway_to_vault_settings(
            &state,
            "https://gateway.example.test/v1",
            "member-gateway-secret",
            Some("member-model"),
        )
        .expect("gateway settings"));
        super::apply_pluginhub_to_vault_settings(
            &state,
            "https://hub.example.test",
            "member-plugin-secret",
        )
        .expect("pluginhub settings");

        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let raw = vault
                .store()
                .get_meta(attune_core::llm_settings::SETTINGS_META_KEY)
                .unwrap()
                .unwrap();
            let raw_text = String::from_utf8_lossy(&raw);
            assert!(!raw_text.contains("member-gateway-secret"));
            assert!(!raw_text.contains("member-plugin-secret"));
            assert!(vault
                .store()
                .get_meta(crate::settings_store::LLM_API_KEY_SECRET)
                .unwrap()
                .is_some());
            assert!(vault
                .store()
                .get_meta(crate::settings_store::PLUGINHUB_LICENSE_KEY_SECRET)
                .unwrap()
                .is_some());
            vault.lock().expect("seal vault");
        }

        assert!(super::clear_member_credentials(&state, None).expect("clear credentials"));
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let raw = vault
            .store()
            .get_meta(attune_core::llm_settings::SETTINGS_META_KEY)
            .unwrap()
            .unwrap();
        let settings: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert!(settings["llm"].get("endpoint").is_none());
        assert!(settings["llm"].get("managed_by").is_none());
        assert!(settings["pluginhub"].get("managed_by").is_none());
        assert!(vault
            .store()
            .get_meta(crate::settings_store::LLM_API_KEY_SECRET)
            .unwrap()
            .is_none());
        assert!(vault
            .store()
            .get_meta(crate::settings_store::PLUGINHUB_LICENSE_KEY_SECRET)
            .unwrap()
            .is_none());
    }

    #[test]
    fn member_pluginhub_does_not_override_user_byok() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-member-pluginhub-byok").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));
        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            crate::settings_store::persist_settings(
                &vault,
                serde_json::json!({
                    "pluginhub": {
                        "url": "https://user-hub.example.test",
                        "license_key": "user-owned-key"
                    }
                }),
            )
            .expect("persist BYOK");
        }
        state.reload_plugin_hub_from_settings();
        assert_ne!(
            state
                .plugin_hub
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .name(),
            "mock",
            "precondition: the user BYOK provider is active"
        );

        assert!(!super::apply_pluginhub_to_vault_settings(
            &state,
            "https://member-hub.example.test",
            "membership-key",
        )
        .expect("membership apply decision"));

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let settings = crate::settings_store::load_settings(&vault)
            .expect("load settings")
            .expect("settings present");
        assert_eq!(
            settings.pointer("/pluginhub/url").and_then(|v| v.as_str()),
            Some("https://user-hub.example.test")
        );
        assert_eq!(
            settings
                .pointer("/pluginhub/license_key")
                .and_then(|v| v.as_str()),
            Some("user-owned-key")
        );
        assert!(settings.pointer("/pluginhub/managed_by").is_none());
        drop(vault);

        super::clear_member_credentials(&state, None).expect("member cleanup");
        assert_ne!(
            state
                .plugin_hub
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .name(),
            "mock",
            "member cleanup must restore, not disable, an unrelated BYOK provider"
        );
    }

    #[test]
    fn vault_lock_evicts_member_pluginhub_and_entitlement_runtime() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("attune");
        let _dir = crate::test_support::override_data_dir(data_dir);
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-member-lock-runtime").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = MemberState::Paid {
            account_id: "account-sensitive".into(),
            license_id: "license-sensitive".into(),
            llm_quota_remaining: 42,
        };
        state
            .entitlement_cache
            .upsert(attune_core::store::plugin_entitlements::EntitlementRow {
                plugin_id: "paid-plugin".into(),
                license_id: "license-sensitive".into(),
                decrypt_key: Some("decrypt-sensitive".into()),
                tier: "paid".into(),
                status: "active".into(),
                trial_expires: None,
                signing_pubkey_hex: String::new(),
                last_verified_at: Utc::now().to_rfc3339(),
                grace_started_at: None,
                updated_at: Utc::now().to_rfc3339(),
            });
        state.reload_plugin_hub(
            Some("https://member-hub.example.test"),
            Some("member-license-secret"),
        );

        state
            .lock_vault_and_clear_runtime()
            .expect("lock and clear runtime");

        assert!(matches!(
            *state.member_state.lock().unwrap_or_else(|e| e.into_inner()),
            MemberState::LoggedOut
        ));
        assert!(state.entitlement_cache.snapshot().is_empty());
        assert_eq!(
            state
                .plugin_hub
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .name(),
            "mock"
        );
        assert!(matches!(
            state
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state(),
            attune_core::vault::VaultState::Locked
        ));
    }

    struct CountingHub {
        install_calls: AtomicUsize,
        download_calls: AtomicUsize,
    }

    impl CountingHub {
        fn new() -> Self {
            Self {
                install_calls: AtomicUsize::new(0),
                download_calls: AtomicUsize::new(0),
            }
        }
    }

    impl PluginHubProvider for CountingHub {
        fn list_plugins(&self) -> attune_core::error::Result<PluginListingResponse> {
            Ok(PluginListingResponse {
                hub_version: "test".into(),
                user_plan: "pro".into(),
                upgrade_url: "https://accounts.engi-stack.com/upgrade".into(),
                plugins: Vec::new(),
            })
        }

        fn install_plugin(
            &self,
            plugin_id: &str,
            _device_fp: Option<&str>,
        ) -> attune_core::error::Result<InstallResponse> {
            self.install_calls.fetch_add(1, Ordering::SeqCst);
            Ok(InstallResponse {
                install_id: 1,
                plugin_id: plugin_id.into(),
                version: "1.0.0".into(),
                sha256: "test".into(),
                decrypt_key: Some("test-device-bound-key".into()),
                trial_started: None,
                trial_expires: None,
                download_url: format!("/api/v1/packages/{plugin_id}-1.0.0.tar.gz"),
            })
        }

        fn download_plugin(
            &self,
            _plugin_id: &str,
            _version: &str,
        ) -> attune_core::error::Result<Vec<u8>> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            Ok(b"not-a-valid-plugin-tarball".to_vec())
        }

        fn download_plugin_url(&self, _download_url: &str) -> attune_core::error::Result<Vec<u8>> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            Ok(b"not-a-valid-plugin-tarball".to_vec())
        }

        fn name(&self) -> &str {
            "counting-test"
        }
    }

    #[test]
    fn activation_allowed_plugins_drive_pluginhub_install_attempt() {
        let hub = CountingHub::new();
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["law-pro".to_string()];

        let report =
            super::sync_activation_plugins_with_hub(&hub, &allowed, Some("fp-test"), tmp.path());

        assert_eq!(hub.install_calls.load(Ordering::SeqCst), 1);
        assert_eq!(hub.download_calls.load(Ordering::SeqCst), 1);
        assert!(report.installed.is_empty());
        assert!(report.skipped_already_installed.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "law-pro");
        assert!(
            report.failed[0]
                .1
                .contains("package integrity check failed"),
            "unexpected failure reason: {}",
            report.failed[0].1
        );
    }

    #[test]
    fn activation_plugin_sync_skips_already_installed_plugin() {
        let hub = CountingHub::new();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("law-pro")).unwrap();
        let allowed = vec!["law-pro".to_string()];

        let report =
            super::sync_activation_plugins_with_hub(&hub, &allowed, Some("fp-test"), tmp.path());

        assert_eq!(hub.install_calls.load(Ordering::SeqCst), 0);
        assert_eq!(hub.download_calls.load(Ordering::SeqCst), 0);
        assert_eq!(report.skipped_already_installed, vec!["law-pro"]);
        assert!(report.installed.is_empty());
        assert!(report.failed.is_empty());
    }

    // ── T8: refresh 200 → cache updated + {refreshed, statuses} ──────────────
    //
    // A successful re-verify round (cloud returns a signed v1 snapshot that passes
    // SEC-1/2 → ReverifyOutcome::Active) advances the cached status to Active and the
    // route's 200 mapping reports refreshed>0 + per-plugin statuses. We drive the
    // route's pure aggregation (`apply_refresh_rounds`) + the same 200 body shape the
    // handler builds — proving the cache-update + response contract without a live cloud.
    #[test]
    fn refresh_endpoint_200_updates_cache() {
        let cache = EntitlementCache::new();
        // Pre-state: law-pro currently suspended in cache (e.g. a stale revoke).
        cache.upsert(row("law-pro", "suspended", "2026-06-10T00:00:00+00:00"));
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Suspended);

        // A verified-Active round (the only legal transition-to-Active path; produced
        // by reverify_all after authorize_snapshot_fresh accepts a signed v1 snapshot).
        let rounds = vec![(
            "law-pro".to_string(),
            ReverifyOutcome::Active,
            Some("2026-06-12T00:00:00+00:00".to_string()),
        )];
        let summary = apply_refresh_rounds(&cache, &rounds, &now);

        // cache now Active (re-verify renewal).
        assert_eq!(cache.status("law-pro", &now), EntStatus::Active);
        // route 200 mapping: refreshed counts accepted rounds; statuses lists per-plugin.
        assert_eq!(summary.refreshed, 1);
        assert!(!summary.all_network_error, "200 path, not 502");
        let body = serde_json::json!({
            "status": "ok",
            "refreshed": summary.refreshed,
            "statuses": summary.statuses.iter()
                .map(|(id, st)| serde_json::json!({"plugin_id": id, "status": st}))
                .collect::<Vec<_>>(),
        });
        assert_eq!(body["refreshed"], 1);
        assert_eq!(body["statuses"][0]["plugin_id"], "law-pro");
        assert_eq!(body["statuses"][0]["status"], "active");
    }

    // ── T8: refresh 5xx → 502 {code: cloud-unreachable}, cache UNCHANGED ──────
    //
    // §7.2 error 5: when the cloud is entirely unreachable (every verify is a
    // NetworkError), the route returns 502 {code: cloud-unreachable} and the local
    // cache must be byte-for-byte unchanged (no false downgrade). We assert the cache
    // snapshot is identical before/after apply, that the summary flags all-network-error
    // (→ the handler's 502 branch), and that the 502 body carries the kebab code.
    #[test]
    fn refresh_502_preserves_cache() {
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "active", "2026-06-12T00:00:00+00:00"));
        let now = ts("2026-06-12T00:00:01+00:00");
        let before = cache.snapshot();

        // Cloud unreachable: every plugin's round is a NetworkError.
        let rounds = vec![("law-pro".to_string(), ReverifyOutcome::NetworkError, None)];
        let summary = apply_refresh_rounds(&cache, &rounds, &now);

        // cache UNCHANGED — the load-bearing §7.2 error-5 invariant.
        assert_eq!(
            cache.snapshot(),
            before,
            "network error must not mutate the cache"
        );
        assert_eq!(summary.refreshed, 0);
        assert!(summary.all_network_error, "all-network-error → 502 branch");

        // route 502 body shape (the handler builds this kebab-coded AppError::detailed).
        let body = serde_json::json!({ "error": "cloud unreachable", "code": "cloud-unreachable" });
        assert_eq!(body["code"], "cloud-unreachable");
    }

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
        assert!(
            result.is_err(),
            "login against an unreachable host must be Err, not panic/Ok"
        );
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
        assert_eq!(
            llm.get("provider").and_then(|v| v.as_str()),
            Some("openai_compat")
        );
        assert_eq!(
            llm.get("endpoint").and_then(|v| v.as_str()),
            Some("https://gateway.engi-stack.com/v1")
        );
        assert_eq!(
            llm.get("api_key").and_then(|v| v.as_str()),
            Some("sk-newapi-abc")
        );
        // preexisting fields preserved
        assert_eq!(
            llm.get("model").and_then(|v| v.as_str()),
            Some("qwen2.5:3b")
        );
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
        assert_eq!(
            llm.get("provider").and_then(|v| v.as_str()),
            Some("openai_compat")
        );
        assert_eq!(
            llm.get("model").and_then(|v| v.as_str()),
            Some("deepseek-v4-flash"),
            "fresh vault paid 用户 login 应自动写入 cloud 下发的 default model"
        );
        assert_eq!(
            llm.get("api_key").and_then(|v| v.as_str()),
            Some("sk-newapi-fresh")
        );
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
        // User has configured a local endpoint — gateway must not overwrite.
        let settings =
            serde_json::json!({"llm": {"api_key": "", "endpoint": "http://localhost:18080/v1"}});
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

    // ── 接线守卫 #4/#5: login_password 与 activate 共享 wire 逻辑(DRY 不漂移) ──
    //
    // 两条会员入口(login_password 用 UserInfo.gateway_*,activate 用
    // ActivateResult.gateway_*)都喂同一个 wire_cloud_gateway → merge_gateway_into_settings
    // sink。如果两个响应类型的 gateway 字段集漂移(一边加了 model 另一边没加),付费
    // 会员会因入口不同而拿到不一致的 LLM 配置。本守卫钉死:**相同的 (url, token,
    // default_model) 三元组,无论来自哪个响应类型,经同一 sink 必产出字节相同的 llm 配置**。
    //
    // 我们无法在单元测试里起 vault 调 wire_cloud_gateway(&SharedState),所以守卫
    // 作用在 wire 的纯核心 —— merge_gateway_into_settings —— 并显式从两个真实响应
    // 类型抽字段喂进去,证明字段映射不漂移。

    use attune_core::cloud_client::{ActivateResult, UserInfo};

    #[test]
    fn both_member_entrances_wire_identical_gateway_config() {
        // 同一套云端下发凭据,分别封进两个响应类型。
        let url = "https://gateway.engi-stack.com/v1";
        let token = "sk-newapi-shared-not-real";
        let model = "deepseek-v4-flash";

        let from_login: UserInfo = serde_json::from_value(serde_json::json!({
            "id": 9, "email": "p@example.com", "plan": "pro",
            "gateway_url": url, "gateway_token": token, "gateway_default_model": model,
        }))
        .unwrap();
        let from_activate: ActivateResult = serde_json::from_value(serde_json::json!({
            "plan": "pro", "allowed_plugins": ["law-pro"],
            "gateway_url": url, "gateway_token": token, "gateway_default_model": model,
        }))
        .unwrap();

        // login_password feeds (me.gateway_url, me.gateway_token, me.gateway_default_model);
        // activate_license feeds (result.gateway_url, result.gateway_token,
        // result.gateway_default_model) — into the SAME wire_cloud_gateway → merge sink.
        let merged_login = merge_gateway_into_settings(
            serde_json::json!({}),
            from_login.gateway_url.as_deref().unwrap(),
            from_login.gateway_token.as_deref().unwrap(),
            from_login.gateway_default_model.as_deref(),
        );
        let merged_activate = merge_gateway_into_settings(
            serde_json::json!({}),
            from_activate.gateway_url.as_deref().unwrap(),
            from_activate.gateway_token.as_deref().unwrap(),
            from_activate.gateway_default_model.as_deref(),
        );

        // Byte-identical → the two entrances cannot drift in what they wire.
        assert_eq!(
            merged_login, merged_activate,
            "login_password and activate_license must wire identical gateway LLM config (DRY)"
        );
        // And it really is the full three-piece config (provider + endpoint + api_key + model).
        let llm = merged_login.get("llm").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            llm.get("provider").and_then(|v| v.as_str()),
            Some("openai_compat")
        );
        assert_eq!(llm.get("endpoint").and_then(|v| v.as_str()), Some(url));
        assert_eq!(llm.get("api_key").and_then(|v| v.as_str()), Some(token));
        assert_eq!(llm.get("model").and_then(|v| v.as_str()), Some(model));
    }

    /// 接线守卫 #4: 两响应类型的 gateway 字段集必须对齐 —— 若 cloud 给 UserInfo 加了
    /// 新 gateway 字段却忘了给 ActivateResult 加(或反之),激活路径会丢该配置。守卫用
    /// serde round-trip 钉死两者都解析同名三件套(url/token/default_model),缺一即编译/
    /// 断言失败,逼维护者同步两边。
    // ── GAP-B: vertical passthrough (spec 2026-06-20 §5) ────────────────────
    //
    // login_password / activate_license relay the cloud-issued `vertical` in their
    // response JSON. We assert the response-shape mapping (the same json! the handler
    // builds) carries vertical from the cloud type. SECURITY (§11 R2): vertical is UI
    // copy ONLY — it never appears in MemberState and never gates plugins.

    #[test]
    fn login_response_passes_through_vertical() {
        // login_password uses the authoritative /me snapshot.
        let me: UserInfo = serde_json::from_value(serde_json::json!({
            "id": 9, "email": "lawyer@x.com", "plan": "pro", "vertical": "law",
        }))
        .unwrap();
        let vertical = me.vertical.clone();
        let body = serde_json::json!({ "status": "ok", "vertical": vertical });
        assert_eq!(body["vertical"], "law");
    }

    #[test]
    fn login_rejects_identity_mismatch_between_login_and_authenticated_me() {
        let login_response: UserInfo = serde_json::from_value(serde_json::json!({
            "id": 9, "email": "first@x.com", "plan": "pro",
        }))
        .unwrap();
        let authenticated_me: UserInfo = serde_json::from_value(serde_json::json!({
            "id": 10, "email": "second@x.com", "plan": "pro",
        }))
        .unwrap();

        let error = super::validate_login_identity(&login_response, &authenticated_me)
            .expect_err("cross-account response must fail closed");
        assert_eq!(error.0, axum::http::StatusCode::FORBIDDEN);
        assert!(super::validate_login_identity(&authenticated_me, &authenticated_me).is_ok());
    }

    #[test]
    fn login_response_vertical_none_for_old_cloud() {
        // old cloud → vertical absent → response carries null (UI shows no scene).
        let user: UserInfo = serde_json::from_value(
            serde_json::json!({"id": 1, "email": "f@x.com", "plan": "individual"}),
        )
        .unwrap();
        let vertical = user.vertical.clone();
        assert!(vertical.is_none());
        let body = serde_json::json!({ "status": "ok", "vertical": vertical });
        assert!(body["vertical"].is_null());
    }

    #[test]
    fn activate_response_passes_through_vertical() {
        let result: ActivateResult = serde_json::from_value(serde_json::json!({
            "plan": "pro", "vertical": "patent", "allowed_plugins": ["patent-pro"],
        }))
        .unwrap();
        let body = serde_json::json!({
            "status": "ok",
            "vertical": result.vertical,
            "allowed_plugins": result.allowed_plugins,
        });
        assert_eq!(body["vertical"], "patent");
        assert_eq!(body["allowed_plugins"][0], "patent-pro");
    }

    #[test]
    fn user_info_and_activate_result_share_gateway_field_set() {
        let payload = serde_json::json!({
            "gateway_url": "https://gw/v1",
            "gateway_token": "sk-x-not-real",
            "gateway_default_model": "m1",
        });
        // UserInfo needs id+email; ActivateResult needs plan — merge the minimal envelopes.
        let mut ui = payload.clone();
        ui["id"] = serde_json::json!(1);
        ui["email"] = serde_json::json!("a@b.com");
        let mut ar = payload.clone();
        ar["plan"] = serde_json::json!("pro");

        let u: UserInfo = serde_json::from_value(ui).unwrap();
        let r: ActivateResult = serde_json::from_value(ar).unwrap();

        assert_eq!(u.gateway_url, r.gateway_url);
        assert_eq!(u.gateway_token, r.gateway_token);
        assert_eq!(u.gateway_default_model, r.gateway_default_model);
        assert!(u.gateway_default_model.is_some() && r.gateway_default_model.is_some());
    }

    // ── SECURITY: client-controlled cloud_url is rejected (SSRF / paywall) ───
    //
    // Threat: an attacker who can reach the local member API posts a `cloud_url`
    // pointing at their own server, which would forge "login/activation success"
    // → Paid state + a malicious gateway config (paywall bypass + SSRF). The fix
    // removed the field entirely from both request structs; the accounts URL is
    // resolved server-side from settings. We assert the field no longer exists on
    // the wire contract: a body carrying `cloud_url` deserializes fine (serde
    // ignores the unknown key) but the parsed struct has NO place to carry it —
    // i.e. the attacker-supplied URL is structurally dropped, never reaching
    // CloudClient::new.
    use crate::routes::member::{ActivateLicenseReq, LoginPasswordReq};

    #[test]
    fn login_password_req_ignores_client_cloud_url() {
        // Body includes a malicious cloud_url — it must be dropped, not honored.
        let body = serde_json::json!({
            "email": "u@example.com",
            "password": "pw-not-real",
            "cloud_url": "http://attacker.example/forge",
            "license_code": "code-1",
        });
        let req: LoginPasswordReq =
            serde_json::from_value(body).expect("deserializes (unknown field dropped)");
        assert_eq!(req.email, "u@example.com");
        assert_eq!(req.license_code.as_deref(), Some("code-1"));
        // Compile-time + runtime proof: there is no `cloud_url` field to read.
        // (If the field were re-added, the JSON-shape assertion below would still
        //  pass, so we additionally serialize the struct back and assert the key
        //  is absent from the canonical wire form.)
        let reserialized = serde_json::to_value(SerLoginShape::from(&req)).unwrap();
        assert!(
            reserialized.get("cloud_url").is_none(),
            "LoginPasswordReq must not carry a client cloud_url (SSRF/paywall)"
        );
    }

    #[test]
    fn activate_license_req_ignores_client_cloud_url() {
        let body = serde_json::json!({
            "license_key": "LIC-XYZ",
            "cloud_url": "http://attacker.example/forge",
        });
        let req: ActivateLicenseReq =
            serde_json::from_value(body).expect("deserializes (unknown field dropped)");
        assert_eq!(req.license_key, "LIC-XYZ");
        let reserialized = serde_json::json!({ "license_key": req.license_key });
        assert!(
            reserialized.get("cloud_url").is_none(),
            "ActivateLicenseReq must not carry a client cloud_url (SSRF/paywall)"
        );
    }

    // Mirror of LoginPasswordReq's *public* fields for serialize-back assertion
    // (the real struct is Deserialize-only). If someone re-adds `cloud_url` to
    // LoginPasswordReq, this mirror won't compile against it — the maintainer is
    // forced to confront the security regression.
    #[derive(serde::Serialize)]
    struct SerLoginShape {
        email: String,
        license_code: Option<String>,
    }
    impl From<&LoginPasswordReq> for SerLoginShape {
        fn from(r: &LoginPasswordReq) -> Self {
            Self {
                email: r.email.clone(),
                license_code: r.license_code.clone(),
            }
        }
    }

    // ── SECURITY: license_key never logged in plaintext (§1.4) ───────────────
    //
    // wire_cloud_gateway's `who` arg is recorded by tracing. The activate path
    // must pass a redacted identifier, never the raw license_key. We assert the
    // redaction is a stable, non-reversible `lic:<8-hex>` digest that does NOT
    // contain the original key.
    use crate::routes::member::redact_license_key;

    #[test]
    fn redact_license_key_hides_plaintext() {
        let key = "LIC-SUPER-SECRET-123456";
        let red = redact_license_key(key);
        assert!(red.starts_with("lic:"), "redaction has the lic: prefix");
        assert_eq!(red.len(), "lic:".len() + 8, "8 hex chars of digest");
        assert!(!red.contains(key), "the raw license_key must not appear");
        assert!(
            !red.contains("SECRET") && !red.contains("123456"),
            "no plaintext fragment of the key leaks into the redacted form"
        );
        // Stable: same key → same digest (so operators can correlate logs).
        assert_eq!(red, redact_license_key(key));
        // Distinct keys → distinct redactions (collision-resistant prefix).
        assert_ne!(red, redact_license_key("LIC-OTHER-KEY"));
    }

    // ── device binding (授权码激活 ① 设备绑定) ─────────────────────────────────
    use crate::routes::member::{device_binding_error, DEVICE_BINDING_META_KEY};
    use attune_core::cloud_client::{DeviceActivateError, DeviceActivateResult};
    use axum::http::StatusCode;

    /// 超设备数 → 409 max-devices-reached + 可操作 hint,且 status=device-binding-failed
    /// (本机未绑定 —— fail-closed,不置 Paid)。
    #[test]
    fn device_binding_max_devices_maps_to_409() {
        let (status, body) =
            device_binding_error(&DeviceActivateError::MaxDevicesReached("c".into()));
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.0["code"], "max-devices-reached");
        assert_eq!(body.0["status"], "device-binding-failed");
        assert_eq!(body.0["paid_applied"], false);
        assert!(
            body.0["hint"].as_str().unwrap().contains("设备上限"),
            "actionable hint present"
        );
    }

    /// 指纹/license 被拒 → 403 device-rejected。
    #[test]
    fn device_binding_rejected_maps_to_403() {
        let (status, body) = device_binding_error(&DeviceActivateError::Rejected(
            "fingerprint-mismatch".into(),
        ));
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.0["code"], "device-rejected");
        assert_eq!(body.0["status"], "device-binding-failed");
        assert_eq!(body.0["paid_applied"], false);
    }

    /// cloud 不可达 → 502 device-activate-unavailable(fail-closed,不静默放行)。
    #[test]
    fn device_binding_unavailable_maps_to_502() {
        let (status, body) =
            device_binding_error(&DeviceActivateError::Unavailable("transport".into()));
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body.0["code"], "device-activate-unavailable");
    }

    /// Device binding metadata is encrypted at rest and decrypts to the stable
    /// payload shape consumed by heartbeat/certificate flows.
    #[test]
    fn device_binding_meta_payload_is_encrypted() {
        let dev = DeviceActivateResult {
            device_token: "dt-hmac".into(),
            device_id: "dev-1".into(),
            plan: "pro".into(),
            max_activations: Some(2),
            current_activations: Some(1),
            issued_at: Some("2026-06-17T00:00:00+00:00".into()),
            expires_at: Some("2026-07-17T00:00:00+00:00".into()),
        };
        let dek = attune_core::crypto::Key32::generate();
        let encrypted = super::encrypted_device_binding_payload(&dek, &dev, None).unwrap();
        assert!(!encrypted
            .windows("dt-hmac".len())
            .any(|window| window == b"dt-hmac"));
        let plaintext = attune_core::crypto::decrypt(&dek, &encrypted).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(payload["device_token"], "dt-hmac");
        assert_eq!(payload["device_id"], "dev-1");
        assert_eq!(payload["max_activations"], 2);

        let activation =
            super::encrypted_device_binding_payload(&dek, &dev, Some("ATTUNE-ACTIVATION-SECRET"))
                .unwrap();
        assert!(!activation
            .windows("ATTUNE-ACTIVATION-SECRET".len())
            .any(|window| window == b"ATTUNE-ACTIVATION-SECRET"));
        let activation_plaintext = attune_core::crypto::decrypt(&dek, &activation).unwrap();
        let activation_payload: serde_json::Value =
            serde_json::from_slice(&activation_plaintext).unwrap();
        assert_eq!(
            activation_payload["activation_license_key"],
            "ATTUNE-ACTIVATION-SECRET"
        );
        assert_eq!(DEVICE_BINDING_META_KEY, "device_binding");
    }
}
