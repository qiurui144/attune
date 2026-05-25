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
}
