//! T1 — Eval-mode seed determinism integration tests.
//!
//! Spec: docs/superpowers/specs/2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md §11 Risk A
//! Plan: docs/superpowers/plans/2026-05-28-kb-bench-integration.md Task 1
//!
//! v1.1.0 BLOCKER — proves `X-Attune-Eval-Seed` / `X-Attune-Eval-Force-Temp-Zero`
//! headers thread all the way to `LlmCallOptions { seed, temperature, top_p }` at
//! provider call site, and that providers advertise their honored determinism level
//! via `EvalBlock { determinism, seed_used }` in the chat response.

use attune_server::test_support::{spawn_eval_server, EvalTestClient};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_header_propagates_to_llm_options() {
    let srv = spawn_eval_server().await;
    let client = EvalTestClient::new(srv.url());

    // Same query, same seed, force temp 0 -> same answer (using MockLlmProvider that hashes seed).
    let r1 = client.chat("what is rust ownership?", Some(42), true).await;
    let r2 = client.chat("what is rust ownership?", Some(42), true).await;

    assert_eq!(
        r1.answer, r2.answer,
        "same seed must produce same answer (mock provider)"
    );
    let eval1 = r1.eval.as_ref().expect("eval block present when seed sent");
    assert_eq!(eval1.seed_used, Some(42));
    assert_eq!(eval1.determinism, "exact");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn different_seeds_produce_different_answers() {
    let srv = spawn_eval_server().await;
    let client = EvalTestClient::new(srv.url());

    let r1 = client.chat("hello", Some(1), true).await;
    let r2 = client.chat("hello", Some(2), true).await;

    assert_ne!(
        r1.answer, r2.answer,
        "different seeds must yield different mock answers"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_provider_degrades_to_temp0() {
    // Anthropic doesn't support seed -> must return determinism="temp0"
    // (test client passes provider_label header so server installs an Anthropic-like
    // mock that reports DeterminismLevel::Temp0).
    let srv = spawn_eval_server().await;
    let client = EvalTestClient::with_provider(srv.url(), "anthropic");

    let r = client.chat("hi", Some(42), true).await;
    let eval = r.eval.as_ref().expect("eval block present when seed sent");
    assert_eq!(
        eval.determinism, "temp0",
        "anthropic must explicitly degrade and surface temp0"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_eval_headers_skips_short_circuit() {
    // Backward-compat assertion: when NO `X-Attune-Eval-*` headers are sent, the
    // chat handler must NOT take the eval-mode short-circuit (= the request
    // falls through to the legacy RAG path which then enforces vault unlock).
    //
    // Operationally we confirm this by sending a plain chat request to a server
    // with a sealed vault and observing the legacy `vault is sealed` 403 — that
    // tells us the eval short-circuit was bypassed (otherwise we'd get the
    // eval-mode response with an `eval` block, even on a sealed vault).
    //
    // This is the structural invariant that protects all production callers
    // (UI, Chrome extension, attune-cli) from accidentally seeing eval payloads.
    let srv = spawn_eval_server().await;
    let url = srv.url();

    // Use raw reqwest so we can inspect the 403 status without `EvalTestClient`'s
    // 2xx-or-panic helper.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/chat", url))
        .json(&serde_json::json!({"message": "plain query", "history": []}))
        .send()
        .await
        .expect("HTTP send");
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(
        status.as_u16(),
        403,
        "no-eval-header request must hit vault_guard (status={status}, body={body})"
    );
    assert!(
        body.contains("vault"),
        "403 body must mention vault, got: {body}"
    );
    // The eval-mode short-circuit would have returned 200 with eval={...};
    // hitting vault_guard's 403 confirms we did NOT short-circuit.
    assert!(
        !body.contains("\"eval\""),
        "no-eval-header request must NOT carry an eval block, got: {body}"
    );
}
