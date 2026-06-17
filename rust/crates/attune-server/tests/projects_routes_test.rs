//! Project REST API integration tests — exercises the REAL axum routes.
//!
//! Two lanes:
//!  1. `projects_endpoints_locked_vault_returns_403` — every endpoint rejects
//!     when the vault is not unlocked (middleware + handler defensive check).
//!  2. `projects_crud_round_trip_unlocked_vault` — full create → list → get →
//!     add-file → list-files → timeline round-trip against an UNLOCKED vault,
//!     plus error cases (empty title → 400, unknown id → 404). This is the real
//!     route logic — previously only the locked path was covered, so handler
//!     wiring / status codes / JSON shapes could drift silently. `/vault/setup`
//!     leaves the vault unlocked, so the happy path is now testable in-process
//!     (no Playwright needed for the route contract).

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

/// Stand up the real router with an in-memory vault. Returns `(base, client)`.
/// `setup_vault=true` runs `/vault/setup` which leaves the vault UNLOCKED.
#[allow(unsafe_code)] // env isolation (AppState uses data_dir() for tantivy/vectors)
async fn spawn(setup_vault: bool) -> (String, reqwest::Client) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    // SAFETY: isolate $HOME per test process so vault files don't leak between
    // runs. `cargo test --test` runs each test binary's tests serially by default
    // for these multi_thread tests sharing env is acceptable (each spawns its own
    // in-memory vault; HOME only affects device.key path which is also per-tmp).
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }

    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    let client = reqwest::Client::new();
    if setup_vault {
        let resp = client
            .post(format!("{}/api/v1/vault/setup", base))
            .json(&serde_json::json!({"password": "test-password-not-real"}))
            .send()
            .await
            .expect("vault setup");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "vault setup must succeed (leaves vault unlocked)"
        );
    }

    Box::leak(Box::new(tmp)); // keep files alive for the (short) test
    (base, client)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_endpoints_locked_vault_returns_403() {
    let (base, client) = spawn(false).await; // no setup → vault sealed/locked
    let projects = format!("{}/api/v1/projects", base);

    // 401 (auth) or 403 (vault not unlocked) both prove the endpoint exists +
    // is guarded. With no_auth=true and no setup, the vault is sealed → 403.
    let acceptable = |s: u16| s == 401 || s == 403;

    let cases: Vec<(reqwest::RequestBuilder, &str)> = vec![
        (client.get(&projects), "GET /projects"),
        (
            client
                .post(&projects)
                .json(&serde_json::json!({"title": "t", "kind": "case"})),
            "POST /projects",
        ),
        (client.get(format!("{}/some-id", projects)), "GET /projects/:id"),
        (
            client.get(format!("{}/some-id/files", projects)),
            "GET /projects/:id/files",
        ),
        (
            client
                .post(format!("{}/some-id/files", projects))
                .json(&serde_json::json!({"file_id": "f1", "role": "evidence"})),
            "POST /projects/:id/files",
        ),
        (
            client.get(format!("{}/some-id/timeline", projects)),
            "GET /projects/:id/timeline",
        ),
    ];
    for (req, label) in cases {
        let resp = req.send().await.expect(label);
        assert!(
            acceptable(resp.status().as_u16()),
            "{label}: expected 401 or 403 (vault locked), got {}",
            resp.status()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_crud_round_trip_unlocked_vault() {
    let (base, client) = spawn(true).await; // setup → vault UNLOCKED
    let projects = format!("{}/api/v1/projects", base);

    // 1. Empty list initially.
    let resp = client.get(&projects).send().await.expect("list");
    assert_eq!(resp.status().as_u16(), 200, "unlocked vault must serve the route");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 0, "fresh vault has no projects");
    assert!(body["projects"].as_array().unwrap().is_empty());

    // 2. Create → 201 with the created project echoed back.
    let resp = client
        .post(&projects)
        .json(&serde_json::json!({"title": "  Acme v Roe  ", "kind": "case"}))
        .send()
        .await
        .expect("create");
    assert_eq!(resp.status().as_u16(), 201, "create returns 201 CREATED");
    let created: serde_json::Value = resp.json().await.expect("json");
    let pid = created["id"].as_str().expect("project id").to_string();
    assert_eq!(created["title"], "Acme v Roe", "handler must trim the title");
    assert_eq!(created["kind"], "case");

    // 3. Empty title → 400 (error case through the real handler).
    let resp = client
        .post(&projects)
        .json(&serde_json::json!({"title": "   "}))
        .send()
        .await
        .expect("create empty");
    assert_eq!(resp.status().as_u16(), 400, "empty title must be rejected by the handler");

    // 4. kind defaults to "generic" when omitted.
    let resp = client
        .post(&projects)
        .json(&serde_json::json!({"title": "No Kind"}))
        .send()
        .await
        .expect("create no-kind");
    assert_eq!(resp.status().as_u16(), 201);
    let v: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(v["kind"], "generic", "missing kind defaults to generic");

    // 5. List now has 2.
    let resp = client.get(&projects).send().await.expect("list 2");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 2);

    // 6. Get the created one by id → 200.
    let resp = client.get(format!("{}/{}", projects, pid)).send().await.expect("get");
    assert_eq!(resp.status().as_u16(), 200);
    let got: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(got["id"], pid);
    assert_eq!(got["title"], "Acme v Roe");

    // 7. Get unknown id → 404.
    let resp = client
        .get(format!("{}/does-not-exist", projects))
        .send()
        .await
        .expect("get missing");
    assert_eq!(resp.status().as_u16(), 404, "unknown project id → 404");

    // 8. Add a file to the project → 201.
    let resp = client
        .post(format!("{}/{}/files", projects, pid))
        .json(&serde_json::json!({"file_id": "file-abc", "role": "evidence"}))
        .send()
        .await
        .expect("add file");
    assert_eq!(resp.status().as_u16(), 201, "add file returns 201");

    // 9. Add a file to a NON-existent project → 404.
    let resp = client
        .post(format!("{}/nope/files", projects))
        .json(&serde_json::json!({"file_id": "x"}))
        .send()
        .await
        .expect("add file missing project");
    assert_eq!(resp.status().as_u16(), 404);

    // 10. List the project's files → contains the one we added.
    let resp = client
        .get(format!("{}/{}/files", projects, pid))
        .send()
        .await
        .expect("list files");
    assert_eq!(resp.status().as_u16(), 200);
    let files: serde_json::Value = resp.json().await.expect("json");
    let arr = files["files"].as_array().expect("files array");
    assert_eq!(arr.len(), 1, "exactly one file linked");
    assert_eq!(arr[0]["file_id"], "file-abc");

    // 11. Timeline route responds 200 with an entries array (may be empty).
    let resp = client
        .get(format!("{}/{}/timeline", projects, pid))
        .send()
        .await
        .expect("timeline");
    assert_eq!(resp.status().as_u16(), 200);
    let tl: serde_json::Value = resp.json().await.expect("json");
    assert!(tl["entries"].is_array(), "timeline returns an entries array");
}
