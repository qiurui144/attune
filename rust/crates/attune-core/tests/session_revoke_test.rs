#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use attune_core::vault::Vault;

    #[test]
    fn token_revoked_after_lock() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("password123").unwrap();
        // 重新 lock 再 unlock 以获取 token
        vault.lock().unwrap();
        let token = vault.unlock("password123").unwrap();

        // token 在 unlock 后应该有效
        assert!(vault.verify_session(&token).is_ok());

        // lock 之后 token 应该失效
        vault.lock().unwrap();
        // vault 已 locked，verify_session 返回错误
        let result = vault.verify_session(&token);
        assert!(result.is_err());
    }

    #[test]
    fn new_token_valid_after_relock_unlock() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("password123").unwrap();
        vault.lock().unwrap();
        let old_token = vault.unlock("password123").unwrap();

        vault.lock().unwrap();
        let new_token = vault.unlock("password123").unwrap();

        // 旧 token nonce 不匹配，应失效
        assert!(vault.verify_session(&old_token).is_err());
        // 新 token 应有效
        assert!(vault.verify_session(&new_token).is_ok());
    }
}
