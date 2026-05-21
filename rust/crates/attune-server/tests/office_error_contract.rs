//! D3.4a — L1 Error Contract Gate.
//!
//! Spec §3.1 + §6.3. Every 4xx/5xx response body MUST be `{error: str, code: kebab}`.
//! 区别于 office_happy_path.rs 的烟测, 此处更系统化覆盖错误码契约矩阵.

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

async fn start_test_server() -> String {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    // Vault setup (vault_guard middleware blocks /api/v1/* otherwise)
    let client = reqwest::Client::new();
    let setup = client
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({"password": "office-err-pw-1234567890"}))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup.status().as_u16(), 200, "vault setup must succeed");

    std::mem::forget(tmp);
    base
}

/// 检查响应是 {error: str, code: kebab-string} 形态.
async fn assert_error_envelope(resp: reqwest::Response, expected_status: u16, expected_code: &str) {
    assert_eq!(
        resp.status().as_u16(),
        expected_status,
        "expected HTTP {expected_status}"
    );
    let body: serde_json::Value = resp.json().await.expect("json body");
    let error_str = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("response missing 'error' field: {body}"));
    assert!(
        !error_str.is_empty(),
        "error message empty for code={expected_code}"
    );
    let code_str = body
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("response missing 'code' field: {body}"));
    assert_eq!(
        code_str, expected_code,
        "expected code={expected_code} got code={code_str}"
    );
    // kebab-string sanity: lowercase + hyphens + no underscores/spaces
    assert!(
        code_str.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "code '{code_str}' is not kebab-case (lowercase + digits + hyphens only)"
    );
    assert!(
        !code_str.starts_with('-') && !code_str.ends_with('-'),
        "code '{code_str}' has leading/trailing hyphen"
    );
}

// ─── OCR error matrix ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_missing_profile_returns_invalid_input() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    let file_part = reqwest::multipart::Part::bytes(b"\x89PNG\r\n\x1a\n".to_vec())
        .file_name("test.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("file", file_part);
    // intentionally no `profile` part
    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    assert_error_envelope(resp, 400, "invalid-input").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_unknown_profile_returns_profile_not_found() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    let file_part = reqwest::multipart::Part::bytes(b"\x89PNG\r\n\x1a\n".to_vec())
        .file_name("test.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("profile", "totally_unknown_xyz");
    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    assert_error_envelope(resp, 404, "profile-not-found").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_exe_file_returns_unsupported_format() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    let file_part = reqwest::multipart::Part::bytes(b"MZ\x90\x00".to_vec())
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
    assert_error_envelope(resp, 400, "unsupported-format").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_empty_file_returns_empty_file() {
    let base = start_test_server().await;
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
    assert_error_envelope(resp, 400, "empty-file").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_id_card_missing_subtype_returns_dedicated_code() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    let file_part = reqwest::multipart::Part::bytes(b"\x89PNG\r\n\x1a\n".to_vec())
        .file_name("id.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("profile", "id_card");
    // missing id_card_subtype
    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    assert_error_envelope(resp, 400, "id-card-subtype-required").await;
}

// ─── Transcribe error matrix ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcribe_missing_file_path_returns_invalid_input() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    // file_path field is required (struct default = no, but path validation finds missing)
    let resp = client
        .post(format!("{}/api/v1/office/transcribe", base))
        .json(&serde_json::json!({"file_path": ""}))
        .send()
        .await
        .expect("post");
    assert_error_envelope(resp, 400, "invalid-input").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcribe_nonexistent_file_returns_invalid_input() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/office/transcribe", base))
        .json(&serde_json::json!({"file_path": "/totally/nonexistent/path.mp3"}))
        .send()
        .await
        .expect("post");
    assert_error_envelope(resp, 400, "invalid-input").await;
}

// ─── Jobs endpoint matrix ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_unknown_job_returns_not_found() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/v1/office/jobs/no-such-job-id", base))
        .send()
        .await
        .expect("get");
    assert_error_envelope(resp, 404, "not-found").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_unknown_job_returns_not_found() {
    let base = start_test_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!("{}/api/v1/office/jobs/no-such-job-id", base))
        .send()
        .await
        .expect("delete");
    assert_error_envelope(resp, 404, "not-found").await;
}

// ─── Kebab-case rigidity verification ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_error_codes_are_strict_kebab_case() {
    // Sanity check that the codes we use across the suite are all valid kebab.
    for code in [
        "invalid-input",
        "empty-file",
        "unsupported-format",
        "id-card-subtype-required",
        "profile-not-found",
        "not-found",
        "job-already-completed",
        "job-already-cancelled",
        "ocr-engine-failed",
        "asr-engine-failed",
        "internal-error",
    ] {
        assert!(
            code.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "code '{code}' not kebab"
        );
        assert!(
            !code.contains('_') && !code.contains(' '),
            "code '{code}' contains forbidden char"
        );
        assert!(!code.starts_with('-') && !code.ends_with('-'), "code '{code}' bad hyphen");
    }
}
