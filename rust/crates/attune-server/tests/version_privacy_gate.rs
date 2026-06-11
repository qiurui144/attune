//! R1.1b route-level regression — `GET /api/v1/version` must NOT reach GitHub
//! when the privacy `telemetry` outbound point is off (the default).
//!
//! Uses the in-process eval server (fresh in-memory vault → privacy settings
//! absent → all 5 egress points default OFF), so the handler must take the
//! gated path: current version only + `update_check: disabled-by-privacy-settings`,
//! and return fast (no 5s GitHub timeout on offline CI).

use attune_server::test_support::spawn_eval_server;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_update_check_gated_off_by_default() {
    let srv = spawn_eval_server().await;
    let client = reqwest::Client::new();

    // /api/v1/version is behind vault_guard (pre-existing) — set up/unlock the
    // in-memory vault first. Privacy settings stay absent → all egress off.
    let resp = client
        .post(format!("{}/api/v1/vault/setup", srv.url()))
        .json(&serde_json::json!({ "password": "P@ss-version-gate-not-real" }))
        .send()
        .await
        .expect("vault setup");
    assert!(resp.status().is_success(), "vault setup failed: {}", resp.status());

    let t = std::time::Instant::now();
    let resp = client
        .get(format!("{}/api/v1/version", srv.url()))
        .send()
        .await
        .expect("GET /api/v1/version");
    assert!(resp.status().is_success(), "got {}", resp.status());
    let body: serde_json::Value = resp.json().await.expect("json");

    assert_eq!(
        body.get("update_check").and_then(|v| v.as_str()),
        Some("disabled-by-privacy-settings"),
        "privacy telemetry default-off must disable the GitHub update check; body={body}"
    );
    assert!(
        body.get("latest_available").is_some_and(|v| v.is_null()),
        "no latest_available when gated; body={body}"
    );
    assert!(
        body.get("current").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()),
        "current version always present; body={body}"
    );
    // Gated path never touches the network — generous CI bound, but well under
    // the 5s GitHub client timeout that would indicate a real fetch attempt.
    assert!(
        t.elapsed() < std::time::Duration::from_secs(3),
        "gated /version took {:?} — did it try to reach GitHub?",
        t.elapsed()
    );
}
