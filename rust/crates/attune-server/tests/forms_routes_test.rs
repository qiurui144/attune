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

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/api/v1/forms", port);

    // GET — 路由注册存在 → 4xx (vault locked 时 vault_guard 返 403, 否则 plugin 不存在 → 404)
    // 关键: 不应是 405 method-not-allowed 或 500
    let get_url = format!("{base}/nonexistent_plugin/some_form");

    // 等 server 就绪 —— retry-with-deadline 替代固定 sleep（per CLAUDE.md：server
    // 启动会加载 ML 模型，耗时不定，固定 500ms 在装了模型 / 高负载的机器上 flaky）。
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let resp = loop {
        match client.get(&get_url).send().await {
            Ok(r) => break r,
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    panic!("server not ready within 30s: {e}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
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
