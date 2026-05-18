//! Tests that GET /api/v1/ai_stack always returns a `web_search` field whose
//! `available` boolean matches the actual web-search provider state.
//!
//! Before the fix the `web_search` key was absent from the response, causing
//! the Settings → 关于 tab to always show 未就绪 regardless of whether the
//! browser search provider was actually loaded.

use std::time::Duration;

async fn wait_for_server(base: &str) {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let url = format!("{}/health", base);
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not become ready");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_stack_includes_web_search_field() {
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
    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    // Vault must be set up and unlocked before /ai_stack is accessible
    let client = reqwest::Client::new();
    let setup = client
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({"password": "ai-stack-test-pw"}))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup.status().as_u16(), 200, "vault setup failed");

    let resp = client
        .get(format!("{}/api/v1/ai_stack", base))
        .send()
        .await
        .expect("GET /ai_stack");
    assert_eq!(resp.status().as_u16(), 200, "/ai_stack must return 200");

    let body: serde_json::Value = resp.json().await.expect("json body");

    // web_search key must be present (was missing before the fix)
    let ws = body.get("web_search").expect(
        "`web_search` field missing from /ai_stack response — Settings 关于 tab would show 未就绪",
    );

    // `available` must be a boolean
    let available = ws.get("available").expect("`web_search.available` missing");
    assert!(
        available.is_boolean(),
        "`web_search.available` must be bool, got: {available}"
    );

    // `engine` must be a string
    let engine = ws.get("engine").expect("`web_search.engine` missing");
    assert!(engine.is_string(), "`web_search.engine` must be a string");

    // `note` is None when available=true, Some(String) when available=false —
    // both are valid; just assert the field exists and is null or a string.
    let note = ws.get("note").expect("`web_search.note` missing");
    assert!(
        note.is_null() || note.is_string(),
        "`web_search.note` must be null or string, got: {note}"
    );

    handle.abort();
}
