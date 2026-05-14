//! System test (Layer 3 of test pyramid, per `docs/TESTING.md`):
//! Full wizard → vault → ingest → search → settings → lock-unlock chain
//! over real HTTP, no browser. Covers F-01-VAULT + F-02-RAG + F-09-FORMFACTOR.
//!
//! Strategy: single `#[tokio::test]` runs an 8-step sequence on a single server
//! instance to avoid 30-60s init cost per scenario. All assertions must pass
//! before the next step executes (fail-fast). This is what TESTING.md §1.1
//! defines as a System test — server spawned, HTTP client black-box, no browser.

use std::sync::Mutex;
use std::time::Duration;

/// Process-wide test mutex. Tests in this file mutate HOME / XDG_* env vars
/// and bind to ports — serialize to avoid cargo test parallel interference.
static TEST_MUTEX: Mutex<()> = Mutex::new(());

async fn wait_for_health(base: &str) -> Result<(), String> {
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
    Err(format!("server at {base} not ready in 15s"))
}

/// F-01-VAULT + F-02-RAG + F-09-FORMFACTOR System test.
///
/// 8-step sequence simulating a fresh user installing attune and going through
/// the wizard end-to-end via REST (no browser):
///
///   1. SEALED state (fresh install) → vault status reports SEALED
///   2. Vault setup with master password → returns session token, transitions to UNLOCKED
///   3. GET /settings (with bearer) → laptop default LLM = openai_compat
///   4. PATCH /settings → user fills LLM endpoint + api_key
///   5. POST /ingest 2 documents → items count = 2
///   6. GET /items → list returns 2 items
///   7. POST /vault/lock → state = LOCKED
///   8. POST /vault/unlock → state = UNLOCKED again, items still queryable
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "slow E2E (~70s, 8-step wizard with full Argon2id); R19 nightly only — run with --include-ignored"]
async fn wizard_full_flow_end_to_end() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    // Isolate vault data dir
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    std::env::remove_var("ATTUNE_FORM_FACTOR"); // ensure laptop default

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = attune_server::ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: false, // realistic — auth enabled, requires bearer for protected routes
    };
    let handle = tokio::spawn(async move {
        let _ = attune_server::run_in_runtime(config).await;
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_health(&base).await.expect("server ready");
    let client = reqwest::Client::new();

    // ── Step 1: SEALED state on fresh install ─────────────────────────────
    // covers: F-01-VAULT (state machine SEALED entry)
    let resp = client
        .get(format!("{}/api/v1/vault/status", base))
        .send()
        .await
        .expect("vault status");
    assert_eq!(resp.status().as_u16(), 200, "vault status should be 200");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["state"].as_str(),
        Some("sealed"),
        "fresh install should be SEALED, got: {:?}",
        body["state"]
    );

    // ── Step 2: Vault setup → token + UNLOCKED ────────────────────────────
    // covers: F-01-VAULT (setup → token)
    let resp = client
        .post(format!("{}/api/v1/vault/setup", base))
        .json(&serde_json::json!({"password": "P@ssw0rd-SystemWizardTest-Step2"}))
        .send()
        .await
        .expect("setup");
    assert_eq!(resp.status().as_u16(), 200, "setup must be 200");
    let setup_body: serde_json::Value = resp.json().await.unwrap();
    let token = setup_body["token"]
        .as_str()
        .expect("setup must return token (per vault_setup_test invariant)")
        .to_string();
    assert_eq!(setup_body["state"].as_str(), Some("unlocked"));

    // ── Step 3: GET /settings → laptop default LLM = openai_compat ────────
    // covers: F-09-FORMFACTOR (default_settings on Laptop form_factor)
    let resp = client
        .get(format!("{}/api/v1/settings", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get settings");
    assert_eq!(resp.status().as_u16(), 200);
    let settings: serde_json::Value = resp.json().await.unwrap();
    let llm = settings.get("llm").expect("llm key");
    assert_eq!(
        llm["provider"].as_str(),
        Some("openai_compat"),
        "Laptop default LLM provider must be openai_compat (v0.6.0 GA invariant)"
    );
    assert!(
        llm["endpoint"].is_null(),
        "Laptop default endpoint must be null (UI guides user to fill)"
    );
    assert_eq!(
        llm["api_key_set"].as_bool(),
        Some(false),
        "Fresh setup must report api_key_set=false (no key configured yet)"
    );

    // ── Step 4: PATCH /settings → user fills LLM endpoint + key ───────────
    // covers: F-01-VAULT (api_key encrypted at rest), F-09-FORMFACTOR (settings update)
    let resp = client
        .patch(format!("{}/api/v1/settings", base))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://api.deepseek.com/v1",
                "model": "deepseek-chat",
                "api_key": "sk-test-NOT-REAL-KEY-FOR-TESTING"
            }
        }))
        .send()
        .await
        .expect("patch settings");
    assert_eq!(resp.status().as_u16(), 200, "settings update should be 200");

    // Verify update persisted, but api_key is REDACTED in GET response
    let resp = client
        .get(format!("{}/api/v1/settings", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get settings after patch");
    let settings: serde_json::Value = resp.json().await.unwrap();
    let llm = settings.get("llm").expect("llm");
    assert_eq!(llm["endpoint"].as_str(), Some("https://api.deepseek.com/v1"));
    assert_eq!(llm["model"].as_str(), Some("deepseek-chat"));
    assert!(
        llm["api_key"].is_null(),
        "Security invariant: GET /settings must redact api_key, got: {:?}",
        llm["api_key"]
    );
    assert_eq!(
        llm["api_key_set"].as_bool(),
        Some(true),
        "api_key_set should be true after PATCH stored a real key"
    );

    // ── Step 5: POST /ingest 2 documents ─────────────────────────────────
    // covers: F-02-RAG (ingest path), F-01-VAULT (encrypted at rest)
    let docs = vec![
        ("Rust Ownership", "Rust uses ownership to manage memory without garbage collection. Each value has a single owner."),
        ("Borrowing Rules", "References allow borrowing without taking ownership. Either one mutable or any number of immutable references."),
    ];
    for (title, content) in &docs {
        let resp = client
            .post(format!("{}/api/v1/ingest", base))
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "title": title,
                "content": content,
                "source_type": "manual"
            }))
            .send()
            .await
            .expect("ingest");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "ingest '{}' should be 200, got: {}",
            title,
            resp.status()
        );
    }

    // ── Step 6: GET /items → list returns 2 items ─────────────────────────
    // covers: F-02-RAG (list path)
    let resp = client
        .get(format!("{}/api/v1/items?limit=10", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list items");
    assert_eq!(resp.status().as_u16(), 200);
    let items_body: serde_json::Value = resp.json().await.unwrap();
    let items = items_body
        .get("items")
        .and_then(|v| v.as_array())
        .expect("items array");
    assert_eq!(items.len(), 2, "should have ingested 2 items, got {}", items.len());

    // ── Step 7: POST /vault/lock → state = LOCKED ────────────────────────
    // covers: F-01-VAULT (lock transition + key zeroization)
    let resp = client
        .post(format!("{}/api/v1/vault/lock", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("lock");
    assert_eq!(resp.status().as_u16(), 200, "lock should be 200");
    let resp = client
        .get(format!("{}/api/v1/vault/status", base))
        .send()
        .await
        .expect("status after lock");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["state"].as_str(), Some("locked"));

    // After lock, items endpoint should refuse with 403 (vault locked)
    let resp = client
        .get(format!("{}/api/v1/items?limit=10", base))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list items after lock");
    assert!(
        resp.status() == 403 || resp.status() == 401,
        "items must be inaccessible after lock, got status: {}",
        resp.status()
    );

    // ── Step 8: POST /vault/unlock with correct password → UNLOCKED ──────
    // covers: F-01-VAULT (unlock with correct password preserves data)
    let resp = client
        .post(format!("{}/api/v1/vault/unlock", base))
        .json(&serde_json::json!({"password": "P@ssw0rd-SystemWizardTest-Step2"}))
        .send()
        .await
        .expect("unlock");
    assert_eq!(resp.status().as_u16(), 200, "unlock should be 200");
    let unlock_body: serde_json::Value = resp.json().await.unwrap();
    let new_token = unlock_body["token"]
        .as_str()
        .expect("unlock returns new token")
        .to_string();

    // Verify items still accessible with new token (data persisted across lock-unlock)
    let resp = client
        .get(format!("{}/api/v1/items?limit=10", base))
        .bearer_auth(&new_token)
        .send()
        .await
        .expect("list items after unlock");
    assert_eq!(resp.status().as_u16(), 200);
    let items_body: serde_json::Value = resp.json().await.unwrap();
    let items = items_body
        .get("items")
        .and_then(|v| v.as_array())
        .expect("items");
    assert_eq!(
        items.len(),
        2,
        "items must persist across lock-unlock (encryption integrity)"
    );

    handle.abort();
}
