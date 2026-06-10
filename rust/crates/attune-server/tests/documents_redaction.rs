//! I1 — document-intelligence cloud egress must be PII-redacted (regression of F-17).
//!
//! Before the fix, `compare` / `deep_summary` / `chapters` sent RAW document content to the cloud
//! LLM. These tests drive each tier-3 op through the real route with a RECORDING LLM provider and
//! assert the wire-facing provider NEVER saw the raw phone / email / Chinese-ID — only redacted
//! placeholders — while the caller still gets a usable (placeholder-restored) response.

use attune_server::test_support::{enable_cloud_llm, spawn_eval_server_with_recording_llm};
use serde_json::{json, Value};

const PHONE: &str = "13800138000";
const EMAIL: &str = "zhangsan@example.com";
const CN_ID: &str = "11010119900307123X";

/// A document carrying all three classic PII kinds, long enough to drive the real pipelines.
fn pii_doc(tag: &str) -> String {
    format!(
        "# 第一章 {tag}\n\n联系人张三，手机 {PHONE}，邮箱 {EMAIL}，身份证 {CN_ID}。\
         本章详细描述了项目背景与目标，并多次提到上述联系方式以便读者核对。\n\n\
         # 第二章 细节\n\n张三再次强调可通过 {PHONE} 或 {EMAIL} 联系，相关材料已归档。"
    )
}

async fn login_paid(base: &str) {
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&json!({ "tier": "paid", "account_id": "u", "license_id": "lic-test", "llm_quota_remaining": 1_000_000 }))
        .send()
        .await
        .expect("login");
    assert!(r.status().is_success(), "paid login must succeed");
}

async fn post(base: &str, path: &str, body: Value) -> (u16, Value) {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/v1/{path}"))
        .json(&body)
        .send()
        .await
        .expect("request");
    let status = resp.status().as_u16();
    let v = resp.json().await.unwrap_or(Value::Null);
    (status, v)
}

/// Assert no raw PII string appears in ANY recorded outbound LLM call (system + user).
fn assert_no_raw_pii_on_wire(rec: &attune_core::llm::RecordingMockLlm) {
    let calls = rec.calls();
    assert!(!calls.is_empty(), "expected at least one LLM call to inspect");
    let wire = calls
        .iter()
        .map(|c| format!("{}\n{}", c.system, c.user))
        .collect::<Vec<_>>()
        .join("\n----\n");
    for (label, raw) in [("phone", PHONE), ("email", EMAIL), ("cn-id", CN_ID)] {
        assert!(
            !wire.contains(raw),
            "RAW {label} reached the cloud LLM (I1/F-17 regression):\n{wire}"
        );
    }
    // Positive signal: redaction actually happened (placeholders present).
    assert!(
        wire.contains("PHONE_") || wire.contains("EMAIL_") || wire.contains("ID_"),
        "expected redaction placeholders in the outbound payload:\n{wire}"
    );
}

#[tokio::test]
async fn deep_summary_redacts_pii_before_cloud_llm() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login_paid(&base).await;
    enable_cloud_llm(&base).await;

    let (status, v) = post(
        &base,
        "documents/summarize",
        json!({ "source": { "text": pii_doc("总结") }, "level": "standard" }),
    )
    .await;
    assert_eq!(status, 200, "paid + egress-on summarize must 200, body={v}");
    assert_no_raw_pii_on_wire(&rec);
}

#[tokio::test]
async fn compare_semantic_redacts_pii_before_cloud_llm() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login_paid(&base).await;
    enable_cloud_llm(&base).await;

    // The two docs differ in a CHANGED line that itself carries PII, so the semantic verdict LLM
    // call receives doc content with a phone/email — which must arrive redacted.
    let left = json!({
        "text": format!(
            "# 第一章 立场\n\n旧联系方式：请拨打 {PHONE} 或邮件 {EMAIL} 联系张三。\n背景说明若干。\n"
        )
    });
    let right = json!({
        "text": format!(
            "# 第一章 立场\n\n新联系方式：身份证 {CN_ID} 的张三已改用其它电话，原 {PHONE} 作废。\n背景说明若干。\n"
        )
    });
    let (status, v) = post(
        &base,
        "documents/compare",
        json!({ "left": left, "right": right, "mode": "semantic" }),
    )
    .await;
    assert_eq!(status, 200, "semantic compare must 200, body={v}");
    assert_no_raw_pii_on_wire(&rec);
}

#[tokio::test]
async fn chapters_ask_redacts_pii_before_cloud_llm() {
    let (srv, rec) = spawn_eval_server_with_recording_llm().await;
    let base = srv.url();
    login_paid(&base).await;
    enable_cloud_llm(&base).await;

    let (status, v) = post(
        &base,
        "documents/chapters",
        json!({ "itemId": null, "text": pii_doc("阅读"), "action": "ask", "chapterIdx": 0, "question": "如何联系张三？" }),
    )
    .await;
    assert_eq!(status, 200, "chapters ask must 200, body={v}");
    assert_no_raw_pii_on_wire(&rec);
}
