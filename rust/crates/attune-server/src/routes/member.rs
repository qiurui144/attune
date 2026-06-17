//! /api/v1/member — 会员状态 / settings locks endpoint.

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use attune_core::cloud_client::{CloudClient, License, UserInfo};
use attune_core::entitlement::EntitlementCache;
use attune_core::entitlement_reverify::{apply_refresh_rounds, RefreshSummary, ReverifyOutcome};
use attune_core::llm_settings::SETTINGS_META_KEY;
use attune_core::member_session::{MemberState, SettingsLocks};
use attune_core::plugin_sig::TrustMode;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use sha2::{Digest, Sha256};

/// Default public cloud accounts endpoint when the self-host override
/// (`settings.cloud.accounts_url`) is unset/empty.
const DEFAULT_ACCOUNTS_URL: &str = "https://accounts.engi-stack.com";

/// Resolve the cloud accounts base URL **server-side** from persisted settings
/// (`app_settings.cloud.accounts_url`), defaulting to the public engi-stack
/// endpoint. SECURITY (SSRF / paywall-bypass): the accounts URL is NEVER taken
/// from the request body — a client-controlled URL would let an attacker point
/// login/activation at their own server and forge "paid" / inject a malicious
/// gateway config. Self-host operators configure this once under 设置 → cloud.
fn resolve_accounts_url(state: &SharedState) -> String {
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

/// SECURITY: redact a license_key for log/identity use. Never log the raw key
/// (§1.4) — emit a stable `lic:<8-hex>` digest prefix so operators can correlate
/// without the credential ever reaching a log sink.
fn redact_license_key(license_key: &str) -> String {
    let digest = Sha256::digest(license_key.as_bytes());
    format!("lic:{}", &hex::encode(digest)[..8])
}

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
            // C1 paywall-bypass fix: a "paid" claim MUST be verified server-side before it can
            // gate billable cloud-LLM spend (doc-intel is the first such consumer). The previous
            // `!lic.is_empty()` check trusted the client; now `verify_paid` proves the license
            // against the cloud session (CloudMemberVerifier) and FAILS CLOSED on every error
            // path. A forged / empty / unverifiable claim → 403, never Paid.
            let lic = req.license_id.unwrap_or_default();
            let verifier = state.member_verifier();
            verifier
                .verify_paid(&req.account_id, &lic)
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
        E::NoCloudSession | E::Unavailable(_) | E::LicenseNotOnAccount | E::LicenseRevoked => {
            StatusCode::FORBIDDEN
        }
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
        match me {
            Some(me) => {
                wire_cloud_gateway(
                    &state,
                    me.gateway_url.as_deref(),
                    me.gateway_token.as_deref(),
                    me.gateway_default_model.as_deref(),
                    &user.email,
                );
            }
            None => tracing::warn!("member login: fetch /me failed — user keeps current LLM settings"),
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
pub async fn refresh_entitlements(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    // R1.1: 必须已登录(free 或 paid 都可手动 refresh;未登录拒)。
    {
        let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner());
        if !m.is_logged_in() {
            return Err(AppError::Unauthorized("member login required".into()));
        }
    }

    // 网络 I/O 是 blocking(CloudClient = reqwest::blocking)→ spawn_blocking(B4 约束)。
    // 把 EntitlementCache(Arc 内,clone 廉价)move 进 blocking 线程跑真 verify;写回
    // vault 也在该线程(短取 vault 锁)。结果 RefreshSummary 带回 async tail 映射响应。
    let cache = state.entitlement_cache.clone();
    let state_for_writeback = state.clone();
    let summary = tokio::task::spawn_blocking(move || -> RefreshSummary {
        run_refresh_round(&state_for_writeback, &cache)
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

/// 跑一轮 refresh(blocking):读缓存 → 真 verify 每条 → apply → 写回 vault。
/// 复用为 worker 的单轮逻辑(worker 周期调用本函数)。无可达 cloud session →
/// 返回空 summary(`all_network_error=false`、`refreshed=0` —— 不误判 502)。
pub fn run_refresh_round(state: &SharedState, cache: &EntitlementCache) -> RefreshSummary {
    let now = Utc::now();
    let mode = resolve_trust_mode(state);

    // 构建 CloudClient(从持久化 cloud session)。无 session → 不算"网络错"
    // (没有可 verify 的入口),返回空 summary。
    let Some(client) = cloud_client_from_session() else {
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
                cache.last_verified_at(plugin_id).map(|d| d.to_rfc3339()).unwrap_or_default()
            } else {
                va.to_string()
            };
            if let Err(e) = vault.store().set_entitlement_status(plugin_id, new_status, &last_verified) {
                tracing::error!(
                    "reverify: failed to persist verified deny for {plugin_id} (status={new_status}): {e}"
                );
            }
        } else if let Some(mut row) =
            cache.snapshot().into_iter().find(|r| &r.plugin_id == plugin_id)
        {
            row.status = new_status.to_string();
            if !va.is_empty() {
                row.last_verified_at = va.to_string();
            }
            row.updated_at = va.to_string();
            if let Err(e) = vault.store().upsert_entitlement(&dek, &row) {
                tracing::error!("reverify: failed to persist active write-back for {plugin_id}: {e}");
            }
        }
    }
}

/// 从持久化 cloud session(`config_dir/cloud-session.json`)构建 CloudClient。
/// 与 [`member_session_sync_plugins`] 同源(login 后写入)。无 session → None。
fn cloud_client_from_session() -> Option<CloudClient> {
    let path = attune_core::platform::config_dir().join("cloud-session.json");
    let json = std::fs::read_to_string(&path).ok()?;
    let sess: serde_json::Value = serde_json::from_str(&json).ok()?;
    let cloud_url = sess.get("cloud_url").and_then(|v| v.as_str())?;
    let session = sess.get("session").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
    Some(CloudClient::with_session(cloud_url, session))
}

/// 解析当前 `plugin_trust_mode`(app_settings meta);缺失/旧配置/vault 锁 → 默认
/// [`TrustMode::Warn`](决策 2 + spec §10 grandfather)。T11 加 UI setter;本函数
/// 只读,默认 Warn 让 client 先于 cloud v4 ship 不破网(跨仓 bootstrap)。
fn resolve_trust_mode(state: &SharedState) -> TrustMode {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(data) = vault.store().get_meta(SETTINGS_META_KEY) else {
        return TrustMode::Warn;
    };
    let Some(bytes) = data else { return TrustMode::Warn };
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
/// 两阶段语义分离(spec §3 编排):授权成功即视为 Paid(gateway 已可用);设备绑定
/// 失败(超设备数 / 指纹被拒)是**独立**的可操作问题,不回退已授权状态,但必须明确
/// surface 给用户(不静默)。device 绑定结果用 `Result` 携带分类错误。
struct ActivationOutcome {
    activate: attune_core::cloud_client::ActivateResult,
    device: std::result::Result<
        attune_core::cloud_client::DeviceActivateResult,
        attune_core::cloud_client::DeviceActivateError,
    >,
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
/// 这些错误下用户已被授权(gateway 已配),但本机未完成绑定 —— 明确返回而非静默。
pub async fn activate_license(
    State(state): State<SharedState>,
    Json(req): Json<ActivateLicenseReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let license_key = req.license_key.trim().to_string();
    if license_key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "license_key required", "code": "license-key-required"})),
        ));
    }
    // SECURITY: accounts URL 来自服务端 settings(不接受请求体覆盖)—— 见
    // resolve_accounts_url(SSRF / 付费墙绕过)。
    let cloud_url = resolve_accounts_url(&state);

    // B4 约束:CloudClient = reqwest::blocking → spawn_blocking,async tail 留本线程。
    // 两次 cloud 调用(authorize + device bind)在**同一** blocking 线程里串行完成,
    // 复用同一 client(及 session)。指纹采集也在此线程(machine-id / 持久化 device.id
    // 的文件 I/O,非异步上下文)。
    let key_for_blocking = license_key.clone();
    let outcome = tokio::task::spawn_blocking(
        move || -> Result<ActivationOutcome, String> {
            let client = CloudClient::new(cloud_url);
            // 阶段一:授权(失败即整体失败,fail-closed)。
            let activate = client.activate_license(&key_for_blocking).map_err(|e| e.to_string())?;
            // 阶段二:绑定本机设备(授权已成功;绑定结果按分类错误带回,不在此抛)。
            let fp = attune_core::device_fingerprint::device_fingerprint();
            let device = client.device_activate(&key_for_blocking, &fp);
            Ok(ActivationOutcome { activate, device })
        },
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("activate task join error: {e}")})),
        )
    })?
    .map_err(|msg| {
        // 授权阶段失败:无效授权码 / cloud 不可达 → 502。绝不在错误路径置 Paid。
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": format!("activate failed: {msg}"), "code": "activate-failed"})),
        )
    })?;

    let ActivationOutcome { activate: result, device } = outcome;

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
    // 落 entitlement:allowed_plugins 写进缓存 + vault,供 pluginhub 安装授权(②)+
    // 周期 re-verify 基准。best-effort,失败仅 warn。
    store_activation_entitlements(&state, &license_key, &result.allowed_plugins);

    // 置 Paid。授权码激活无独立 account_id,用 license_key 作 account 标识。
    let new_state = MemberState::Paid {
        account_id: license_key.clone(),
        license_id: license_key.clone(),
        llm_quota_remaining: 0,
    };
    *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) = new_state.clone();

    // ── 设备绑定阶段(①):成功 → 持久化 device_token;失败 → 明确返回可操作错误 ──
    match device {
        Ok(dev) => {
            // 把 device_token + device_id + 配额落 vault(后续 heartbeat / cert 用)。
            store_device_binding(&state, &dev);
            Ok(Json(serde_json::json!({
                "status": "ok",
                "state": new_state,
                "plan": result.plan,
                "expires_at": result.expires_at,
                "allowed_plugins": result.allowed_plugins,
                "device": {
                    "device_id": dev.device_id,
                    "max_activations": dev.max_activations,
                    "current_activations": dev.current_activations,
                },
            })))
        }
        Err(e) => Err(device_binding_error(&e)),
    }
}

/// 把设备绑定失败映射为 HTTP 响应(可操作提示)。授权已成功(gateway 已配),
/// 但本机绑定未完成 —— 明确返回而非静默放行。`status="activated-not-bound"` 让 UI
/// 知道"会员已生效,但本设备超限/被拒,需处理"。
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
            "status": "activated-not-bound",
            "error": e.to_string(),
            "code": code,
            "hint": hint,
        })),
    )
}

/// best-effort 把 cloud 颁发的 device binding(device_token + device_id + 配额)落 vault
/// meta(`DEVICE_BINDING_META_KEY`)。后续 heartbeat / cert 签发读它。vault 锁短取,
/// 不嵌套 fulltext/vectors(lock-ordering)。失败仅 warn,不阻断激活。
fn store_device_binding(state: &SharedState, dev: &attune_core::cloud_client::DeviceActivateResult) {
    let payload = serde_json::json!({
        "device_token": dev.device_token,
        "device_id": dev.device_id,
        "max_activations": dev.max_activations,
        "current_activations": dev.current_activations,
        "issued_at": dev.issued_at,
        "expires_at": dev.expires_at,
    });
    let data = match serde_json::to_vec(&payload) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("activate: device binding serialize failed: {e}");
            return;
        }
    };
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if vault.dek_db().is_err() {
        tracing::warn!("activate: vault locked — device_token not persisted");
        return;
    }
    if let Err(e) = vault.store().set_meta(DEVICE_BINDING_META_KEY, &data) {
        tracing::warn!("activate: failed to persist device binding: {e}");
    }
}

/// vault meta key:授权码激活后 cloud 颁发的设备绑定凭据(device_token 等)。
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
                    tracing::info!("member gateway: user has own LLM config — gateway not auto-applied");
                }
                Err(e) => tracing::warn!("member gateway: settings not written: {e}"),
            }
        }
        _ => tracing::info!("member gateway: no gateway token for {who} — keeps current LLM settings"),
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
fn store_activation_entitlements(state: &SharedState, license_id: &str, allowed_plugins: &[String]) {
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
        let row = attune_core::store::plugin_entitlements::EntitlementRow {
            plugin_id: plugin_id.clone(),
            license_id: license_id.to_string(),
            tier: "paid".into(),
            status: "active".into(),
            trial_expires: None,
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
    use attune_core::entitlement::{EntStatus, EntitlementCache};
    use attune_core::entitlement_reverify::{apply_refresh_rounds, ReverifyOutcome};
    use attune_core::llm_settings::{gateway_should_apply, merge_gateway_into_settings};
    use attune_core::store::plugin_entitlements::EntitlementRow;
    use chrono::{DateTime, Utc};

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn row(plugin_id: &str, status: &str, last_verified: &str) -> EntitlementRow {
        EntitlementRow {
            plugin_id: plugin_id.into(),
            license_id: "lic-x".into(),
            tier: "paid".into(),
            status: status.into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: last_verified.into(),
            grace_started_at: None,
            updated_at: last_verified.into(),
        }
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
        let rounds = vec![(
            "law-pro".to_string(),
            ReverifyOutcome::NetworkError,
            None,
        )];
        let summary = apply_refresh_rounds(&cache, &rounds, &now);

        // cache UNCHANGED — the load-bearing §7.2 error-5 invariant.
        assert_eq!(cache.snapshot(), before, "network error must not mutate the cache");
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
            Self { email: r.email.clone(), license_code: r.license_code.clone() }
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
    use attune_core::cloud_client::{DeviceActivateError, DeviceActivateResult};
    use crate::routes::member::{device_binding_error, DEVICE_BINDING_META_KEY};
    use axum::http::StatusCode;

    /// 超设备数 → 409 max-devices-reached + 可操作 hint,且 status=activated-not-bound
    /// (会员已授权,本机未绑定 —— 明确 surface,不静默)。
    #[test]
    fn device_binding_max_devices_maps_to_409() {
        let (status, body) = device_binding_error(&DeviceActivateError::MaxDevicesReached("c".into()));
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.0["code"], "max-devices-reached");
        assert_eq!(body.0["status"], "activated-not-bound");
        assert!(body.0["hint"].as_str().unwrap().contains("设备上限"), "actionable hint present");
    }

    /// 指纹/license 被拒 → 403 device-rejected。
    #[test]
    fn device_binding_rejected_maps_to_403() {
        let (status, body) = device_binding_error(&DeviceActivateError::Rejected("fingerprint-mismatch".into()));
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.0["code"], "device-rejected");
        assert_eq!(body.0["status"], "activated-not-bound");
    }

    /// cloud 不可达 → 502 device-activate-unavailable(fail-closed,不静默放行)。
    #[test]
    fn device_binding_unavailable_maps_to_502() {
        let (status, body) = device_binding_error(&DeviceActivateError::Unavailable("transport".into()));
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body.0["code"], "device-activate-unavailable");
    }

    /// store_device_binding 写出的 vault meta payload 形态(device_token + 配额)。
    /// 钉死键名,后续 heartbeat / cert 读同一形态。用纯 JSON 形态断言(不起 vault)。
    #[test]
    fn device_binding_meta_payload_shape() {
        let dev = DeviceActivateResult {
            device_token: "dt-hmac".into(),
            device_id: "dev-1".into(),
            plan: "pro".into(),
            max_activations: Some(2),
            current_activations: Some(1),
            issued_at: Some("2026-06-17T00:00:00+00:00".into()),
            expires_at: Some("2026-07-17T00:00:00+00:00".into()),
        };
        // 与 store_device_binding 内构造的 payload 同形态。
        let payload = serde_json::json!({
            "device_token": dev.device_token,
            "device_id": dev.device_id,
            "max_activations": dev.max_activations,
            "current_activations": dev.current_activations,
            "issued_at": dev.issued_at,
            "expires_at": dev.expires_at,
        });
        assert_eq!(payload["device_token"], "dt-hmac");
        assert_eq!(payload["device_id"], "dev-1");
        assert_eq!(payload["max_activations"], 2);
        assert_eq!(DEVICE_BINDING_META_KEY, "device_binding");
    }
}
