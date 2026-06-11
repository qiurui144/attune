//! embed_queue 表 — 异步 embedding / classification 任务队列。

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    // --- embed_queue ---

    /// 将文本块加入 embedding 队列
    pub fn enqueue_embedding(
        &self,
        item_id: &str,
        chunk_idx: usize,
        chunk_text: &str,
        priority: i32,
        level: i32,
        section_idx: usize,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO embed_queue (item_id, chunk_idx, chunk_text, level, section_idx, priority, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![item_id, chunk_idx as i64, chunk_text.as_bytes(), level, section_idx as i64, priority, now],
        )?;
        Ok(())
    }

    /// 从队列中取出一批 pending 任务，标记为 processing
    /// SELECT + UPDATE 在同一事务中执行，防止并发 worker 重复拾取同一任务。
    ///
    /// v0.6 fix (Phase B benchmark)：按 `task_type='embed'` 过滤。embed_queue 共享
    /// embed + classify 两类任务（classify worker 在 server 层独立运行）。
    /// 当 classifier 未加载（默认情况下 dev / bench / 无 LLM 配置），classify 任务
    /// 会被 embed worker dequeue + 进 partition 的 other 分支 + mark_task_pending 重置
    /// → 反复 cycling 永远不结束，最终 embed_queue tail 卡 ~30 个 classify 任务。
    /// 加 task_type 过滤后 embed worker 只看自己的任务，classify 任务静默 pending
    /// 等 classifier 上线（无 worker 时不阻塞 embed 流水线）。
    pub fn dequeue_embeddings(&self, batch_size: usize) -> Result<Vec<QueueTask>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut stmt = tx.prepare(
            "SELECT id, item_id, chunk_idx, chunk_text, level, section_idx, priority, attempts, task_type
             FROM embed_queue WHERE status = 'pending' AND task_type = 'embed'
             ORDER BY priority ASC, created_at ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![batch_size as i64], |row| {
            let chunk_blob: Vec<u8> = row.get(3)?;
            Ok(QueueTask {
                id: row.get(0)?,
                item_id: row.get(1)?,
                chunk_idx: row.get(2)?,
                chunk_text: String::from_utf8_lossy(&chunk_blob).into_owned(),
                level: row.get(4)?,
                section_idx: row.get(5)?,
                priority: row.get(6)?,
                attempts: row.get(7)?,
                task_type: row.get(8)?,
            })
        })?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        drop(stmt);
        // 批量标记为 processing（与 SELECT 在同一事务内，防止并发重复拾取）
        for task in &tasks {
            tx.execute(
                "UPDATE embed_queue SET status = 'processing' WHERE id = ?1",
                params![task.id],
            )?;
        }
        tx.commit()?;
        Ok(tasks)
    }

    /// 标记队列任务为完成
    pub fn mark_embedding_done(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE embed_queue SET status = 'done' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// 检查 embed_queue 任务行是否仍存在。
    ///
    /// embed worker dequeue 任务后（标 processing，行仍在），到写向量之间有窗口。
    /// 若期间 reindex / delete 调 `purge_embed_queue_for_item`，会 DELETE 该行
    /// （含 processing 状态）。worker 写向量前查此函数：
    /// - 行已不在 → 该 chunk 已被 reindex 作废（item 被 PATCH 重切 / 被删）
    ///   → 跳过写向量，防 stale 向量（旧内容）/ orphan 向量（已删 item）。
    /// - 行还在 → 正常写。
    ///
    /// 实测：100KB 文档 1278 chunk，PATCH 后 embedding worker 仍在写旧 chunk 向量
    /// → 编辑后旧关键词仍搜得到。本检查根治该 update 竞态。
    pub fn embed_task_exists(&self, task_id: i64) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// 标记队列任务为失败，超过最大尝试次数则标记为 abandoned
    /// 三步操作包裹在事务中保证原子性，防止并发 worker 导致 attempts 计数错误
    pub fn mark_embedding_failed(&self, id: i64, max_attempts: i32) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE embed_queue SET attempts = attempts + 1 WHERE id = ?1",
            params![id],
        )?;
        let attempts: i32 = tx.query_row(
            "SELECT attempts FROM embed_queue WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        let new_status = if attempts >= max_attempts { "abandoned" } else { "pending" };
        tx.execute(
            "UPDATE embed_queue SET status = ?1 WHERE id = ?2",
            params![new_status, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 查询 pending 状态的队列任务数量
    /// 仅统计 embed 任务的 pending 数。classify 任务不计入（由独立 worker 处理，
    /// 在 classifier 未加载时会静默 pending；如果计入会导致 indexer status 永远不为 0）。
    pub fn pending_embedding_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE status = 'pending' AND task_type = 'embed'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 按 task_type 查询 pending 状态任务数量（用于进度推送）
    pub fn pending_count_by_type(&self, task_type: &str) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE status = 'pending' AND task_type = ?1",
            [task_type],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 为 item 入队一个分类任务 (task_type='classify')
    pub fn enqueue_classify(&self, item_id: &str, priority: i32) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO embed_queue (item_id, chunk_idx, chunk_text, level, section_idx, priority, status, created_at, task_type)
             VALUES (?1, 0, ?2, 0, 0, ?3, 'pending', ?4, 'classify')",
            params![item_id, Vec::<u8>::new(), priority, now],
        )?;
        Ok(())
    }

    /// 将 processing 任务重新标记为 pending（用于未实现处理时占位）
    pub fn mark_task_pending(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE embed_queue SET status = 'pending' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// v0.6 fix (Phase B benchmark)：启动时复位 stuck 在 processing 的任务回 pending。
    /// 上次进程崩溃 / kill 时 dequeue 已 mark processing 但还没 mark_done，
    /// 不复位则永远停在 processing。返回复位的任务数。
    pub fn reset_stuck_processing(&self) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE embed_queue SET status = 'pending' WHERE status = 'processing'",
            [],
        )?;
        Ok(n)
    }

    /// 测试辅助：统计某 level 在 embed_queue 中的 pending 任务数。
    pub fn count_embed_queue_by_level(&self, level: i32) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE level = ?1 AND task_type = 'embed'",
            params![level],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// 测试辅助：取某 item 的全部 embedding chunk_text 明文列表，按 chunk_idx 排序。
    /// 只取 task_type='embed' 的任务，跳过 classify 占位行（其 chunk_text 为空字节）。
    /// embed_queue.chunk_text 存明文字节，此处直接 UTF-8 解码。
    pub fn peek_embed_queue_chunk_texts(&self, item_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT chunk_text FROM embed_queue WHERE item_id = ?1 AND task_type = 'embed' ORDER BY chunk_idx",
        )?;
        let rows = stmt.query_map(params![item_id], |row| row.get::<_, Vec<u8>>(0))?;
        let mut out = Vec::new();
        for blob in rows {
            out.push(String::from_utf8_lossy(&blob?).into_owned());
        }
        Ok(out)
    }

    /// QW-1 (storage cleanup): 清理 embed_queue 中已完成 / 已放弃的行。
    ///
    /// 旧实现 `mark_embedding_done` 只 UPDATE status='done'，行永远留在表里。
    /// 长期运行的 vault 会累积百万级 done 行，让 dequeue 走的索引扫描越来越慢
    /// 且 SQLite 文件持续膨胀。本函数 DELETE 终态行（done / abandoned）：
    ///
    /// - 由 `Store::open()` 一次性调一次（清启动前累积的 done/abandoned）。
    /// - 由后台 cleanup worker 周期（默认每周）调一次。
    ///
    /// 返回删除的行数（便于诊断 / 测试断言）。
    pub fn purge_completed_embed_queue(&self) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM embed_queue WHERE status IN ('done', 'abandoned')",
            [],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QW-1: done / abandoned 终态行应被 purge_completed_embed_queue 删除；
    /// pending / processing 行保留。
    #[test]
    fn purge_completed_removes_done_and_abandoned_only() {
        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let item_id = store
            .insert_item(&dek, "t", "body", None, "note", None, None)
            .unwrap();

        // 1 pending
        store.enqueue_embedding(&item_id, 0, "p", 2, 1, 0).unwrap();
        // 1 processing
        store.enqueue_embedding(&item_id, 1, "ing", 2, 1, 0).unwrap();
        store
            .conn
            .execute(
                "UPDATE embed_queue SET status = 'processing' WHERE chunk_idx = 1",
                [],
            )
            .unwrap();
        // 1 done
        store.enqueue_embedding(&item_id, 2, "d", 2, 1, 0).unwrap();
        store
            .conn
            .execute(
                "UPDATE embed_queue SET status = 'done' WHERE chunk_idx = 2",
                [],
            )
            .unwrap();
        // 1 abandoned
        store.enqueue_embedding(&item_id, 3, "a", 2, 1, 0).unwrap();
        store
            .conn
            .execute(
                "UPDATE embed_queue SET status = 'abandoned' WHERE chunk_idx = 3",
                [],
            )
            .unwrap();

        let removed = store.purge_completed_embed_queue().unwrap();
        assert_eq!(removed, 2, "应删除 done + abandoned 两行");

        let remaining: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM embed_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 2, "保留 pending + processing");
    }

    /// QW-1: 空表 / 全 pending 时 purge 不报错且返回 0。
    #[test]
    fn purge_completed_on_empty_returns_zero() {
        let store = Store::open_memory().unwrap();
        assert_eq!(store.purge_completed_embed_queue().unwrap(), 0);
    }
}
