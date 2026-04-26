//! Project REST API integration test
//!
//! 仅测接口存在 + vault locked → 403/401（happy path 留 Phase D E2E 用 Playwright 完成
//! vault setup → unlock → CRUD 全链路）。
//!
//! 设计取舍：vault setup/unlock 涉及 device-secret 生成 + Argon2 KDF，纯 Rust
//! integration test 不便复现完整 setup 流程；这里只验证 axum routing 注册正确，
//! 即 5 个端点 URL 都能命中 handler 并被 vault_guard middleware 拦截返 403/401。
//! 完整 happy path 在 Sprint 1 Phase D 由 Playwright 驱动真实 UI 完成。

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_endpoints_locked_vault_returns_403() {
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

    let base = format!("http://127.0.0.1:{}/api/v1/projects", port);

    // 接受 403（vault locked）或 401（auth missing）— 任一都说明端点存在 + middleware 拦截
    let acceptable = |status: u16| status == 401 || status == 403;

    // GET /api/v1/projects
    let resp = reqwest::Client::new()
        .get(&base)
        .send()
        .await
        .expect("list");
    assert!(
        acceptable(resp.status().as_u16()),
        "GET /api/v1/projects: expected 401 or 403 (vault locked), got {}",
        resp.status()
    );

    // POST /api/v1/projects
    let resp = reqwest::Client::new()
        .post(&base)
        .json(&serde_json::json!({"title": "test", "kind": "case"}))
        .send()
        .await
        .expect("create");
    assert!(
        acceptable(resp.status().as_u16()),
        "POST /api/v1/projects: expected 401 or 403, got {}",
        resp.status()
    );

    // GET /api/v1/projects/some-id
    let resp = reqwest::Client::new()
        .get(format!("{}/some-id", base))
        .send()
        .await
        .expect("get");
    assert!(
        acceptable(resp.status().as_u16()),
        "GET /api/v1/projects/:id: expected 401 or 403, got {}",
        resp.status()
    );

    // GET /api/v1/projects/some-id/files
    let resp = reqwest::Client::new()
        .get(format!("{}/some-id/files", base))
        .send()
        .await
        .expect("list files");
    assert!(
        acceptable(resp.status().as_u16()),
        "GET /api/v1/projects/:id/files: expected 401 or 403, got {}",
        resp.status()
    );

    // POST /api/v1/projects/some-id/files
    let resp = reqwest::Client::new()
        .post(format!("{}/some-id/files", base))
        .json(&serde_json::json!({"file_id": "f1", "role": "evidence"}))
        .send()
        .await
        .expect("add file");
    assert!(
        acceptable(resp.status().as_u16()),
        "POST /api/v1/projects/:id/files: expected 401 or 403, got {}",
        resp.status()
    );

    // GET /api/v1/projects/some-id/timeline
    let resp = reqwest::Client::new()
        .get(format!("{}/some-id/timeline", base))
        .send()
        .await
        .expect("list timeline");
    assert!(
        acceptable(resp.status().as_u16()),
        "GET /api/v1/projects/:id/timeline: expected 401 or 403, got {}",
        resp.status()
    );

    handle.abort();
}
