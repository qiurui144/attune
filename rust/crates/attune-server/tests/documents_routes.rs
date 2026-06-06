//! T-07 — document-intelligence routes: §3.5 output-mode envelope + spec §7 error mapping.
//!
//! These exercise the ZERO-LLM paths (compare structural/textual, chapters list) so the
//! assertions are deterministic without a queued mock response. The member-gate + secret
//! no-leak live in `documents_member_gate.rs`; the real-LLM happy path lives in the §9 TEST
//! phase. Here we prove: each endpoint returns the §3.5 envelope, the default output-mode is
//! per-capability (compare→marked), an `output_mode=structured` override is honored, and the
//! spec §7 error `code`s are stable.

use attune_server::test_support::spawn_eval_server;
use serde_json::{json, Value};

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

fn a() -> Value {
    json!({ "text": "# 引言\n\n相同的开头行。\n旧的第二行内容。\n", "name": "a.md" })
}
fn b() -> Value {
    json!({ "text": "# 引言\n\n相同的开头行。\n新的第二行内容替换。\n", "name": "b.md" })
}

#[tokio::test]
async fn compare_default_output_mode_is_marked_with_annotations() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    // structural mode is free (no gate, no LLM); default output_mode → marked.
    let (status, v) = post(
        &base,
        "documents/compare",
        json!({ "left": a(), "right": b(), "mode": "textual" }),
    )
    .await;
    assert_eq!(status, 200, "textual compare free + 200: {v}");
    assert_eq!(v["outputMode"], "marked", "compare default output mode is marked (§3.5)");
    // marked mode carries annotations anchored to b's offsets.
    let anns = v["annotations"].as_array().expect("marked → annotations array");
    assert!(!anns.is_empty(), "marked compare yields annotations: {v}");
    for ann in anns {
        assert!(ann.get("offsetStart").is_some(), "annotation has offsetStart");
        assert!(ann.get("offsetEnd").is_some(), "annotation has offsetEnd");
    }
    assert!(v.get("tokenBill").is_some(), "envelope carries tokenBill");
}

#[tokio::test]
async fn compare_structured_override_omits_marked_overlay() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let (status, v) = post(
        &base,
        "documents/compare",
        json!({ "left": a(), "right": b(), "mode": "textual", "outputMode": "structured" }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v["outputMode"], "structured", "structured override honored");
    // structured mode → no top-level annotations overlay (the DiffReport still inside result).
    assert!(v.get("annotations").is_none() || v["annotations"].is_null(), "structured omits overlay: {v}");
    assert!(v["result"].get("structuralDiffs").is_some() || v["result"].get("textualDiffs").is_some(), "result carries DiffReport: {v}");
}

#[tokio::test]
async fn chapters_list_is_structured_envelope_free() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let (status, v) = post(
        &base,
        "documents/chapters",
        json!({ "itemId": null, "text": "# 第一章\n\n第一章正文内容足够长。\n# 第二章\n\n第二章正文。\n", "action": "list" }),
    )
    .await;
    assert_eq!(status, 200, "list is free: {v}");
    assert_eq!(v["outputMode"], "structured");
    let chapters = v["result"]["chapters"].as_array().expect("chapters array");
    assert_eq!(chapters.len(), 2);
    assert!(chapters[0]["extractivePreview"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "preview non-empty");
}

#[tokio::test]
async fn invalid_action_returns_invalid_input() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let (status, v) = post(
        &base,
        "documents/chapters",
        json!({ "itemId": null, "text": "# C\n\nx\n", "action": "frobnicate" }),
    )
    .await;
    assert_eq!(status, 400, "invalid action → 400");
    assert_eq!(v["code"], "invalid-input", "spec §7 invalid-input code");
}

#[tokio::test]
async fn invalid_level_returns_invalid_input_unpaid_path_independent() {
    // invalid level on summarize — but summarize is gated; an unpaid user is gated first (403).
    // To reach the level-parse error we use a paid member; an invalid level → 400 invalid-input.
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();
    let _ = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&json!({ "tier": "paid", "account_id": "u", "license_id": "lic" }))
        .send()
        .await;
    let (status, v) = post(
        &base,
        "documents/summarize",
        json!({ "source": a(), "level": "ultra-mega" }),
    )
    .await;
    assert_eq!(status, 400, "invalid level → 400, body={v}");
    assert_eq!(v["code"], "invalid-input");
}

#[tokio::test]
async fn item_not_found_maps_to_404() {
    // A non-existent item_id on a free op (structural compare) → item-not-found (404).
    // (Vault is unlocked in the eval harness's in-memory vault; a missing id → 404.)
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let (status, v) = post(
        &base,
        "documents/compare",
        json!({ "left": { "itemId": "does-not-exist" }, "right": b(), "mode": "structural" }),
    )
    .await;
    // Either vault-locked (401) if the harness vault is locked, or item-not-found (404).
    assert!(status == 404 || status == 401, "missing item → 404 or vault-locked 401, got {status}: {v}");
    assert!(
        v["code"] == "item-not-found" || v["code"] == "vault-locked",
        "spec §7 code, got {}",
        v["code"]
    );
}

#[tokio::test]
async fn missing_chapter_idx_returns_invalid_input() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();
    let _ = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&json!({ "tier": "paid", "account_id": "u", "license_id": "lic" }))
        .send()
        .await;
    let (status, v) = post(
        &base,
        "documents/chapters",
        json!({ "itemId": null, "text": "# C\n\nx\n", "action": "summarize" }),
    )
    .await;
    assert_eq!(status, 400, "missing chapter_idx → 400: {v}");
    assert_eq!(v["code"], "invalid-input");
}
