//! /api/v1/memory/* route integration tests — exercises the REAL axum routes
//! for memory-continuity (2026-06-15, plan Task 4).
//!
//! Like organize_route_test, the vault is set up, unlocked, and seeded DIRECTLY
//! through the store (not via the HTTP vault/setup + ingest routes, which spin up
//! the reranker / model load / many background workers that overwhelm a 2-worker
//! test runtime). A MockEmbeddingProvider (dim 4 → model key "embed-dim4") is set
//! on AppState so the status route has a current embedding signature; the seeded
//! memory vector is stored under a DIFFERENT model ("old-model") so it must report
//! as stale.

use std::sync::Arc;
use std::time::Duration;

use attune_core::embed::MockEmbeddingProvider;
use attune_core::vault::Vault;

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

/// Build an in-memory vault, unlock it, set a dim-4 mock embedder, and seed one
/// memory whose vector is stored under `vec_model`. Returns `(base, client)`.
#[allow(unsafe_code)] // env isolation (AppState uses data_dir() for tantivy/vectors)
async fn spawn_with_stale_vector(vec_model: &str) -> (String, reqwest::Client) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    // SAFETY: isolate $HOME per test process so vault/device files don't leak.
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }

    let vault = Vault::open_memory(tmp.path()).expect("open in-memory vault");
    vault.setup("test-password-not-real").expect("setup");
    vault.unlock("test-password-not-real").expect("unlock");
    let dek = vault.dek_db().expect("dek");

    // Seed a memory + a vector under the OLD model so it reports stale vs the
    // active dim-4 embedder ("embed-dim4").
    vault
        .store()
        .insert_memory(
            &dek,
            "episodic",
            1,
            2,
            &["h1".into()],
            "a memory summary about rust ownership",
            "old-model",
            1,
        )
        .expect("insert memory");
    let id = vault.store().list_recent_memories(&dek, 1).expect("list")[0].id.clone();
    vault
        .store()
        .put_memory_vector(&id, &[0.0; 3], vec_model, 1)
        .expect("put vector");

    // require_auth=false: with the vault Unlocked, vault_guard lets routes through.
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    // Active embedder: dim 4 → model_name "embed-dim4" (the dimension key).
    state.set_embedding(Some(Arc::new(MockEmbeddingProvider::new(4))));
    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    Box::leak(Box::new(tmp)); // keep files alive for the (short) test
    (base, reqwest::Client::new())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migration_status_reports_current_model_and_stale_count() {
    // Vector stored under "old-model" → stale vs active "embed-dim4".
    let (base, client) = spawn_with_stale_vector("old-model").await;

    let resp = client
        .get(format!("{}/api/v1/memory/migration/status", base))
        .send()
        .await
        .expect("status");
    assert_eq!(resp.status().as_u16(), 200, "status returns 200");
    let body: serde_json::Value = resp.json().await.expect("json");

    assert_eq!(
        body["current_model"], "embed-dim4",
        "current_model is the active embedder's dimension key"
    );
    assert_eq!(body["current_dim"], 4, "current_dim mirrors the embedder dims");
    assert_eq!(
        body["stale"].as_u64().unwrap(),
        1,
        "the old-model vector is reported stale"
    );
    assert_eq!(body["paused"], false, "reindex not paused by default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reports_zero_stale_when_vector_matches_current_model() {
    // Vector already stored under the active dimension key → nothing stale.
    let (base, client) = spawn_with_stale_vector("embed-dim4").await;

    let body: serde_json::Value = client
        .get(format!("{}/api/v1/memory/migration/status", base))
        .send()
        .await
        .expect("status")
        .json()
        .await
        .expect("json");
    assert_eq!(body["stale"].as_u64().unwrap(), 0, "matching model ⇒ 0 stale");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reindex_post_toggles_pause_and_returns_200() {
    let (base, client) = spawn_with_stale_vector("old-model").await;

    // Pause: 200 + paused=true reflected in the next status read.
    let r = client
        .post(format!("{}/api/v1/memory/reindex", base))
        .json(&serde_json::json!({ "pause": true }))
        .send()
        .await
        .expect("reindex pause");
    assert_eq!(r.status().as_u16(), 200, "reindex returns 200");
    let rb: serde_json::Value = r.json().await.expect("json");
    assert_eq!(rb["paused"], true, "pause echoed");
    assert_eq!(rb["running"], false, "running is the inverse of paused");

    let st: serde_json::Value = client
        .get(format!("{}/api/v1/memory/migration/status", base))
        .send()
        .await
        .expect("status after pause")
        .json()
        .await
        .expect("json");
    assert_eq!(st["paused"], true, "status reflects the paused flag");

    // Resume: empty body ⇒ pause defaults to false.
    let r2 = client
        .post(format!("{}/api/v1/memory/reindex", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("reindex resume");
    assert_eq!(r2.status().as_u16(), 200, "resume returns 200");
    let rb2: serde_json::Value = r2.json().await.expect("json");
    assert_eq!(rb2["paused"], false, "empty body ⇒ resumed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migration_status_locked_vault_returns_403() {
    // Vault opened + unlocked + seeded, then locked → status route must reject.
    let (base, client) = spawn_with_stale_vector("old-model").await;
    // Lock the vault via the lock endpoint so the guard rejects subsequent reads.
    let lock = client
        .post(format!("{}/api/v1/vault/lock", base))
        .send()
        .await
        .expect("lock");
    assert!(lock.status().is_success(), "vault lock succeeds");

    let resp = client
        .get(format!("{}/api/v1/memory/migration/status", base))
        .send()
        .await
        .expect("status locked");
    let s = resp.status().as_u16();
    assert!(s == 401 || s == 403, "locked vault ⇒ 401/403, got {s}");
}
