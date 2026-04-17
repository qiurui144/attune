#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use attune_core::vault::Vault;

    #[test]
    fn change_password_and_relock_unlock_with_new_password() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("old_password").unwrap();

        // 变更密码
        vault.change_password("old_password", "new_password").unwrap();

        // 旧密码不能 unlock
        vault.lock().unwrap();
        assert!(vault.unlock("old_password").is_err());

        // 新密码可以 unlock
        assert!(vault.unlock("new_password").is_ok());
    }

    #[test]
    fn change_password_with_wrong_old_password_fails() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("correct_password").unwrap();
        let result = vault.change_password("wrong_password", "new_password");
        assert!(result.is_err());
    }

    #[test]
    fn change_password_with_empty_new_password_fails() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("correct_password").unwrap();
        let result = vault.change_password("correct_password", "");
        assert!(result.is_err(), "change_password with empty new_password must return Err");
        // 确保失败后 vault 仍处于 Unlocked 状态且 DEK 未损坏
        assert!(vault.dek_db().is_ok(), "vault must remain unlocked after rejection");
    }
}
