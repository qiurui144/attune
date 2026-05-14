//! Integration test covering F-09-FORMFACTOR end-to-end.
//!
//! Validates the full chain: env var override → `HardwareProfile::detect()` at
//! server startup → `/api/v1/status/diagnostics` exposes `form_factor` + `prefers_local_llm`
//! → `/api/v1/settings` (post-vault-setup) reflects K3-specific LLM defaults.
//!
//! These tests cover what 4 form_factor unit tests in `platform::tests` and 4
//! `routes::settings::tests` cannot: they validate the wiring between
//! `HardwareProfile::form_factor` field and the routes that read it.
//!
//! Concurrency: we use a process-wide Mutex to serialize tests because they all
//! mutate the same global `ATTUNE_FORM_FACTOR` env var + `HOME`/`XDG_*` overrides.
//! Without this, parallel cargo test execution interleaves env mutations and
//! HardwareProfile::detect() picks up unpredictable values.

use std::sync::Mutex;
use std::time::Duration;

/// Process-wide test mutex. All tests in this file acquire this before mutating
/// env vars or starting a server. cargo test runs tests in parallel by default.
static TEST_MUTEX: Mutex<()> = Mutex::new(());

/// Wait for server's /api/v1/status/health to return 200, or fail after timeout.
async fn wait_for_server_ready(base: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let url = format!("{}/api/v1/status/health", base);
    while std::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("server at {base} did not become ready within 15s"))
}

/// Helper: setup vault and return session token for protected route access.
async fn setup_vault_get_token(base: &str) -> String {
    let resp = reqwest::Client::new()
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({
            "password": "P@ssw0rd-FormFactorIntegrationTest"
        }))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(resp.status().as_u16(), 200, "vault setup must succeed");
    let body: serde_json::Value = resp.json().await.expect("json");
    body["token"]
        .as_str()
        .expect("setup must return session token (per vault_setup_test invariant)")
        .to_string()
}

/// Helper: spawn a server with isolated tempdir + given env vars, return base URL.
async fn spawn_isolated_server(
    form_factor_env: Option<&str>,
    tmp: &tempfile::TempDir,
) -> (String, tokio::task::JoinHandle<()>) {
    // Isolate vault data dir
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    if let Some(v) = form_factor_env {
        std::env::set_var("ATTUNE_FORM_FACTOR", v);
    } else {
        std::env::remove_var("ATTUNE_FORM_FACTOR");
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = attune_server::ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: true,
    };
    let handle = tokio::spawn(async move {
        let _ = attune_server::run_in_runtime(config).await;
    });

    let base = format!("http://127.0.0.1:{}", port);
    // Wait for server health endpoint to respond (replaces fixed sleep — more robust)
    wait_for_server_ready(&base)
        .await
        .expect("server failed to start");
    (base, handle)
}

/// F-09-FORMFACTOR Integration — K3 env var → diagnostics returns form_factor=k3
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~60s/test, full Argon2id setup); R19 nightly only — run with --include-ignored"]
async fn k3_env_var_propagates_to_diagnostics() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().expect("tmp");
    let (base, handle) = spawn_isolated_server(Some("k3"), &tmp).await;
    let token = setup_vault_get_token(&base).await;

    let resp = reqwest::Client::new()
        .get(format!("{}/api/v1/status/diagnostics", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get diagnostics");
    assert_eq!(resp.status().as_u16(), 200, "diagnostics should be 200");

    let body: serde_json::Value = resp.json().await.expect("json body");
    let hardware = body.get("hardware").expect("hardware key");

    assert_eq!(
        hardware["form_factor"].as_str(),
        Some("k3"),
        "ATTUNE_FORM_FACTOR=k3 should produce form_factor=k3, got: {:?}",
        hardware["form_factor"]
    );
    assert_eq!(
        hardware["prefers_local_llm"].as_bool(),
        Some(true),
        "K3 form factor should prefer local LLM"
    );

    handle.abort();
    std::env::remove_var("ATTUNE_FORM_FACTOR");
}

/// F-09-FORMFACTOR Integration — laptop default (no env) → diagnostics form_factor=laptop
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~60s/test, full Argon2id setup); R19 nightly only — run with --include-ignored"]
async fn laptop_default_propagates_to_diagnostics() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().expect("tmp");
    let (base, handle) = spawn_isolated_server(None, &tmp).await;
    let token = setup_vault_get_token(&base).await;

    let resp = reqwest::Client::new()
        .get(format!("{}/api/v1/status/diagnostics", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get diagnostics");
    let body: serde_json::Value = resp.json().await.expect("json body");
    let hardware = body.get("hardware").expect("hardware key");

    let ff = hardware["form_factor"].as_str().unwrap_or("");
    // CI / dev machines normally aren't K3 / Jetson — DMI keyword detection should
    // produce "laptop" or possibly "unknown" if /sys/class/dmi/id/product_name absent.
    // Either way, prefers_local_llm must be false (only K3 prefers local).
    assert!(
        ff == "laptop" || ff == "unknown",
        "non-K3 system should report laptop or unknown form_factor, got: {ff}"
    );
    assert_eq!(
        hardware["prefers_local_llm"].as_bool(),
        Some(false),
        "non-K3 form factor must NOT prefer local LLM"
    );

    handle.abort();
}

/// F-09-FORMFACTOR Integration — invalid env var falls back to laptop (per detect_form_factor)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~60s/test, full Argon2id setup); R19 nightly only — run with --include-ignored"]
async fn invalid_env_var_falls_back_to_laptop() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().expect("tmp");
    let (base, handle) = spawn_isolated_server(Some("garbage_value_xyz"), &tmp).await;
    let token = setup_vault_get_token(&base).await;

    let resp = reqwest::Client::new()
        .get(format!("{}/api/v1/status/diagnostics", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get diagnostics");
    let body: serde_json::Value = resp.json().await.expect("json body");
    let hardware = body.get("hardware").expect("hardware key");

    assert_eq!(
        hardware["form_factor"].as_str(),
        Some("laptop"),
        "unrecognized ATTUNE_FORM_FACTOR value should fall back to laptop, got: {:?}",
        hardware["form_factor"]
    );

    handle.abort();
    std::env::remove_var("ATTUNE_FORM_FACTOR");
}

/// F-09-FORMFACTOR + F-01-VAULT Integration — settings.llm reflects K3 default after vault setup
///
/// This test exercises the cross-cutting wire: form_factor (F-09) drives
/// default_settings (F-09 unit) which is exposed via the GET /settings endpoint
/// (F-01 vault-guarded). Without form_factor wiring, K3 image ships with empty
/// LLM endpoint and a confused first-launch UX.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~60s/test, full Argon2id setup); R19 nightly only — run with --include-ignored"]
async fn k3_form_factor_drives_settings_llm_default() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().expect("tmp");
    let (base, handle) = spawn_isolated_server(Some("k3"), &tmp).await;
    let client = reqwest::Client::new();

    // Setup vault to obtain session token + unlock state
    let setup_resp = client
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({
            "password": "P@ssw0rd-FormFactorTest-K3"
        }))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(setup_resp.status().as_u16(), 200, "vault setup should be 200");
    let setup_body: serde_json::Value = setup_resp.json().await.expect("json");
    let token = setup_body["token"]
        .as_str()
        .expect("setup must return session token (per vault_setup_test invariant)")
        .to_string();

    // GET /settings with bearer token
    let settings_resp = client
        .get(format!("{}/api/v1/settings", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get settings");
    assert_eq!(settings_resp.status().as_u16(), 200);
    let settings: serde_json::Value = settings_resp.json().await.expect("json");

    // K3 form_factor → llm.provider="ollama" + endpoint preset, but no pinned chat model
    let llm = settings.get("llm").expect("llm key in settings");
    assert_eq!(
        llm["provider"].as_str(),
        Some("ollama"),
        "K3 form factor should default LLM provider to 'ollama', got: {:?}",
        llm["provider"]
    );
    assert_eq!(
        llm["endpoint"].as_str(),
        Some("http://localhost:11434/v1"),
        "K3 form factor should preset Ollama endpoint, got: {:?}",
        llm["endpoint"]
    );
    assert!(llm["model"].is_null(),
        "K3 form factor should leave chat model unset so runtime can auto-detect a lighter local model, got: {:?}",
        llm["model"]);

    handle.abort();
    std::env::remove_var("ATTUNE_FORM_FACTOR");
}
