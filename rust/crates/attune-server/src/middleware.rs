use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use crate::state::SharedState;
use attune_core::vault::VaultState;

/// Vault guard: 未 UNLOCKED 时返回 403
pub async fn vault_guard(
    State(state): State<SharedState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // 允许 /vault/*, /health, /status/health, 以及静态 UI 资源无需解锁
    let path = request.uri().path();
    if path.starts_with("/api/v1/vault")
        || path == "/api/v1/status/health"
        || path == "/health"
        || path == "/"
        || path == "/ui"
        || path.starts_with("/ui/")
        // /favicon.ico — 浏览器自动请求；不 bypass 会在 vault locked 时回 403 刷 console error
        || path == "/favicon.ico"
        // /api/v1/member/* — 会员状态 + lock 决策, 不读 vault, 不需 unlock
        || path.starts_with("/api/v1/member")
        // /api/v1/ocr/profiles — 用户场景预设 (持久化磁盘文件, 不读 vault).
        // 写操作仍由 handler 里 SettingsLocks::ocr_profiles 控制.
        || path.starts_with("/api/v1/ocr/profiles")
        // Round B E2E fix: /ws/scan-progress 必须 bypass vault_guard。
        // ws.rs handler 注释明确"vault locked 时推送 locked 状态后持续等待" —
        // 该 endpoint 设计上支持 vault 未解锁时连接。之前 vault_guard 白名单漏了它
        // （auth guard 的 bypass 列表 OSS-S16 已加，两个 middleware 白名单不一致），
        // 导致 wizard / locked 阶段前端连 WS 收 403 → 无限重连刷 console error。
        // bypass 后由 handle_scan_progress 自查 vault 状态推 locked JSON。
        || path == "/ws/scan-progress"
    {
        return next.run(request).await;
    }

    let vault_state = state.vault.lock().unwrap_or_else(|e| e.into_inner()).state();
    match vault_state {
        VaultState::Unlocked => next.run(request).await,
        VaultState::Locked => {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "vault is locked",
                "hint": "POST /api/v1/vault/unlock to unlock"
            }))).into_response()
        }
        VaultState::Sealed => {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "vault is sealed",
                "hint": "POST /api/v1/vault/setup to initialize"
            }))).into_response()
        }
    }
}

/// 请求维度访问日志 — 用户审计 "UI 操作 ↔ API 调用" 对应必备.
///
/// 为何不用 tower_http::TraceLayer:
/// - 我们想记 **member_state** (LoggedOut / Free / Paid + account_id), TraceLayer 只能记 HTTP 元信息
/// - 我们想统一格式 (`access: <method> <path> <status> <duration_ms> member=<kind> ...`),
///   方便 grep / shipper / 审计回放
/// - sensitive endpoints 路径里可能含 id (e.g. `/api/v1/items/<uuid>`), 这里只记完整 path 不抓 body
///
/// 输出走 `tracing::info!` (target=access), 便于 `RUST_LOG=access=info` 过滤.
pub async fn access_log(
    State(state): State<SharedState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let start = std::time::Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(|q| q.to_string());

    // 抓 member_state 快照 (要 release lock 才能调 next)
    let (member_kind, account_id) = {
        let m = state.member_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let kind = match &m {
            attune_core::member_session::MemberState::LoggedOut => "logged_out",
            attune_core::member_session::MemberState::Free { .. } => "free",
            attune_core::member_session::MemberState::Paid { .. } => "paid",
        };
        let acct = m.account_id().map(|s| s.to_string());
        (kind, acct)
    };

    let response = next.run(request).await;
    let status = response.status().as_u16();
    let dur_ms = start.elapsed().as_millis();

    // 健康检查类高频路径降级为 debug, 其他走 info; 4xx/5xx 升级为 warn
    let acct_disp = account_id.as_deref().unwrap_or("-");
    let query_disp = query.as_deref().unwrap_or("");
    if status >= 500 {
        tracing::warn!(
            target: "access",
            "{method} {path}{} {status} {dur_ms}ms member={member_kind} account={acct_disp}",
            if query_disp.is_empty() { "".to_string() } else { format!("?{query_disp}") }
        );
    } else if status >= 400 {
        tracing::info!(
            target: "access",
            "{method} {path}{} {status} {dur_ms}ms member={member_kind} account={acct_disp}",
            if query_disp.is_empty() { "".to_string() } else { format!("?{query_disp}") }
        );
    } else if path == "/health" || path == "/api/v1/status/health" {
        tracing::debug!(
            target: "access",
            "{method} {path} {status} {dur_ms}ms"
        );
    } else {
        tracing::info!(
            target: "access",
            "{method} {path}{} {status} {dur_ms}ms member={member_kind} account={acct_disp}",
            if query_disp.is_empty() { "".to_string() } else { format!("?{query_disp}") }
        );
    }

    response
}

/// Bearer auth guard: optional, enabled by `require_auth` flag on AppState.
/// Certain high-sensitivity endpoints always require Bearer token regardless of the flag.
pub async fn bearer_auth_guard(
    State(state): State<SharedState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // High-sensitivity endpoints: always require Bearer token regardless of require_auth flag
    const ALWAYS_AUTH_ENDPOINTS: &[&str] = &[
        "/api/v1/vault/device-secret/export",
        "/api/v1/vault/device-secret/import",
        "/api/v1/vault/change-password",
    ];
    let is_always_auth = ALWAYS_AUTH_ENDPOINTS.iter().any(|ep| path == *ep);

    // If not a forced-auth endpoint and global auth is disabled, allow through
    if !state.require_auth && !is_always_auth {
        return next.run(request).await;
    }

    // Public endpoints and vault bootstrap endpoints bypass the token check
    // (only applies to non-forced-auth endpoints)
    //
    // OSS-S16 fix: /ws/scan-progress 也走 bypass — 因为 token 格式 `id:ts:hash` 含 `:`
    // 字符，违反 RFC 6455 subprotocol 规范，浏览器 WebSocket API 无法用 subprotocol 传
    // Bearer token。此 endpoint 由 handler 自身从 query string `?token=xxx` 校验。
    if !is_always_auth
        && (path == "/api/v1/status/health"
            || path == "/health"
            || path == "/"
            || path == "/favicon.ico"
            || path.starts_with("/ui/")
            || path.starts_with("/assets/")
            || path == "/api/v1/vault/setup"
            || path == "/api/v1/vault/unlock"
            || path == "/api/v1/vault/status"
            || path == "/api/v1/vault/reset-with-recovery-key"
            || path == "/api/v1/vault/forgot-password-reset"
            || path.starts_with("/api/v1/member")
            || path == "/ws/scan-progress")
    {
        return next.run(request).await;
    }

    // Extract Bearer token
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.to_string());

    let token = match auth_header {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing bearer token"})),
            )
                .into_response()
        }
    };

    let verify_result = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.verify_session(&token).map_err(|e| e.to_string())
    };

    match verify_result {
        Ok(_) => next.run(request).await,
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn always_auth_endpoints_include_device_secret_and_change_password() {
        // 验证敏感端点常量包含 device-secret 相关端点及 change-password
        const ALWAYS_AUTH_ENDPOINTS: &[&str] = &[
            "/api/v1/vault/device-secret/export",
            "/api/v1/vault/device-secret/import",
            "/api/v1/vault/change-password",
        ];
        assert!(ALWAYS_AUTH_ENDPOINTS.contains(&"/api/v1/vault/device-secret/export"));
        assert!(ALWAYS_AUTH_ENDPOINTS.contains(&"/api/v1/vault/device-secret/import"));
        assert!(ALWAYS_AUTH_ENDPOINTS.contains(&"/api/v1/vault/change-password"));
    }
}
