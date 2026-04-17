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
}
