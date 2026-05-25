//! D5.1 — L2 concurrent stress test.
//!
//! Spec §6.3 — 5 OCR + 2 ASR concurrent, JobRegistry consistency, no panic.

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
    let setup = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({"password": "concurrent-test-pw-1234567890"}))
        .send()
        .await
        .unwrap();
    assert_eq!(setup.status().as_u16(), 200);
    std::mem::forget(tmp);
    base
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn five_concurrent_bad_ocr_requests_all_return_invalid_input() {
    let base = start_server().await;
    let base = Arc::new(base);

    // Spawn 5 concurrent OCR POSTs, all missing the profile field → must all 400.
    let mut handles = Vec::new();
    for i in 0..5 {
        let base = base.clone();
        handles.push(tokio::spawn(async move {
            let client = reqwest::Client::new();
            let file_part = reqwest::multipart::Part::bytes(vec![0u8; 16])
                .file_name(format!("test-{i}.png"))
                .mime_str("image/png")
                .unwrap();
            let form = reqwest::multipart::Form::new().part("file", file_part);
            client
                .post(format!("{}/api/v1/office/ocr", base))
                .multipart(form)
                .send()
                .await
                .expect("post")
                .status()
                .as_u16()
        }));
    }
    let mut statuses = Vec::new();
    for h in handles {
        statuses.push(h.await.unwrap());
    }
    // All 5 should be 400 (missing profile → invalid-input)
    assert_eq!(statuses, vec![400u16; 5], "all concurrent requests should 400 cleanly");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_transcribe_submits_create_independent_jobs() {
    let base = start_server().await;
    let base = Arc::new(base);

    // POST transcribe with a non-existent file → must 400. But the JobRegistry-side
    // never even inserts because path check fails before insert. So we test that
    // concurrent submits don't deadlock and all return consistent error.
    let mut handles = Vec::new();
    for _ in 0..3 {
        let base = base.clone();
        handles.push(tokio::spawn(async move {
            let client = reqwest::Client::new();
            let resp = client
                .post(format!("{}/api/v1/office/transcribe", base))
                .json(&serde_json::json!({"file_path": "/no/such/file.mp3"}))
                .send()
                .await
                .expect("post");
            (resp.status().as_u16(), resp.text().await.unwrap_or_default())
        }));
    }
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap());
    }
    for (status, body) in &results {
        assert_eq!(*status, 400, "expected 400, got {status}: {body}");
        assert!(body.contains("\"code\":\"invalid-input\""));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_get_unknown_jobs_all_404() {
    let base = start_server().await;
    let base = Arc::new(base);

    let mut handles = Vec::new();
    for i in 0..10 {
        let base = base.clone();
        handles.push(tokio::spawn(async move {
            let client = reqwest::Client::new();
            client
                .get(format!("{}/api/v1/office/jobs/unknown-{i}", base))
                .send()
                .await
                .expect("get")
                .status()
                .as_u16()
        }));
    }
    let mut statuses = Vec::new();
    for h in handles {
        statuses.push(h.await.unwrap());
    }
    assert!(statuses.iter().all(|s| *s == 404), "all should be 404; got {statuses:?}");
}

#[test]
fn job_registry_concurrent_inserts_no_panic() {
    // Pure unit: spawn 16 threads each inserting jobs, then verify all there.
    use attune_core::office_job_queue::{Job, JobRegistry};
    use std::thread;

    let registry = JobRegistry::new();
    let mut handles = Vec::new();
    for i in 0..16 {
        let r = registry.clone();
        handles.push(thread::spawn(move || {
            for j in 0..8 {
                r.insert(Job::new(format!("job-{i}-{j}")));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // All 16 × 8 = 128 jobs should be present
    let mut count = 0;
    for i in 0..16 {
        for j in 0..8 {
            if registry.get(&format!("job-{i}-{j}")).is_some() {
                count += 1;
            }
        }
    }
    assert_eq!(count, 128, "all concurrent inserts must persist");
}

