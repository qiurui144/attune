//! Office helper REST 端点 happy-path 烟测 (D1.4).
//!
//! 仅测端点存在 + 错误码契约 (per spec §3 + CLAUDE.md error contract).
//! 不测真实 OCR/ASR 输出（那些走 D3 golden gate）— PP-OCR + whisper-cli
//! 在 CI 不一定可用, 此处的目标是端点契约而非引擎。
//!
//! 6 个 case:
//!   - ocr_missing_file_returns_400
//!   - ocr_unsupported_format_returns_400
//!   - ocr_id_card_missing_subtype_returns_400
//!   - transcribe_missing_file_returns_400
//!   - get_unknown_job_returns_404
//!   - delete_unknown_job_returns_404

use std::sync::Arc;
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become ready in 10s");
}

async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let vault = attune_core::vault::Vault::open_memory(tmp.path())
        .expect("open in-memory vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    // vault_guard middleware blocks all /api/v1/* until vault is set up.
    // POST /vault/setup creates + unlocks the vault.
    let client = reqwest::Client::new();
    let setup = client
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({"password": "office-test-pw-1234567890"}))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup.status().as_u16(), 200, "vault setup must succeed");

    // tmp lives via leak — test process exits so OS reclaims.
    std::mem::forget(tmp);
    (base, handle)
}

// ─── OCR endpoint contract ───────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_missing_file_returns_400_invalid_input() {
    let (base, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // multipart with no `file` part
    let form = reqwest::multipart::Form::new().text("profile", "receipt");

    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status().as_u16(), 400, "missing file should be 400");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "invalid-input");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_unsupported_format_returns_400() {
    let (base, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let file_part = reqwest::multipart::Part::bytes(b"junk".to_vec())
        .file_name("evil.exe")
        .mime_str("application/octet-stream")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("profile", "receipt");

    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "unsupported-format");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_id_card_missing_subtype_returns_400() {
    let (base, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let file_part = reqwest::multipart::Part::bytes(b"\x89PNG\r\n\x1a\n".to_vec())
        .file_name("id.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("profile", "id_card");
    // intentionally no id_card_subtype

    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "id-card-subtype-required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_empty_file_returns_400() {
    let (base, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let file_part = reqwest::multipart::Part::bytes(Vec::<u8>::new())
        .file_name("empty.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("profile", "receipt");

    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "empty-file");
}

// ─── Transcribe endpoint contract ───────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcribe_missing_file_returns_400() {
    let (base, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/office/transcribe", base))
        .json(&serde_json::json!({"file_path": "/nonexistent/xyz.mp3"}))
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "invalid-input");
}

// ─── Jobs endpoint contract ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_unknown_job_returns_404() {
    let (base, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/v1/office/jobs/no-such-id", base))
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status().as_u16(), 404);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "not-found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_unknown_job_returns_404() {
    let (base, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!("{}/api/v1/office/jobs/no-such-id", base))
        .send()
        .await
        .expect("delete");
    assert_eq!(resp.status().as_u16(), 404);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["code"], "not-found");
}
