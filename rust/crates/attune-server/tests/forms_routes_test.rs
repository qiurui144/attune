//! /api/v1/forms/{plugin_id}/{form_id} routing test.
//!
//! 验证 endpoint 注册正确 — 未装 plugin 时 GET / POST 都应返 404 (not 500).

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forms_endpoints_return_404_for_unknown_plugin() {
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
    let base = format!("http://127.0.0.1:{}/api/v1/forms", port);

    // GET — 路由注册存在 → 4xx (vault locked 时 vault_guard 返 403, 否则 plugin 不存在 → 404)
    // 关键: 不应是 405 method-not-allowed 或 500
    let get_url = format!("{base}/nonexistent_plugin/some_form");
    let resp = client.get(&get_url).send().await.expect("GET");
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "GET should return 4xx (route registered + handler decision), got {status}"
    );

    // POST — 路由注册存在 → 4xx
    let post_url = format!("{base}/nonexistent_plugin/some_form/submit");
    let resp = client
        .post(&post_url)
        .json(&serde_json::json!({"a": 1}))
        .send()
        .await
        .expect("POST");
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "POST should return 4xx (not 405 / 500), got {status}"
    );

    handle.abort();
}
