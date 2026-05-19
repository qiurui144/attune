//! Reindex — 文档生命周期事务式协调：清旧 → 切 chunk → 入队 → 加 FTS
//!
//! v0.7 记忆护城河增强（per 用户决策 2026-05-15）。
//!
//! ## 解决的问题
//!
//! `Store` (attune-core) 只持有 SQL 连接，`VectorIndex` + `FulltextIndex` 是 server 层
//! AppState 独立 Mutex 的资源。在 0.6 之前各 update path 各写一份"删旧加新"，结果：
//!
//! - `routes/items.rs::update_item` 完全没 re-embed → UI 编辑后 search 永远返回旧内容
//! - `routes/upload.rs` 同名重传不删旧向量 → HNSW 出现重复 chunk
//! - `routes/items.rs::delete_item` 不调 `vectors::delete_by_item_id` /
//!   `fulltext::delete_document` → orphan 向量永久残留
//! - `vectors::delete_by_item_id` 和 `fulltext::delete_document` 已实现但全仓 0 处调用
//!
//! ## 设计
//!
//! 本模块提供两个高层 API，强约束三资源（DB / vectors / fulltext / embed_queue）
//! **同时变更**，避免下游路径再各自漏一个：
//!
//! - [`reindex_item`] — 内容变更后完整重建：清旧 → re-chunk → 入队 + FTS upsert
//! - [`purge_item_indexes`] — 删除路径：清向量 + 清 FTS + 清队列（DB 软删由 caller）
//!
//! ## content_hash 短路
//!
//! [`reindex_item`] 内部不做短路 — caller 应在 [`Store::update_item`] 返回的
//! [`crate::store::items::UpdateOutcome`] 上判断 `content_changed`，只有 true 才调
//! 本函数，避免每次 metadata-only 的 update 都触发 embedding pipeline。

use crate::chunker;
use crate::error::Result;
use crate::index::FulltextIndex;
use crate::store::Store;
use crate::vectors::VectorIndex;

/// 一次 reindex 的统计，便于 caller 写 audit log 与回归测试断言。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReindexStats {
    /// 从 HNSW 中删除的旧向量数（含所有 level）
    pub vectors_deleted: usize,
    /// 从 embed_queue 中清掉的 pending 任务数
    pub queue_cleared: usize,
    /// 新入队的 chunk 数（Level 1 章节 + Level 2 段落块）
    pub chunks_enqueued: usize,
}

/// 完整重建某 item 的索引（content 已经在 store 里更新到最新）。
///
/// 调用顺序（每步失败立刻返回 — 让 caller 决定是否回滚 content）：
/// 1. `vectors.delete_by_item_id(item_id)` — 清旧向量
/// 2. `fulltext.delete_document(item_id)` — 清旧 FTS
/// 3. `store.purge_embed_queue_for_item(item_id)` — 清旧队列
/// 4. `fulltext.add_document(...)` — 写新 FTS（搜索立即可用，不等 embedding）
/// 5. `chunker::extract_sections + chunker::chunk` → `store.enqueue_embedding` —
///    入队 Level 1 章节 + Level 2 段落块，worker 后台消费写新向量
///
/// 不调 `breadcrumbs` 与 `classify` enqueue — 这两条 caller 按需调（保持 API 单一职责）。
pub fn reindex_item(
    store: &Store,
    vectors: &mut VectorIndex,
    fulltext: &FulltextIndex,
    item_id: &str,
    title: &str,
    content: &str,
    source_type: &str,
) -> Result<ReindexStats> {
    let vectors_deleted = vectors.delete_by_item_id(item_id)?;
    fulltext.delete_document(item_id)?;
    let queue_cleared = store.purge_embed_queue_for_item(item_id)?;
    fulltext.add_document(item_id, title, content, source_type)?;

    let mut chunk_counter: usize = 0;
    let sections = chunker::extract_sections(content);
    for (section_idx, section_text) in &sections {
        if section_text.trim().is_empty() {
            continue;
        }
        store.enqueue_embedding(item_id, chunk_counter, section_text, 1, 1, *section_idx)?;
        chunk_counter += 1;
    }
    for (section_idx, section_text) in &sections {
        for chunk_text in chunker::chunk(section_text, chunker::DEFAULT_CHUNK_SIZE, chunker::DEFAULT_OVERLAP) {
            store.enqueue_embedding(item_id, chunk_counter, &chunk_text, 2, 2, *section_idx)?;
            chunk_counter += 1;
        }
    }

    Ok(ReindexStats { vectors_deleted, queue_cleared, chunks_enqueued: chunk_counter })
}

/// 删除路径：清向量 + 清 FTS + 清队列。DB 软删由 caller 单独调
/// [`Store::delete_item`]（本函数后面再 store 删，因为 delete_item 内部已清 queue，
/// 重复调用幂等无害但 caller 顺序应：先 `purge_item_indexes` → 后 `delete_item`，
/// 让 search worker 在删除窗口内不会读到 partial 状态）。
pub fn purge_item_indexes(
    store: &Store,
    vectors: &mut VectorIndex,
    fulltext: &FulltextIndex,
    item_id: &str,
) -> Result<ReindexStats> {
    let vectors_deleted = vectors.delete_by_item_id(item_id)?;
    fulltext.delete_document(item_id)?;
    let queue_cleared = store.purge_embed_queue_for_item(item_id)?;
    Ok(ReindexStats { vectors_deleted, queue_cleared, chunks_enqueued: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;
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
    fn reindex_clears_queue_and_reenqueues() {
        let (_t, store, mut vec, ft, dek) = setup();
        let id = store
            .insert_item(&dek, "title", "# H1\n\nbody one\n\n# H2\n\nbody two", None, "note", None, None)
            .unwrap();
        // 入旧队列
        store.enqueue_embedding(&id, 0, "stale", 2, 1, 0).unwrap();
        store.enqueue_embedding(&id, 1, "stale2", 2, 2, 0).unwrap();

        let stats = reindex_item(&store, &mut vec, &ft, &id, "title", "# H1\n\nNEW BODY", "note").unwrap();
        assert_eq!(stats.queue_cleared, 2, "旧两条 stale 必须清掉");
        assert!(stats.chunks_enqueued >= 1, "新内容至少入 1 个 chunk");
    }

    #[test]
    fn purge_clears_queue() {
        let (_t, store, mut vec, ft, dek) = setup();
        let id = store.insert_item(&dek, "t", "body", None, "note", None, None).unwrap();
        store.enqueue_embedding(&id, 0, "x", 2, 1, 0).unwrap();
        let stats = purge_item_indexes(&store, &mut vec, &ft, &id).unwrap();
        assert_eq!(stats.queue_cleared, 1);
        assert_eq!(stats.chunks_enqueued, 0);
    }
}
