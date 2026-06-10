//! Test support harness — spawns an in-process eval-mode attune-server with
//! [`MockLlmProvider`] for deterministic integration tests.
//!
//! Per plan docs/superpowers/plans/2026-05-28-kb-bench-integration.md Task 1 Step 9.
//! Per spec docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md §5.1.1.
//!
//! Declared `#[doc(hidden)] pub mod test_support` in `lib.rs` so the symbol is
//! not advertised on the public API surface — production callers (Chrome ext /
//! Web UI / attune-cli) never import this module. Compiled unconditionally
//! (not feature-gated) so the integration-test crate can
//! `use attune_server::test_support::…` without a separate cargo `--features`
//! flag.
//!
//! Usage from an integration test:
//! ```ignore
//! use attune_server::test_support::{spawn_eval_server, EvalTestClient};
//!
//! let srv = spawn_eval_server().await;
//! let client = EvalTestClient::new(srv.url());
//! let r = client.chat("hello", Some(42), true).await;
//! assert_eq!(r.eval.unwrap().determinism, "exact");
//! ```

use std::sync::Arc;

use attune_core::llm::{DeterminismLevel, MockLlmProvider};
use attune_core::member_verifier::WhitelistMemberVerifier;

/// The ONE license id the eval harness's member verifier approves. A `login-token` "paid" claim
/// reaches `MemberState::Paid` only when its `license_id` equals this — i.e. it must pass a real
/// verification step (a match), NOT the old blanket non-empty check. Any other license id is
/// rejected exactly like production (C1 paywall-bypass fix).
pub const EVAL_PAID_LICENSE: &str = "lic-test";

/// Spawned in-process server. Listens on a random ephemeral port. Server is
/// stopped when this struct is dropped (handle is aborted).
pub struct EvalServer {
    addr: std::net::SocketAddr,
    handle: tokio::task::JoinHandle<()>,
    _tmp: tempfile::TempDir,
}

impl EvalServer {
    /// Base URL for HTTP calls (no trailing slash).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for EvalServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Build an in-memory attune-server, wire `MockLlmProvider` into `state.llm`,
/// register a route handler that reads the `X-Attune-Test-Provider-Label` header
/// and resets the mock's [`DeterminismLevel`] per request (so a single server
/// can serve both `mock` and `anthropic` flavors of the same test).
///
/// **Important**: the mock used by `seed_header_propagates_to_llm_options` /
/// `different_seeds_produce_different_answers` is shared across requests — its
/// determinism level is `Exact` by default. The `with_provider("anthropic")`
/// client variant flips it to `Temp0` per-request via the label header.
pub async fn spawn_eval_server() -> EvalServer {
    // Per ai_stack_web_search_test.rs pattern: isolate vault to a tempdir.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    // SAFETY: integration tests share a process — std::env::set_var is unsound
    // when called concurrently with another thread reading env vars. We accept
    // the residual race because:
    //   (1) test setup happens before any tokio worker is spawned for this test;
    //   (2) integration tests run with --test-threads=1 by default for this binary
    //       (eval_determinism_test has 4 small tests, no concurrency benefit);
    //   (3) the production server boots from the same env path under same constraint.
    // Mirrors the precedent in attune-core/src/backup.rs::with_temp_home.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }

    let vault =
        attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let state = Arc::new(crate::state::AppState::new(vault, false /* require_auth */));

    // Install a shared MockLlmProvider. Tests adjust its determinism via the
    // X-Attune-Test-Provider-Label header (see chat route).
    let mock = Arc::new(MockLlmProvider::new("eval-mock"));
    mock.set_determinism_level(DeterminismLevel::Exact);
    state.set_llm(Some(mock.clone()));

    // C1: install a verifier that approves ONLY `EVAL_PAID_LICENSE`. A "paid" login-token must
    // present that exact license to reach Paid — a real verification step. A forged/other license
    // is rejected, so the member-gate test exercises the production reject path (not a bypass).
    state.set_member_verifier(Arc::new(WhitelistMemberVerifier::new(EVAL_PAID_LICENSE)));

    let router = crate::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        // serve_with_graceful_shutdown isn't needed — drop aborts the JoinHandle.
        let _ = axum::serve(listener, router.into_make_service()).await;
    });

    // Wait until /health responds (route always available, no guard).
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let url = format!("http://{}/health", addr);
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    EvalServer {
        addr,
        handle,
        _tmp: tmp,
    }
}

/// HTTP client for the eval-mode test server. Owns a reusable reqwest client +
/// the provider label (mock / anthropic) used to choose mock determinism per call.
pub struct EvalTestClient {
    base_url: String,
    provider_label: String,
    client: reqwest::Client,
}

impl EvalTestClient {
    pub fn new(url: String) -> Self {
        Self {
            base_url: url,
            provider_label: "mock".into(),
            client: reqwest::Client::new(),
        }
    }

    /// Construct a client that pretends to be a specific provider — `"anthropic"`
    /// triggers the chat route to flip MockLlmProvider's determinism level to
    /// [`DeterminismLevel::Temp0`] before processing the request.
    pub fn with_provider(url: String, provider: &str) -> Self {
        Self {
            base_url: url,
            provider_label: provider.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Send a chat with optional eval-mode headers and parse the response.
    pub async fn chat(
        &self,
        msg: &str,
        seed: Option<u64>,
        force_temp_zero: bool,
    ) -> ChatTestResponse {
        let mut req = self.client
            .post(format!("{}/api/v1/chat", self.base_url))
            .json(&serde_json::json!({"message": msg, "history": []}));
        if let Some(s) = seed {
            req = req.header("X-Attune-Eval-Seed", s.to_string());
        }
        if force_temp_zero {
            req = req.header("X-Attune-Eval-Force-Temp-Zero", "true");
        }
        // Test-only header: lets the chat route reconfigure the mock provider
        // (mock vs anthropic-flavored mock). Production handler ignores unknown headers.
        req = req.header("X-Attune-Test-Provider-Label", &self.provider_label);
        let resp = req.send().await.expect("HTTP send");
        let status = resp.status();
        let body = resp.text().await.expect("HTTP body");
        if !status.is_success() {
            panic!("chat returned non-2xx: status={status} body={body}");
        }
        serde_json::from_str::<ChatTestResponse>(&body)
            .unwrap_or_else(|e| panic!("parse chat response failed: {e}; body={body}"))
    }
}

/// Subset of the chat route's JSON response that integration tests assert on.
/// Tests use `serde(default)` semantics — unknown fields in the real response
/// (citations / cost / etc.) are silently ignored so this struct can stay focused.
#[derive(Debug, serde::Deserialize)]
pub struct ChatTestResponse {
    pub answer: String,
    #[serde(default)]
    pub eval: Option<EvalBlock>,
}

/// Determinism block surfaced via the chat response when any eval header is set.
/// Mirrors the production `routes::chat::EvalBlock` payload (kept in test_support
/// so the test crate doesn't need to depend on the production type).
#[derive(Debug, serde::Deserialize)]
pub struct EvalBlock {
    pub determinism: String,
    #[serde(default)]
    pub seed_used: Option<u64>,
    #[serde(default)]
    pub abstained: bool,
    #[serde(default)]
    pub abstention_reason: Option<String>,
}
