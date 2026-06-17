//! Tests that GET /api/v1/ai_stack exposes the cross-vendor EP selection +
//! on-demand EP-runtime-stack installer state:
//!   - `accel.ep_chain`       — ordered EP selection chain, last element "cpu"
//!   - `accel.active_ep`      — best-effort active EP (string)
//!   - `accel.compiled_eps`   — EPs compiled into this artifact (contains "cpu")
//!   - `ep_runtime_stacks`    — stack installer snapshot (`{stacks: {...}}`)
//!
//! On a CI runner with no GPU EP feature compiled (default workspace build),
//! the chain must be exactly ["cpu"] and the stacks map must be present (empty
//! or only what bootstrap scheduled). This locks the §5/§7 API contract and the
//! "末位永远 CPU" invariant end-to-end through the HTTP layer.

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_stack_exposes_ep_chain_and_runtime_stacks() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    // Offline so neither model nor stack bootstrap hangs on network.
    std::env::set_var("HF_HUB_OFFLINE", "1");

    let vault =
        attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    vault.setup("ep-matrix-test-pw").expect("vault setup");
    vault.unlock("ep-matrix-test-pw").expect("vault unlock");

    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/v1/ai_stack", base))
        .send()
        .await
        .expect("GET /ai_stack");
    assert_eq!(resp.status().as_u16(), 200, "/ai_stack must return 200");
    let body: serde_json::Value = resp.json().await.expect("json body");

    // ── accel.ep_chain ──────────────────────────────────────────────────────
    let accel = body.get("accel").expect("`accel` field missing");
    let chain = accel
        .get("ep_chain")
        .and_then(|v| v.as_array())
        .expect("`accel.ep_chain` must be an array");
    assert!(!chain.is_empty(), "ep_chain must be non-empty");
    // Invariant: last element is always "cpu".
    assert_eq!(
        chain.last().unwrap().as_str(),
        Some("cpu"),
        "ep_chain must end with cpu (兜底不变量): {chain:?}"
    );

    // active_ep is a string.
    let active = accel.get("active_ep").expect("`accel.active_ep` missing");
    assert!(active.is_string(), "active_ep must be a string: {active}");

    // compiled_eps contains "cpu".
    let compiled = accel
        .get("compiled_eps")
        .and_then(|v| v.as_array())
        .expect("`accel.compiled_eps` must be an array");
    assert!(
        compiled.iter().any(|e| e.as_str() == Some("cpu")),
        "compiled_eps must contain cpu: {compiled:?}"
    );

    // active_ep_approx is a bool (best-effort flag, R3).
    assert!(
        accel.get("active_ep_approx").map(|v| v.is_boolean()).unwrap_or(false),
        "active_ep_approx must be bool"
    );

    // ── ep_runtime_stacks ───────────────────────────────────────────────────
    let stacks = body
        .get("ep_runtime_stacks")
        .expect("`ep_runtime_stacks` field missing");
    assert!(
        stacks.get("stacks").map(|s| s.is_object()).unwrap_or(false),
        "ep_runtime_stacks.stacks must be an object: {stacks}"
    );

    // On default CI build (no GPU EP feature), chain is CPU-only → no userspace
    // stack scheduled → fallback_reason present.
    if chain.len() == 1 {
        let fr = accel.get("fallback_reason").expect("fallback_reason key present");
        assert!(
            fr.as_str().map(|s| s.contains("accel-ep-not-compiled")).unwrap_or(false),
            "CPU-only chain must explain fallback: {fr}"
        );
    }

    server_handle.abort();
    std::env::remove_var("HF_HUB_OFFLINE");
}
