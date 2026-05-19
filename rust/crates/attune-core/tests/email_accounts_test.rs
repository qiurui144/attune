//! email_accounts 表加密持久化集成测试。
//!
//! email_accounts.dir_id REFERENCES bound_dirs(id)，每个测试先用
//! bind_directory 建 bound_dirs 行，再 upsert email 账户配置。

use attune_core::crypto::Key32;
use attune_core::store::email_accounts::EmailAccountInput;
use attune_core::store::Store;

fn make_account(store: &Store, path_suffix: &str) -> (String, EmailAccountInput) {
    let dir_id = store
        .bind_directory(&format!("email:imap.gmail.com/{path_suffix}"), false, &["eml"])
        .unwrap();
    let input = EmailAccountInput {
        dir_id: dir_id.clone(),
        host: "imap.gmail.com".into(),
        port: 993,
        username: "alice@gmail.com".into(),
        password: "app-specific-pw".into(),
        folders: vec!["INBOX".into(), "Sent".into()],
        corpus_domain: "general".into(),
    };
    (dir_id, input)
}

#[test]
fn upsert_then_get_round_trips_with_decrypted_password() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let (dir_id, input) = make_account(&store, "alice");
    store.upsert_email_account(&dek, &input).unwrap();

    let got = store
        .get_email_account(&dek, &dir_id)
        .unwrap()
        .expect("account row exists");
    assert_eq!(got.host, "imap.gmail.com");
    assert_eq!(got.port, 993);
    assert_eq!(got.username, "alice@gmail.com");
    assert_eq!(got.password, "app-specific-pw", "password 必须能解密回明文");
    assert_eq!(got.folders, vec!["INBOX".to_string(), "Sent".to_string()]);
    assert_eq!(got.corpus_domain, "general");
}

#[test]
fn password_is_not_stored_in_plaintext() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let (dir_id, mut input) = make_account(&store, "plain-check");
    input.password = "PLAINTEXT_MARKER_XYZ".into();
    store.upsert_email_account(&dek, &input).unwrap();

    let raw = store.debug_raw_email_password_enc(&dir_id).unwrap();
    assert!(!raw.is_empty(), "password_enc 应已写入");
    assert!(
        !raw.windows(20).any(|w| w == b"PLAINTEXT_MARKER_XYZ"),
        "password_enc 列绝不能含明文密码"
    );
}

#[test]
fn list_email_accounts_returns_all_configured() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    for i in 0..3 {
        let (_, input) = make_account(&store, &format!("user{i}"));
        store.upsert_email_account(&dek, &input).unwrap();
    }
    let all = store.list_email_accounts(&dek).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn upsert_is_idempotent_on_dir_id() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let (dir_id, mut input) = make_account(&store, "idem");
    store.upsert_email_account(&dek, &input).unwrap();
    input.host = "imap.fastmail.com".into();
    input.password = "new-pw".into();
    store.upsert_email_account(&dek, &input).unwrap();

    let all = store.list_email_accounts(&dek).unwrap();
    assert_eq!(all.len(), 1, "同 dir_id 二次 upsert 不新增行");
    let got = store.get_email_account(&dek, &dir_id).unwrap().unwrap();
    assert_eq!(got.host, "imap.fastmail.com");
    assert_eq!(got.password, "new-pw");
}

#[test]
fn delete_email_account_removes_row() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let (dir_id, input) = make_account(&store, "del");
    store.upsert_email_account(&dek, &input).unwrap();
    store.set_folder_uid(&dir_id, "INBOX", 999).unwrap();
    store.delete_email_account(&dir_id).unwrap();
    assert!(store.get_email_account(&dek, &dir_id).unwrap().is_none());
    assert_eq!(
        store.get_folder_uid(&dir_id, "INBOX").unwrap(),
        0,
        "email_folder_uids 应随账户级联删除"
    );
}

#[test]
fn touch_email_account_sync_sets_last_sync() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let (dir_id, input) = make_account(&store, "touch");
    store.upsert_email_account(&dek, &input).unwrap();

    let row = store.get_email_account(&dek, &dir_id).unwrap().unwrap();
    assert!(row.last_sync.is_none(), "刚 upsert 的账户 last_sync 应为 None");

    store.touch_email_account_sync(&dir_id).unwrap();

    let row = store.get_email_account(&dek, &dir_id).unwrap().unwrap();
    assert!(row.last_sync.is_some(), "touch 后 last_sync 应为 Some(_)");
}

#[test]
fn folder_uid_cursor_round_trips() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let (dir_id, input) = make_account(&store, "uid");
    store.upsert_email_account(&dek, &input).unwrap();

    assert_eq!(store.get_folder_uid(&dir_id, "INBOX").unwrap(), 0, "未设置时默认 0");
    store.set_folder_uid(&dir_id, "INBOX", 1234).unwrap();
    assert_eq!(store.get_folder_uid(&dir_id, "INBOX").unwrap(), 1234);
    store.set_folder_uid(&dir_id, "INBOX", 5678).unwrap();
    assert_eq!(store.get_folder_uid(&dir_id, "INBOX").unwrap(), 5678, "upsert 覆盖");
    assert_eq!(store.get_folder_uid(&dir_id, "Sent").unwrap(), 0, "不同 folder 独立");
}
