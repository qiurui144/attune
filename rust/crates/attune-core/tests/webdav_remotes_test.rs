//! webdav_remotes 表加密持久化集成测试。
//!
//! webdav_remotes.dir_id REFERENCES bound_dirs(id)，每个测试先用
//! bind_directory 建 bound_dirs 行，再 upsert remote 配置。

use attune_core::crypto::Key32;
use attune_core::store::webdav_remotes::WebDavRemoteInput;
use attune_core::store::Store;

#[test]
fn upsert_then_get_round_trips_with_decrypted_password() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();

    // FK 前置：建 bound_dirs 行，取得真实 dir_id。
    let dir_id = store
        .bind_directory("webdav:https://dav.example.com/remote.php/dav/files/u/", false, &["md"])
        .unwrap();

    let input = WebDavRemoteInput {
        dir_id: dir_id.clone(),
        url: "https://dav.example.com/remote.php/dav/files/u/".into(),
        username: Some("alice".into()),
        password: Some("s3cr3t-app-pw".into()),
        depth: 1,
        corpus_domain: "legal".into(),
    };
    store.upsert_webdav_remote(&dek, &input).unwrap();

    let got = store
        .get_webdav_remote(&dek, &dir_id)
        .unwrap()
        .expect("remote row exists");
    assert_eq!(got.url, input.url);
    assert_eq!(got.username.as_deref(), Some("alice"));
    assert_eq!(got.password.as_deref(), Some("s3cr3t-app-pw"), "password 必须能解密回明文");
    assert_eq!(got.depth, 1);
    assert_eq!(got.corpus_domain, "legal");
}

#[test]
fn password_is_not_stored_in_plaintext() {
    // 安全回归：password_enc 列不得出现明文密码字节。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();

    let dir_id = store
        .bind_directory("webdav:https://dav.example.com/plain-check/", false, &["md"])
        .unwrap();

    let input = WebDavRemoteInput {
        dir_id: dir_id.clone(),
        url: "https://dav.example.com/plain-check/".into(),
        username: Some("bob".into()),
        password: Some("PLAINTEXT_MARKER_XYZ".into()),
        depth: 1,
        corpus_domain: "general".into(),
    };
    store.upsert_webdav_remote(&dek, &input).unwrap();
    let raw = store.debug_raw_webdav_password_enc(&dir_id).unwrap();
    assert!(!raw.is_empty(), "password_enc 应已写入");
    assert!(
        !raw.windows(20).any(|w| w == b"PLAINTEXT_MARKER_XYZ"),
        "password_enc 列绝不能含明文密码"
    );
}

#[test]
fn list_webdav_remotes_returns_all_configured() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    for i in 0..3 {
        let url = format!("https://dav.example.com/u{i}/");
        let dir_id = store
            .bind_directory(&format!("webdav:{url}"), false, &["md"])
            .unwrap();
        store
            .upsert_webdav_remote(
                &dek,
                &WebDavRemoteInput {
                    dir_id,
                    url,
                    username: None,
                    password: None,
                    depth: 1,
                    corpus_domain: "general".into(),
                },
            )
            .unwrap();
    }
    let all = store.list_webdav_remotes(&dek).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn upsert_is_idempotent_on_dir_id() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();

    let dir_id = store
        .bind_directory("webdav:https://dav.example.com/idem/", false, &["md"])
        .unwrap();

    let mut input = WebDavRemoteInput {
        dir_id: dir_id.clone(),
        url: "https://dav.example.com/old/".into(),
        username: Some("u".into()),
        password: Some("old-pw".into()),
        depth: 1,
        corpus_domain: "general".into(),
    };
    store.upsert_webdav_remote(&dek, &input).unwrap();
    input.url = "https://dav.example.com/new/".into();
    input.password = Some("new-pw".into());
    store.upsert_webdav_remote(&dek, &input).unwrap();

    let all = store.list_webdav_remotes(&dek).unwrap();
    assert_eq!(all.len(), 1, "同 dir_id 二次 upsert 不新增行");
    let got = store.get_webdav_remote(&dek, &dir_id).unwrap().unwrap();
    assert_eq!(got.url, "https://dav.example.com/new/");
    assert_eq!(got.password.as_deref(), Some("new-pw"));
}
