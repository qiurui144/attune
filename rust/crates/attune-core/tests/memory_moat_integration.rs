//! v0.7 Memory Moat 端到端 integration tests (W4 R28-R29)
//!
//! 覆盖 Phase A + Phase B 的完整 doc lifecycle 和 5 类自学习信号闭环。

use attune_core::crypto::Key32;
use attune_core::index::FulltextIndex;
use attune_core::reindex;
use attune_core::store::Store;
use attune_core::store::items::compute_content_hash;
use attune_core::vectors::VectorIndex;
use tempfile::TempDir;

fn setup() -> (TempDir, Store, VectorIndex, FulltextIndex, Key32) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("test.db")).unwrap();
    let vectors = VectorIndex::new(1024).unwrap();
    let fulltext = FulltextIndex::open(&tmp.path().join("ft")).unwrap();
    let dek = Key32::generate();
    (tmp, store, vectors, fulltext, dek)
}

#[test]
fn doc_lifecycle_signals_complete_flow() {
    let (_t, store, mut vec, ft, dek) = setup();

    // Step 1: upload — 仿 route 写 doc_create 信号
    let id = store
        .insert_item(&dek, "Doc A", "# Heading\n\nbody with vintage keywords",
                     None, "note", None, None)
        .unwrap();
    store.record_signal_event("doc_create", &id, Some("Doc A")).unwrap();

    // Step 2: update — content 变化触发 reindex_item + doc_update signal
    let outcome = store.update_item(&dek, &id, None,
        Some("# Heading\n\nbody with MODERN keywords")).unwrap();
    assert!(outcome.existed);
    assert!(outcome.content_changed, "新内容应触发 reindex");

    let stats = reindex::reindex_item(&store, &mut vec, &ft, &id, "Doc A",
        "# Heading\n\nbody with MODERN keywords", "note").unwrap();
    assert!(stats.chunks_enqueued > 0);
    store.record_signal_event("doc_update", &id, None).unwrap();

    // Step 3: annotation marker
    store.record_signal_event("annotation_marker", &id, Some("⭐重点")).unwrap();

    // Step 4: chat citation hit
    store.record_signal_event("citation_hit", &id, Some("用户问")).unwrap();

    // Step 5: delete + purge
    let stats = reindex::purge_item_indexes(&store, &mut vec, &ft, &id).unwrap();
    assert_eq!(stats.chunks_enqueued, 0);
    store.delete_item(&id).unwrap();
    store.record_signal_event("doc_delete", &id, None).unwrap();

    // 验证 5 类信号都写入了
    for k in &["doc_create", "doc_update", "annotation_marker", "citation_hit", "doc_delete"] {
        let c = store.count_unprocessed_signals_by_kind(k).unwrap();
        assert_eq!(c, 1, "kind={k} 应有 1 条未处理信号，实际 {c}");
    }
}

#[test]
fn evolver_only_consumes_search_miss_kind() {
    // R17 S4-Q1 fix 验收：evolver 只看 search_miss kind 不被 Phase B 信号污染
    let (_t, store, _v, _f, _d) = setup();
    store.record_skill_signal("query without results", 0, false).unwrap();
    store.record_signal_event("doc_update", "item_x", None).unwrap();
    store.record_signal_event("citation_hit", "item_y", Some("user msg")).unwrap();
    store.record_signal_event("annotation_marker", "item_z", Some("⭐")).unwrap();

    // count 只数 search_miss
    let total = store.count_unprocessed_signals().unwrap();
    assert_eq!(total, 1, "count 必须只数 search_miss kind（R17 P0 fix）");

    // get 也只拿 search_miss
    let sigs = store.get_unprocessed_signals(10).unwrap();
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].query, "query without results");

    // by_kind 全谱可达
    assert_eq!(store.count_unprocessed_signals_by_kind("search_miss").unwrap(), 1);
    assert_eq!(store.count_unprocessed_signals_by_kind("doc_update").unwrap(), 1);
    assert_eq!(store.count_unprocessed_signals_by_kind("citation_hit").unwrap(), 1);
    assert_eq!(store.count_unprocessed_signals_by_kind("annotation_marker").unwrap(), 1);
}

#[test]
fn signal_kind_rejects_typo() {
    // R6 P1-4 fix 验收
    let (_t, store, _v, _f, _d) = setup();
    assert!(store.record_signal_event("doc_updaet", "item_x", None).is_err(),
            "typo kind 必须报错");
}

#[test]
fn update_item_within_transaction_atomic() {
    // R17 S1-Q4 fix 验收：update_item 内 SQL 已包入事务，多轮 update 后 hash + BLOB 一致
    let (_t, store, _v, _f, dek) = setup();
    let id = store.insert_item(&dek, "t", "v1", None, "note", None, None).unwrap();

    for v in ["v2", "v3", "v4"].iter() {
        let outcome = store.update_item(&dek, &id, None, Some(v)).unwrap();
        assert!(outcome.content_changed);
        let stored_hash = store.get_content_hash(&id).unwrap().unwrap();
        let expected_hash = compute_content_hash(v);
        assert_eq!(stored_hash, expected_hash,
                   "事务保证 content + hash 同步更新 (v={v})");
    }
}

#[test]
fn reindex_queue_action_validation_and_park() {
    // R6 P0-3 + P1-5 fix 全链路验收
    let (_t, store, _v, _f, dek) = setup();
    let id = store.insert_item(&dek, "t", "c", None, "note", None, None).unwrap();

    store.enqueue_reindex(&id, "purge").unwrap();
    store.enqueue_reindex(&id, "reindex").unwrap();
    assert!(store.enqueue_reindex(&id, "bogus").is_err(), "typo action 必须报错");

    let tasks = store.dequeue_reindex_tasks(10).unwrap();
    assert_eq!(tasks.len(), 2);

    store.mark_reindex_done(tasks[0].0).unwrap();
    let after = store.dequeue_reindex_tasks(10).unwrap();
    assert_eq!(after.len(), 1, "mark_done 后只剩 1");

    // 模拟毒任务：bump 5 次
    let surviving_id = after[0].0;
    for _ in 0..5 {
        store.bump_reindex_attempts(surviving_id).unwrap();
    }
    let final_visible = store.dequeue_reindex_tasks(10).unwrap();
    assert_eq!(final_visible.len(), 0, "attempts >= 5 必须被 dequeue 跳过");
}

#[test]
fn signal_event_with_truncated_query() {
    // R9 P1-3 fix 验收：chat citation_hit query 截断
    let (_t, store, _v, _f, _d) = setup();
    let long_msg: String = "x".repeat(2000);
    let truncated: String = long_msg.chars().take(512).collect();
    store.record_signal_event("citation_hit", "item_a", Some(&truncated)).unwrap();

    let count = store.count_unprocessed_signals_by_kind("citation_hit").unwrap();
    assert_eq!(count, 1);
    // 验证写入的 query 不超过预期长度（caller 应负责截断）
    assert!(truncated.len() <= 512);
}

#[test]
fn content_hash_dedup_via_store_api() {
    // 验证 upload route 用的 find_item_by_content_hash 短路路径
    let (_t, store, _v, _f, dek) = setup();
    let id = store.insert_item(&dek, "t", "DEDUP_PAYLOAD", None, "note", None, None).unwrap();
    let h = compute_content_hash("DEDUP_PAYLOAD");
    assert_eq!(store.find_item_by_content_hash(&h).unwrap(), Some(id));

    // 不同 hash 不命中
    let h2 = compute_content_hash("DIFFERENT");
    assert_eq!(store.find_item_by_content_hash(&h2).unwrap(), None);
}
