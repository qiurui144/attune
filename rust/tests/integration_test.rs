use tempfile::TempDir;
use attune_core::error::VaultError;
use attune_core::vault::{Vault, VaultState};

fn setup_vault() -> (Vault, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("data/vault.db");
    let config_dir = tmp.path().join("config");
    let vault = Vault::open(&db_path, &config_dir).unwrap();
    (vault, tmp)
}

#[test]
fn e2e_full_lifecycle() {
    let (vault, _tmp) = setup_vault();

    // 1. Initial state: SEALED
    assert_eq!(vault.state(), VaultState::Sealed);

    // 2. Setup → UNLOCKED
    vault.setup("master-pw-123").unwrap();
    assert_eq!(vault.state(), VaultState::Unlocked);

    // 3. Insert encrypted item
    let dek = vault.dek_db().unwrap();
    let id = vault.store().insert_item(
        &dek, "我的笔记", "这是机密内容：API key = sk-12345",
        Some("https://notes.example.com"), "note", Some("notes.example.com"),
        Some(&["工作".into(), "密钥".into()]),
    ).unwrap();

    // 4. Read back — content decrypted
    let item = vault.store().get_item(&dek, &id).unwrap().unwrap();
    assert_eq!(item.title, "我的笔记");
    assert_eq!(item.content, "这是机密内容：API key = sk-12345");
    assert_eq!(item.tags.unwrap(), vec!["工作", "密钥"]);

    // 5. Lock → LOCKED
    vault.lock().unwrap();
    assert_eq!(vault.state(), VaultState::Locked);

    // 6. DEK inaccessible when locked
    assert!(matches!(vault.dek_db(), Err(VaultError::Locked)));

    // 7. Unlock with wrong password → still LOCKED
    assert!(vault.unlock("wrong-pw").is_err());
    assert_eq!(vault.state(), VaultState::Locked);

    // 8. Unlock with correct password → UNLOCKED + data intact
    let token = vault.unlock("master-pw-123").unwrap();
    assert!(!token.is_empty());
    assert_eq!(vault.state(), VaultState::Unlocked);

    let dek2 = vault.dek_db().unwrap();
    let item2 = vault.store().get_item(&dek2, &id).unwrap().unwrap();
    assert_eq!(item2.content, "这是机密内容：API key = sk-12345");

    // 9. Session token valid
    vault.verify_session(&token).unwrap();

    // 10. Change password
    vault.change_password("master-pw-123", "new-password").unwrap();
    vault.lock().unwrap();
    assert!(vault.unlock("master-pw-123").is_err());
    vault.unlock("new-password").unwrap();
    let dek3 = vault.dek_db().unwrap();
    let item3 = vault.store().get_item(&dek3, &id).unwrap().unwrap();
    assert_eq!(item3.content, "这是机密内容：API key = sk-12345", "Data survives password change");

    // 11. Delete item
    assert!(vault.store().delete_item(&id).unwrap());
    assert!(vault.store().get_item(&dek3, &id).unwrap().is_none());
    assert_eq!(vault.store().item_count().unwrap(), 0);
}

#[test]
fn e2e_content_encrypted_at_rest() {
    let (vault, tmp) = setup_vault();
    vault.setup("pw").unwrap();

    let dek = vault.dek_db().unwrap();
    let distinctive_title = "DistinctivePlaintextTitleForVerification";
    vault.store().insert_item(&dek, distinctive_title, "SUPER_SECRET_CONTENT_THAT_MUST_BE_ENCRYPTED", None, "note", None, None).unwrap();

    // Flush WAL to main database file so we can inspect it
    vault.store().checkpoint().ok();

    // Read raw SQLite file bytes
    let db_path = tmp.path().join("data/vault.db");
    let raw_bytes = std::fs::read(&db_path).unwrap();
    let raw_str = String::from_utf8_lossy(&raw_bytes);

    // Content should NOT appear as plaintext
    assert!(
        !raw_str.contains("SUPER_SECRET_CONTENT_THAT_MUST_BE_ENCRYPTED"),
        "Content should be encrypted at rest in the SQLite file"
    );

    // Title SHOULD be plaintext (by design)
    assert!(
        raw_str.contains(distinctive_title),
        "Title should be stored in plaintext (by design)"
    );
}

#[test]
fn e2e_multiple_items() {
    let (vault, _tmp) = setup_vault();
    vault.setup("pw").unwrap();
    let dek = vault.dek_db().unwrap();

    for i in 0..10 {
        vault.store().insert_item(
            &dek, &format!("Item {i}"), &format!("Content {i}"), None, "note", None, None,
        ).unwrap();
    }

    assert_eq!(vault.store().item_count().unwrap(), 10);
    let items = vault.store().list_items(5, 0).unwrap();
    assert_eq!(items.len(), 5);
    let items_page2 = vault.store().list_items(5, 5).unwrap();
    assert_eq!(items_page2.len(), 5);
}
