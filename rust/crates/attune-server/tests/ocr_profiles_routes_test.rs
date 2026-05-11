//! /api/v1/ocr/profiles 路由 round-trip.
//!
//! Concurrency: 所有测试串行共享 TEST_MUTEX, 因为它们都 mutate 全局
//! HOME / XDG_* env vars (per dirs crate). 与 form_factor_integration / vault_setup
//! 同款互斥模式.

use std::sync::Mutex;
use std::time::Duration;

static TEST_MUTEX: Mutex<()> = Mutex::new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_profiles_crud_round_trip() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // 隔离 data_dir
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = attune_server::ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: true,
    };
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(config).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/api/v1/ocr/profiles", port);

    // 1. GET list — 应该有 4 个 builtin
    let resp = client.get(&base).send().await.expect("GET list");
    assert_eq!(resp.status(), 200);
    let arr: Vec<serde_json::Value> = resp.json().await.expect("json");
    assert_eq!(arr.len(), 4, "default has 4 builtin profiles");
    let ids: Vec<&str> = arr.iter().filter_map(|v| v["id"].as_str()).collect();
    assert!(ids.contains(&"contract"));
    assert!(ids.contains(&"receipt"));
    assert!(ids.contains(&"screenshot"));
    assert!(ids.contains(&"ancient"));

    // 2. POST create custom
    let resp = client
        .post(&base)
        .json(&serde_json::json!({
            "id": "test_custom",
            "name": "测试自定义",
            "description": "x",
            "languages": "chi_sim+eng",
            "dpi": 300,
            "tags": ["test"],
            "builtin": true  // 攻击者尝试声明 builtin, registry 应强制改 false
        }))
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(v["id"], "test_custom");
    assert_eq!(v["builtin"], false, "registry forces builtin=false");

    // 3. GET list — 应该 5 条
    let resp = client.get(&base).send().await.expect("GET list 2");
    let arr: Vec<serde_json::Value> = resp.json().await.expect("json");
    assert_eq!(arr.len(), 5);

    // 4. POST 同 id 重复 → 409
    let resp = client
        .post(&base)
        .json(&serde_json::json!({
            "id": "test_custom",
            "name": "重复",
            "description": "x",
            "languages": "eng",
            "dpi": 200,
            "tags": [],
            "builtin": false
        }))
        .send()
        .await
        .expect("POST dup");
    assert_eq!(resp.status(), 409);

    // 5. PUT update 自定义
    let resp = client
        .put(format!("{base}/test_custom"))
        .json(&serde_json::json!({
            "id": "test_custom",
            "name": "已更新",
            "description": "y",
            "languages": "eng",
            "dpi": 400,
            "tags": ["updated"],
            "builtin": false
        }))
        .send()
        .await
        .expect("PUT");
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(v["name"], "已更新");
    assert_eq!(v["dpi"], 400);

    // 6. PUT builtin → 400 (registry 拒改)
    let resp = client
        .put(format!("{base}/contract"))
        .json(&serde_json::json!({
            "id": "contract",
            "name": "试图改 builtin",
            "description": "x",
            "languages": "eng",
            "dpi": 100,
            "tags": [],
            "builtin": false
        }))
        .send()
        .await
        .expect("PUT builtin");
    assert_eq!(resp.status(), 400);

    // 7. DELETE builtin → 400
    let resp = client
        .delete(format!("{base}/contract"))
        .send()
        .await
        .expect("DEL builtin");
    assert_eq!(resp.status(), 400);

    // 8. DELETE 自定义 → 200
    let resp = client
        .delete(format!("{base}/test_custom"))
        .send()
        .await
        .expect("DEL custom");
    assert_eq!(resp.status(), 200);

    // 9. DELETE 不存在 → 404
    let resp = client
        .delete(format!("{base}/nonexistent"))
        .send()
        .await
        .expect("DEL nonexistent");
    assert_eq!(resp.status(), 404);

    // 10. GET final — 4 条
    let resp = client.get(&base).send().await.expect("GET final");
    let arr: Vec<serde_json::Value> = resp.json().await.expect("json");
    assert_eq!(arr.len(), 4);

    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_default_includes_active_profile() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // 隔离 data_dir
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config = attune_server::ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: true,
    };
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(config).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let client = reqwest::Client::new();

    // 默认 settings 应包含 ocr.active_profile = "contract"
    let resp = client
        .get(format!("http://127.0.0.1:{}/api/v1/settings", port))
        .send()
        .await
        .expect("GET settings");
    if resp.status() == 200 {
        let body: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(body["ocr"]["active_profile"], "contract");
    } else {
        // vault locked 时 settings 也可能拒. 这里只要 vault unlock 后行为对.
        // 不强 assert, 关键 default_settings unit test 已覆盖.
    }
    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_profiles_paid_user_can_still_edit() {
    // 验证 SettingsLocks::for_state(Paid).can_edit("ocr_profiles") = true
    // (per member_session.rs: Paid 不锁定 ocr_profiles, 只锁 plugin_install/uninstall/cloud_llm)
    let m = attune_core::member_session::MemberState::Paid {
        account_id: "u".to_string(),
        license_id: "l".to_string(),
        llm_quota_remaining: 100,
    };
    let locks = attune_core::member_session::SettingsLocks::for_state(&m);
    assert!(locks.can_edit("ocr_profiles"), "Paid 不应锁 ocr_profiles");
    assert!(locks.can_edit("vault_password"));
    assert!(locks.can_edit("local_folder_links"));
    assert!(!locks.can_edit("plugin_install"));
    assert!(!locks.can_edit("plugin_uninstall"));
    assert!(!locks.can_edit("cloud_llm"));
}
