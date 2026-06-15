//! POST /api/v1/organize/analyze integration test — exercises the REAL axum route.
//!
//! Verifies the full Task-8 wiring end-to-end: gather (item ids, store
//! titles/snippets, vector lookup), member-gate (no member yields tier-2 plus a
//! hint code), analyze_items, save_proposal, and the JSON response shape.
//!
//! The vault is set up, unlocked, and seeded DIRECTLY (not via the HTTP
//! vault/setup and ingest routes): those routes spin up the reranker/model load
//! and many background workers, which overwhelm a 2-worker test runtime (the
//! same reason vault_setup_test is ignored). Seeding through the store keeps the
//! test fast and focused on the organize route under test. Items have no vectors
//! in-test so they land in noise_items, but the proposal must still cover every
//! input item (the union of groups and noise equals the inputs).

use std::sync::Arc;
use std::time::Duration;

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

/// Build an in-memory vault, optionally unlock + seed `seed_ids` items, then stand
/// up the real router. Returns `(base, client, seeded_ids)`.
#[allow(unsafe_code)] // env isolation (AppState uses data_dir() for tantivy/vectors)
async fn spawn(unlock: bool, seed: usize) -> (String, reqwest::Client, Vec<String>) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    // SAFETY: isolate $HOME per test process so vault/device files don't leak.
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }

    let vault = Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let mut seeded = Vec::new();
    if unlock {
        vault.setup("test-password-not-real").expect("setup");
        vault.unlock("test-password-not-real").expect("unlock");
        let dek = vault.dek_db().expect("dek");
        for i in 0..seed {
            let id = vault
                .store()
                .insert_item(
                    &dek,
                    &format!("file {i}"),
                    &format!("content body number {i} for organize analyze test"),
                    None,
                    "note",
                    None,
                    None,
                )
                .expect("insert item");
            seeded.push(id);
        }
    }

    // require_auth=false: with the vault Unlocked, vault_guard lets the route through.
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    Box::leak(Box::new(tmp)); // keep files alive for the (short) test
    (base, reqwest::Client::new(), seeded)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_returns_proposal_covering_all_bound_items() {
    let (base, client, ids) = spawn(true, 3).await;

    let resp = client
        .post(format!("{}/api/v1/organize/analyze", base))
        .json(&serde_json::json!({
            "scope": { "item_ids": ids },
            "options": { "min_cluster_size": 2 }
        }))
        .send()
        .await
        .expect("analyze");
    assert_eq!(resp.status().as_u16(), 200, "analyze returns 200");
    let body: serde_json::Value = resp.json().await.expect("json");

    assert!(body["proposal_id"].is_string(), "proposal_id present");
    // No member configured in test ⇒ tier-2 (extractive) + member-required hint.
    assert_eq!(body["cost"]["tier"], 2, "no member ⇒ tier-2 (extractive labels)");
    assert_eq!(
        body["code"], "member-required-for-llm-label",
        "non-member response carries the tier hint code"
    );

    // Every seeded item must appear somewhere in groups+noise (no loss/dup).
    let mut covered: Vec<String> = Vec::new();
    let empty = Vec::new();
    for g in body["groups"].as_array().unwrap_or(&empty) {
        for it in g["items"].as_array().unwrap_or(&empty) {
            covered.push(it["item_id"].as_str().unwrap().to_string());
        }
    }
    for n in body["noise_items"].as_array().unwrap_or(&empty) {
        covered.push(n["item_id"].as_str().unwrap().to_string());
    }
    covered.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(covered, want, "all 3 input items covered exactly once (groups ∪ noise)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_empty_scope_returns_400() {
    let (base, client, _) = spawn(true, 0).await;
    let resp = client
        .post(format!("{}/api/v1/organize/analyze", base))
        .json(&serde_json::json!({ "scope": { "item_ids": [] } }))
        .send()
        .await
        .expect("analyze empty");
    assert_eq!(resp.status().as_u16(), 400, "empty scope ⇒ 400");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_locked_vault_returns_403() {
    // Vault opened but NOT unlocked → sealed → vault_guard rejects.
    let (base, client, _) = spawn(false, 0).await;
    let resp = client
        .post(format!("{}/api/v1/organize/analyze", base))
        .json(&serde_json::json!({ "scope": { "item_ids": ["x"] } }))
        .send()
        .await
        .expect("analyze locked");
    // 401 (auth) or 403 (vault not unlocked) — both prove the route exists + is guarded.
    let s = resp.status().as_u16();
    assert!(s == 401 || s == 403, "locked vault ⇒ 401/403, got {s}");
}
