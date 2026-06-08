//! AppError — 统一的 attune-server 错误类型 + IntoResponse.
//!
//! 历史 (ARCH-A / R20 audit): 38 个 route 各自手写
//! `Err((StatusCode::BAD_REQUEST, Json(json!({"error": "..."}))))` 形式, 错误
//! JSON shape 不一致, 客户端 (Chrome 扩展 / attune-pro / Tauri webview) 解析需要
//! sniff. 本模块抽统一 `AppError` enum + `IntoResponse`, route 用 `?` 自动转换.
//!
//! 使用模板:
//! ```text
//! use crate::error::{AppError, AppResult};
//!
//! pub async fn my_route() -> AppResult<Json<MyResponse>> {
//!     let item = state.store.get(id).map_err(AppError::NotFound)?;
//!     // 任何 `From<E> for AppError` 都可以直接 `?` 抛出
//!     Ok(Json(item.into()))
//! }
//! ```
//!
//! 客户端契约: 所有错误返回都是这个 shape:
//! ```json
//! { "error": "human-readable msg", "code": "kebab-case-tag" }
//! ```
//! 渐进 migration: 旧 route 用 (StatusCode, Json) tuple 仍能 build, 不阻塞.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

/// attune-server 路由层统一错误类型. 各 variant 对应 HTTP status + 客户端
/// 可读 code (kebab-case, 稳定字符串, 用于客户端定向处理).
#[derive(Debug, Error)]
pub enum AppError {
    // Option B (2026-06-04 B4): Display == 原始内层 message, 不含类别前缀.
    // 类别由 IntoResponse 的 `code` 字段承载 (parts()). 这样旧 tuple route 迁移
    // 到 AppError 时 wire `error` 文本保持不变 (纯加性: 只多了 code 字段).
    // 类别信息在日志侧由 #[derive(Debug)] 的 variant 名保留.

    /// 400 Bad Request — 输入校验失败 / 参数错误 / 路径不合法.
    #[error("{0}")]
    BadRequest(String),

    /// 401 Unauthorized — 缺 token / 未登录 / vault 锁定 (需要解锁前置).
    #[error("{0}")]
    Unauthorized(String),

    /// 403 Forbidden — 有 token 但无权限 (membership tier 不够 / plugin 未购买).
    #[error("{0}")]
    Forbidden(String),

    /// 404 Not Found — 资源不存在 (item id / project id / plugin slug).
    #[error("{0}")]
    NotFound(String),

    /// 409 Conflict — 资源已存在 / state 不匹配 (e.g. vault already initialized).
    #[error("{0}")]
    Conflict(String),

    /// 413 Payload Too Large — 上传体积超限 (file upload / chat context).
    #[error("{0}")]
    PayloadTooLarge(String),

    /// 422 Unprocessable Entity — 语义校验失败 (输入合规但业务规则拒绝).
    #[error("{0}")]
    Unprocessable(String),

    /// 429 Too Many Requests — 速率限制 / 资源配额上限 (e.g. 单 item 批注数上限).
    #[error("{0}")]
    TooManyRequests(String),

    /// 502 Bad Gateway — 调上游服务 (Ollama / cloud accounts / plugin hub) 失败.
    #[error("{0}")]
    BadGateway(String),

    /// 503 Service Unavailable — 系统组件初始化中 / 后台任务繁忙 / 资源不足.
    #[error("{0}")]
    ServiceUnavailable(String),

    /// 500 Internal Server Error — fallback. 用于 anyhow / 未分类的 attune-core
    /// error. 客户端不应特殊处理, 显示通用 "服务器内部错误" 即可.
    #[error("{0}")]
    Internal(String),

    /// 结构化错误 (Option 2, 2026-06-04 B4) — 携带 `error` 之外额外字段的响应
    /// (backpressure `retry_after_seconds` / agent `code`+`message`+`agent_id` /
    /// `hint`). `body` 原样作为响应体, `status` 原样 → wire 字节级保持, 让 rich-error
    /// tuple 也能统一走 AppError 而不丢任何字段. 用 `AppError::detailed(status, body)`
    /// 构造.
    #[error("{status}")]
    Detailed {
        status: StatusCode,
        body: serde_json::Value,
    },
}

impl AppError {
    /// 构造结构化错误 (携带额外字段, wire 字节级保持). 见 `Detailed` variant.
    pub fn detailed(status: StatusCode, body: serde_json::Value) -> Self {
        AppError::Detailed { status, body }
    }

    /// 将 AppError 映射到 HTTP status + 稳定 code 字符串 (客户端契约).
    /// `Detailed` 由 `into_response` 短路处理, 不经此函数 (此 arm 仅为穷尽性).
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            AppError::Detailed { status, .. } => (*status, "detailed"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad-request"),
            AppError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not-found"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "payload-too-large"),
            AppError::Unprocessable(_) => (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable"),
            AppError::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, "too-many-requests"),
            AppError::BadGateway(_) => (StatusCode::BAD_GATEWAY, "bad-gateway"),
            AppError::ServiceUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "service-unavailable"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

/// 统一 IntoResponse: HTTP status + `{"error": msg, "code": kebab}` shape.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Detailed 原样输出 body (wire 字节级保持), 不套 {"error","code"} shape.
        if let AppError::Detailed { status, body } = self {
            return (status, Json(body)).into_response();
        }
        let (status, code) = self.parts();
        let msg = self.to_string();
        (status, Json(json!({"error": msg, "code": code}))).into_response()
    }
}

/// AppResult alias — route handler 返回类型简化.
pub type AppResult<T> = Result<T, AppError>;

// ── From impl: 让 `?` 自动转换 ──────────────────────────────────────────────

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        // io::Error 大多是底层 fs / network 故障, 默认 Internal. 路由侧若需要
        // 区分 (e.g. NotFound), 应在调用处显式 .map_err(AppError::NotFound).
        match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound(e.to_string()),
            std::io::ErrorKind::PermissionDenied => AppError::Forbidden(e.to_string()),
            _ => AppError::Internal(format!("io: {e}")),
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::BadRequest(format!("json: {e}"))
    }
}

impl From<attune_core::error::VaultError> for AppError {
    fn from(e: attune_core::error::VaultError) -> Self {
        // attune-core VaultError 多数是 crypto / store 失败 → Internal;
        // 个别 (NotFound / Locked / InvalidPassword / Sealed) 是用户可恢复,
        // 走对应 status 让客户端能针对性提示.
        use attune_core::error::VaultError;
        match e {
            VaultError::NotFound(s) => AppError::NotFound(s),
            VaultError::Locked => AppError::Unauthorized("vault locked".into()),
            VaultError::Sealed => AppError::ServiceUnavailable("vault not initialized".into()),
            VaultError::InvalidPassword => AppError::Unauthorized("invalid password".into()),
            VaultError::AlreadyInitialized => AppError::Conflict("vault already initialized".into()),
            VaultError::AlreadyUnlocked => AppError::Conflict("vault already unlocked".into()),
            VaultError::SessionExpired | VaultError::SessionInvalid => {
                AppError::Unauthorized(e.to_string())
            }
            VaultError::InvalidInput(s) => AppError::BadRequest(s),
            // F-17: an OutboundGate refusal is a user-policy block (privacy
            // setting off / vault locked / L0-to-cloud), not a server fault →
            // 403 Forbidden so the client can surface "you disabled this".
            VaultError::OutboundBlocked(s) => AppError::Forbidden(s),
            _ => AppError::Internal(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn bad_request_into_response_has_correct_status_and_code() {
        let resp = AppError::BadRequest("bad input".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], "bad-request");
        assert!(v["error"].as_str().unwrap().contains("bad input"));
    }

    #[tokio::test]
    async fn not_found_into_response_has_correct_status_and_code() {
        let resp = AppError::NotFound("item xyz".into()).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], "not-found");
    }

    #[test]
    fn io_error_not_found_maps_to_app_not_found() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::NotFound(_)));
    }

    #[test]
    fn io_error_permission_denied_maps_to_forbidden() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "perm");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::Forbidden(_)));
    }

    #[tokio::test]
    async fn too_many_requests_maps_to_429_with_code() {
        let resp = AppError::TooManyRequests("rate limit".into()).into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], "too-many-requests");
        assert_eq!(v["error"], "rate limit");
    }

    /// Option B 契约: wire `error` 字段是原始 message, 不带类别前缀
    /// (类别由 `code` 承载). 防止迁移后客户端显示 "bad request: ..." 噪声.
    #[tokio::test]
    async fn wire_message_has_no_category_prefix() {
        let resp = AppError::BadRequest("message too long".into()).into_response();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"], "message too long"); // 精确相等, 非 contains
        assert_eq!(v["code"], "bad-request");
    }

    /// Detailed 契约 (Option 2): status + body 原样输出, wire 字节级保持.
    /// 用于迁移 rich-error tuple (backpressure retry 信号 / agent 结构化拒绝)
    /// 不丢任何额外字段.
    #[tokio::test]
    async fn detailed_preserves_status_and_full_body() {
        let original = json!({
            "error": "embedding queue backpressure",
            "pending_embeddings": 12000,
            "retry_after_seconds": 30,
        });
        let resp = AppError::detailed(StatusCode::SERVICE_UNAVAILABLE, original.clone())
            .into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, original); // 整个 body 字节级一致 (含额外字段)
    }
}
