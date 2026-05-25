//! D5.3 — L2 failure recovery test.
//!
//! Spec §6.3:
//!   - corrupt PDF bytes → ocr-engine-failed or invalid-input, no panic
//!   - 0-byte file → empty-file
//!   - JobRegistry::cancel_all_running flips running job to Cancelled (server restart)

use std::sync::Arc;
use std::time::Duration;

async fn wait_for_server(base: &str) {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(format!("{}/health", base)).send().await {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server not ready");
}

async fn start_server() -> String {
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).unwrap();
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    let base = format!("http://127.0.0.1:{port}");
    wait_for_server(&base).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({"password": "recovery-test-pw-1234567890"}))
        .send()
        .await
        .unwrap();
    std::mem::forget(tmp);
    base
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_pdf_bytes_returns_error_not_panic() {
    let base = start_server().await;
    let client = reqwest::Client::new();
    // Garbage bytes with .pdf extension → either ocr-engine-failed or invalid-input,
    // but NOT a 500 panic.
    let file_part = reqwest::multipart::Part::bytes(b"%PDF-garbage-not-a-real-pdf\x00\xff\xfe".to_vec())
        .file_name("corrupt.pdf")
        .mime_str("application/pdf")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("profile", "document");
    let resp = client
        .post(format!("{}/api/v1/office/ocr", base))
        .multipart(form)
        .send()
        .await
        .expect("post");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("json");
    // Could be 200 (PDF lines empty per D1 limitation) OR 500 ocr-engine-failed.
    // Critical: NOT a panic, server still alive.
    assert!(status == 200 || status == 500 || status == 400,
        "unexpected status {status}: {body}");
    if status != 200 {
        let code = body["code"].as_str().expect("error response has code");
        assert!(
            ["ocr-engine-failed", "invalid-input", "pdf-parse-failed"].contains(&code),
            "unexpected code {code}: {body}"
        );
    }
    // Server still alive: send a sanity request
    let resp2 = client.get(format!("{}/health", base)).send().await.expect("alive");
    assert_eq!(resp2.status().as_u16(), 200, "server died after corrupt PDF");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_byte_file_returns_empty_file_code() {
    let base = start_server().await;
    let client = reqwest::Client::new();
    let file_part = reqwest::multipart::Part::bytes(Vec::<u8>::new())
        .file_name("zero.png")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn random_junk_bytes_with_image_ext_no_panic() {
    let base = start_server().await;
    let client = reqwest::Client::new();
    // Random bytes posed as PNG — likely ocr-engine-failed or successful but empty.
    let junk = (0..200u8).collect::<Vec<u8>>();
    let file_part = reqwest::multipart::Part::bytes(junk)
        .file_name("junk.png")
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
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 500,
        "expected 200 or 500, got {status}"
    );
    // Server still alive
    let alive = client.get(format!("{}/health", base)).send().await.expect("alive");
    assert_eq!(alive.status().as_u16(), 200);
}

#[test]
fn registry_cancel_all_running_simulates_server_restart() {
    use attune_core::office_job_queue::{Job, JobRegistry, JobState};

    let registry = JobRegistry::new();
    // Pre-restart: 3 in-flight, 1 done
    let mut q = Job::new("queued".into());
    q.state = JobState::Queued;
    registry.insert(q);
    let mut r = Job::new("running".into());
    r.state = JobState::Running;
    registry.insert(r);
    let mut d = Job::new("done".into());
    d.state = JobState::Done;
    d.result_json = Some("{}".into());
    registry.insert(d);

    // Simulate server restart
    registry.cancel_all_running();

    assert_eq!(registry.get("queued").unwrap().state, JobState::Cancelled);
    assert_eq!(registry.get("running").unwrap().state, JobState::Cancelled);
    // Done preserved (terminal state)
    assert_eq!(registry.get("done").unwrap().state, JobState::Done);

    // Both former-in-flight have restart warning
    for id in ["queued", "running"] {
        let j = registry.get(id).unwrap();
        assert!(
            j.warnings.iter().any(|w| w.contains("server restarted")),
            "job '{id}' missing restart warning"
        );
    }
}

#[test]
fn registry_resubmit_after_failure_creates_new_job_id() {
    use attune_core::office_job_queue::{Job, JobError, JobRegistry, JobState};

    let registry = JobRegistry::new();
    let mut failed = Job::new("attempt-1".into());
    failed.state = JobState::Failed;
    failed.error = Some(JobError {
        message: "transient".into(),
        code: "asr-engine-failed".into(),
    });
    registry.insert(failed);

    // User resubmits → new job with different id (caller's responsibility)
    let retry = Job::new("attempt-2".into());
    registry.insert(retry);

    assert_eq!(registry.get("attempt-1").unwrap().state, JobState::Failed);
    assert_eq!(registry.get("attempt-2").unwrap().state, JobState::Queued);
    // Both coexist
    assert!(registry.in_flight_count() >= 1);
}
