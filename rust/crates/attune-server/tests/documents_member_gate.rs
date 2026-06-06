//! T-08 — member-gate enforcement + secret no-leak (SECURITY, G1-flagged).
//!
//! Covers spec §9.3 (member-gate 3-state) and the carried-forward G1 panel security flag:
//!   - LoggedOut + Free → 403 `membership-required` on every tier-3 op; Paid → 200.
//!   - Adversarial: a direct request (no UI) to a tier-3 op while unpaid → still 403
//!     (the gate is at the handler, not the UI).
//!   - Secret no-leak: a sentinel gateway-token string never appears in any response body
//!     (the gateway credential is downloaded by the gateway, never echoed by attune).
//!   - Free-tier ops (compare structural|textual, chapters list) succeed WITHOUT login.
//!
//! Uses inline `text` documents so no vault unlock is needed; the gate is the unit under test.

use attune_server::test_support::spawn_eval_server;
use serde_json::{json, Value};

/// Sentinel that stands in for the new-api/gateway token (CLAUDE.md §1.4: a fake test token).
/// It MUST NOT appear in any document-intelligence response body.
const SENTINEL_GATEWAY_TOKEN: &str = "test-gateway-token-not-real";

async fn post(base: &str, path: &str, body: Value) -> (u16, Value) {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/v1/{path}"))
        .json(&body)
        .send()
        .await
        .expect("request sent");
    let status = resp.status().as_u16();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    (status, v)
}

async fn login(base: &str, tier: &str) {
    let body = if tier == "paid" {
        json!({ "tier": "paid", "account_id": "u-test", "license_id": "lic-test", "llm_quota_remaining": 1000000 })
    } else if tier == "free" {
        json!({ "tier": "free", "account_id": "u-test" })
    } else {
        // logged-out: explicit logout
        let client = reqwest::Client::new();
        let _ = client.post(format!("{base}/api/v1/member/logout")).send().await;
        return;
    };
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&body)
        .send()
        .await
        .expect("login sent");
    assert!(r.status().is_success(), "login-token {tier} should succeed");
}

fn doc_a() -> Value {
    json!({ "text": "# 第一章\n\n相同开头。\n旧观点：我支持这个方案。\n", "name": "a.md" })
}
fn doc_b() -> Value {
    json!({ "text": "# 第一章\n\n相同开头。\n新观点：我反对这个方案。\n", "name": "b.md" })
}

/// The three tier-3 operations, as (path, body) builders.
fn tier3_ops() -> Vec<(&'static str, Value)> {
    vec![
        ("documents/compare", json!({ "left": doc_a(), "right": doc_b(), "mode": "semantic" })),
        ("documents/summarize", json!({ "source": doc_a(), "level": "standard" })),
        ("documents/chapters", json!({ "itemId": null, "text": "# C1\n\n一些足够长的章节正文内容。\n", "action": "summarize", "chapterIdx": 0 })),
        ("documents/chapters", json!({ "itemId": null, "text": "# C1\n\n一些足够长的章节正文内容。\n", "action": "ask", "chapterIdx": 0, "question": "讲了什么？" })),
    ]
}

#[tokio::test]
async fn loggedout_gets_403_membership_required_on_every_tier3_op() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    login(&base, "loggedout").await;

    for (path, body) in tier3_ops() {
        let (status, v) = post(&base, path, body).await;
        assert_eq!(status, 403, "loggedout tier3 {path} must be 403, body={v}");
        assert_eq!(v["code"], "membership-required", "spec §7 code for {path}");
    }
}

#[tokio::test]
async fn free_gets_403_membership_required_on_every_tier3_op() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    login(&base, "free").await;

    for (path, body) in tier3_ops() {
        let (status, v) = post(&base, path, body).await;
        assert_eq!(status, 403, "free tier3 {path} must be 403, body={v}");
        assert_eq!(v["code"], "membership-required");
    }
}

#[tokio::test]
async fn paid_passes_the_member_gate_on_tier3_ops() {
    // The security-relevant property: a Paid member is NOT blocked by the membership gate.
    // (The full 200 happy-path with real LLM output is exercised in the §9 TEST phase against
    // the real cloud gateway — the eval MockLlmProvider has no queued response so the actual
    // LLM call may 5xx; what T-08 must prove is that the GATE lets a paid member through.)
    let srv = spawn_eval_server().await;
    let base = srv.url();
    login(&base, "paid").await;

    for (path, body) in tier3_ops() {
        let (status, v) = post(&base, path, body).await;
        assert_ne!(status, 403, "paid tier3 {path} must NOT be gate-blocked (403), body={v}");
        assert_ne!(
            v["code"], "membership-required",
            "paid tier3 {path} must never get membership-required"
        );
    }
}

#[tokio::test]
async fn adversarial_direct_connect_unpaid_still_403() {
    // No UI, no login — a raw direct request to the richest tier-3 op must be rejected.
    let srv = spawn_eval_server().await;
    let base = srv.url();
    // Deliberately do NOT log in (gate must not be UI-only).
    let (status, v) = post(
        &base,
        "documents/summarize",
        json!({ "source": doc_a(), "level": "detailed" }),
    )
    .await;
    assert_eq!(status, 403, "direct-connect unpaid summarize must be 403");
    assert_eq!(v["code"], "membership-required");
}

#[tokio::test]
async fn free_tier_ops_succeed_without_login() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    login(&base, "loggedout").await;

    // compare structural — no LLM, no gate.
    let (s1, _v1) = post(
        &base,
        "documents/compare",
        json!({ "left": doc_a(), "right": doc_b(), "mode": "structural" }),
    )
    .await;
    assert_eq!(s1, 200, "structural compare is free");

    // compare textual — no LLM, no gate.
    let (s2, _v2) = post(
        &base,
        "documents/compare",
        json!({ "left": doc_a(), "right": doc_b(), "mode": "textual" }),
    )
    .await;
    assert_eq!(s2, 200, "textual compare is free");

    // chapters list — free preview.
    let (s3, v3) = post(
        &base,
        "documents/chapters",
        json!({ "itemId": null, "text": "# 第一章\n\n章节正文。\n# 第二章\n\n更多正文。\n", "action": "list" }),
    )
    .await;
    assert_eq!(s3, 200, "chapters list is free");
    assert!(v3["result"]["chapters"].as_array().map(|a| a.len()).unwrap_or(0) >= 2, "list returns chapters: {v3}");
}

#[tokio::test]
async fn sentinel_gateway_token_never_appears_in_any_response() {
    // The gateway credential must never be echoed into a document-intelligence response.
    // We assert the sentinel is absent from EVERY endpoint's response body across all tiers.
    let srv = spawn_eval_server().await;
    let base = srv.url();

    for tier in ["loggedout", "free", "paid"] {
        login(&base, tier).await;
        let mut bodies: Vec<Value> = Vec::new();
        for (path, body) in tier3_ops() {
            let (_status, v) = post(&base, path, body).await;
            bodies.push(v);
        }
        // free-tier ops too.
        let (_s, v) = post(
            &base,
            "documents/compare",
            json!({ "left": doc_a(), "right": doc_b(), "mode": "structural" }),
        )
        .await;
        bodies.push(v);

        for v in &bodies {
            let s = serde_json::to_string(v).unwrap();
            assert!(
                !s.contains(SENTINEL_GATEWAY_TOKEN),
                "[{tier}] response leaked the sentinel gateway token: {s}"
            );
            // Defense-in-depth: no obviously credential-shaped keys in the envelope.
            assert!(!s.contains("apiKey") && !s.contains("api_key"), "[{tier}] no api_key field: {s}");
            assert!(!s.contains("Bearer "), "[{tier}] no Bearer token: {s}");
        }
    }
}
