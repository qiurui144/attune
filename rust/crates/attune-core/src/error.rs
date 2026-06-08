use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault is sealed: run setup first")]
    Sealed,

    #[error("vault is locked: unlock required")]
    Locked,

    #[error("vault is already unlocked")]
    AlreadyUnlocked,

    #[error("vault is already initialized")]
    AlreadyInitialized,

    #[error("invalid password")]
    InvalidPassword,

    #[error("device secret missing: {0}")]
    DeviceSecretMissing(String),

    #[error("device secret mismatch")]
    DeviceSecretMismatch,

    #[error("session expired")]
    SessionExpired,

    #[error("session invalid")]
    SessionInvalid,

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("llm unavailable: {0}")]
    LlmUnavailable(String),

    /// An [`crate::OutboundGate`] refused a network egress (disabled by the
    /// user / vault locked / L0-tagged content to a cloud destination /
    /// redactor unavailable). Distinct from `LlmUnavailable` so the server can
    /// map it to 403 Forbidden (user-policy refusal), not 502/503 (upstream
    /// failure).
    #[error("outbound blocked: {0}")]
    OutboundBlocked(String),

    #[error("classification failed: {0}")]
    Classification(String),

    #[error("taxonomy error: {0}")]
    Taxonomy(String),

    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("model load error: {0}")]
    ModelLoad(String),
}

pub type Result<T> = std::result::Result<T, VaultError>;

/// wasm-safe leaf 错误 → native VaultError 桥接(方向 core→leaf 单一,无环)。
/// 内部 agent 返回 `AgentResult` 时,在 `crate::error::Result` 的 `?` 边界自动转,
/// 调用方无感。`AgentError` 是 `#[non_exhaustive]`,catch-all arm 兜底未来新增变体
/// (新增变体应在此显式补 arm —— 兜底仅防编译失败,不是放任不映射)。
/// An OutboundGate refusal propagates as [`VaultError::OutboundBlocked`] so the
/// `?` operator works at every egress call site and the server maps it to 403.
impl From<crate::outbound_gate::OutboundError> for VaultError {
    fn from(e: crate::outbound_gate::OutboundError) -> Self {
        VaultError::OutboundBlocked(e.to_string())
    }
}

impl From<attune_agent_sdk::AgentError> for VaultError {
    fn from(e: attune_agent_sdk::AgentError) -> Self {
        use attune_agent_sdk::AgentError as A;
        match e {
            A::InvalidInput(s) => VaultError::InvalidInput(s),
            A::Computation(s) => VaultError::Classification(s),
            // serde_json::Error 无法从 String 凭空回造 → 落 InvalidInput(带前缀保信息)
            A::Serialization(s) => VaultError::InvalidInput(format!("serialization: {s}")),
            A::RedLine(s) => VaultError::InvalidInput(format!("red line: {s}")),
            other => VaultError::InvalidInput(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        assert_eq!(VaultError::Sealed.to_string(), "vault is sealed: run setup first");
        assert_eq!(VaultError::Locked.to_string(), "vault is locked: unlock required");
        assert_eq!(VaultError::InvalidPassword.to_string(), "invalid password");
        assert_eq!(
            VaultError::DeviceSecretMissing("/path".into()).to_string(),
            "device secret missing: /path"
        );
    }

    #[test]
    fn error_display_all_static_variants() {
        // 锁定所有无参 variant 的 display 输出 (PR 改文案会破坏前端 error code 映射)
        assert_eq!(VaultError::AlreadyUnlocked.to_string(), "vault is already unlocked");
        assert_eq!(VaultError::AlreadyInitialized.to_string(), "vault is already initialized");
        assert_eq!(VaultError::DeviceSecretMismatch.to_string(), "device secret mismatch");
        assert_eq!(VaultError::SessionExpired.to_string(), "session expired");
        assert_eq!(VaultError::SessionInvalid.to_string(), "session invalid");
    }

    #[test]
    fn error_display_parameterized_variants() {
        assert_eq!(VaultError::Crypto("decrypt failed".into()).to_string(), "crypto error: decrypt failed");
        assert_eq!(VaultError::LlmUnavailable("timeout".into()).to_string(), "llm unavailable: timeout");
        assert_eq!(VaultError::Classification("bad json".into()).to_string(), "classification failed: bad json");
        assert_eq!(VaultError::Taxonomy("cycle".into()).to_string(), "taxonomy error: cycle");
        assert_eq!(VaultError::NotFound("item-123".into()).to_string(), "not found: item-123");
        assert_eq!(VaultError::InvalidInput("empty".into()).to_string(), "invalid input: empty");
        assert_eq!(VaultError::ModelLoad("size".into()).to_string(), "model load error: size");
    }

    // From<io::Error> conversion
    #[test]
    fn error_from_io_preserves_message() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let v: VaultError = io_err.into();
        assert!(matches!(v, VaultError::Io(_)));
        let msg = v.to_string();
        assert!(msg.starts_with("io error:"));
        assert!(msg.contains("missing file"));
    }

    // From<serde_json::Error>
    #[test]
    fn error_from_serde_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("{not valid").unwrap_err();
        let v: VaultError = json_err.into();
        assert!(matches!(v, VaultError::Json(_)));
        assert!(v.to_string().starts_with("json error:"));
    }

    // From<serde_yaml::Error>
    #[test]
    fn error_from_serde_yaml() {
        let yaml_err = serde_yaml::from_str::<serde_yaml::Value>("[unclosed").unwrap_err();
        let v: VaultError = yaml_err.into();
        assert!(matches!(v, VaultError::Yaml(_)));
        assert!(v.to_string().starts_with("yaml parse error:"));
    }

    // Result<T, VaultError> type alias 可用
    #[test]
    fn result_alias_usable() {
        fn helper(ok: bool) -> Result<i32> {
            if ok { Ok(42) } else { Err(VaultError::InvalidInput("nope".into())) }
        }
        assert_eq!(helper(true).unwrap(), 42);
        assert!(helper(false).is_err());
    }

    // I18n: Unicode error messages 不破坏
    #[test]
    fn error_unicode_in_messages() {
        let v = VaultError::InvalidInput("中文输入 🔒".into());
        let msg = v.to_string();
        assert!(msg.contains("中文输入"));
        assert!(msg.contains("🔒"));
    }

    // Debug impl 也可用 (用于 anyhow / tracing 上下文)
    #[test]
    fn error_debug_format_includes_variant() {
        let v = VaultError::Locked;
        let dbg = format!("{v:?}");
        assert_eq!(dbg, "Locked");
    }

    // 边界: 空字符串 payload — 不 panic
    #[test]
    fn error_empty_string_payload() {
        let v = VaultError::Crypto(String::new());
        assert_eq!(v.to_string(), "crypto error: ");
    }

    // 超长 payload — 不 panic / 不截断
    #[test]
    fn error_huge_payload_preserved() {
        let big = "x".repeat(100_000);
        let v = VaultError::NotFound(big.clone());
        assert!(v.to_string().contains(&big));
    }

    // From<AgentError> for VaultError — 每变体映射断言(桥接 wasm-safe leaf 错误)
    #[test]
    fn from_agent_error_invalid_input() {
        let v: VaultError = attune_agent_sdk::AgentError::InvalidInput("x".into()).into();
        assert!(matches!(v, VaultError::InvalidInput(ref s) if s == "x"));
    }

    #[test]
    fn from_agent_error_computation_maps_classification() {
        let v: VaultError = attune_agent_sdk::AgentError::Computation("overflow".into()).into();
        assert!(matches!(v, VaultError::Classification(ref s) if s == "overflow"));
    }

    #[test]
    fn from_agent_error_serialization_maps_invalid_input_prefixed() {
        // serde_json::Error 无法从 String 回造 → 落 InvalidInput 带 "serialization:" 前缀
        let v: VaultError = attune_agent_sdk::AgentError::Serialization("bad".into()).into();
        assert!(matches!(v, VaultError::InvalidInput(ref s) if s == "serialization: bad"));
    }

    #[test]
    fn from_agent_error_red_line_maps_invalid_input_prefixed() {
        let v: VaultError = attune_agent_sdk::AgentError::RedLine("usury".into()).into();
        assert!(matches!(v, VaultError::InvalidInput(ref s) if s == "red line: usury"));
    }

    // ? 边界自动桥接 — agent 返回 AgentResult,在 crate::error::Result 上下文 ? 转
    #[test]
    fn agent_error_propagates_into_vault_result() {
        fn agent_call() -> attune_agent_sdk::AgentResult<i32> {
            Err(attune_agent_sdk::AgentError::Computation("boom".into()))
        }
        fn vault_ctx() -> Result<i32> {
            let v = agent_call()?; // From<AgentError> for VaultError 自动转
            Ok(v)
        }
        assert!(matches!(vault_ctx(), Err(VaultError::Classification(ref s)) if s == "boom"));
    }
}
