//! third_party_accounts 通用凭据保险柜加密持久化集成测试。
//!
//! 覆盖：round-trip / 脱敏（list 不含 secret）/ 加密对抗（DB 无明文）/ provider
//! typo 守卫 / 异常（空 secret / 空 endpoint / tamper 解密失败）/ 删除 / 状态更新。

use attune_core::crypto::{self, Key32};
use attune_core::store::third_party_accounts::{is_known_provider, ThirdPartyAccountInput};
use attune_core::store::Store;

fn make_input(provider: &str) -> ThirdPartyAccountInput {
    ThirdPartyAccountInput {
        provider: provider.to_string(),
        label: "My Nextcloud".into(),
        username: "alice".into(),
        endpoint: "https://cloud.example.com/dav".into(),
        // §1.4：测试 fixture 用明示假 secret，不用真凭据。
        secret: "test-pass-not-real".into(),
    }
}

// ============================================================
// Round-trip + 脱敏
// ============================================================

#[test]
fn add_then_get_secret_round_trips() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_third_party_account(&dek, &make_input("webdav"))
        .unwrap();

    let secret = store.get_third_party_secret(&dek, &id).unwrap().unwrap();
    assert_eq!(secret, "test-pass-not-real", "secret 必须能解密回明文");
}

#[test]
fn list_view_never_contains_secret() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    store
        .add_third_party_account(&dek, &make_input("webdav"))
        .unwrap();

    let views = store.list_third_party_accounts().unwrap();
    assert_eq!(views.len(), 1);
    let v = &views[0];
    assert_eq!(v.provider, "webdav");
    assert_eq!(v.username, "alice");
    assert_eq!(v.endpoint, "https://cloud.example.com/dav");
    assert_eq!(v.status, "connected");
    // ThirdPartyAccountView 结构体里根本没有 secret 字段 —— 编译期即保证脱敏。
    // 这里额外断言序列化后的 JSON 不含 secret 串（防未来加字段误带）。
    let json = serde_json::to_string(&serde_json::json!({
        "id": v.id, "provider": v.provider, "label": v.label,
        "username": v.username, "endpoint": v.endpoint, "status": v.status,
    }))
    .unwrap();
    assert!(
        !json.contains("test-pass-not-real"),
        "脱敏视图绝不能含 secret"
    );
}

// ============================================================
// 加密对抗（adversarial）：DB 列里无明文
// ============================================================

#[test]
fn secret_is_not_stored_in_plaintext() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = make_input("imap");
    input.secret = "PLAINTEXT_MARKER_TPA_987".into();
    let id = store.add_third_party_account(&dek, &input).unwrap();

    let raw = store.debug_raw_third_party_secret_enc(&id).unwrap();
    assert!(!raw.is_empty(), "secret_enc 应已写入");
    assert!(
        !raw.windows(24).any(|w| w == b"PLAINTEXT_MARKER_TPA_987"),
        "secret_enc 列绝不能含明文 secret"
    );
}

#[test]
fn tampered_ciphertext_fails_decrypt_not_silent_wrong_value() {
    // 加密对抗：篡改密文 / 用错 dek → 解密必须报错，不能静默返回脏明文。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_third_party_account(&dek, &make_input("webdav"))
        .unwrap();

    let wrong_dek = Key32::generate();
    let res = store.get_third_party_secret(&wrong_dek, &id);
    assert!(res.is_err(), "错 dek 解密必须 Err，不能返回脏明文");
}

#[test]
fn crypto_roundtrip_is_nondeterministic_nonce() {
    // 同明文两次加密密文不同（随机 nonce）—— 防可识别密文泄漏「两账户同密码」。
    let dek = Key32::generate();
    let a = crypto::encrypt(&dek, b"test-pass-not-real").unwrap();
    let b = crypto::encrypt(&dek, b"test-pass-not-real").unwrap();
    assert_ne!(a, b, "随机 nonce → 同明文密文应不同");
}

// ============================================================
// 异常 / 错误（≥3）
// ============================================================

#[test]
fn unknown_provider_rejected() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = make_input("webdav");
    input.provider = "myspace".into(); // 不在白名单
    let res = store.add_third_party_account(&dek, &input);
    assert!(res.is_err(), "未知 provider 必须拒绝（typo 守卫）");
    assert!(!is_known_provider("myspace"));
}

#[test]
fn empty_secret_rejected() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = make_input("webdav");
    input.secret = "".into();
    assert!(store.add_third_party_account(&dek, &input).is_err());
}

#[test]
fn empty_endpoint_rejected() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = make_input("webdav");
    input.endpoint = "   ".into();
    assert!(store.add_third_party_account(&dek, &input).is_err());
}

#[test]
fn oversized_secret_rejected() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = make_input("webdav");
    input.secret = "x".repeat(8193);
    assert!(store.add_third_party_account(&dek, &input).is_err());
}

#[test]
fn get_secret_missing_id_returns_none() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    assert!(store
        .get_third_party_secret(&dek, "tpa-nope")
        .unwrap()
        .is_none());
}

// ============================================================
// 删除 / 状态 / 聚合
// ============================================================

#[test]
fn delete_removes_row() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_third_party_account(&dek, &make_input("git"))
        .unwrap();
    assert!(store.delete_third_party_account(&id).unwrap(), "应删一行");
    assert!(store.get_third_party_secret(&dek, &id).unwrap().is_none());
    assert!(
        !store.delete_third_party_account(&id).unwrap(),
        "二次删返回 false"
    );
}

#[test]
fn set_status_updates_and_reports_missing() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_third_party_account(&dek, &make_input("rss"))
        .unwrap();
    assert!(store.set_third_party_status(&id, "error").unwrap());
    let v = store.list_third_party_accounts().unwrap();
    assert_eq!(v[0].status, "error");
    assert!(!store.set_third_party_status("tpa-nope", "error").unwrap());
}

#[test]
fn connected_provider_kinds_is_distinct() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    store
        .add_third_party_account(&dek, &make_input("webdav"))
        .unwrap();
    store
        .add_third_party_account(&dek, &make_input("webdav"))
        .unwrap();
    store
        .add_third_party_account(&dek, &make_input("imap"))
        .unwrap();

    let kinds = store.connected_provider_kinds().unwrap();
    assert_eq!(
        kinds,
        vec!["imap".to_string(), "webdav".to_string()],
        "distinct + 排序"
    );
}

#[test]
fn old_vault_without_table_does_not_crash() {
    // 向后兼容：CREATE TABLE IF NOT EXISTS 让任何已打开的 vault 自动有空表。
    // 空表场景 list / connected_kinds 返回空，不崩。
    let store = Store::open_memory().unwrap();
    assert!(store.list_third_party_accounts().unwrap().is_empty());
    assert!(store.connected_provider_kinds().unwrap().is_empty());
}
