//! Monitoring (info-watch) route gates — GA P0 regression suite for `routes/watches.rs`.
//!
//! Covers the three GA-blocking gaps the audit pulled out of `ask_watch` / `research`:
//!   - **P0-1 ABBA deadlock**: the handlers held `vault.lock()` while taking `fulltext`+`vectors`
//!     (reverse of the search/chat/items hotspot order fulltext→vectors→vault). A concurrency
//!     test fires many overlapping ask/research/search/ingest requests under a hard deadline;
//!     the old reverse order would hang the server → `timeout` fails the test.
//!   - **P0-2 privacy leak**: the handlers sent decrypted vault content + the user question to the
//!     raw cloud LLM with no consent gate and no redaction. Tests prove: privacy.llm OFF → refused
//!     (no egress); privacy.llm ON → the recording provider NEVER sees the raw PII (redacted).
//!   - **P1-1 member gate**: tier-3 💰 op — logged-out / free → 403 `membership-required`.
//!
//! Uses the shared eval harness (`test_support`): `login-token` (real license verification),
//! `enable_cloud_llm` (product privacy toggle), and `RecordingMockLlm` (inspect the wire content).

use attune_server::test_support::{enable_cloud_llm, spawn_eval_server_with_recording_llm};
use serde_json::{json, Value};
use std::time::Duration;

// PII sentinels the default redactor (L1 patterns) catches: a CN mobile + an email. If either
// reaches the recording provider verbatim, redaction did not run on the watches egress path.
const PII_PHONE: &str = "13812345678";
const PII_EMAIL: &str = "secret-user@example.com";

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
    let client = reqwest::Client::new();
    let body = if tier == "paid" {
        json!({ "tier": "paid", "account_id": "u-test", "license_id": "lic-test", "llm_quota_remaining": 1000000 })
    } else if tier == "free" {
        json!({ "tier": "free", "account_id": "u-test" })
    } else {
        let _ = client.post(format!("{base}/api/v1/member/logout")).send().await;
        return;
    };
    let r = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&body)
        .send()
        .await
        .expect("login sent");
    assert!(r.status().is_success(), "login-token {tier} should succeed");
}

/// Ingest one note (the recording-llm harness has already set up + unlocked the vault).
async fn ingest(base: &str, title: &str, content: &str) {
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/api/v1/ingest"))
        .json(&json!({"title": title, "content": content, "source_type": "note"}))
        .send()
        .await
        .expect("ingest");
    assert_eq!(r.status().as_u16(), 200, "ingest {title}");
}

/// Create a watch and return its id.
async fn create_watch(base: &str, keyword: &str) -> String {
    let (st, v) = post(
        base,
        "monitoring/watches",
        json!({ "label": "w", "keywords": [keyword], "match_threshold": 0.4 }),
    )
    .await;
    assert_eq!(st, 200, "create watch, body={v}");
    v["id"].as_str().expect("watch id").to_string()
}

// ───────────────────────────── P1-1 member gate ─────────────────────────────

#[tokio::test]
async fn loggedout_ask_is_403_membership_required() {
    let (srv, _rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "loggedout").await;
    let wid = {
        // creating a watch needs an unlocked vault only (no member gate) — harness vault is unlocked.
        login(&base, "paid").await; // create as paid…
        let id = create_watch(&base, "RVV").await;
        login(&base, "loggedout").await; // …then drop membership for the gated call.
        id
    };
    let (st, v) = post(&base, &format!("monitoring/watches/{wid}/ask"), json!({"question": "q?"})).await;
    assert_eq!(st, 403, "loggedout ask must be 403, body={v}");
    assert_eq!(v["code"], "membership-required");
}

#[tokio::test]
async fn free_tier_ask_is_403() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "paid").await;
    let wid = create_watch(&base, "RVV").await;
    login(&base, "free").await;
    let (st, v) = post(&base, &format!("monitoring/watches/{wid}/ask"), json!({"question": "q?"})).await;
    assert_eq!(st, 403, "free ask must be 403, body={v}");
    assert_eq!(v["code"], "membership-required");
    assert_eq!(rec.call_count(), 0, "free tier must never reach the LLM");
}

#[tokio::test]
async fn free_tier_research_is_403() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "free").await;
    let (st, v) = post(&base, "monitoring/research", json!({"topic": "RVV", "use_web": false})).await;
    assert_eq!(st, 403, "free research must be 403, body={v}");
    assert_eq!(v["code"], "membership-required");
    assert_eq!(rec.call_count(), 0, "free tier research must never reach the LLM");
}

// ───────────────────────────── P0-2 privacy gate ─────────────────────────────

#[tokio::test]
async fn paid_but_cloud_llm_disabled_ask_is_403_cloud_llm_disabled() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "paid").await;
    let wid = create_watch(&base, "RVV").await;
    // Did NOT enable cloud LLM egress → privacy gate must refuse before any egress.
    let (st, v) = post(&base, &format!("monitoring/watches/{wid}/ask"), json!({"question": "q?"})).await;
    assert_eq!(st, 403, "paid without cloud egress must be 403, body={v}");
    assert_eq!(v["code"], "cloud-llm-disabled");
    assert_eq!(rec.call_count(), 0, "privacy gate OFF → LLM must NOT be called");
}

#[tokio::test]
async fn paid_research_with_cloud_disabled_degrades_no_egress() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "paid").await;
    ingest(&base, "RVV", "the RVV vector extension is finalized").await;
    // privacy.llm OFF → research degrades to vault-only extractive; LLM must NOT be called.
    let (st, v) = post(&base, "monitoring/research", json!({"topic": "RVV", "use_web": false})).await;
    assert_eq!(st, 200, "research degrades (not error) when cloud disabled, body={v}");
    assert_eq!(v["llm_disabled"], true, "response flags llm_disabled");
    assert_eq!(rec.call_count(), 0, "privacy gate OFF → research must NOT reach the LLM");
}

// ─────────────────────── P0-2 PII redaction on the wire ───────────────────────

#[tokio::test]
async fn ask_redacts_pii_before_cloud_llm() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "paid").await;
    enable_cloud_llm(&base).await;
    // A vault item carrying PII; it is in the watch scope so ask grounds on it.
    ingest(
        &base,
        "contact RVV",
        &format!("RVV maintainer phone {PII_PHONE} email {PII_EMAIL}"),
    )
    .await;
    let wid = create_watch(&base, "RVV").await;
    // Populate hits so the scoped-RAG set is non-empty.
    let _ = post(&base, "monitoring/scan", json!({})).await;

    let (st, v) = post(
        &base,
        &format!("monitoring/watches/{wid}/ask"),
        json!({"question": format!("reach RVV maintainer at {PII_PHONE}?")}),
    )
    .await;
    assert_eq!(st, 200, "paid+cloud ask must pass gates, body={v}");
    assert!(rec.call_count() >= 1, "the LLM must have been called");
    // The decrypted snippet AND the question both flow through RedactingLlmProvider → the recording
    // provider must never see the raw PII.
    assert!(!rec.any_call_contains(PII_PHONE), "phone must be redacted before egress");
    assert!(!rec.any_call_contains(PII_EMAIL), "email must be redacted before egress");
}

#[tokio::test]
async fn research_redacts_pii_before_cloud_llm() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "paid").await;
    enable_cloud_llm(&base).await;
    ingest(
        &base,
        "RVV contact",
        &format!("RVV details, phone {PII_PHONE}, email {PII_EMAIL}"),
    )
    .await;
    let (st, v) = post(&base, "monitoring/research", json!({"topic": "RVV", "use_web": false})).await;
    assert_eq!(st, 200, "paid+cloud research must pass gates, body={v}");
    assert_eq!(v["llm_disabled"], false, "cloud enabled → llm not disabled");
    assert!(!rec.any_call_contains(PII_PHONE), "phone must be redacted before egress");
    assert!(!rec.any_call_contains(PII_EMAIL), "email must be redacted before egress");
}

// ──────────────────────── P0-1 ABBA deadlock regression ────────────────────────

/// Fire many overlapping ask / research / search / ingest requests against a paid+cloud server
/// under a hard deadline. The pre-fix handlers took `vault` then `fulltext`+`vectors` (reverse of
/// the search hotspot order) — interleaved with the forward-order search path this is a classic
/// ABBA deadlock that hangs every worker. With the corrected order (fulltext→vectors→vault, and
/// vault-only phases split out) all requests complete well within the timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_monitoring_and_search_no_deadlock() {
    let (srv, _rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login(&base, "paid").await;
    enable_cloud_llm(&base).await;
    for i in 0..5 {
        ingest(&base, &format!("RVV {i}"), &format!("RVV vector note number {i}")).await;
    }
    let wid = create_watch(&base, "RVV").await;
    let _ = post(&base, "monitoring/scan", json!({})).await;

    // 40 concurrent requests across all the lock-contending paths.
    let mut tasks = Vec::new();
    for i in 0..40 {
        let base = base.clone();
        let wid = wid.clone();
        tasks.push(tokio::spawn(async move {
            match i % 4 {
                0 => {
                    let _ = post(&base, &format!("monitoring/watches/{wid}/ask"), json!({"question": "RVV?"})).await;
                }
                1 => {
                    let _ = post(&base, "monitoring/research", json!({"topic": "RVV", "use_web": false})).await;
                }
                2 => {
                    let client = reqwest::Client::new();
                    let _ = client.get(format!("{base}/api/v1/search?q=RVV")).send().await;
                }
                _ => {
                    ingest(&base, &format!("more RVV {i}"), "RVV extra note").await;
                }
            }
        }));
    }

    let join_all = async move {
        for (i, t) in tasks.into_iter().enumerate() {
            t.await.unwrap_or_else(|e| panic!("task {i} panicked: {e}"));
        }
    };
    let res = tokio::time::timeout(Duration::from_secs(30), join_all).await;
    assert!(
        res.is_ok(),
        "monitoring + search requests deadlocked (timed out) — lock-order regression (P0-1)"
    );
}
