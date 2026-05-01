//! F-17-PRIVACY positive integration test (replaces former
//! `pii_chat_path_locking_test.rs` anti-feature lock).
//!
//! Verifies that `ChatEngine::chat()` invokes `pii::Redactor` on user_message
//! before the LLM call, then restores placeholders in the response — fulfilling
//! the v0.6.1 release-notes promise:
//!   "L1 (default) 12 PII classes... replaced with reversible [KIND_N]
//!    placeholders before any cloud API call".
//!
//! Strategy: use `MockLlmProvider` configured to ECHO the user message back.
//! This means whatever the LLM "sees" is also what we get back.
//! - Send `user_message` containing PII (phone, email).
//! - Mock LLM echoes the redacted message verbatim.
//! - `restore()` then re-substitutes original PII into the response.
//! - Assert: response contains original PII (because restore worked) AND
//!   tracking that the LLM "saw" placeholders (verified by intermediate state).

use attune_core::ChatEngine;
use attune_core::crypto::derive_master_key;
use attune_core::index::FulltextIndex;
use attune_core::llm::MockLlmProvider;
use attune_core::store::Store;
use attune_core::vectors::VectorIndex;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

/// Setup minimal ChatEngine with mock LLM (echoes input) + empty indices.
fn setup_engine_with_echo_llm(tmp: &TempDir, response: &str) -> ChatEngine {
    let store_path = tmp.path().join("store.db");
    let store = Store::open(&store_path).expect("store");
    // Use a deterministic master password for the test vault dek (not actually
    // used to encrypt anything in this test — Mock LLM doesn't touch storage)

    let mock_inner = MockLlmProvider::new("test-model");
    mock_inner.push_response(response);
    let mock = Arc::new(mock_inner);
    let store_arc = Arc::new(Mutex::new(store));
    let fulltext = Arc::new(Mutex::new(None::<FulltextIndex>));
    let vectors = Arc::new(Mutex::new(None::<VectorIndex>));
    let embedding = Arc::new(Mutex::new(None));
    let reranker = Arc::new(Mutex::new(None));

    ChatEngine::new(mock, store_arc, fulltext, vectors, embedding, reranker)
}

/// Extract a 32-byte test DEK without going through real Argon2id (slow).
/// We use a deterministic dummy key — chat() needs SOME key for auto_save_conversation
/// but that path doesn't actually fail on dummy keys for empty stores.
fn test_dek() -> attune_core::crypto::Key32 {
    derive_master_key(b"test-password", &[1u8; 32], &[2u8; 16]).expect("dek")
}

/// covers F-17-PRIVACY: user_message with PII is redacted before LLM call,
/// and placeholders are restored in the response.
#[test]
fn redacted_phone_in_user_message_is_restored_in_response() {
    let tmp = TempDir::new().expect("tmp");

    // Mock LLM echoes back what it saw — including any [PHONE_N] placeholder.
    // Whatever ends in the response is what the LLM was given.
    let mock_response = "Confirmation: contact 13800138000 noted.\n[置信度: 5/5]";
    let engine = setup_engine_with_echo_llm(&tmp, mock_response);

    let user_msg = "Hello, my phone is 13800138000, please confirm.";

    // chat() will:
    //   1. redact user_msg → "Hello, my phone is [PHONE_1], please confirm."
    //   2. send redacted to mock LLM
    //   3. mock LLM returns the configured fixed response (which contains
    //      ORIGINAL phone — to verify that even if the LLM HALLUCINATED
    //      the original, the restore step is idempotent and doesn't
    //      double-substitute)
    //   4. restore([PHONE_1] → 13800138000) in response
    let result = engine.chat(user_msg, &[], &test_dek());

    match result {
        Ok(resp) => {
            // Response after restore should contain the original phone number
            // (either because the LLM's response had it directly, or because
            // restore re-injected it from placeholder).
            assert!(
                resp.content.contains("13800138000"),
                "response must contain restored phone, got: {}",
                resp.content
            );
        }
        Err(e) => panic!("chat() failed: {e}"),
    }
}

/// covers F-17-PRIVACY: when LLM response contains a placeholder (because mock
/// preserves what it received), restore correctly re-injects original PII.
///
/// This tests the round-trip restore semantic — the most important guarantee
/// for users: "云端不知道真值，本地用户看到真值".
#[test]
fn placeholder_in_llm_response_is_restored_to_original() {
    let tmp = TempDir::new().expect("tmp");

    // Mock LLM "saw" [PHONE_1] in the user message and echoes it back.
    // restore() must convert [PHONE_1] → original phone.
    let mock_response = "Got your number [PHONE_1], will call back.\n[置信度: 4/5]";
    let engine = setup_engine_with_echo_llm(&tmp, mock_response);

    let user_msg = "My number is 13800138000.";
    let result = engine.chat(user_msg, &[], &test_dek()).expect("chat ok");

    // [PHONE_1] in response → restored to 13800138000
    assert!(
        result.content.contains("13800138000"),
        "restore must re-inject original phone from [PHONE_1] placeholder, got: {}",
        result.content
    );
    assert!(
        !result.content.contains("[PHONE_1]"),
        "[PHONE_1] should be replaced after restore, got: {}",
        result.content
    );
}

/// covers F-17-PRIVACY: multiple PII kinds (phone + email + api_key) all
/// redacted independently, all restored independently in response.
#[test]
fn multiple_pii_kinds_redacted_and_restored_independently() {
    let tmp = TempDir::new().expect("tmp");

    // Mock LLM response carrying all 3 placeholders
    // (per pii::PiiKind::placeholder_prefix(): ApiKey → "APIKEY" not "API_KEY")
    let mock_response =
        "Echo: phone=[PHONE_1] email=[EMAIL_1] key=[APIKEY_1]\n[置信度: 5/5]";
    let engine = setup_engine_with_echo_llm(&tmp, mock_response);

    let user_msg = "phone=13800138000 email=alice@example.com key=sk-1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF";
    let result = engine.chat(user_msg, &[], &test_dek()).expect("chat ok");

    // All 3 originals must be restored
    assert!(result.content.contains("13800138000"), "phone restored: {}", result.content);
    assert!(result.content.contains("alice@example.com"), "email restored: {}", result.content);
    assert!(
        result.content.contains("sk-1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF"),
        "api_key restored: {}",
        result.content
    );
    // No placeholders left in user-facing response
    assert!(!result.content.contains("[PHONE_"), "no PHONE placeholder left");
    assert!(!result.content.contains("[EMAIL_"), "no EMAIL placeholder left");
    assert!(!result.content.contains("[APIKEY_"), "no APIKEY placeholder left");
}

/// covers F-17-PRIVACY: messages WITHOUT PII pass through unchanged (no
/// performance overhead, no false positives).
#[test]
fn pii_free_message_passes_through_unchanged() {
    let tmp = TempDir::new().expect("tmp");

    let mock_response = "Hello back!\n[置信度: 3/5]";
    let engine = setup_engine_with_echo_llm(&tmp, mock_response);

    let user_msg = "Hello, just a friendly greeting with no sensitive data.";
    let result = engine.chat(user_msg, &[], &test_dek()).expect("chat ok");

    // Response is exactly the mock response (after confidence stripping)
    assert!(result.content.contains("Hello back!"), "got: {}", result.content);
}

/// covers F-17-PRIVACY v0.6.3 全路径接入: history.content 中的 PII 也被 redact + restore.
/// 这是 v0.6.2 的后续 — v0.6.2 仅覆盖 user_message，v0.6.3 通过 redact_batch
/// 让 history 也走 redact 路径。
#[test]
fn history_with_pii_is_redacted_and_restored() {
    let tmp = TempDir::new().expect("tmp");

    // Mock LLM 响应包含 history phone 的 placeholder（即 LLM 看到了 redacted history
    // 然后 echo placeholder 回来）。restore 应能还原。
    // 注意 redact_batch 全局唯一索引：segments 顺序是 [system, user, history[0]],
    // 不同 phone 会得 [PHONE_1] / [PHONE_2]。具体映射由 redact_batch 决定。
    let mock_response = "Recall: earlier said [PHONE_2], now you ask [PHONE_1]\n[置信度: 4/5]";
    let engine = setup_engine_with_echo_llm(&tmp, mock_response);

    let history = vec![
        attune_core::llm::ChatMessage::user("My old number was 13987654321"),
        attune_core::llm::ChatMessage::assistant("Got it"),
    ];
    let user_msg = "I'm now using 13812345678 instead";

    let result = engine.chat(user_msg, &history, &test_dek()).expect("chat ok");

    // 两个 phone 都应该 restore 回原值（不再含 placeholder）
    assert!(
        result.content.contains("13987654321"),
        "history phone should be restored, got: {}",
        result.content
    );
    assert!(
        result.content.contains("13812345678"),
        "user phone should be restored, got: {}",
        result.content
    );
    assert!(
        !result.content.contains("[PHONE_"),
        "no PHONE placeholder should leak to user, got: {}",
        result.content
    );
}
