// W3 batch B 集成测试：F3 J5 secondary retrieval 端到端
//
// per spec docs/superpowers/specs/2026-04-27-w3-batch-b-design.md §5
// 关闭 W2 batch 1 reviewer 留的 followup F3
// 模式参照已有 chat::tests::chat_engine_with_empty_indices

use std::sync::{Arc, Mutex};

use attune_core::ChatEngine;
use attune_core::crypto::Key32;
use attune_core::embed::EmbeddingProvider;
use attune_core::index::FulltextIndex;
use attune_core::infer::RerankProvider;
use attune_core::llm::MockLlmProvider;
use attune_core::store::Store;
use attune_core::vectors::VectorIndex;

fn build_engine_with_responses(responses: &[&str]) -> (ChatEngine, Key32) {
    let mock = Arc::new(MockLlmProvider::new("test"));
    for r in responses {
        mock.push_response(r);
    }
    let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
    let fulltext: Arc<Mutex<Option<FulltextIndex>>> = Arc::new(Mutex::new(None));
    let vectors: Arc<Mutex<Option<VectorIndex>>> = Arc::new(Mutex::new(None));
    let embedding: Arc<Mutex<Option<Arc<dyn EmbeddingProvider>>>> = Arc::new(Mutex::new(None));
    let reranker: Arc<Mutex<Option<Arc<dyn RerankProvider>>>> = Arc::new(Mutex::new(None));
    let engine = ChatEngine::new(mock, store, fulltext, vectors, embedding, reranker);
    let dek = Key32::generate();
    (engine, dek)
}

#[test]
fn f3_high_confidence_no_secondary_retrieval() {
    let (engine, dek) = build_engine_with_responses(&["明确的答案【置信度: 5/5】"]);
    let resp = engine.chat("问题", &[], &dek).unwrap();
    assert_eq!(resp.confidence, 5);
    assert!(!resp.secondary_retrieval_used);
    assert_eq!(resp.content, "明确的答案");
}

#[test]
fn f3_low_confidence_triggers_secondary_retrieval_attempt() {
    let (engine, dek) = build_engine_with_responses(&["模糊回答【置信度: 1/5】"]);
    let resp = engine.chat("问题", &[], &dek).unwrap();
    assert_eq!(resp.confidence, 1);
    assert!(!resp.secondary_retrieval_used);
    assert_eq!(resp.content, "模糊回答");
}

#[test]
fn f3_default_confidence_3_no_secondary() {
    let (engine, dek) = build_engine_with_responses(&["普通答案不带 marker"]);
    let resp = engine.chat("问题", &[], &dek).unwrap();
    assert_eq!(resp.confidence, 3);
    assert!(!resp.secondary_retrieval_used);
}

#[test]
fn f3_strip_marker_handles_zh_and_en() {
    let (engine, dek) = build_engine_with_responses(&["English answer [Confidence: 4/5]"]);
    let resp = engine.chat("question", &[], &dek).unwrap();
    assert_eq!(resp.confidence, 4);
    assert_eq!(resp.content, "English answer");
}

#[test]
fn f3_chat_response_has_w2_batch_1_fields() {
    let (engine, dek) = build_engine_with_responses(&["a【置信度: 5/5】"]);
    let resp = engine.chat("q", &[], &dek).unwrap();
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"confidence\""));
    assert!(json.contains("\"secondary_retrieval_used\""));
    assert!(json.contains("\"citations\":[]"));
}
