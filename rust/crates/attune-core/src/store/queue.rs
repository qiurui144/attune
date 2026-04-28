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
}
