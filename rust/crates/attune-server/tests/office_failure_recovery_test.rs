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
fn durable_recovery_simulates_server_restart() {
    // G5: restart no longer mass-cancels — recover_on_boot requeues idempotent
    // (at_least_once) Running jobs; Queued + Done are preserved untouched.
    use attune_core::office_job_queue::{JobKind, JobState};
    use attune_core::store::Store;

    let store = Store::open_memory().unwrap();
    let running = store.enqueue_job(JobKind::Asr, "{}", 5, None).unwrap();
    let queued = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    let done = store.enqueue_job(JobKind::Asr, "{}", 9, None).unwrap();
    let c = store.claim_next_job().unwrap().unwrap();
    assert_eq!(c.id, done);
    store.complete_job(&done, "{}").unwrap();
    store.claim_next_job().unwrap(); // `running` (prio 5) → Running

    // Simulate server restart (install_job_store runs this once per boot).
    let summary = store.recover_on_boot().unwrap();
    assert_eq!(summary.requeued, 1);
    assert_eq!(summary.failed_no_retry, 0);

    assert_eq!(store.get_job(&running).unwrap().unwrap().state, JobState::Queued);
    assert_eq!(store.get_job(&queued).unwrap().unwrap().state, JobState::Queued);
    // Done preserved (terminal state)
    assert_eq!(store.get_job(&done).unwrap().unwrap().state, JobState::Done);
}

#[test]
fn durable_retry_after_failure_requeues_or_coexists() {
    // After a failure the operator can requeue the SAME job (id preserved,
    // error cleared), or the user can resubmit as a new job — both coexist.
    use attune_core::office_job_queue::{JobKind, JobState};
    use attune_core::store::Store;

    let store = Store::open_memory().unwrap();
    let attempt1 = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    store.claim_next_job().unwrap();
    store.fail_job(&attempt1, "asr-engine-failed", "transient").unwrap();
    assert_eq!(store.get_job(&attempt1).unwrap().unwrap().state, JobState::Failed);

    // Path A: user resubmits → new independent job id.
    let attempt2 = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    assert_ne!(attempt1, attempt2);
    assert_eq!(store.get_job(&attempt2).unwrap().unwrap().state, JobState::Queued);
    assert_eq!(store.get_job(&attempt1).unwrap().unwrap().state, JobState::Failed);

    // Path B: operator requeues the failed job in place (error cleared).
    assert!(store.requeue_job(&attempt1).unwrap());
    let j = store.get_job(&attempt1).unwrap().unwrap();
    assert_eq!(j.state, JobState::Queued);
    assert!(j.error.is_none(), "requeue clears the previous error");
    assert_eq!(store.in_flight_job_count().unwrap(), 2);
}
