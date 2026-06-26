//! Integration test for `GET /api/v1/diagnostics/capabilities`.
//!
//! Plan 2026-06-26-capability-registry-p0.md Task 6.
//!
//! Serves the real `build_router` (full middleware stack incl. vault_guard) over
//! a loopback TCP listener and drives it with reqwest. The vault is LOCKED (never
//! unlocked) — the endpoint must still return 200 because /api/v1/diagnostics is
//! on the vault_guard bypass list (read-only capability metadata, usable on the
//! lock screen).

use std::sync::Arc;

async fn serve_locked() -> (String, tokio::task::JoinHandle<()>) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    // Memory vault → Locked state (never unlocked).
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    std::mem::forget(tmp);
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let app = attune_server::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_capabilities_returns_registry_projection() {
    let (base, handle) = serve_locked().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{base}/api/v1/diagnostics/capabilities"))
        .send()
        .await
        .expect("request");
    // Bypasses vault_guard even though the vault is LOCKED.
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let v: serde_json::Value = resp.json().await.expect("json");
    let arr = v.as_array().expect("array of capabilities");
    assert!(arr.len() >= 9, "expected >=9 capabilities, got {}", arr.len());

    // Each entry carries the full schema.
    let first = &arr[0];
    for key in [
        "id",
        "name",
        "kind",
        "installed",
        "enabled",
        "requires_member",
        "requires_local_model",
        "allows_outbound",
        "tier",
        "build_profile",
        "health",
        "ui_visible",
    ] {
        assert!(first.get(key).is_some(), "missing field {key} in {first}");
    }

    // id-sorted projection (BTreeMap order).
    let ids: Vec<&str> = arr.iter().map(|c| c["id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "wire response must be id-sorted");

    // OSS boundary audit on the wire (spec §9 核心断言): only oss-tier, no verticals.
    for c in arr {
        assert_eq!(c["tier"], "oss", "wire response must contain only oss-tier caps");
        let id = c["id"].as_str().unwrap();
        for banned in ["law", "patent", "presales", "medical", "academic"] {
            assert!(
                !id.contains(banned),
                "pro vertical id leaked onto the wire: {id}"
            );
        }
    }

    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_capabilities_reflects_health_refresh() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    std::mem::forget(tmp);
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    // Mark embedding ready BEFORE serving — the handler's refresh must reflect it.
    state.model_bootstrap.mark_ready("embedding");

    let app = attune_server::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();
    let v: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/diagnostics/capabilities"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");

    let arr = v.as_array().expect("array");
    let emb = arr
        .iter()
        .find(|c| c["id"] == "embedding")
        .expect("embedding capability present");
    assert_eq!(
        emb["health"], "ok",
        "embedding health must be ok after refresh (model_bootstrap ready)"
    );
    assert_eq!(emb["enabled"], true);

    handle.abort();
}
