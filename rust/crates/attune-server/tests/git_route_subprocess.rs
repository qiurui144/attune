//! GitConnector route 集成测试 —— 真起 Axum server + unlocked vault，对 bind-git
//! 跑 SSRF / 错误契约（per #146 subprocess-env 教训：route 层真触发，错误从
//! spawn_blocking 子任务真实冒泡）。
//!
//! 安全契约锁定：bind-git 只接受 http(s) 托管仓 host（allowlist），拒 file:// /
//! 内网 / 云 metadata / ssh / 非 allowlist host。clone+ingest 的 happy path 全链由
//! `attune-core/tests/git_connector.rs`（真 libgit2 clone 本地 bare repo）覆盖，
//! 真平台仓（rust-lang/book / CS-Notes）走手动 / nightly soak（per spec §9）。

use std::sync::Arc;
use std::time::Duration;

async fn wait_for_server(base: &str) {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let url = format!("{base}/health");
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

async fn spawn_server() -> (String, reqwest::Client) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    wait_for_server(&base).await;

    let client = reqwest::Client::new();
    let setup = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({"password": "test-password-not-real"}))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup.status().as_u16(), 200, "vault setup failed");

    Box::leak(Box::new(tmp));
    (base, client)
}

async fn post_bind_git(base: &str, client: &reqwest::Client, url: &str) -> (u16, serde_json::Value) {
    let resp = client
        .post(format!("{base}/api/v1/index/bind-git"))
        .json(&serde_json::json!({ "url": url }))
        .send()
        .await
        .expect("bind-git request");
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_git_rejects_file_scheme() {
    // file:// 只允许在 core 测试 fixture 用；route 层 SSRF guard 拒非 http(s)。
    let (base, client) = spawn_server().await;
    let (status, body) = post_bind_git(&base, &client, "file:///etc/passwd").await;
    assert_eq!(status, 400, "file:// 应被 SSRF guard 拒");
    assert_eq!(body.get("code").and_then(|c| c.as_str()), Some("invalid-git-url"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_git_rejects_ssrf_loopback() {
    let (base, client) = spawn_server().await;
    let (status, body) = post_bind_git(&base, &client, "http://127.0.0.1:8080/o/r").await;
    assert_eq!(status, 400, "loopback 必须拒");
    let code = body.get("code").and_then(|c| c.as_str()).unwrap_or("");
    assert!(
        code == "git-url-not-allowed" || code == "invalid-git-url",
        "SSRF code, got {code}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_git_rejects_metadata_endpoint() {
    let (base, client) = spawn_server().await;
    let (status, _) =
        post_bind_git(&base, &client, "http://169.254.169.254/latest/meta-data").await;
    assert_eq!(status, 400, "云 metadata 端点必须拒");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_git_rejects_ssh_scheme() {
    let (base, client) = spawn_server().await;
    let (status, body) = post_bind_git(&base, &client, "ssh://git@github.com/o/r").await;
    assert_eq!(status, 400, "ssh scheme 必须拒 (v1 仅 https)");
    assert_eq!(body.get("code").and_then(|c| c.as_str()), Some("invalid-git-url"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_git_rejects_non_allowlisted_host() {
    let (base, client) = spawn_server().await;
    let (status, body) = post_bind_git(&base, &client, "https://evil.example.com/o/r").await;
    assert_eq!(status, 400, "allowlist 外 host 必须拒");
    assert_eq!(body.get("code").and_then(|c| c.as_str()), Some("git-url-not-allowed"));
}
