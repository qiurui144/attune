//! 集中错误响应 helper。
//!
//! route 层不应把 VaultError / 内部异常的 to_string() 直接回给客户端。
//! VaultError 可能含文件路径、crypto 细节（AesGcm tag 失败、Argon2 参数等），
//! 暴露给 Chrome 扩展 / Web UI 是 fingerprinting + reconnaissance 风险。
//!
//! 本模块提供统一 helper：
//! - `vault_locked()` → 403 "vault locked or unavailable"（不分 actually-locked 还是 dek 派生失败）
//! - `internal(scope, e)` → 500 "internal server error"，**只 log 内部 e** 不上 wire
//!
//! 统一消息让客户端能可靠 grep/i18n，同时 server 端日志保留完整诊断。
//!
//! 渐进迁移：现 120 处 `e.to_string()` 改造成本高，本 helper 优先用于
//! vault 入口 + 新增 route。其他路径作 W5 followup 渐进迁移。

use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

pub type RouteError = (StatusCode, Json<serde_json::Value>);

/// vault unlocked 检查失败统一响应。不区分 "locked" / "dek 派生失败" / "keystore missing"。
pub fn vault_locked() -> RouteError {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error": "vault locked or unavailable"})),
    )
}

/// 内部服务器错误统一响应。`scope` 进 log 用于诊断，不出现在响应。
/// 用法：`.map_err(|e| internal("clear_web_search_cache", e))`
pub fn internal<E: std::fmt::Display>(scope: &'static str, e: E) -> RouteError {
    tracing::warn!(scope = scope, "{}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal server error"})),
    )
}

/// 客户端输入错误，message 是 user-facing 的，已经过审查不含 PII。
pub fn bad_request(message: impl Into<String>) -> RouteError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": message.into()})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_locked_message_does_not_leak_details() {
        let (code, body) = vault_locked();
        assert_eq!(code, StatusCode::FORBIDDEN);
        let s = serde_json::to_string(&body.0).unwrap();
        // 关键：响应体不应含 "AesGcm" / "Argon2" / "/home" / "keystore" 等内部细节
        assert!(!s.contains("AesGcm"));
        assert!(!s.contains("Argon2"));
        assert!(!s.contains("/home"));
        assert!(!s.contains("keystore"));
        assert!(s.contains("vault locked"));
    }

    #[test]
    fn internal_error_response_is_generic() {
        let (code, body) = internal("test_scope", "AesGcm: invalid tag at byte 42");
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
        let s = serde_json::to_string(&body.0).unwrap();
        assert!(!s.contains("AesGcm"), "内部错误细节不应出现在响应: {s}");
        assert!(!s.contains("byte 42"));
        assert_eq!(s, r#"{"error":"internal server error"}"#);
    }
}
