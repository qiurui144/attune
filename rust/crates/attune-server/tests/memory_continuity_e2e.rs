//! memory-continuity END-TO-END — boots the REAL axum server and drives the two
//! product flows the plan (Task 9) calls out:
//!
//!   (a) change embedding model → reindex → /migration/status reports stale=0 and
//!       the old memory is recall-able again under the new model;
//!   (b) export → import into a fresh vault round-trips (imported ≥ 1) and a second
//!       import of the same bundle is a pure idempotent skip.
//!
//! Boot discipline (per office_concurrent_test + the office HF-download fix
//! 22eec99): set HF_HUB_OFFLINE=1 and install_job_store() BEFORE serving, so
//! init_search_engines never tries a synchronous hf-hub download in a request path
//! (which would async-drop a runtime → panic / 503). The memory routes exercised
//! here (status / reindex / export / import) don't touch the search engines, but we
//! keep the safe boot so the harness can't regress into the office trap.
//!
//! The vault is seeded DIRECTLY through the store (not the HTTP ingest pipeline,
//! which spins the reranker + many workers and overwhelms a 2-worker test runtime)
//! — the same lightweight seed memory_route_test.rs uses.

use std::sync::Arc;
use std::time::Duration;

use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider};
use attune_core::memory::migration::reindex_one;
use attune_core::vault::Vault;
use attune_server::state::AppState;

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

/// Boot an unlocked, env-isolated server with the given active embedder dims.
/// Returns `(base_url, client, leaked_state)` — the state is leaked (kept alive
/// for the test) so the harness can drive the real reindex code path on its store.
#[allow(unsafe_code)] // env isolation, same pattern as memory_route_test.rs
fn spawn_unlocked(active_dims: usize) -> (Arc<AppState>, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    // SAFETY: isolate $HOME per test process so vault/device files don't leak.
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        // WHY: keep init_search_engines from a synchronous hf-hub download in a
        // request path (office trap, fix 22eec99).
        std::env::set_var("HF_HUB_OFFLINE", "1");
    }

    let vault = Vault::open_memory(tmp.path()).expect("open in-memory vault");
    vault.setup("test-password-not-real").expect("setup");
    vault.unlock("test-password-not-real").expect("unlock");

    let state = Arc::new(AppState::new(vault, false));
    state.install_job_store(); // job-store ready before any route runs
    state.set_embedding_with_locality(
        Some(Arc::new(MockEmbeddingProvider::new(active_dims))),
        true,
    );
    (state, tmp)
}

async fn serve(state: Arc<AppState>) -> (String, reqwest::Client) {
    let router = attune_server::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    wait_for_server(&base).await;
    (base, reqwest::Client::new())
}

/// Seed one memory + an OLD-model vector directly through the store.
fn seed_stale_memory(state: &AppState, hash: &str, summary: &str, old_dims: usize) {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().expect("dek");
    let store = vault.store();
    store
        .insert_memory(
            &dek,
            "episodic",
            1,
            2,
            &[hash.to_string()],
            summary,
            "old-model",
            1,
        )
        .expect("insert memory");
    let id = store
        .find_memory_id_by_source("episodic", &[hash.to_string()])
        .unwrap()
        .unwrap();
    // OLD-model vector (under-dimensioned vs the active embedder) → reads stale.
    let old_emb = MockEmbeddingProvider::new(old_dims);
    let (v, _) = old_emb.embed(&[summary]).unwrap();
    store
        .put_memory_vector(&id, &v[0], "old-model", 1)
        .expect("put old vector");
}

/// Drive the real reindex (the same `reindex_one` the background batch worker
/// calls) over every currently-stale memory, under the active embedder.
fn drive_reindex_to_completion(state: &AppState) {
    let embedder = state.embedding().expect("embedder");
    let cur_model = attune_core::embed::current_embedding_signature(embedder.as_ref()).model;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().expect("dek");
    let store = vault.store();
    for id in store.list_stale_memory_ids(&cur_model).unwrap() {
        reindex_one(store, &dek, embedder.as_ref(), &id).expect("reindex_one");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_model_then_reindex_clears_stale_over_http() {
    // Active embedder is dim-8 ("embed-dim8"); seed a memory whose vector is a
    // dim-4 OLD-model vector → it must report stale until reindexed.
    let (state, _tmp) = spawn_unlocked(8);
    seed_stale_memory(&state, "rust1", "rust ownership and borrowing model", 4);
    let (base, client) = serve(Arc::clone(&state)).await;

    // Before reindex: HTTP status reports exactly the one stale vector.
    let before: serde_json::Value = client
        .get(format!("{base}/api/v1/memory/migration/status"))
        .send()
        .await
        .expect("status")
        .json()
        .await
        .expect("json");
    assert_eq!(
        before["current_model"], "embed-dim8",
        "active model is dim-8 key"
    );
    assert_eq!(
        before["stale"].as_u64().unwrap(),
        1,
        "old-model vector is stale"
    );

    // Reindex via the real reindex_one path (the same code the worker batch runs).
    drive_reindex_to_completion(&state);

    // After reindex: HTTP status reports stale=0 — old memory recovered into the
    // current model's vector space (its vector is now dim-8 / "embed-dim8").
    let after: serde_json::Value = client
        .get(format!("{base}/api/v1/memory/migration/status"))
        .send()
        .await
        .expect("status after")
        .json()
        .await
        .expect("json");
    assert_eq!(
        after["stale"].as_u64().unwrap(),
        0,
        "no stale vectors remain after reindex"
    );

    // The reindexed vector is genuinely under the active model (recall path basis).
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let id = vault
        .store()
        .find_memory_id_by_source("episodic", &["rust1".into()])
        .unwrap()
        .unwrap();
    let v = vault.store().get_memory_vector(&id).unwrap().unwrap();
    assert_eq!(
        v.model, "embed-dim8",
        "reindexed to the active dimension key"
    );
    assert_eq!(v.dim, 8, "vector re-embedded at the new model's dimension");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_import_round_trip_and_idempotent_skip_over_http() {
    // Source: an unlocked vault with two seeded memories.
    let (src_state, _src_tmp) = spawn_unlocked(4);
    seed_stale_memory(
        &src_state,
        "e1",
        "tokio async runtime scheduling internals",
        4,
    );
    seed_stale_memory(&src_state, "e2", "argon2id key derivation hardening", 4);
    let (src_base, client) = serve(Arc::clone(&src_state)).await;

    let bundle = client
        .post(format!("{src_base}/api/v1/memory/export"))
        .json(&serde_json::json!({ "passphrase": "pw12345678" }))
        .send()
        .await
        .expect("export")
        .bytes()
        .await
        .expect("bundle");
    assert!(!bundle.is_empty(), "export yields a non-empty bundle");

    // Destination: a FRESH unlocked vault (different DEK, no memories).
    let (dst_state, _dst_tmp) = spawn_unlocked(4);
    let (dst_base, _c) = serve(Arc::clone(&dst_state)).await;

    let import = |b: Vec<u8>| {
        let dst_base = dst_base.clone();
        async move {
            let form = reqwest::multipart::Form::new()
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(b).file_name("memory.attune"),
                )
                .text("passphrase", "pw12345678");
            reqwest::Client::new()
                .post(format!("{dst_base}/api/v1/memory/import"))
                .multipart(form)
                .send()
                .await
                .expect("import")
        }
    };

    // First import: both memories land.
    let r1 = import(bundle.to_vec()).await;
    assert_eq!(r1.status().as_u16(), 200, "first import ⇒ 200");
    let b1: serde_json::Value = r1.json().await.expect("json");
    assert!(
        b1["imported"].as_u64().unwrap() >= 1,
        "≥1 imported, got {b1}"
    );
    assert_eq!(
        b1["skipped"].as_u64().unwrap(),
        0,
        "nothing skipped on a clean vault"
    );

    // Second import of the SAME bundle: pure idempotent skip (zero new rows).
    let r2 = import(bundle.to_vec()).await;
    assert_eq!(r2.status().as_u16(), 200, "second import ⇒ 200");
    let b2: serde_json::Value = r2.json().await.expect("json");
    assert_eq!(
        b2["imported"].as_u64().unwrap(),
        0,
        "re-import adds nothing"
    );
    assert!(
        b2["skipped"].as_u64().unwrap() >= 1,
        "re-import skips the existing rows, got {b2}"
    );

    // Destination row count is stable after the duplicate import (no growth).
    let vault = dst_state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().expect("dek");
    let rows = vault
        .store()
        .list_recent_memories(&dek, usize::MAX)
        .unwrap()
        .len();
    assert_eq!(
        rows,
        b1["imported"].as_u64().unwrap() as usize,
        "row count == first-import count"
    );
}
