//! Tests that GET /api/v1/ai_stack returns a `web_search` field whose
//! `available` boolean tracks the actual `state.web_search` Arc — not a
//! hardcoded value.
//!
//! Two sub-cases are tested within a single server instance:
//!   1. Provider absent  (state.web_search = None)  → available=false, note=String
//!   2. Provider present (state.web_search = Some(_)) → available=true,  note=null
//!
//! This proves the route reads the live Arc rather than returning a
//! compile-time constant — a hardcoded `false` would fail case 2.

use std::sync::Arc;
use std::time::Duration;
use attune_core::error::Result as CoreResult;
use attune_core::web_search::{WebSearchProvider, WebSearchResult};

// ── Minimal stub that satisfies WebSearchProvider ────────────────────────────

struct StubWebSearch;

impl WebSearchProvider for StubWebSearch {
    fn search(&self, _q: &str, _limit: usize) -> CoreResult<Vec<WebSearchResult>> {
        Ok(vec![])
    }
    fn provider_name(&self) -> &str {
        "stub"
    }
    fn is_configured(&self) -> bool {
        true
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

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

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Verify that `available` in the `/api/v1/ai_stack` response directly
/// reflects `state.web_search` by exercising both the None and Some branches
/// on the same running server instance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_stack_web_search_available_tracks_state() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    // Build AppState directly so we hold a reference for later mutation.
    let vault =
        attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false /* require_auth */));

    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    // Vault setup is required so vault_guard permits /ai_stack.
    let client = reqwest::Client::new();
    let setup = client
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({"password": "ai-stack-test-pw"}))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup.status().as_u16(), 200, "vault setup failed");

    // Give the background provider-init task a moment to finish (it runs after
    // vault setup), then explicitly clear web_search so Case 1 starts from a
    // known-None baseline regardless of whether Chrome is installed on the host.
    tokio::time::sleep(Duration::from_millis(500)).await;
    state.set_web_search(None);

    // ── Case 1: no provider (state.web_search = None) ────────────────────────
    {
        let resp = client
            .get(format!("{}/api/v1/ai_stack", base))
            .send()
            .await
            .expect("GET /ai_stack (none case)");
        assert_eq!(resp.status().as_u16(), 200, "/ai_stack must return 200");

        let body: serde_json::Value = resp.json().await.expect("json body");
        let ws = body
            .get("web_search")
            .expect("`web_search` field missing from /ai_stack response");

        let available = ws.get("available").expect("`web_search.available` missing");
        assert!(
            available.is_boolean(),
            "`web_search.available` must be bool: {available}"
        );
        assert_eq!(
            available.as_bool().unwrap(),
            false,
            "available must be false when no provider is loaded"
        );

        // Route must provide an explanatory note when unavailable.
        let note = ws.get("note").expect("`web_search.note` missing");
        assert!(
            note.is_string(),
            "`web_search.note` must be a string when available=false, got: {note}"
        );

        let engine = ws.get("engine").expect("`web_search.engine` missing");
        assert!(engine.is_string(), "`web_search.engine` must be a string");
    }

    // ── Case 2: inject stub provider → available must flip to true ───────────
    // This is the critical assertion: a hardcoded `false` would fail here.
    state.set_web_search(Some(Arc::new(StubWebSearch)));

    {
        let resp = client
            .get(format!("{}/api/v1/ai_stack", base))
            .send()
            .await
            .expect("GET /ai_stack (some case)");
        assert_eq!(resp.status().as_u16(), 200, "/ai_stack must return 200");

        let body: serde_json::Value = resp.json().await.expect("json body");
        let ws = body
            .get("web_search")
            .expect("`web_search` field missing");

        let available = ws.get("available").expect("`web_search.available` missing");
        assert_eq!(
            available.as_bool().unwrap_or(false),
            true,
            "available must be true after injecting a provider — \
             the route must read the live state.web_search Arc"
        );

        // No install hint needed when the provider is present.
        let note = ws.get("note").expect("`web_search.note` missing");
        assert!(
            note.is_null(),
            "`web_search.note` must be null when available=true, got: {note}"
        );
    }

    server_handle.abort();
}
