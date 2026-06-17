//! Organize engine end-to-end integration test — folder → organized Project.
//!
//! Exercises the FULL Task 1-9 wiring through the REAL axum routes:
//!   seed items + vectors → POST /organize/analyze (real HDBSCAN clustering)
//!   → GET /organize/proposals/{id} → POST /organize/apply
//!   → assert files filed into the new Project + the "organized" timeline event.
//!
//! ## Why the lightweight seed mode (NOT HTTP /vault/setup + /ingest)
//!
//! The HTTP setup + ingest routes spin up the reranker/model load and many
//! background workers; on a 2-worker test runtime the worker drop in async
//! context overwhelms the executor (the same reason `vault_setup_test` is
//! `#[ignore]`d, observed in Task 8). So we set the vault up DIRECTLY
//! (Vault::open_memory + setup + unlock), insert items through the store, and
//! seed the vector index in-process. This keeps the test fast and deterministic
//! while still driving the organize routes exactly as production does.
//!
//! ## Why we seed vectors directly into `state.vectors`
//!
//! The analyze route gathers each item's embedding from `state.vectors`
//! (an `Option<VectorIndex>`, `None` until a dimension is known). To exercise
//! the REAL HDBSCAN grouping path (not the dir fallback) we need ≥ `max(min, 5)`
//! same-dimension vectors. We install a fresh `VectorIndex` and `add()` one
//! mean-vector per item, arranged as two tight, well-separated clusters.

use std::sync::Arc;
use std::time::Duration;

use attune_core::vault::Vault;
use attune_core::vectors::{VectorIndex, VectorMeta};

const DIM: usize = 8;

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

/// Build an unlocked in-memory vault, seed `n` items, then seed one vector each
/// arranged so HDBSCAN finds REAL structure, stand up the real router, and return
/// `(base, client, seeded_ids)`.
///
/// Geometry: hdbscan 0.11 `default_hyper_params` uses `min_cluster_size = 5`,
/// `min_samples = 5`, and `allow_single_cluster = false`. With single clusters
/// disallowed, the condensed tree must SPLIT into ≥ 2 stable clusters for any
/// point to escape noise — a lone blob collapses to noise. So we build TWO
/// well-separated dense lobes (near axis 0 and axis 4), each with ≥ 5 points.
/// `n` is therefore split evenly across the two lobes; n = 12 → two 6-point
/// clusters that HDBSCAN reliably resolves (verified empirically, Task 12).
#[allow(unsafe_code)] // env isolation (AppState uses data_dir() for tantivy/vectors)
async fn spawn_with_vectors(n: usize) -> (String, reqwest::Client, Vec<String>) {
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

    let mut seeded = Vec::new();
    for i in 0..n {
        let id = vault
            .store()
            .insert_item(
                &dek,
                &format!("file {i} report"),
                &format!("content body number {i} for organize e2e test"),
                None,
                "note",
                None,
                None,
            )
            .expect("insert item");
        seeded.push(id);
    }

    let state = Arc::new(attune_server::state::AppState::new(vault, false));

    // Seed vectors directly: two dense, well-separated lobes (axis 0 and axis 4),
    // each ≥ min_cluster_size=5, so HDBSCAN splits into ≥ 2 real clusters.
    {
        let mut idx = VectorIndex::new(DIM).expect("vector index");
        let half = n / 2;
        for (i, id) in seeded.iter().enumerate() {
            let mut v = vec![0.0f32; DIM];
            let lobe = if i < half { 0 } else { 4 };
            v[lobe] = 1.0;
            v[lobe + 1] = 0.01 * (i as f32 + 1.0); // tiny jitter, keep the lobe tight
            idx.add(
                &v,
                VectorMeta { item_id: id.clone(), chunk_idx: 0, level: 2, section_idx: 0 },
            )
            .expect("add vector");
        }
        *state.vectors.lock().unwrap_or_else(|e| e.into_inner()) = Some(idx);
    }

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

/// Full happy path: 12 items in two clustered lobes → analyze (real HDBSCAN) →
/// get → apply → files filed into projects + "organized" timeline event present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_seed_to_organized_project_e2e() {
    // 12 items, two 6-point lobes ≥ max(min_cluster_size=5, HDBSCAN_FLOOR=5) →
    // REAL clustering path (allow_single_cluster=false needs a ≥2-way split).
    let (base, client, ids) = spawn_with_vectors(12).await;

    // 1. analyze — must hit the HDBSCAN path (clean.len()=6 ≥ 5), not fallback.
    let analyze = client
        .post(format!("{}/api/v1/organize/analyze", base))
        .json(&serde_json::json!({
            "scope": { "item_ids": ids },
            "options": { "min_cluster_size": 5 }
        }))
        .send()
        .await
        .expect("analyze");
    assert_eq!(analyze.status().as_u16(), 200, "analyze returns 200");
    let prop: serde_json::Value = analyze.json().await.expect("json");
    let pid = prop["proposal_id"].as_str().expect("proposal_id").to_string();

    // Real clustering path: with 6 same-dim vectors none are dropped for dim
    // mismatch, so mismatch count is 0 (proves vectors reached the engine).
    assert_eq!(prop["dimension_mismatch_count"], 0, "all 12 vectors same dim → 0 mismatch");

    // Union invariant: every input item appears exactly once across groups+noise.
    let covered = covered_ids(&prop);
    let mut want = ids.clone();
    want.sort();
    assert_eq!(covered, want, "analyze proposal covers all 12 input items exactly once");

    // At least one non-noise group must form (the two tight lobes cluster).
    let group_count = prop["groups"].as_array().map(|a| a.len()).unwrap_or(0);
    assert!(group_count >= 1, "real clustering yields ≥1 named group, got {group_count}");
    // GenericStrategy (no member) labels every group non-empty (extractive).
    for g in prop["groups"].as_array().unwrap() {
        assert!(
            !g["label"].as_str().unwrap_or("").is_empty(),
            "every group carries a non-empty extractive label"
        );
    }

    // 2. get — decrypt round-trips the cached proposal.
    let got = client
        .get(format!("{}/api/v1/organize/proposals/{}", base, pid))
        .send()
        .await
        .expect("get_one");
    assert_eq!(got.status().as_u16(), 200, "get_one 200");
    let got_body: serde_json::Value = got.json().await.expect("json");
    assert_eq!(got_body["proposal_id"], pid, "decrypted proposal id matches");

    // 3. apply — confirm every group as create; file ALL group items.
    let confirmed: Vec<serde_json::Value> = prop["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            serde_json::json!({
                "group_id": g["group_id"],
                "action": "create",
                "title": g["label"],
                "kind": g["suggested_kind"],
                "items": g["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| serde_json::json!({ "item_id": i["item_id"] }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    let grouped_item_count: usize = prop["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["items"].as_array().unwrap().len())
        .sum();

    let apply = client
        .post(format!("{}/api/v1/organize/apply", base))
        .json(&serde_json::json!({ "proposal_id": pid, "confirmed_groups": confirmed }))
        .send()
        .await
        .expect("apply");
    assert_eq!(apply.status().as_u16(), 200, "apply 200");
    let applied: serde_json::Value = apply.json().await.expect("json");
    assert_eq!(
        applied["filed_count"].as_u64().unwrap() as usize,
        grouped_item_count,
        "every grouped item is filed into a project"
    );
    assert!(
        !applied["created"].as_array().unwrap().is_empty(),
        "at least one project created"
    );
    assert_eq!(applied["already_applied"], false, "first apply not a short-circuit");
}

/// Idempotency through the E2E route surface: re-applying the same proposal files
/// nothing and reports already_applied (no duplicate projects).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_apply_is_idempotent() {
    let (base, client, ids) = spawn_with_vectors(12).await;
    let prop: serde_json::Value = client
        .post(format!("{}/api/v1/organize/analyze", base))
        .json(&serde_json::json!({ "scope": { "item_ids": ids }, "options": { "min_cluster_size": 5 } }))
        .send()
        .await
        .expect("analyze")
        .json()
        .await
        .expect("json");
    let pid = prop["proposal_id"].as_str().unwrap().to_string();

    let cg = serde_json::json!([{
        "group_id": 0, "action": "create", "title": "案卷A", "kind": "collection",
        "items": [{ "item_id": ids[0] }, { "item_id": ids[1] }]
    }]);
    let first = client
        .post(format!("{}/api/v1/organize/apply", base))
        .json(&serde_json::json!({ "proposal_id": pid, "confirmed_groups": cg }))
        .send()
        .await
        .expect("apply 1");
    assert_eq!(first.status().as_u16(), 200);
    let first_body: serde_json::Value = first.json().await.expect("json");
    assert_eq!(first_body["filed_count"], 2);

    let again = client
        .post(format!("{}/api/v1/organize/apply", base))
        .json(&serde_json::json!({ "proposal_id": pid, "confirmed_groups": [] }))
        .send()
        .await
        .expect("apply 2");
    assert_eq!(again.status().as_u16(), 200);
    let again_body: serde_json::Value = again.json().await.expect("json");
    assert_eq!(again_body["already_applied"], true, "re-apply idempotent");
    assert_eq!(again_body["filed_count"], 0, "re-apply files nothing");
}

/// HEAVY-PATH E2E (真 bind 目录 + HTTP /index/bind + /ingest) — kept honest and
/// `#[ignore]`d. Per the Task-8 lesson, the HTTP setup/ingest routes load the
/// reranker and drop background workers in async context, which hangs a 2-worker
/// test runtime. This stub documents the intended full-fidelity path so it is
/// not silently dropped; run it manually with a real model stack:
///   `cargo test -p attune-server --test organize_e2e_test -- --ignored`
/// (PENDING-VERIFY against a real ingest stack; not run in CI per §6.3.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "heavy: HTTP /vault/setup + /index/bind + /ingest load reranker + drop workers in async ctx → hangs 2-worker runtime (Task-8 lesson); run manually with a real model stack"]
async fn folder_http_bind_ingest_organize_e2e_heavy() {
    // Intentionally a documented stub, NOT a fake green: the lightweight test
    // above is the CI-running E2E. This marker preserves the heavy-path intent.
    panic!("heavy-path E2E is a documented #[ignore] stub — see doc comment; not auto-run");
}

/// Collect every item_id appearing across groups+noise, sorted.
fn covered_ids(proposal: &serde_json::Value) -> Vec<String> {
    let empty = Vec::new();
    let mut covered: Vec<String> = Vec::new();
    for g in proposal["groups"].as_array().unwrap_or(&empty) {
        for it in g["items"].as_array().unwrap_or(&empty) {
            covered.push(it["item_id"].as_str().unwrap().to_string());
        }
    }
    for n in proposal["noise_items"].as_array().unwrap_or(&empty) {
        covered.push(n["item_id"].as_str().unwrap().to_string());
    }
    covered.sort();
    covered
}
