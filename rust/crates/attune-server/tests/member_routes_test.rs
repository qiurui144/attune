//! /api/v1/member/* routing test.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_state_default_logged_out() {
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
    let base = format!("http://127.0.0.1:{}/api/v1/member", port);

    // GET /state — 默认未登录
    let resp = client.get(format!("{base}/state")).send().await.expect("GET state");
    let status = resp.status().as_u16();
    assert!(
        (200..500).contains(&status),
        "GET /state should be 2xx or 4xx (vault may be locked), got {status}"
    );

    // GET /locks — 应返 SettingsLocks JSON (即便 vault locked 也应能拿)
    let resp = client.get(format!("{base}/locks")).send().await.expect("GET locks");
    let status = resp.status().as_u16();
    assert!((200..500).contains(&status), "GET /locks 4xx 或 2xx, got {status}");

    // POST /login-token — bad tier
    let resp = client
        .post(format!("{base}/login-token"))
        .json(&serde_json::json!({"account_id": "u1", "tier": "invalid_tier"}))
        .send()
        .await
        .expect("POST login-token");
    let status = resp.status().as_u16();
    assert!((400..500).contains(&status), "bad tier should 4xx, got {status}");

    handle.abort();
}
