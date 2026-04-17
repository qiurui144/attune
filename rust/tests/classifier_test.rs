use std::sync::Arc;
use tempfile::TempDir;
use attune_core::classifier::Classifier;
use attune_core::llm::MockLlmProvider;
use attune_core::tag_index::TagIndex;
use attune_core::taxonomy::{ClassificationResult, Taxonomy};
use attune_core::vault::Vault;

fn setup_vault() -> (Vault, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("data/vault.db");
    let config_dir = tmp.path().join("config");
    let vault = Vault::open(&db_path, &config_dir).unwrap();
    vault.setup("test-password").unwrap();
    (vault, tmp)
}

const MOCK_RESPONSE: &str = r#"{
    "core": {
        "domain": ["技术"],
        "topic": ["Rust 加密"],
        "purpose": ["参考资料"],
        "project": ["npu-vault"],
        "entities": ["rustls"]
    },
    "universal": {
        "difficulty": "进阶",
        "freshness": "常青",
        "action_type": "学习"
    },
    "plugin": {}
}"#;

#[test]
fn e2e_classify_flow() {
    let (vault, _tmp) = setup_vault();
    let dek = vault.dek_db().unwrap();

    // Ingest some items
    let id1 = vault.store().insert_item(&dek, "Rust 加密笔记", "关于 AES-GCM 的研究", None, "note", None, None).unwrap();
    let _id2 = vault.store().insert_item(&dek, "Python 脚本", "数据处理脚本", None, "note", None, None).unwrap();

    // Setup classifier with mock
    let mock = Arc::new(MockLlmProvider::new("mock-model"));
    mock.push_response(MOCK_RESPONSE);
    let taxonomy = Arc::new(Taxonomy::default());
    let classifier = Classifier::new(taxonomy, mock).with_batch_size(1);

    // Classify
    let result1 = classifier.classify_one("Rust 加密笔记", "关于 AES-GCM 的研究").unwrap();
    assert_eq!(result1.core["domain"], vec!["技术"]);

    // Write to store
    let json1 = serde_json::to_string(&result1).unwrap();
    vault.store().update_tags(&dek, &id1, &json1).unwrap();

    // Verify retrieval
    let retrieved = vault.store().get_tags_json(&dek, &id1).unwrap().unwrap();
    let parsed: ClassificationResult = serde_json::from_str(&retrieved).unwrap();
    assert_eq!(parsed.core["domain"], vec!["技术"]);

    // Build TagIndex from store
    let index = TagIndex::build(vault.store(), &dek).unwrap();
    assert_eq!(index.item_count(), 1); // Only id1 has tags, id2 not yet classified

    let tech_items = index.query("domain", "技术");
    assert_eq!(tech_items.len(), 1);
    assert_eq!(tech_items[0], id1);
}

#[test]
fn e2e_reclassify_flow() {
    let (vault, _tmp) = setup_vault();
    let dek = vault.dek_db().unwrap();

    let id = vault.store().insert_item(&dek, "Item", "content", None, "note", None, None).unwrap();

    // First classification
    let mock = Arc::new(MockLlmProvider::new("mock-v1"));
    mock.push_response(MOCK_RESPONSE);
    let taxonomy = Arc::new(Taxonomy::default());
    let classifier = Classifier::new(taxonomy, mock);

    let result = classifier.classify_one("Item", "content").unwrap();
    vault.store().update_tags(&dek, &id, &serde_json::to_string(&result).unwrap()).unwrap();

    // Second classification (re-classify)
    let mock2 = Arc::new(MockLlmProvider::new("mock-v2"));
    let new_response = r#"{
        "core": {
            "domain": ["法律"],
            "topic": ["合同审查"],
            "purpose": ["参考资料"],
            "project": ["none"],
            "entities": []
        },
        "universal": {
            "difficulty": "入门",
            "freshness": "常青",
            "action_type": "参考"
        },
        "plugin": {}
    }"#;
    mock2.push_response(new_response);
    let taxonomy2 = Arc::new(Taxonomy::default());
    let classifier2 = Classifier::new(taxonomy2, mock2);

    let result2 = classifier2.classify_one("Item", "content").unwrap();
    vault.store().update_tags(&dek, &id, &serde_json::to_string(&result2).unwrap()).unwrap();

    // Verify new tags replaced old
    let index = TagIndex::build(vault.store(), &dek).unwrap();
    assert_eq!(index.query("domain", "技术").len(), 0);
    assert_eq!(index.query("domain", "法律").len(), 1);
}

#[test]
fn e2e_classify_lock_unlock_persistence() {
    let (vault, _tmp) = setup_vault();
    let dek = vault.dek_db().unwrap();

    let id = vault.store().insert_item(&dek, "Persistent", "c", None, "note", None, None).unwrap();

    // Classify and save
    let mock = Arc::new(MockLlmProvider::new("mock"));
    mock.push_response(MOCK_RESPONSE);
    let taxonomy = Arc::new(Taxonomy::default());
    let classifier = Classifier::new(taxonomy, mock);
    let result = classifier.classify_one("Persistent", "c").unwrap();
    vault.store().update_tags(&dek, &id, &serde_json::to_string(&result).unwrap()).unwrap();

    // Lock the vault
    vault.lock().unwrap();
    assert!(vault.dek_db().is_err());

    // Unlock and rebuild index
    vault.unlock("test-password").unwrap();
    let dek2 = vault.dek_db().unwrap();
    let index = TagIndex::build(vault.store(), &dek2).unwrap();
    assert_eq!(index.item_count(), 1);
    assert_eq!(index.query("domain", "技术").len(), 1);
}
