//! chat_reliability — integration test (≥1).
//!
//! Exercises the agent through a fresh `Store`, mirroring how a future
//! background worker would consume RAG hits. Writes encrypted items via
//! `store.insert_item` → reads them back via `store.get_item` →
//! constructs `RetrievedChunk` from the decrypted content → calls
//! `evaluate_response`. This catches any incompatibility between the
//! agent's `RetrievedChunk` shape and the persistence layer's actual
//! content shape (e.g. trailing whitespace from re-encoding /
//! line-ending normalization / etc.).
//!
//! per `attune/CLAUDE.md` §"Agent 验证铁律": **integration ≥ 1** — this is
//! the canonical "this is how the worker integrates" reference.

use attune_core::chat_reliability::{
    evaluate_response, ChatReliabilityConfig, CitationStatus, RetrievedChunk,
};
use attune_core::crypto::Key32;
use attune_core::store::Store;

/// Insert two items into a fresh in-memory store, read them back into
/// `RetrievedChunk`s the way a real RAG pipeline would after decryption,
/// run the agent, and assert that the resulting report exhibits the same
/// behaviors the unit-level tests verified — but now end-to-end through
/// the encrypted persistence layer.
#[test]
fn integration_store_roundtrip_then_evaluate() {
    let store = Store::open_memory().expect("open in-memory store");
    let dek = Key32::generate();

    let content_a = "所有权规则：每个值在 Rust 中都有一个所有者；同一时刻只能有一个所有者；\
        当所有者离开作用域时，这个值将被丢弃。";
    let content_b =
        "借用允许你引用某个值而不获取其所有权。借用必须在所有者的作用域内有效。";

    let id_a = store
        .insert_item(&dek, "Rust 所有权章节", content_a, None, "test", None, None)
        .expect("insert item a");
    let id_b = store
        .insert_item(&dek, "Rust 借用章节", content_b, None, "test", None, None)
        .expect("insert item b");

    // Read items back (simulating the RAG retrieval step).
    let row_a = store
        .get_item(&dek, &id_a)
        .expect("get_item ok")
        .expect("item exists");
    let row_b = store
        .get_item(&dek, &id_b)
        .expect("get_item ok")
        .expect("item exists");

    // Build the same RetrievedChunk shape the chat path produces.
    let chunks = vec![
        RetrievedChunk::new(row_a.id.clone(), row_a.content.clone()),
        RetrievedChunk::new(row_b.id.clone(), row_b.content.clone()),
    ];

    // Compose a response that cites the first item with good token overlap.
    let response = format!(
        "Rust 的所有权规则：每个值有唯一所有者；当所有者离开作用域时\
        值被丢弃 [item:{id_a}]。"
    );

    let cfg = ChatReliabilityConfig::default();
    let report = evaluate_response(&response, &chunks, "rust 所有权", &cfg);

    // Citation must be Grounded — the chunk text we just persisted has
    // ample overlap with the response.
    assert_eq!(report.citation_grounded.len(), 1, "{:?}", report);
    assert_eq!(report.citation_grounded[0].item_id, id_a);
    assert_eq!(
        report.citation_grounded[0].status,
        CitationStatus::Grounded,
        "store roundtrip should preserve enough text for grounding"
    );

    // No date / money / org / person in response → no other signals.
    assert!(
        report.contradictions.is_empty(),
        "expected no contradictions, got {:?}",
        report.contradictions
    );
    assert!(
        report.hallucination_flags.is_empty(),
        "expected no hallucinations, got {:?}",
        report.hallucination_flags
    );

    // Confidence is exactly 1.0 — same end-to-end as the unit test, no
    // regression introduced by the encryption / storage layer.
    assert!(
        (report.overall_confidence - 1.0).abs() < 1e-6,
        "confidence {} != 1.0 — encryption / round-trip must not corrupt grounding",
        report.overall_confidence
    );
}

/// Integration verification of the negative case end-to-end: persist a
/// chunk, write a response that contains a date NOT in any persisted
/// content, confirm the agent flags it via the same persistence path.
#[test]
fn integration_persisted_chunk_with_hallucinated_date_flagged() {
    let store = Store::open_memory().expect("open in-memory store");
    let dek = Key32::generate();

    let content = "Rust 是一门系统编程语言，强调内存安全。设计目标是替代 C/C++。";
    let id = store
        .insert_item(&dek, "Rust 简介", content, None, "test", None, None)
        .expect("insert item");
    let row = store
        .get_item(&dek, &id)
        .expect("get ok")
        .expect("present");
    let chunks = vec![RetrievedChunk::new(row.id, row.content)];

    // Response asserts a specific date not anywhere in the persisted chunk.
    let response = "Rust 在 2099-12-31 发布了 1.0 版本。";

    let report = evaluate_response(response, &chunks, "rust 发布", &ChatReliabilityConfig::default());

    assert_eq!(
        report.hallucination_flags.len(),
        1,
        "expected exactly one hallucination, got {:?}",
        report.hallucination_flags
    );
    assert_eq!(report.hallucination_flags[0].token, "2099-12-31");
    // Confidence penalty: 1 hallucination / 4 saturate * 0.30 weight = 0.075
    let expected = 1.0 - 0.075_f32;
    assert!(
        (report.overall_confidence - expected).abs() < 1e-5,
        "got {} expected {}",
        report.overall_confidence,
        expected
    );
}
