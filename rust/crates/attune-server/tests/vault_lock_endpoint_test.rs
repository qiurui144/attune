//! Regression test for UI-S8 (2026-05-02): "Lock vault" 菜单 no-op fix。
//!
//! Sidebar Account menu 的 Lock vault 按钮之前 onClick 仅 onClose()，**不调** /api/v1/vault/lock。
//! 修复后必须真实调 server endpoint，server vault.state 必须从 unlocked → locked。
//! 本测试只验后端契约（API），UI 层走 Playwright（docs/screenshots-2026-05-02/amd-fix-01-*.png）。

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~30s, Argon2id setup); R19 nightly only — run with --include-ignored"]
async fn vault_lock_endpoint_transitions_state() {
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
        no_auth: false,
    };
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(config).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // 1. setup vault → unlocked + token
    let resp = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({"password": "test-pass-12345"}))
        .send()
        .await
        .expect("setup");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert_eq!(body["state"], "unlocked");

    // 2. status 确认 unlocked
    let resp = client
        .get(format!("{base}/api/v1/vault/status"))
        .send()
        .await
        .unwrap();
    let s: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(s["state"], "unlocked", "vault must be unlocked after setup");

    // 3. POST /api/v1/vault/lock — 这是 UI Lock vault 按钮必须打的请求
    let resp = client
        .post(format!("{base}/api/v1/vault/lock"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("lock request must reach server");
    assert_eq!(resp.status(), 200, "lock should return 200 OK");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["state"], "locked", "lock endpoint must report locked");

    // 4. status 必须显示 locked（**这是 UI-S8 的 regression assertion**）
    let resp = client
        .get(format!("{base}/api/v1/vault/status"))
        .send()
        .await
        .unwrap();
    let s: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        s["state"], "locked",
        "REGRESSION (UI-S8): vault.state must transition to 'locked' after /vault/lock — \
         如果是 unlocked 说明 Lock vault 按钮再次变成 no-op"
    );

    // 5. lock 后旧 token 必须失效（不能继续访问 protected 端点）
    let resp = client
        .get(format!("{base}/api/v1/projects"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "post-lock token should be rejected (got {})",
        resp.status()
    );

    handle.abort();
}
