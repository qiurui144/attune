//! E2E for K3 G3① locked-mode ingest staging + drain.
//!
//! Verifies the full degrade chain over the real HTTP server:
//!   1. setup → unlock → lock (vault LOCKED)
//!   2. upload while LOCKED → 200 with status="staged" (NOT 403; file not lost)
//!   3. staging-status reports pending > 0 while LOCKED
//!   4. unlock → drain worker ingests the staged doc
//!   5. the doc becomes searchable (proves it went through the real pipeline)
//!   6. staging-status drops back to 0 (idempotent done point)
//!
//! Marked `#[ignore]` like the sibling vault E2E tests: each setup/unlock runs Argon2id
//! (~1-2s) so the test is slow; run with `--include-ignored` on nightly.

use std::time::Duration;

async fn wait_staging_pending(
    client: &reqwest::Client,
    base: &str,
    target: i64,
    timeout: Duration,
) -> i64 {
    let deadline = std::time::Instant::now() + timeout;
    let mut last = -1;
    while std::time::Instant::now() < deadline {
        if let Ok(resp) = client
            .get(format!("{base}/api/v1/vault/staging-status"))
            .send()
            .await
        {
            if let Ok(v) = resp.json::<serde_json::Value>().await {
                last = v["pending"].as_i64().unwrap_or(-1);
                if last == target {
                    return last;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    last
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~30s, Argon2id setup); nightly only — run with --include-ignored"]
async fn locked_mode_stages_and_drains_on_unlock() {
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
        // Isolate the LOCK dimension (G3): bearer auth is an orthogonal middleware
        // covered by other tests. With no_auth the upload reaches the handler so we can
        // assert the locked→stage→drain chain, not the auth gate.
        no_auth: true,
    };
    let _handle = tokio::spawn(async move { attune_server::run_in_runtime(config).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let pw = "test-pass-locked-mode-1";

    // 1. setup (auto-unlocked) → lock
    let r = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({"password": pw}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "setup");
    let token = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let r = client
        .post(format!("{base}/api/v1/vault/lock"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "lock");

    // 2. upload while LOCKED → 200 staged (not 403)
    let unique = "ZEBRA-LOCKED-MARKER-42";
    let body = format!("staged document body containing {unique} for search");
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(body.clone().into_bytes())
            .file_name("locked_note.txt")
            .mime_str("text/plain")
            .unwrap(),
    );
    let r = client
        .post(format!("{base}/api/v1/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "locked upload must be accepted (staged), not rejected"
    );
    let jv = r.json::<serde_json::Value>().await.unwrap();
    assert_eq!(
        jv["status"], "staged",
        "locked upload must report staged: {jv}"
    );
    assert!(jv["staging_id"].is_string());

    // 3. staging-status reports pending == 1 while LOCKED (no DEK needed)
    let pending = wait_staging_pending(&client, &base, 1, Duration::from_secs(5)).await;
    assert_eq!(pending, 1, "one staged item pending while locked");

    // 4. unlock → drain worker runs
    let r = client
        .post(format!("{base}/api/v1/vault/unlock"))
        .json(&serde_json::json!({"password": pw}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "unlock");

    // 5. staging drains back to 0 (the idempotent done point)
    let pending = wait_staging_pending(&client, &base, 0, Duration::from_secs(20)).await;
    assert_eq!(pending, 0, "staging must drain to 0 after unlock");

    // 6. the staged doc is now in the vault (searchable via FTS, no LLM/embedding needed).
    let new_token = client
        .post(format!("{base}/api/v1/vault/unlock"))
        .json(&serde_json::json!({"password": pw}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Poll search a few times — FTS indexing is async post-ingest.
    let mut found = false;
    for _ in 0..20 {
        let r = client
            .get(format!("{base}/api/v1/search"))
            .query(&[("q", unique)])
            .header("authorization", format!("Bearer {new_token}"))
            .send()
            .await
            .unwrap();
        if r.status() == 200 {
            let body = r.text().await.unwrap();
            if body.contains(unique) || body.contains("locked_note") {
                found = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    assert!(
        found,
        "drained doc must be searchable (proves it went through the real ingest pipeline)"
    );
}
