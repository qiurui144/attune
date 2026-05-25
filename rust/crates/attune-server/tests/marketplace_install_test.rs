//! POST /api/v1/marketplace/plugins/{id}/install behaviour.
//!
//! P0 regression (2026-05-20): the handler used to silently return HTTP 200 +
//! mock InstallResponse when the server ran with the default `MockPluginHubProvider`,
//! making the desktop UI report "installed" while no file landed on disk. Now
//! mock-provider installs must surface HTTP 503 + `error=pluginhub_not_configured`
//! so the UI can guide the user to configure `pluginhub.url` / `license_key`.
//!
//! Marked `#[ignore]` for nightly-slow tier because vault setup runs Argon2id
//! (~60s on stock builders); same gate as `vault_setup_test.rs`.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~60s, full Argon2id setup); nightly only — run with --include-ignored"]
async fn mock_provider_install_returns_503_not_silent_success() {
    // Isolated dirs so we don't touch the developer's real ~/.local/share/attune.
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = attune_server::ServerConfig {
        host: "127.0.0.1".into(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: false,
    };
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(config).await });
    tokio::time::sleep(Duration::from_millis(600)).await;

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // 1. Setup + unlock vault — install endpoint sits behind vault_guard.
    let setup = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({"password": "test-pass-12345"}))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup.status(), 200, "vault setup should succeed");
    let setup_body: serde_json::Value = setup.json().await.expect("setup json");
    let token = setup_body["token"]
        .as_str()
        .expect("token field present")
        .to_string();

    // 2. Hit install with the default Mock provider — must NOT return 200.
    let resp = client
        .post(format!("{base}/api/v1/marketplace/plugins/law-pro/install"))
        .header("authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("POST install");

    let status = resp.status().as_u16();
    assert_ne!(
        status, 200,
        "mock provider must NOT return 200 (the P0 silent-success bug)"
    );
    assert_eq!(
        status, 503,
        "mock provider must return 503 pluginhub_not_configured, got {status}"
    );

    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(
        body.get("error").and_then(|v| v.as_str()),
        Some("pluginhub_not_configured"),
        "error code must be pluginhub_not_configured for actionable UI hint, got body: {body}"
    );
    let detail = body.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        detail.contains("pluginhub.url") && detail.contains("license_key"),
        "detail must mention pluginhub.url + license_key configure path, got: {detail}"
    );

    handle.abort();
}
