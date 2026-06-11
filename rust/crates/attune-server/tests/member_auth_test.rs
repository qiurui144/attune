//! R1.1a regression — /api/v1/member/* must NOT bypass bearer auth.
//!
//! Before the fix, `middleware.rs::bearer_auth_guard` exempted the whole
//! `/api/v1/member` prefix from the token check, so with `--require-auth`
//! (NAS mode) an unauthenticated client could mutate member_state
//! (login-token / logout) or forward credentials (login-password).
//!
//! Caller audit (see middleware.rs comment): every legitimate member call
//! happens after a session token exists, so no member endpoint needs an
//! anonymous exemption.
//!
//! Single test fn — the server redirects HOME/XDG to a tempdir via
//! `std::env::set_var`, which is unsound across parallel tests in one binary.

use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_routes_require_bearer_when_auth_enabled() {
    // Isolate vault/config side-effects to a tempdir (mirrors test_support.rs).
    let tmp = tempfile::TempDir::new().expect("tempdir");
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }

    let vault =
        attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    // require_auth = true → bearer_auth_guard active on every non-exempt route.
    let state = Arc::new(attune_server::state::AppState::new(vault, true));
    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service()).await;
    });
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // Wait for readiness via the always-public /health.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(format!("{base}/health")).send().await {
            if r.status().is_success() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // ── 1) every member route without a token → 401 ─────────────────────────
    for (method, path) in [
        ("GET", "/api/v1/member/state"),
        ("GET", "/api/v1/member/locks"),
        ("POST", "/api/v1/member/login-token"),
        ("POST", "/api/v1/member/login-password"),
        ("POST", "/api/v1/member/logout"),
    ] {
        let req = match method {
            "GET" => client.get(format!("{base}{path}")),
            _ => client
                .post(format!("{base}{path}"))
                .json(&serde_json::json!({})),
        };
        let resp = req.send().await.expect("send");
        assert_eq!(
            resp.status().as_u16(),
            401,
            "{method} {path} without bearer token must be 401 (R1.1a)"
        );
    }

    // ── 2) bootstrap exemptions stay anonymous-usable ────────────────────────
    let resp = client
        .get(format!("{base}/api/v1/vault/status"))
        .send()
        .await
        .expect("vault status");
    assert!(
        resp.status().is_success(),
        "GET /api/v1/vault/status must stay public (bootstrap), got {}",
        resp.status()
    );

    // ── 3) with a valid token (issued by vault setup) member routes work ────
    let resp = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({ "password": "P@ss-member-auth-not-real" }))
        .send()
        .await
        .expect("vault setup");
    assert!(resp.status().is_success(), "vault setup failed: {}", resp.status());
    let body: serde_json::Value = resp.json().await.expect("setup json");
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .expect("setup must return a session token")
        .to_string();

    let resp = client
        .get(format!("{base}/api/v1/member/state"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("member state with token");
    assert!(
        resp.status().is_success(),
        "GET /member/state WITH bearer token must succeed, got {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.expect("state json");
    assert_eq!(
        body.get("is_logged_in").and_then(|v| v.as_bool()),
        Some(false),
        "fresh server defaults to logged-out"
    );

    server.abort();
}
