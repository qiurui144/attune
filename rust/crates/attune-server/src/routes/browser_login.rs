//! 浏览器登入协助 REST(GAP-A,桌面可点)。
//!
//! 把 INT-1 的 [`attune_core::browser_login::SidecarController`] + 登入协助会话
//! 保险柜接到 REST 面,让桌面用户**点得到**:
//! - `POST   /api/v1/browser-login/connect`        → 触发登入(scan / 人在回路 login)→
//!   捕获会话入保险柜 → 返回脱敏状态。
//! - `GET    /api/v1/browser-login/sessions`       → 列已连接登入会话(脱敏:只返
//!   provider + 域名 + 状态,**绝不返凭据 / storage_state**)。
//! - `DELETE /api/v1/browser-login/sessions/{id}`  → 移除某登入会话。
//!
//! ## 安全契约(§1.4 + spec §9 / L-3 / L-5 / L-7)
//!
//! - **凭据 / 会话绝不出现在响应 / 日志**:`storage_state` 经 dek 加密落
//!   `third_party_accounts.secret_enc`(provider=`browser_login`);列表 / connect
//!   响应只回 `{id, provider, domain, status}`,无 secret 字段(编译期保证 —— 用
//!   [`SessionView`] 而非原始 row)。
//! - **auto 默认关(L-5)**:`mode` 默认 `scan` / `login`(人在回路)。`auto`(LLM 表单
//!   识别 + 凭据填充)需 ① body 显式 `auto:true` ② 付费会员 tier(💰,经 cloud LLM)。
//!   本 slice **不接通 auto 的真实 LLM 路径**(留 §4.5 A-G 3-tier gate 前),`auto:true`
//!   一律返 `auto-mode-disabled`(明确告知,而非静默降级)。
//! - **OutboundGate allowlist + SSRF(L-7)**:`entry_url` 经
//!   [`attune_core::browser_login::validate_entry_url`] 校验 —— http(s) only / 拒裸
//!   IP / 拒 loopback·private·link-local·metadata;allowlist = 从 entry_url 自身派
//!   生的 host(用户在 UI 显式输入即 = 用户批准的源)。
//! - **出网受控**:走 `OutboundKind::BrowserCrawl` 隐私开关 + vault-unlocked 双门
//!   (privacy settings `browser_crawl` 关 → `browser-crawl-disabled`;vault 锁 →
//!   401)。
//! - **工具缺失优雅降级**:`SidecarController::locate()` 失败 → 503
//!   `browser-tool-not-found`(友好提示装 community-browser-automation),不 panic。
//!
//! per docs/superpowers/specs/2026-06-20-browser-autologin-integration.md §5 / §4.1。

use std::time::Duration;

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use attune_core::browser_login::{
    validate_entry_url, LoginRecipe, RunOutcome, SidecarCommand, SidecarController, SidecarError,
};
use attune_core::net::url_guard::system_resolve;
use attune_core::store::third_party_accounts::{ThirdPartyAccountInput, ThirdPartyAccountView};

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// browser_login 会话在通用账号表里的 provider 标识(已入 KNOWN_PROVIDERS 白名单)。
const PROVIDER_BROWSER_LOGIN: &str = "browser_login";

/// 隐私开关里 BrowserCrawl 出网点的 key(`OutboundKind::BrowserCrawl::as_str()`)。
const PRIVACY_KEY_BROWSER_CRAWL: &str = "browser_crawl";

/// 人在回路 login 默认等待秒数(用户在弹出的真浏览器里手动完成登录)。上限防滥用。
const DEFAULT_WAIT_SECONDS: u32 = 120;
const MAX_WAIT_SECONDS: u32 = 600;

/// 触发请求体。`mode` ∈ {`scan`, `login`};`auto` 默认 false(L-5)。
#[derive(Deserialize)]
pub struct ConnectRequest {
    /// 用户可读源名(显示用,不算 secret);为空时用域名兜底。
    #[serde(default)]
    pub source_name: String,
    /// 登入入口 URL(必填),经 L-7 SSRF + allowlist 校验。
    pub entry_url: String,
    /// 触发模式:`scan`(只探测登录/验证码信号,零会话捕获)或 `login`(弹真浏览器,
    /// 人在回路完成登录后捕获 storage_state 入保险柜)。默认 `login`。
    #[serde(default = "default_mode")]
    pub mode: String,
    /// 是否启用 auto(LLM 表单识别自动填充)。**默认 false**;本 slice 一律拒绝
    /// (`auto-mode-disabled`),需会员 tier + 3-tier LLM gate 通过后才接通(未来)。
    #[serde(default)]
    pub auto: bool,
    /// 人在回路 login 的等待秒数(可选,默认 120,上限 600)。
    #[serde(default)]
    pub wait_seconds: Option<u32>,
}

fn default_mode() -> String {
    "login".to_string()
}

/// 脱敏会话视图 —— **编译期无 secret / storage_state 字段**。API 只回这个。
#[derive(serde::Serialize)]
pub struct SessionView {
    pub id: String,
    pub provider: String,
    /// 源名(label)。
    pub source_name: String,
    /// 登入域名(脱敏后唯一对外可见的目标信息)。
    pub domain: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ThirdPartyAccountView> for SessionView {
    fn from(v: ThirdPartyAccountView) -> Self {
        SessionView {
            id: v.id,
            provider: v.provider,
            source_name: v.label,
            // endpoint 存的是登入域名(我们写入时只放 host,不放凭据 / path)。
            domain: v.endpoint,
            status: v.status,
            created_at: v.created_at,
            updated_at: v.updated_at,
        }
    }
}

/// 从 entry_url 抽 host(allowlist 派生 + 脱敏域名)。非法 URL → BadRequest。
fn host_of(entry_url: &str) -> Result<String, AppError> {
    let parsed = url::Url::parse(entry_url.trim())
        .map_err(|e| AppError::BadRequest(format!("invalid-entry-url: {e}")))?;
    parsed
        .host_str()
        .map(|h| h.to_string())
        .ok_or_else(|| AppError::BadRequest("invalid-entry-url: missing host".into()))
}

/// 读取 BrowserCrawl 出网点的隐私开关(privacy settings 的 `browser_crawl` key)。
/// 缺省 false(出网默认关,用户须显式开)。
fn browser_crawl_enabled(state: &SharedState) -> bool {
    use attune_core::llm_settings::SETTINGS_META_KEY;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let meta = vault.store().get_meta(SETTINGS_META_KEY).ok().flatten();
    let settings: serde_json::Value = match meta {
        Some(data) => serde_json::from_slice(&data).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };
    settings
        .get("privacy")
        .and_then(|p| p.get(PRIVACY_KEY_BROWSER_CRAWL))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 把 SidecarError 映射成 HTTP 状态 + 稳定 kebab code(spec §7)。
/// **绝不透传可能含凭据 / 内网细节的内层 message** —— 只回稳定 code + 通用解释。
fn sidecar_error(e: &SidecarError) -> AppError {
    match e {
        SidecarError::ToolNotFound => AppError::ServiceUnavailable(
            "browser-tool-not-found: browser login helper is not installed; \
             install community-browser-automation or set ATTUNE_BROWSER_TOOL"
                .into(),
        ),
        SidecarError::SpawnFailed(_) => {
            AppError::ServiceUnavailable("browser-tool-spawn-failed".into())
        }
        SidecarError::Timeout(_) => {
            AppError::BadGateway("browser-login-timeout: login did not complete in time".into())
        }
        SidecarError::BadOutput(_) | SidecarError::SchemaMismatch { .. } => {
            AppError::BadGateway("browser-tool-bad-output".into())
        }
        SidecarError::Io(_) => AppError::Internal("browser-tool-io".into()),
    }
}

/// POST /api/v1/browser-login/connect — 触发登入(scan / 人在回路 login)。
///
/// 成功(login)→ 捕获 storage_state 入保险柜(provider=browser_login,加密落
/// secret_enc)→ 返回脱敏 `{id, provider, domain, status}`。
/// scan → 只回探测结果状态(无会话捕获)。
pub async fn connect(
    State(state): State<SharedState>,
    Json(req): Json<ConnectRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // ── L-5: auto 默认关。本 slice 不接通 auto 真实 LLM 路径 —— 显式拒绝,不静默降级。
    if req.auto {
        // 即便未来接通,也必须付费会员(💰,经 cloud LLM)。先给出明确门控提示。
        let is_paid = {
            let m = state
                .member_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            m.is_paid()
        };
        if !is_paid {
            return Err(AppError::Forbidden(
                "auto-mode-requires-membership: auto form-fill needs a Pro membership (cloud LLM)"
                    .into(),
            ));
        }
        return Err(AppError::BadRequest(
            "auto-mode-disabled: automatic form-fill is not enabled in this build; \
             use human-in-the-loop login instead"
                .into(),
        ));
    }

    let mode = req.mode.trim().to_lowercase();
    if mode != "scan" && mode != "login" {
        return Err(AppError::BadRequest(format!(
            "invalid-mode: {:?} (expected 'scan' or 'login')",
            req.mode
        )));
    }

    // ── L-7: SSRF + allowlist。host 从 entry_url 自身派生(用户在 UI 显式输入即批准)。
    let domain = host_of(&req.entry_url)?;
    let allowlist = vec![domain.clone()];
    validate_entry_url(req.entry_url.trim(), &allowlist, &system_resolve)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // ── 出网受控:BrowserCrawl 隐私开关 + vault-unlocked 双门。
    if !browser_crawl_enabled(&state) {
        return Err(AppError::Forbidden(
            "browser-crawl-disabled: enable the browser-crawl outbound point in Privacy settings"
                .into(),
        ));
    }
    {
        // vault 必须 unlocked(dek 可派生)—— 会话要加密落库。
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.dek_db().map_err(|_| {
            AppError::Unauthorized("vault-locked: unlock the vault to capture a login session".into())
        })?;
    }

    // ── 定位 sidecar(工具缺失 → 503 友好提示,不 panic)。
    let controller = SidecarController::locate().map_err(|e| sidecar_error(&e))?;

    // 构造 credential-free recipe(凭据绝不进 recipe;auto 才走 stdin,本路径无 auto)。
    let recipe = LoginRecipe {
        id: format!("bl-{domain}"),
        name: if req.source_name.trim().is_empty() {
            domain.clone()
        } else {
            req.source_name.trim().to_string()
        },
        start_url: req.entry_url.trim().to_string(),
        login_url: String::new(),
        credentials: None,
        signals: Default::default(),
        extract: Vec::new(),
    };
    let recipe_json = recipe
        .to_json()
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // recipe 写临时文件(credential-free)。sidecar 读它;run() 后清理 state 文件。
    let recipe_file = tempfile::Builder::new()
        .prefix("attune-bl-recipe-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| AppError::Internal(format!("tempfile: {e}")))?;
    std::fs::write(recipe_file.path(), recipe_json.as_bytes())
        .map_err(|e| AppError::Internal(format!("write recipe: {e}")))?;
    let recipe_path = recipe_file.path().to_path_buf();

    if mode == "scan" {
        // scan:只探测,无会话捕获。阻塞调用走 spawn_blocking 不卡 axum worker。
        let (result, outcome) = tokio::task::spawn_blocking(move || {
            controller.run(SidecarCommand::Scan { recipe_path })
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(|e| sidecar_error(&e))?;
        // 不回 records / url 细节里的潜在敏感内容,只回探测结论。
        return Ok(Json(json!({
            "mode": "scan",
            "domain": domain,
            "outcome": outcome_label(&outcome),
            "needs_login": outcome == RunOutcome::SessionExpired
                || outcome == RunOutcome::NeedsHuman,
            "restricted": outcome == RunOutcome::Restricted,
            "status": result.status,
        })));
    }

    // mode == "login":人在回路。state 文件接收 storage_state,捕获后读出加密入库。
    let state_file = tempfile::Builder::new()
        .prefix("attune-bl-state-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| AppError::Internal(format!("tempfile: {e}")))?;
    let state_path = state_file.path().to_path_buf();
    let wait = req
        .wait_seconds
        .unwrap_or(DEFAULT_WAIT_SECONDS)
        .clamp(10, MAX_WAIT_SECONDS);

    // controller 的内部 timeout 必须 ≥ wait_seconds(否则 login 还在等人就被 kill)。
    let controller = controller.with_timeout(Duration::from_secs((wait as u64) + 30));
    let login_recipe_path = recipe_path.clone();
    let login_state_path = state_path.clone();
    let captured = tokio::task::spawn_blocking(move || -> Result<(RunOutcome, Option<String>), SidecarError> {
        let cmd = SidecarCommand::Login {
            recipe_path: login_recipe_path,
            state_path: login_state_path,
            wait_seconds: wait,
        };
        run_login_and_capture(&controller, cmd)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(|e| sidecar_error(&e))?;

    let (outcome, storage_state) = captured;

    if outcome != RunOutcome::Success {
        // 人在回路未完成 / 需要人工 / 受限 —— 不写会话,回明确状态(不报错,UI 可重试)。
        return Ok(Json(json!({
            "mode": "login",
            "domain": domain,
            "outcome": outcome_label(&outcome),
            "captured": false,
            "needs_human": outcome == RunOutcome::NeedsHuman,
            "restricted": outcome == RunOutcome::Restricted,
        })));
    }

    let Some(storage_state) = storage_state else {
        return Err(AppError::BadGateway(
            "browser-login-no-session: login reported success but captured no session".into(),
        ));
    };

    // ── 捕获成功:storage_state 加密入保险柜(provider=browser_login)。
    let id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|_| {
            AppError::Unauthorized("vault-locked: unlock the vault to store the session".into())
        })?;
        let input = ThirdPartyAccountInput {
            provider: PROVIDER_BROWSER_LOGIN.to_string(),
            label: recipe.name.clone(),
            // username 存源名(用户可读,非凭据);不放任何密码。
            username: recipe.name.clone(),
            endpoint: domain.clone(),
            // secret = storage_state JSON;落库即 AES-256-GCM 加密,绝不回显。
            secret: storage_state,
        };
        vault
            .store()
            .add_third_party_account(&dek, &input)
            .map_err(|e| AppError::Internal(format!("persist session: {e}")))?
    };

    // 回脱敏结果 —— **无 secret / storage_state**。
    Ok(Json(json!({
        "mode": "login",
        "id": id,
        "provider": PROVIDER_BROWSER_LOGIN,
        "domain": domain,
        "captured": true,
        "status": "connected",
    })))
}

/// GET /api/v1/browser-login/sessions — 列已连接登入会话(**脱敏,无凭据**)。
pub async fn list_sessions(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?; // 须解锁(列表读 DB)。
    let rows = vault.store().list_third_party_accounts()?;
    let sessions: Vec<SessionView> = rows
        .into_iter()
        .filter(|r| r.provider == PROVIDER_BROWSER_LOGIN)
        .map(Into::into)
        .collect();
    Ok(Json(json!({ "sessions": sessions })))
}

/// DELETE /api/v1/browser-login/sessions/{id} — 移除某登入会话。
pub async fn delete_session(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    // 防越权删非 browser_login 账号:先确认它是 browser_login 会话。
    let is_session = vault
        .store()
        .list_third_party_accounts()?
        .into_iter()
        .any(|r| r.id == id && r.provider == PROVIDER_BROWSER_LOGIN);
    if !is_session {
        return Err(AppError::NotFound(format!("browser-login session {id}")));
    }
    let deleted = vault.store().delete_third_party_account(&id)?;
    if !deleted {
        return Err(AppError::NotFound(format!("browser-login session {id}")));
    }
    Ok(Json(json!({ "deleted": id })))
}

/// 稳定 outcome 标签(对外脱敏,UI 用)。
fn outcome_label(o: &RunOutcome) -> &'static str {
    match o {
        RunOutcome::Success => "success",
        RunOutcome::NeedsHuman => "needs-human",
        RunOutcome::Restricted => "restricted",
        RunOutcome::SessionExpired => "session-expired",
        RunOutcome::Usage => "usage-error",
        RunOutcome::Internal => "internal-error",
    }
}

/// 运行 human-in-the-loop login 并捕获 `storage_state`。
///
/// WHY 后台轮询线程:`SidecarController::run` 在返回前会 zeroize+unlink `--state`
/// 临时文件(INT-1 契约:会话工作副本不留盘),所以 `run()` 返回后磁盘上已无会话。
/// 为既不回退 INT-1 的 cleanup 行为、又能拿到捕获的会话,这里在 run() 期间起一个
/// 轮询线程,在 sidecar 写出 state 文件、run() cleanup **之前**抢读一份快照到内存。
fn run_login_and_capture(
    controller: &SidecarController,
    cmd: SidecarCommand,
) -> Result<(RunOutcome, Option<String>), SidecarError> {
    let state_path = match &cmd {
        SidecarCommand::Login { state_path, .. } => state_path.clone(),
        _ => {
            return Err(SidecarError::Io(
                "run_login_and_capture requires a Login command".into(),
            ))
        }
    };
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let captured_w = captured.clone();
    let watch_path = state_path.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_w = stop.clone();
    // 后台轮询线程:sidecar 一旦写出 state 文件,立即读快照(在 run() cleanup 前)。
    let watcher = std::thread::spawn(move || {
        while !stop_w.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok(bytes) = std::fs::read(&watch_path) {
                if !bytes.is_empty() {
                    if let Ok(s) = String::from_utf8(bytes) {
                        // 仅当看起来是非空 JSON 才接受(storage_state 是 JSON 对象)。
                        if s.trim_start().starts_with('{') {
                            *captured_w.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
                            break;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    let run_res = controller.run(cmd);
    // 通知 watcher 停止并 join。
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = watcher.join();

    let (_, outcome) = run_res?;
    let storage_state = captured.lock().unwrap_or_else(|e| e.into_inner()).take();
    Ok((outcome, storage_state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_extraction() {
        assert_eq!(host_of("https://www.cnki.net/login").unwrap(), "www.cnki.net");
        assert!(host_of("not a url").is_err());
        assert!(host_of("file:///etc/passwd").is_err());
    }

    #[test]
    fn session_view_has_no_secret_field() {
        // Compile-time guarantee: SessionView is built from ThirdPartyAccountView
        // (which itself carries no secret). Serializing it never emits storage_state.
        let v = SessionView {
            id: "tpa-1".into(),
            provider: PROVIDER_BROWSER_LOGIN.into(),
            source_name: "CNKI".into(),
            domain: "cnki.net".into(),
            status: "connected".into(),
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("secret"), "session view must not carry secret");
        assert!(!json.contains("storage_state"), "no session blob in view");
        assert!(json.contains("cnki.net"));
    }

    #[test]
    fn outcome_labels_are_stable_kebab() {
        assert_eq!(outcome_label(&RunOutcome::Success), "success");
        assert_eq!(outcome_label(&RunOutcome::NeedsHuman), "needs-human");
        assert_eq!(outcome_label(&RunOutcome::Restricted), "restricted");
        assert_eq!(outcome_label(&RunOutcome::SessionExpired), "session-expired");
    }

    #[test]
    fn tool_not_found_maps_to_503_with_kebab() {
        let e = sidecar_error(&SidecarError::ToolNotFound);
        match e {
            AppError::ServiceUnavailable(msg) => {
                assert!(msg.contains("browser-tool-not-found"));
            }
            other => panic!("expected ServiceUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn sidecar_error_never_leaks_inner_message() {
        // SpawnFailed / Io carry an inner string that could include a path; the
        // mapped AppError must NOT echo it (only a stable kebab code).
        let e = sidecar_error(&SidecarError::SpawnFailed(
            "/secret/path/leak topsecret".into(),
        ));
        let rendered = format!("{e:?}");
        assert!(!rendered.contains("topsecret"), "inner detail leaked: {rendered}");
        assert!(!rendered.contains("/secret/path"), "path leaked: {rendered}");
    }
}
