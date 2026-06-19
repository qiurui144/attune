//! 第三方账号 + 建议卡 REST API 集成测试。
//!
//! 与 projects_routes_test.rs 同取舍：纯 Rust integration test 不便复现完整
//! vault setup（device-secret + Argon2 KDF）→ unlock 流程，这里验证 4 个端点
//! URL 都命中 handler 并被 vault_guard middleware 拦截（locked vault → 401/403）。
//! 凭据加密 round-trip / 脱敏 / DB 无明文 的 happy-path 在 store 层
//! `attune-core/tests/third_party_accounts_test.rs` 已全覆盖。

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn third_party_endpoints_locked_vault_guarded() {
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
    tokio::time::sleep(Duration::from_millis(300)).await;

    let acceptable = |s: u16| s == 401 || s == 403;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // GET /api/v1/accounts
    let r = client
        .get(format!("{base}/api/v1/accounts"))
        .send()
        .await
        .expect("list accounts");
    assert!(
        acceptable(r.status().as_u16()),
        "GET /accounts expected 401/403, got {}",
        r.status()
    );

    // POST /api/v1/accounts — secret 必须不进日志（handler 不打印），这里仅验 routing。
    let r = client
        .post(format!("{base}/api/v1/accounts"))
        .json(&serde_json::json!({
            "provider": "webdav",
            "endpoint": "https://cloud.example.com/dav",
            "secret": "test-pass-not-real"
        }))
        .send()
        .await
        .expect("add account");
    assert!(
        acceptable(r.status().as_u16()),
        "POST /accounts expected 401/403, got {}",
        r.status()
    );

    // DELETE /api/v1/accounts/{id}
    let r = client
        .delete(format!("{base}/api/v1/accounts/tpa-xyz"))
        .send()
        .await
        .expect("delete account");
    assert!(
        acceptable(r.status().as_u16()),
        "DELETE /accounts/{{id}} expected 401/403, got {}",
        r.status()
    );

    // GET /api/v1/suggestions
    let r = client
        .get(format!("{base}/api/v1/suggestions"))
        .send()
        .await
        .expect("suggestions");
    assert!(
        acceptable(r.status().as_u16()),
        "GET /suggestions expected 401/403, got {}",
        r.status()
    );

    handle.abort();
}
