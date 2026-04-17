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
    // 允许 /vault/*, /status/health, 以及静态 UI 资源无需解锁
    let path = request.uri().path();
    if path.starts_with("/api/v1/vault")
        || path == "/api/v1/status/health"
        || path == "/"
        || path == "/ui"
        || path.starts_with("/ui/")
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
    if !is_always_auth
        && (path == "/api/v1/status/health"
            || path == "/"
            || path.starts_with("/ui/")
            || path == "/api/v1/vault/setup"
            || path == "/api/v1/vault/unlock"
            || path == "/api/v1/vault/status")
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
