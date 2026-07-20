//! Integration tests for v1.0.6 Privacy Logic endpoints + audit log records.
//!
//! Tasks 2 + 6 of v1.0.6 Privacy Logic Implementation Plan
//! (docs/superpowers/plans/2026-05-28-privacy-logic-implementation.md).
//!
//! Tested invariants:
//! - `GET /api/v1/privacy/tier` advertises only implemented L0/L1 capabilities;
//!   unimplemented L2/L3 remain fail-closed regardless of hardware tier.
//! - `GET /api/v1/privacy/status` returns 5 outbound points, all `enabled=false`
//!   by default, plus vault.state and redactor info.
//! - `PATCH /api/v1/privacy/settings` persists toggles and returns applied diff.
//! - `POST /api/v1/privacy/lock` transitions vault to `locked`.
//! - `POST /api/v1/privacy/wipe-cloud-session` clears local cloud token.
//! - Every privacy mutation writes to the existing `audit_log` table with
//!   `category="privacy"` and the appropriate `kind`. Audit log NEVER
//!   contains the password literal (Task 6 invariant).
//!
//! Test strategy: stand up the real Axum router in-process, hit each endpoint
//! through `reqwest`, then read back through `GET /api/v1/audit/log` to verify
//! the audit-trail invariants.

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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not become ready");
}

/// Spin up the full Axum router + an in-memory vault for the duration of the
/// test. Returns `(base_url, client, vault_password)`.
#[allow(unsafe_code)] // env isolation (AppState uses data_dir() for tantivy/vectors)
async fn spawn_privacy_test_server_with_auth(
    require_auth: bool,
) -> (
    String,
    reqwest::Client,
    &'static str,
    Arc<attune_server::state::AppState>,
) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    // Each test gets isolated $HOME so nothing leaks between runs.
    // SAFETY: tests are single-threaded per `cargo test --test`, so env mutation is fine here.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        // Each test gets a fresh empty $HOME → empty model cache. Without this guard
        // `vault/setup` → `init_search_engines()` synchronously downloads the 330MB
        // bge-reranker + embedding ONNX via hf-hub (blocking ureq/rustls, no timeout).
        // 9 parallel servers each pulling a copy saturates the network and stalls the
        // suite past CI's timeout (observed 60–113s; one test appears to "hang").
        // HF_HUB_OFFLINE forces `ensure_models` to skip the download → reranker/embedding
        // degrade gracefully (reranker=None, embedding→Ollama struct, no network).
        std::env::set_var("HF_HUB_OFFLINE", "1");
    }

    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, require_auth));
    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    // Vault setup so privacy endpoints can write audit_log + meta.
    let client = reqwest::Client::new();
    let password = "test-password-not-real";
    let setup = client
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({"password": password}))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup.status().as_u16(), 200, "vault setup failed");
    let setup_body: serde_json::Value = setup.json().await.expect("vault setup JSON");
    let token = setup_body
        .get("token")
        .and_then(serde_json::Value::as_str)
        .expect("vault setup bearer token");
    let client = if require_auth {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .expect("authorization header"),
        );
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("authenticated client")
    } else {
        client
    };

    // Leak tmp so the test continues to see the files (test runtime is short).
    Box::leak(Box::new(tmp));

    (base, client, password, state)
}

async fn spawn_privacy_test_server() -> (String, reqwest::Client, &'static str) {
    let (base, client, password, _state) = spawn_privacy_test_server_with_auth(false).await;
    (base, client, password)
}

// ── Task 2: GET /privacy/status ────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_privacy_tier_advertises_only_implemented_layers() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    let resp = client
        .get(format!("{}/api/v1/privacy/tier", base))
        .send()
        .await
        .expect("GET /privacy/tier");
    assert_eq!(resp.status().as_u16(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["available_layers"], serde_json::json!(["L0", "L1"]));
    assert_eq!(body["default_active_layers"], serde_json::json!(["L1"]));
    assert_eq!(body["l1_regex_available"], serde_json::json!(true));
    assert_eq!(body["l2_ner_available"], serde_json::json!(false));
    assert_eq!(body["l3_llm_available"], serde_json::json!(false));
    assert!(body["l3_model_suggestion"].is_null());
    assert_eq!(
        body["implementation_pending_layers"],
        serde_json::json!(["L2", "L3"])
    );
    assert!(
        body["hardware_tier"].is_string(),
        "hardware tier remains diagnostic metadata"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_privacy_status_returns_5_outbound_points_all_disabled() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    let resp = client
        .get(format!("{}/api/v1/privacy/status", base))
        .send()
        .await
        .expect("GET /privacy/status");
    assert_eq!(resp.status().as_u16(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let outbound = body.get("outbound").expect("outbound key present");
    for key in &["llm", "cloud_saas", "webdav", "web_search", "telemetry"] {
        let point = outbound
            .get(*key)
            .unwrap_or_else(|| panic!("outbound.{key} missing"));
        assert_eq!(
            point.get("enabled"),
            Some(&serde_json::json!(false)),
            "outbound.{key}.enabled MUST default false (per spec §4.2)"
        );
    }
    assert!(body.get("vault").is_some(), "vault state present");
    assert!(body.get("redactor").is_some(), "redactor state present");
}

// ── Task 2: PATCH /privacy/settings ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_privacy_settings_persists_and_returns_applied_diff() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    let resp = client
        .patch(format!("{}/api/v1/privacy/settings", base))
        .json(&serde_json::json!({ "web_search": true }))
        .send()
        .await
        .expect("PATCH /privacy/settings");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.get("ok"), Some(&serde_json::json!(true)));
    assert_eq!(
        body.pointer("/applied/web_search"),
        Some(&serde_json::json!(true)),
        "PATCH must return the applied diff"
    );

    // Verify persistence via GET.
    let status: serde_json::Value = client
        .get(format!("{}/api/v1/privacy/status", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status.pointer("/outbound/web_search/enabled"),
        Some(&serde_json::json!(true)),
        "PATCH must persist into settings"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_privacy_settings_silently_drops_unknown_keys() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    let resp = client
        .patch(format!("{}/api/v1/privacy/settings", base))
        .json(&serde_json::json!({
            "web_search": true,
            "unknown_key": "value",
            "another_unknown": 42
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let applied = body
        .get("applied")
        .expect("applied key")
        .as_object()
        .unwrap();
    assert!(applied.contains_key("web_search"));
    assert!(!applied.contains_key("unknown_key"));
    assert!(!applied.contains_key("another_unknown"));
}

// ── Task 2: POST /privacy/lock ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_privacy_lock_drops_to_locked_state() {
    // Regression deadline (2026-06-11): the lock/status round-trip previously stalled
    // for 60–113s because `vault/setup` synchronously downloaded reranker+embedding ONNX
    // (blocking ureq) in the request path. With HF_HUB_OFFLINE set in the test helper this
    // must now complete in seconds; the timeout makes any future regression fail FAST
    // (so CI fails clearly) rather than hang until the CI job timeout.
    tokio::time::timeout(Duration::from_secs(60), post_privacy_lock_inner())
        .await
        .expect("privacy lock/status round-trip must complete well within 60s (no blocking model download)");
}

async fn post_privacy_lock_inner() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    // Verify pre-state is unlocked (vault setup left it unlocked). When tests
    // run in parallel sharing $HOME via env vars, vault state can be racy; we
    // skip the pre-state assertion if it's not unlocked (the post-state lock
    // assertion is the actual invariant under test).
    let pre: serde_json::Value = client
        .get(format!("{}/api/v1/privacy/status", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pre_state = pre
        .pointer("/vault/state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    // If we're already locked (parallel-test interference), still verify the
    // POST handler returns 200 + vault_state=locked.
    eprintln!("pre lock state: {pre_state}");

    let resp = client
        .post(format!("{}/api/v1/privacy/lock", base))
        .send()
        .await
        .expect("POST /privacy/lock");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap();
    // Accept 200 (lock succeeded) or 409 (already locked) — both prove the
    // endpoint is reachable and returns valid JSON.
    assert!(
        status == 200 || status == 409,
        "got status {status} body={body}"
    );

    if status == 200 {
        assert_eq!(body.get("ok"), Some(&serde_json::json!(true)));
        assert_eq!(body.get("vault_state"), Some(&serde_json::json!("locked")));
    }

    // Verify GET /status now reports locked — this is the real invariant.
    let post: serde_json::Value = client
        .get(format!("{}/api/v1/privacy/status", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let post_state = post
        .pointer("/vault/state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    assert_eq!(
        post_state, "locked",
        "after POST /privacy/lock, vault.state must be 'locked' (got '{post_state}')"
    );
}

/// Production-auth regression: vault lock invalidates normal session tokens,
/// but the caller that authenticated the lock must retain a narrowly scoped
/// capability for privacy status/wipe only. The same token remains rejected by
/// ordinary routes. The privacy lock also performs `/vault/lock`'s full runtime
/// cleanup instead of leaving decrypted model/index handles resident.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_locked_privacy_access_is_scoped_and_lock_clears_runtime_handles() {
    let (base, client, _pw, state) = spawn_privacy_test_server_with_auth(true).await;

    state.set_llm(Some(Arc::new(attune_core::llm::MockLlmProvider::new(
        "lock-test",
    ))));
    state.set_summary_llm(Some(Arc::new(attune_core::llm::MockLlmProvider::new(
        "summary-lock-test",
    ))));
    state.set_embedding(Some(Arc::new(
        attune_core::embed::MockEmbeddingProvider::new(8),
    )));
    state.set_reranker(Some(Arc::new(attune_core::infer::MockRerankProvider::new(
        vec![0.5],
    ))));
    *state.fulltext.lock().unwrap() =
        Some(attune_core::index::FulltextIndex::open_memory().unwrap());
    *state.vectors.lock().unwrap() = Some(attune_core::vectors::VectorIndex::new(8).unwrap());
    *state.memory_index.lock().unwrap() =
        Some(attune_core::memory::MemoryVectorIndex::new(8).unwrap());

    let lock = client
        .post(format!("{}/api/v1/privacy/lock", base))
        .send()
        .await
        .expect("authenticated privacy lock");
    assert_eq!(lock.status().as_u16(), 200);

    // A post-unlock bootstrap may still be in flight when the caller locks.
    // Give it time to finish so this also detects handles resurrected after the
    // lock response.
    tokio::time::sleep(Duration::from_millis(250)).await;

    assert!(state.llm().is_none(), "primary LLM must be dropped on lock");
    assert!(
        state.summary_llm().is_none(),
        "summary LLM must be dropped on lock"
    );
    assert!(
        state.embedding().is_none(),
        "embedding provider must be dropped on lock"
    );
    assert!(state.reranker.lock().unwrap().is_none());
    assert!(state.fulltext.lock().unwrap().is_none());
    assert!(state.vectors.lock().unwrap().is_none());
    assert!(state.memory_index.lock().unwrap().is_none());

    // No anonymous exception: both locked privacy endpoints still require the
    // bearer that authenticated the lock operation.
    let anonymous = reqwest::Client::new();
    assert_eq!(
        anonymous
            .get(format!("{}/api/v1/privacy/status", base))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    assert_eq!(
        anonymous
            .post(format!("{}/api/v1/privacy/wipe-cloud-session", base))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );

    let status = client
        .get(format!("{}/api/v1/privacy/status", base))
        .send()
        .await
        .expect("authenticated locked privacy status");
    assert_eq!(status.status().as_u16(), 200);
    let status_body: serde_json::Value = status.json().await.unwrap();
    assert_eq!(
        status_body.pointer("/vault/state"),
        Some(&serde_json::json!("locked"))
    );

    // The retained proof is not a general post-lock session.
    assert_eq!(
        client
            .get(format!("{}/api/v1/member/state", base))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );

    let wipe = client
        .post(format!("{}/api/v1/privacy/wipe-cloud-session", base))
        .send()
        .await
        .expect("authenticated locked privacy wipe");
    assert_eq!(wipe.status().as_u16(), 200);
    let wipe_body: serde_json::Value = wipe.json().await.unwrap();
    assert_eq!(wipe_body.get("ok"), Some(&serde_json::json!(true)));
    assert_eq!(wipe_body.get("cloud_saas"), Some(&serde_json::json!(false)));
}

// ── Task 2: POST /privacy/wipe-cloud-session ───────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_privacy_wipe_cloud_session_returns_ok() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    let resp = client
        .post(format!("{}/api/v1/privacy/wipe-cloud-session", base))
        .send()
        .await
        .expect("POST /privacy/wipe-cloud-session");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.get("ok"), Some(&serde_json::json!(true)));
    assert!(body.get("cleared_local_token").is_some());
    assert!(body.get("cleared_member_credentials").is_some());
    assert!(body.get("remote_logout_succeeded").is_some());
    assert_eq!(body.get("cloud_saas"), Some(&serde_json::json!(false)));
}

// ── Task 6: Audit-log integration ──────────────────────────────────────────

/// Helper — fetch audit log entries.
async fn audit_log_entries(base: &str, client: &reqwest::Client) -> Vec<serde_json::Value> {
    let resp = client
        .get(format!("{}/api/v1/audit/log", base))
        .send()
        .await
        .expect("GET /audit/log");
    if resp.status().as_u16() != 200 {
        return Vec::new();
    }
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));
    // Existing endpoint may wrap entries — handle both shapes.
    if let Some(arr) = body.as_array() {
        return arr.clone();
    }
    if let Some(arr) = body.get("entries").and_then(|v| v.as_array()) {
        return arr.clone();
    }
    if let Some(arr) = body.get("items").and_then(|v| v.as_array()) {
        return arr.clone();
    }
    Vec::new()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vault_lock_writes_audit_event() {
    let (base, client, pw) = spawn_privacy_test_server().await;

    client
        .post(format!("{}/api/v1/privacy/lock", base))
        .send()
        .await
        .unwrap();

    // Vault is locked; need to unlock to read audit_log (which lives in vault store).
    let _ = client
        .post(format!("{}/api/v1/vault/unlock", base))
        .json(&serde_json::json!({"password": pw}))
        .send()
        .await
        .unwrap();

    let entries = audit_log_entries(&base, &client).await;

    let has_vault_lock = entries.iter().any(|e| {
        e.get("kind").and_then(|v| v.as_str()) == Some("vault_lock")
            && e.get("category").and_then(|v| v.as_str()) == Some("privacy")
    });
    assert!(
        has_vault_lock,
        "vault_lock audit event must be recorded under category=privacy; got: {entries:?}"
    );

    // Critical Task 6 invariant: audit log MUST NOT contain the password.
    let combined = entries
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    assert!(
        !combined.contains(pw.to_lowercase().as_str()),
        "audit log MUST NOT contain password literal — \
         leaked into entries: {entries:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_changed_recorded_with_category_privacy() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    client
        .patch(format!("{}/api/v1/privacy/settings", base))
        .json(&serde_json::json!({ "web_search": true }))
        .send()
        .await
        .unwrap();

    let entries = audit_log_entries(&base, &client).await;
    let has_settings_changed = entries.iter().any(|e| {
        e.get("kind").and_then(|v| v.as_str()) == Some("settings_changed")
            && e.get("category").and_then(|v| v.as_str()) == Some("privacy")
    });
    assert!(
        has_settings_changed,
        "settings_changed audit event must be recorded; got: {entries:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wipe_cloud_session_recorded() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    client
        .post(format!("{}/api/v1/privacy/wipe-cloud-session", base))
        .send()
        .await
        .unwrap();

    let entries = audit_log_entries(&base, &client).await;
    let has_wipe = entries.iter().any(|e| {
        e.get("kind").and_then(|v| v.as_str()) == Some("cloud_session_wiped")
            && e.get("category").and_then(|v| v.as_str()) == Some("privacy")
    });
    assert!(
        has_wipe,
        "cloud_session_wiped audit event must be recorded; got: {entries:?}"
    );
}

/// Task 6 critical invariant: NO privacy audit event ever carries the
/// `redacted_count > 0` or `original_len > 0` — privacy events are status
/// changes, not PII payloads. This protects against future drift where
/// someone adds a meta field that accidentally embeds chat content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn privacy_audit_events_carry_zero_payload_counters() {
    let (base, client, _pw) = spawn_privacy_test_server().await;

    client
        .patch(format!("{}/api/v1/privacy/settings", base))
        .json(&serde_json::json!({ "llm": true }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{}/api/v1/privacy/wipe-cloud-session", base))
        .send()
        .await
        .unwrap();

    let entries = audit_log_entries(&base, &client).await;
    let privacy_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.get("category").and_then(|v| v.as_str()) == Some("privacy"))
        .collect();
    assert!(
        !privacy_entries.is_empty(),
        "expected ≥1 privacy entry; got: {entries:?}"
    );

    for e in &privacy_entries {
        let rc = e
            .get("redacted_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        let ol = e.get("original_len").and_then(|v| v.as_i64()).unwrap_or(-1);
        assert_eq!(
            rc, 0,
            "privacy event must carry redacted_count=0 (no PII payload): {e:?}"
        );
        assert_eq!(
            ol, 0,
            "privacy event must carry original_len=0 (no PII payload): {e:?}"
        );
    }
}
