//! 验证 vault_setup 直接返 session token（首次安装 UX）
//!
//! Sprint 2 Phase 1 smoke 发现：vault_setup 设密码成功后 vault 已是内存 Unlocked
//! 状态但**不返 token**，客户端无法继续操作（必须 restart server 后再 unlock 才能拿
//! token）。本测试 lock 住该行为：setup 直接返 `{status, state, token}`，且 token
//! 可用于带 vault_guard middleware 的 protected 端点（如 /api/v1/projects）。

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~64s, full Argon2id setup); R19 nightly only — run with --include-ignored"]
async fn vault_setup_returns_session_token() {
    // 隔离 vault 数据目录：dirs crate 依赖 XDG_DATA_HOME / XDG_CONFIG_HOME / HOME，
    // 三个都重定向到独立 tempdir，避免污染开发者 home 下的真实 vault.db。
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

    // POST /api/v1/vault/setup
    let setup_url = format!("http://127.0.0.1:{}/api/v1/vault/setup", port);
    let resp = reqwest::Client::new()
        .post(&setup_url)
        .json(&serde_json::json!({"password": "test-pass-12345"}))
        .send()
        .await
        .expect("setup");
    assert_eq!(resp.status(), 200, "vault_setup should return 200");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["state"], "unlocked");
    let token = body["token"]
        .as_str()
        .expect("token field must be present in vault_setup response");
    assert!(!token.is_empty(), "token should not be empty");
    assert!(token.len() > 20, "token too short to be real session token");

    // 用 setup 拿到的 token 调一个 vault_guard 保护的端点（/projects 走 vault_guard）
    let projects_url = format!("http://127.0.0.1:{}/api/v1/projects", port);
    let resp = reqwest::Client::new()
        .get(&projects_url)
        .header("authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("projects");
    assert_eq!(
        resp.status(),
        200,
        "token from vault_setup should authorize protected endpoint /projects"
    );

    handle.abort();
}
