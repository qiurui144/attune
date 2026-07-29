//! Regression for the unlock/search-bootstrap LLM ownership boundary.
//!
//! BYOK credentials are stored outside the plaintext `app_settings` row. The
//! unlock path first rebuilds the LLM from the decrypted settings view and then
//! initializes search engines. Search initialization must not reconstruct and
//! replace that provider from the secret-free row.

use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(unsafe_code)]
async fn encrypted_byok_provider_survives_unlock_search_initialization() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    // This integration-test binary contains only this test, so its process-wide
    // data-directory override cannot race another test in the same process.
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::set_var("HF_HUB_OFFLINE", "1");
    }

    const PASSWORD: &str = "P@ss-encrypted-byok-unlock";
    const BYOK: &str = "sk-encrypted-byok-regression";

    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open vault");
    vault.setup(PASSWORD).expect("setup vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind server");
    let address = listener.local_addr().expect("server address");
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve router");
    });

    let response = reqwest::Client::new()
        .patch(format!("http://{address}/api/v1/settings"))
        .json(&serde_json::json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://byok.example.test/v1",
                "model": "byok-test-model",
                "api_key": BYOK
            }
        }))
        .send()
        .await
        .expect("persist BYOK through settings route");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(
        state.llm().is_some(),
        "settings hot reload must install LLM"
    );

    // Prove the fixture exercises split encrypted persistence, not the legacy
    // plaintext representation that originally masked this regression.
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let raw = vault
            .store()
            .get_meta("app_settings")
            .expect("read settings row")
            .expect("settings row");
        let raw_settings: serde_json::Value =
            serde_json::from_slice(&raw).expect("parse settings row");
        assert!(raw_settings.pointer("/llm/api_key").is_none());
        assert!(!raw
            .windows(BYOK.len())
            .any(|window| window == BYOK.as_bytes()));

        let encrypted = vault
            .store()
            .get_meta("app_secret.llm_api_key.v1")
            .expect("read encrypted secret")
            .expect("encrypted LLM secret row");
        assert!(!encrypted
            .windows(BYOK.len())
            .any(|window| window == BYOK.as_bytes()));
    }

    state
        .lock_vault_and_clear_runtime()
        .expect("lock and clear runtime");
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.unlock(PASSWORD).expect("unlock vault");
    }

    // This mirrors `spawn_post_unlock_services`: decrypted LLM restore first,
    // search/taxonomy initialization second.
    state.reload_llm();
    let restored = state.llm().expect("restore encrypted BYOK provider");
    state.init_search_engines();
    let initialized = state.llm().expect("LLM after search initialization");

    assert!(
        Arc::ptr_eq(&restored, &initialized),
        "search initialization replaced the decrypted BYOK provider"
    );
    assert!(state.taxonomy.lock().unwrap().is_some());
    assert!(
        state.classifier().is_some(),
        "taxonomy must bind its classifier to the restored LLM"
    );

    server.abort();
}
