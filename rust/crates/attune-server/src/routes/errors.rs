//! 集中错误响应 helper。
//!
//! route 层不应把 VaultError / 内部异常的 to_string() 直接回给客户端。
//! VaultError 可能含文件路径、crypto 细节（AesGcm tag 失败、Argon2 参数等），
//! 暴露给 Chrome 扩展 / Web UI 是 fingerprinting + reconnaissance 风险。
//!
//! 本模块提供统一 helper（B4 2026-06-04 起统一返回 [`AppError`]，保留原"log 详情 +
//! wire 通用消息"安全行为，并额外带上稳定 `code` 字段）：
//! - `vault_locked()` → 403 "vault locked or unavailable"（不分 actually-locked 还是 dek 派生失败）
//! - `internal(scope, e)` → 500 "internal server error"，**只 log 内部 e** 不上 wire
//! - `bad_request(msg)` → 400，message 是 user-facing 已审查不含 PII
//!
//! 统一消息让客户端能可靠 grep/i18n，同时 server 端日志保留完整诊断。

use crate::error::AppError;

/// vault unlocked 检查失败统一响应。不区分 "locked" / "dek 派生失败" / "keystore missing"。
pub fn vault_locked() -> AppError {
    AppError::Forbidden("vault locked or unavailable".into())
}

/// 内部服务器错误统一响应。`scope` 进 log 用于诊断，**不出现在响应**（防 crypto/path
/// fingerprinting）。用法：`.map_err(|e| internal("clear_web_search_cache", e))`
pub fn internal<E: std::fmt::Display>(scope: &'static str, e: E) -> AppError {
    tracing::warn!(scope = scope, "{}", e);
    AppError::Internal("internal server error".into())
}

/// 客户端输入错误，message 是 user-facing 的，已经过审查不含 PII。
pub fn bad_request(message: impl Into<String>) -> AppError {
    AppError::BadRequest(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn vault_locked_message_does_not_leak_details() {
        let resp = vault_locked().into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let s = String::from_utf8_lossy(&body);
        // 关键：响应体不应含 "AesGcm" / "Argon2" / "/home" / "keystore" 等内部细节
        assert!(!s.contains("AesGcm"));
        assert!(!s.contains("Argon2"));
        assert!(!s.contains("/home"));
        assert!(!s.contains("keystore"));
        assert!(s.contains("vault locked"));
    }

    #[tokio::test]
    async fn internal_error_response_is_generic() {
        let resp = internal("test_scope", "AesGcm: invalid tag at byte 42").into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let s = String::from_utf8_lossy(&body);
        // 内部错误细节不应出现在响应（安全行为保留）
        assert!(!s.contains("AesGcm"), "内部错误细节不应出现在响应: {s}");
        assert!(!s.contains("byte 42"));
        // Option B: wire 现含 code 字段, error 消息仍是通用 "internal server error"
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"], "internal server error");
        assert_eq!(v["code"], "internal");
    }
}
