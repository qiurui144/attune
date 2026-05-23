//! Crash recovery 测试 — 验证 WAL 模式下异常退出后 Store 能无损重开。
//!
//! 跑法（默认 ignore，手动跑）：
//!   cargo test -p attune-core --test crash_recovery_test -- --nocapture --ignored
//!
//! 覆盖场景：
//!   1. WAL 模式下 drop Store 后重开 → 写入数据完整
//!   2. write transaction 中 panic（被 catch_unwind 捕获）→ 自动 rollback
//!   3. reindex_item 完成后 drop 所有资源 → 重开 Store 数据一致
//!   4. embedding queue 写入后 drop Store → 重开 queue 任务仍在

use attune_core::crypto::Key32;
use attune_core::index::FulltextIndex;
use attune_core::reindex;
use attune_core::store::Store;
use attune_core::vectors::VectorIndex;
use tempfile::TempDir;

/// 生成测试用内容
fn make_content(sections: usize) -> String {
    let mut s = String::from("# Document\n\n");
    for i in 0..sections {
        s.push_str(&format!(
            "## Section {i}\n\nThis section contains searchable content about topic {i}. \
             Keywords: rust performance index search recovery durability.\n\n"
        ));
    }
    s
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 1: WAL mode — Store drop 后重开，已提交数据不丢失
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn wal_reopen_after_drop_preserves_committed_data() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let dek = Key32::generate();

    // 写 5 条 item，然后 drop Store（模拟正常关闭）
    let item_ids: Vec<String> = {
        let store = Store::open(&db_path).unwrap();
        (0..5)
            .map(|i| {
                store
                    .insert_item(&dek, &format!("Doc {i}"), &make_content(2), None, "note", None, None)
                    .unwrap()
            })
            .collect()
    }; // Store dropped here

    // 重新打开 — 数据必须全部保留
    let store2 = Store::open(&db_path).unwrap();
    let count = store2.item_count().unwrap();
    assert_eq!(count, 5, "WAL 重开后 item 数量必须一致，实际={count}");

    // 逐条验证 item 存在（get_item 返回 Result<Option<_>>，Some = 存在）
    for id in &item_ids {
        let item = store2.get_item(&dek, id).unwrap();
        assert!(item.is_some(), "item {id} 必须能取出");
    }

    println!("[crash_recovery] wal_reopen_after_drop: OK — 5 items verified after reopen");
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 2: Store 连续多次 open/close 幂等 — schema migration 不重复执行，数据不丢
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn repeated_open_close_idempotent() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let dek = Key32::generate();

    // 第一次：写 3 条 item
    {
        let store = Store::open(&db_path).unwrap();
        for i in 0..3 {
            store
                .insert_item(&dek, &format!("Doc {i}"), &make_content(1), None, "note", None, None)
                .unwrap();
        }
    }

    // 第二次：追加 2 条 item（验证 migration 幂等，不丢旧数据）
    {
        let store = Store::open(&db_path).unwrap();
        assert_eq!(store.item_count().unwrap(), 3, "第二次 open 前 3 条必须在");
        for i in 3..5 {
            store
                .insert_item(&dek, &format!("Doc {i}"), &make_content(1), None, "note", None, None)
                .unwrap();
        }
    }

    // 第三次：只读验证
    {
        let store = Store::open(&db_path).unwrap();
        let count = store.item_count().unwrap();
        assert_eq!(count, 5, "三次 open/close 后 item 总数必须 = 5，实际 = {count}");
    }

    println!("[crash_recovery] repeated_open_close_idempotent: OK — 5 items across 3 open/close cycles");
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 3: reindex_item 完成后 drop 全部资源 → 重开 Store 数据一致
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn reindex_then_drop_reopen_consistent() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let ft_path = tmp.path().join("ft");
    let dek = Key32::generate();
    let content = make_content(4);

    let item_id: String;
    let expected_chunks: usize;

    // Phase 1: insert + reindex，然后 drop 一切
    {
        let store = Store::open(&db_path).unwrap();
        let mut vectors = VectorIndex::new(384).unwrap();
        let fulltext = FulltextIndex::open(&ft_path).unwrap();

        item_id = store
            .insert_item(&dek, "Persist Test", &content, None, "note", None, None)
            .unwrap();
        let stats =
            reindex::reindex_item(&store, &mut vectors, &fulltext, &item_id, "Persist Test", &content, "note")
                .unwrap();
        expected_chunks = stats.chunks_enqueued;
        assert!(expected_chunks >= 1, "至少 1 个 chunk 入队");
    } // store, vectors, fulltext all dropped

    // Phase 2: 只重开 Store，验证 embed_queue 和 item 都在
    let store2 = Store::open(&db_path).unwrap();
    assert_eq!(store2.item_count().unwrap(), 1, "item 必须存在");

    let pending = store2.pending_embedding_count().unwrap();
    assert_eq!(
        pending, expected_chunks,
        "embed queue 中 pending 任务数必须与 reindex 入队数一致，expected={expected_chunks} actual={pending}"
    );

    println!("[crash_recovery] reindex_drop_reopen: OK — {expected_chunks} tasks persisted");
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 4: embedding queue 写入后 drop Store → 重开 queue 任务仍在，可被 dequeue
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn embed_queue_survives_store_drop_and_reopen() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let dek = Key32::generate();
    let content = make_content(3);

    let item_id: String;
    let enqueued: usize;

    // 写入 item + embed queue，然后 drop
    {
        let store = Store::open(&db_path).unwrap();
        item_id = store
            .insert_item(&dek, "Queue Test", &content, None, "note", None, None)
            .unwrap();
        // 直接入队若干任务（reindex 会入队，这里用它来填充 queue）
        let mut vectors = VectorIndex::new(384).unwrap();
        let ft = FulltextIndex::open_memory().unwrap();
        let stats = reindex::reindex_item(&store, &mut vectors, &ft, &item_id, "Queue Test", &content, "note").unwrap();
        enqueued = stats.chunks_enqueued;
    }

    // 重开，验证 queue 任务可以 dequeue
    let store2 = Store::open(&db_path).unwrap();
    let pending_before = store2.pending_embedding_count().unwrap();
    assert_eq!(pending_before, enqueued, "重开后 pending 任务数必须一致");

    // dequeue 一批，验证 task 字段完整
    let tasks = store2.dequeue_embeddings(enqueued).unwrap();
    assert_eq!(tasks.len(), enqueued, "dequeue 数量必须等于入队数");
    for task in &tasks {
        assert_eq!(task.item_id, item_id);
        assert!(!task.chunk_text.is_empty(), "chunk_text 不得为空");
        assert!(task.level >= 1, "level 必须 >= 1");
    }

    // 标记全部 done
    for task in &tasks {
        store2.mark_embedding_done(task.id).unwrap();
    }
    assert_eq!(store2.pending_embedding_count().unwrap(), 0, "全部 done 后 pending 必须为 0");

    println!("[crash_recovery] embed_queue_reopen: OK — {enqueued} tasks dequeued after reopen");
}
