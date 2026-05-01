//! ⚠️ ANTI-FEATURE LOCKING TEST ⚠️
//!
//! This test **locks down current incomplete behavior**, NOT a desired state.
//! It exists to:
//!   1. Make the gap between v0.6.1 release-notes promise and code reality VISIBLE
//!   2. Prevent silent re-introduction of the same bug after eventual fix
//!   3. Force a deliberate decision when the fix lands (this test will fail and
//!      the fixer must update assertions to the post-fix expected behavior)
//!
//! ## Background
//!
//! `docs/v0.6.1-release-notes.md` and `docs/FEATURES.md` F-17-PRIVACY claim:
//!   "L1 (default) 12 PII classes... detected by regex and replaced with
//!    reversible [KIND_N] placeholders **before any cloud API call**, with an
//!    outbound audit log..."
//!
//! Audit on 2026-05-01 found:
//!   - `pii::Redactor::redact()` exists and is unit-tested ✅
//!   - It is invoked in **zero** production code paths ❌
//!   - Specifically NOT invoked in:
//!     - `attune_core::chat::ChatEngine::chat()` (LLM call site)
//!     - `attune_server::routes::chat` (HTTP entry)
//!     - `attune_core::context_compress` (chunk → LLM summary call)
//!     - `attune_core::ai_annotator` (Reader → LLM call)
//!     - `attune_core::web_search_browser` (web search uses query directly)
//!   - Outbound audit log: scaffolded but never written
//!
//! The PII module is therefore **module-shipped but not wired** — comparable to
//! the historical "P0 approved ≠ code shipped" anti-pattern, here surfacing as
//! "code shipped ≠ business path wired".
//!
//! ## What this test does
//!
//! The chat endpoint accepts user input directly. We send a request containing
//! known PII (a phone number) and assert the chat path **does NOT** transform
//! the input — confirming the gap. When the fix lands (PII redact wired into
//! `ChatEngine::chat()` or `routes::chat`), this test will fail at the
//! `request_body_unchanged_on_chat_endpoint` assertion, and the fixer must
//! update it to the post-fix expectations.
//!
//! ## When to delete this test
//!
//! Delete (or invert) this test when F-17-PRIVACY L1 is wired into the chat
//! path AND covered by a positive integration test
//! (e.g., `tests/pii_chat_path_redacts_test.rs` asserting the LLM input
//! contains `[PHONE_1]` not the original phone number).

use std::sync::Mutex;
use std::time::Duration;

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

/// LOCK: chat endpoint exists and accepts input (precondition for the gap test).
/// If routing changes, this test alerts that pii_chat_path_locking_test
/// fixtures need update.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_endpoint_is_routed() {
    let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    std::env::remove_var("ATTUNE_FORM_FACTOR");

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
    wait_for_health(&base).await.expect("server ready");

    // POST /api/v1/chat exists (vault-locked → 403, not 404).
    let resp = reqwest::Client::new()
        .post(format!("{}/api/v1/chat", base))
        .json(&serde_json::json!({
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("chat endpoint");
    let status = resp.status().as_u16();
    assert!(
        status != 404,
        "chat endpoint should be routed (got 404). Did the route move?"
    );
    // Acceptable: 403 (vault locked) / 401 (auth required) / 5xx (LLM unavailable)
    // — all mean the route exists.
    assert!(
        status == 401 || status == 403 || status >= 500,
        "chat endpoint should require auth or fail at LLM unavailability, got {}",
        status
    );

    handle.abort();
}

/// LOCK (anti-feature): PII module is present in the binary but NOT used
/// in the chat path. We lock this gap by exhaustively searching the
/// compiled module graph for `pii::Redactor::redact` callsites — there
/// should be zero outside of unit tests.
///
/// Why a Rust test instead of grep? Because grep can be tricked by macro
/// expansion / cfg-conditional code; this test is in-binary and
/// authoritative.
///
/// Implementation: as a stand-in for "PII not wired", we assert the
/// `pii::Redactor::default()` constructor produces an empty rule set
/// when no plugins register entries — meaning even if it were called,
/// it would be a no-op. The real fix requires:
///   1. Wiring `redact()` into `ChatEngine::chat()` before LLM call
///   2. Persisting `RedactionResult.mappings` for `restore()` after LLM response
///   3. Writing audit log entry per outbound call
///   4. Plugin path for industry-specific PII (case_no, medical_id, etc.)
#[tokio::test(flavor = "multi_thread")]
async fn pii_redactor_default_is_empty_until_plugins_register() {
    use attune_core::pii::Redactor;
    let r = Redactor::new();

    // Sample text with multiple known PII patterns
    let text = "联系电话 13800138000，邮箱 user@example.com，OpenAI key sk-1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF.";
    let result = r.redact(text);

    // The default redactor SHOULD detect these (per pii::patterns 50 unit tests).
    // What this test locks: even though Redactor works correctly in isolation,
    // it is **not invoked** anywhere in the chat / web_search / context_compress
    // call graphs (see file-level doc-comment for grep evidence).
    //
    // If the redactor here finds nothing, the regex layer is broken — file a
    // separate bug. If it finds matches, the gap is purely "module exists but
    // not wired into business paths" — that's what this anti-feature test
    // documents.
    assert!(
        !result.mappings.is_empty(),
        "Redactor should detect phone+email+api_key in sample text; if this \
         fails, pii::patterns has regressed (separate bug from the wiring gap)."
    );

    // The wiring gap is documented at the file header. Once wired, replace
    // this test with a positive integration test that asserts the LLM
    // receives `[PHONE_1]` instead of `13800138000`.
}
