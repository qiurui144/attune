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

use attune_server::test_support::{enable_cloud_llm, spawn_eval_server};
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
    // I2: the cloud-LLM egress toggle must be ON for a tier-3 cloud op to proceed.
    enable_cloud_llm(&base).await;

    for (path, body) in tier3_ops() {
        let (status, v) = post(&base, path, body).await;
        assert_ne!(status, 403, "paid tier3 {path} must NOT be gate-blocked (403), body={v}");
        assert_ne!(
            v["code"], "membership-required",
            "paid tier3 {path} must never get membership-required"
        );
        assert_ne!(
            v["code"], "cloud-llm-disabled",
            "paid tier3 {path} with egress enabled must not be cloud-llm-disabled"
        );
    }
}

/// I2 regression: a paid member who has NOT enabled cloud-LLM egress in Privacy settings must NOT
/// have private doc content silently sent to the cloud — the tier-3 op is refused with a clear
/// `cloud-llm-disabled` (not a 200, not a silent send).
#[tokio::test]
async fn paid_but_cloud_llm_disabled_refuses_tier3_not_silently_sends() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    login(&base, "paid").await;
    // Deliberately do NOT enable_cloud_llm — privacy.llm defaults false.

    for (path, body) in tier3_ops() {
        let (status, v) = post(&base, path, body).await;
        assert_eq!(status, 403, "tier3 {path} with cloud LLM disabled must be refused, body={v}");
        assert_eq!(
            v["code"], "cloud-llm-disabled",
            "tier3 {path} must refuse with cloud-llm-disabled, not send to the cloud"
        );
    }
}

/// C1 paywall-bypass regression (§5.2.0b CRITICAL): a self-asserted, UNVERIFIED `{tier:"paid"}`
/// claim must NOT reach a billable tier-3 op. The eval harness installs a verifier that approves
/// only `EVAL_PAID_LICENSE`; a forged license is rejected exactly like production (no cloud
/// session). Before the fix, ANY non-empty `license_id` flipped the member to Paid and the
/// billable summarize went through. Now the forged claim is rejected at login (403
/// paid-verification-failed) AND, even if a client skips that signal and calls the tier-3 op
/// directly, the member stays unpaid → membership-required.
#[tokio::test]
async fn forged_unverified_paid_claim_cannot_reach_billable_tier3() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();

    // Attacker asserts paid with a license the verifier does not approve.
    let (login_status, login_body) = {
        let r = client
            .post(format!("{base}/api/v1/member/login-token"))
            .json(&json!({
                "tier": "paid",
                "account_id": "attacker",
                "license_id": "self-asserted-unsigned-license",
                "llm_quota_remaining": 1_000_000
            }))
            .send()
            .await
            .expect("login sent");
        let s = r.status().as_u16();
        (s, r.json::<Value>().await.unwrap_or(Value::Null))
    };
    assert_eq!(login_status, 403, "forged paid login must be rejected: {login_body}");
    assert_eq!(login_body["code"], "paid-verification-failed");

    // The billable tier-3 op must NOT be reachable — the member was never granted Paid.
    let (status, v) = post(
        &base,
        "documents/summarize",
        json!({ "source": doc_a(), "level": "detailed" }),
    )
    .await;
    assert_eq!(status, 403, "forged-paid summarize must be gate-blocked, body={v}");
    assert_eq!(v["code"], "membership-required", "the forged claim never reached a billable op");
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

/// §9.2 TEST-phase leg (c): a LIVE paid-member summarize against a REAL LLM returns 200 with a
/// real summary, carries a `token_bill.path`, and STILL never leaks the gateway token.
///
/// `#[ignore]` — opt-in real-LLM run only. Enable by exporting (CLAUDE.md §1.4 — key from env,
/// never committed): ATTUNE_TEST_REAL_LLM_ENDPOINT / _KEY / _MODEL, then
///   cargo test -p attune-server --test documents_member_gate -- --ignored real_llm
/// The real LLM is wired via the production path (PATCH /settings → reload_llm builds an
/// OpenAiLlmProvider), so this also exercises the real settings → provider hot-reload wiring.
#[tokio::test]
#[ignore = "real-LLM live leg; run manually with ATTUNE_TEST_REAL_LLM_* env set"]
async fn real_llm_paid_summarize_returns_200_happy_path() {
    let (Ok(ep), Ok(key), Ok(model)) = (
        std::env::var("ATTUNE_TEST_REAL_LLM_ENDPOINT"),
        std::env::var("ATTUNE_TEST_REAL_LLM_KEY"),
        std::env::var("ATTUNE_TEST_REAL_LLM_MODEL"),
    ) else {
        eprintln!("SKIP: ATTUNE_TEST_REAL_LLM_* not all set");
        return;
    };
    if key.is_empty() {
        eprintln!("SKIP: ATTUNE_TEST_REAL_LLM_KEY empty");
        return;
    }
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();

    // Realistic happy-path: a paid member with an UNLOCKED vault. The summarize route fetches the
    // DEK for its cache layer even for inline text, so the vault must be unlocked. (Follow-up:
    // the route could fetch the DEK lazily only when item_id is present — out of T-13 scope.)
    let r = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&json!({ "password": "P@ssw0rd-RealLlmLiveLeg-not-real" }))
        .send()
        .await
        .expect("vault setup");
    assert_eq!(r.status().as_u16(), 200, "vault setup must unlock");

    // Wire the real LLM via the PRODUCTION path: PATCH /settings → reload_llm builds the provider.
    // (The key is in-process only — never logged; CLAUDE.md §1.4.)
    let r = client
        .patch(format!("{base}/api/v1/settings"))
        .json(&json!({ "llm": { "provider": "openai_compat", "endpoint": ep, "api_key": key, "model": model } }))
        .send()
        .await
        .expect("patch settings");
    assert_eq!(r.status().as_u16(), 200, "settings PATCH must 200");

    login(&base, "paid").await;

    let (status, v) = post(
        &base,
        "documents/summarize",
        json!({
            "source": { "text": "# 第一章 所有权\n\nRust 的所有权系统在编译期保证内存安全而无需垃圾回收。\
                每个值有唯一的所有者，赋值会移动所有权，离开作用域时自动释放。\n\n# 第二章 借用\n\n\
                借用允许在不取得所有权的情况下读取或修改值，借用检查器在编译期拒绝悬垂引用与数据竞争。" },
            "level": "standard"
        }),
    )
    .await;

    assert_eq!(status, 200, "paid + real LLM summarize must 200, body={v}");
    let overview = v["result"]["overview"].as_str().unwrap_or("");
    assert!(!overview.is_empty(), "real summary overview must be non-empty: {v}");
    // The bypass/pipeline path is recorded (short doc here → single-call).
    assert!(
        v["tokenBill"]["path"].as_str().map(|p| !p.is_empty()).unwrap_or(false),
        "token_bill.path must be present: {v}"
    );
    // Real LLM in the loop — STILL no gateway-token leak in the live response.
    let s = serde_json::to_string(&v).unwrap();
    assert!(!s.contains(SENTINEL_GATEWAY_TOKEN), "live response leaked gateway token: {s}");
    assert!(!s.contains("Bearer "), "live response must not echo a Bearer token: {s}");
}
