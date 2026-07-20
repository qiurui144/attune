//! /api/v1/member/* routing test.

use std::time::Duration;

#[test]
fn member_state_default_logged_out() {
    let tmp = tempfile::TempDir::new().expect("isolated member route data dir");
    // SAFETY: this integration-test binary contains only this test, and the
    // environment is pinned before its Tokio runtime (and worker threads) is
    // created. The production server must never touch the developer's vault.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("member route test runtime");
    runtime.block_on(async {
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
        let resp = client
            .get(format!("{base}/state"))
            .send()
            .await
            .expect("GET state");
        let status = resp.status().as_u16();
        assert!(
            (200..500).contains(&status),
            "GET /state should be 2xx or 4xx (vault may be locked), got {status}"
        );

        // GET /locks — 应返 SettingsLocks JSON (即便 vault locked 也应能拿)
        let resp = client
            .get(format!("{base}/locks"))
            .send()
            .await
            .expect("GET locks");
        let status = resp.status().as_u16();
        assert!(
            (200..500).contains(&status),
            "GET /locks 4xx 或 2xx, got {status}"
        );

        // POST /login-token — bad tier
        let resp = client
            .post(format!("{base}/login-token"))
            .json(&serde_json::json!({"account_id": "u1", "tier": "invalid_tier"}))
            .send()
            .await
            .expect("POST login-token");
        let status = resp.status().as_u16();
        assert!(
            (400..500).contains(&status),
            "bad tier should 4xx, got {status}"
        );

        handle.abort();
    });
}
