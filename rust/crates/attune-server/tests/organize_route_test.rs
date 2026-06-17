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

/// Run analyze and return the resulting proposal_id (drives the Task-9 lifecycle tests).
async fn make_proposal(base: &str, client: &reqwest::Client, ids: &[String]) -> String {
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
    body["proposal_id"].as_str().expect("proposal_id").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_files_items_then_is_idempotent() {
    let (base, client, ids) = spawn(true, 3).await;
    let pid = make_proposal(&base, &client, &ids).await;

    // First apply: create one project, file two items into it → filed_count == 2.
    let apply = client
        .post(format!("{}/api/v1/organize/apply", base))
        .json(&serde_json::json!({
            "proposal_id": pid,
            "confirmed_groups": [{
                "group_id": 0, "action": "create", "title": "案卷A", "kind": "collection",
                "items": [
                    { "item_id": ids[0], "role": "证据" },
                    { "item_id": ids[1] }
                ]
            }]
        }))
        .send()
        .await
        .expect("apply");
    assert_eq!(apply.status().as_u16(), 200, "apply returns 200");
    let body: serde_json::Value = apply.json().await.expect("json");
    assert_eq!(body["filed_count"], 2, "two items filed");
    assert_eq!(
        body["created"].as_array().map(|a| a.len()),
        Some(1),
        "exactly one project created"
    );
    assert_eq!(body["already_applied"], false, "first apply not idempotent short-circuit");

    // Second apply of the SAME proposal_id → idempotent short-circuit, no new project.
    let again = client
        .post(format!("{}/api/v1/organize/apply", base))
        .json(&serde_json::json!({ "proposal_id": pid, "confirmed_groups": [] }))
        .send()
        .await
        .expect("apply again");
    assert_eq!(again.status().as_u16(), 200, "re-apply returns 200");
    let again_body: serde_json::Value = again.json().await.expect("json");
    assert_eq!(again_body["already_applied"], true, "re-apply is idempotent");
    assert_eq!(again_body["filed_count"], 0, "re-apply files nothing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_one_returns_decrypted_proposal_else_404() {
    let (base, client, ids) = spawn(true, 3).await;
    let pid = make_proposal(&base, &client, &ids).await;

    // get_one must decrypt the cached proposal back to plaintext JSON (uses dek).
    let got = client
        .get(format!("{}/api/v1/organize/proposals/{}", base, pid))
        .send()
        .await
        .expect("get_one");
    assert_eq!(got.status().as_u16(), 200, "get_one returns 200");
    let body: serde_json::Value = got.json().await.expect("json");
    assert_eq!(body["proposal_id"], pid, "decrypted proposal carries its id");
    // Plaintext round-trip: groups+noise cover every seeded item.
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
    assert_eq!(covered, want, "decrypted proposal covers all input items");

    // Unknown id → 404.
    let missing = client
        .get(format!("{}/api/v1/organize/proposals/does-not-exist", base))
        .send()
        .await
        .expect("get_one missing");
    assert_eq!(missing.status().as_u16(), 404, "unknown proposal ⇒ 404");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_paginates_proposals() {
    let (base, client, ids) = spawn(true, 3).await;
    // Create two distinct proposals.
    let p1 = make_proposal(&base, &client, &ids).await;
    let p2 = make_proposal(&base, &client, &ids).await;
    assert_ne!(p1, p2, "two distinct proposals");

    // Page 1: limit=1 returns exactly one (most recent first).
    let page1 = client
        .get(format!("{}/api/v1/organize/proposals?limit=1&offset=0", base))
        .send()
        .await
        .expect("list page1");
    assert_eq!(page1.status().as_u16(), 200, "list returns 200");
    let body1: serde_json::Value = page1.json().await.expect("json");
    let arr1 = body1.as_array().expect("array");
    assert_eq!(arr1.len(), 1, "limit=1 ⇒ one row");
    assert!(arr1[0]["id"].is_string(), "row carries id");
    assert_eq!(arr1[0]["status"], "draft", "fresh proposal is draft");

    // Page 2: offset=1 returns the second row, different id from page 1.
    let page2 = client
        .get(format!("{}/api/v1/organize/proposals?limit=1&offset=1", base))
        .send()
        .await
        .expect("list page2");
    let body2: serde_json::Value = page2.json().await.expect("json");
    let arr2 = body2.as_array().expect("array");
    assert_eq!(arr2.len(), 1, "offset=1 ⇒ one row");
    assert_ne!(arr1[0]["id"], arr2[0]["id"], "pagination yields distinct rows");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_discards_proposal_returns_204() {
    let (base, client, ids) = spawn(true, 3).await;
    let pid = make_proposal(&base, &client, &ids).await;

    let del = client
        .delete(format!("{}/api/v1/organize/proposals/{}", base, pid))
        .send()
        .await
        .expect("delete");
    assert_eq!(del.status().as_u16(), 204, "delete returns 204");

    // After discard, the proposal is no longer listed among drafts.
    let listed = client
        .get(format!("{}/api/v1/organize/proposals?status=draft", base))
        .send()
        .await
        .expect("list after delete");
    let body: serde_json::Value = listed.json().await.expect("json");
    let ids_listed: Vec<String> = body
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|r| r["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(!ids_listed.contains(&pid), "discarded proposal not in draft list");
}
