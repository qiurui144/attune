// npu-vault/crates/vault-core/src/queue.rs

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use crate::embed::EmbeddingProvider;
use crate::error::{Result, VaultError};
use crate::index::FulltextIndex;
use crate::resource_governor::{global_registry, TaskKind};
use crate::store::{QueueTask, Store};
use crate::vectors::{VectorIndex, VectorMeta};

/// Embedding batch size. 由 10 提到 32：ROCm/CUDA 上 bge-m3 并行吞吐更好
/// （实测 Radeon 780M: 短中文 10→32 提速 ~2x）。>64 会让长文档 chunk
/// 的 tokenized tensor 堆到内存上限。
const BATCH_SIZE: usize = 32;
const POLL_INTERVAL_MS: u64 = 2000;
const MAX_ATTEMPTS: i32 = 3;

/// Embedding 队列 Worker
pub struct QueueWorker {
    running: Arc<AtomicBool>,
}

impl Default for QueueWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueWorker {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 启动 worker（在后台线程运行）
    ///
    /// H1：在 [`global_registry`] 注册 [`TaskKind::EmbeddingQueue`] governor。
    /// 每次循环顶部 check `should_run`，超 budget 或全局 pause 时短 sleep；
    /// 处理一批后用 `after_work` 决定下次 sleep（throttle 退让）。
    pub fn start(
        &self,
        store: Arc<Mutex<Store>>,
        embedding: Arc<dyn EmbeddingProvider>,
        vectors: Arc<Mutex<VectorIndex>>,
        fulltext: Arc<Mutex<FulltextIndex>>,
    ) -> std::thread::JoinHandle<()> {
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let governor = global_registry().register(TaskKind::EmbeddingQueue);

        std::thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                if !governor.should_run() {
                    // 被全局 pause 或本任务超 CPU budget — 短 sleep 后重试
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                match Self::process_batch(&store, &embedding, &vectors, &fulltext) {
                    Ok(processed) => {
                        if processed == 0 {
                            // 队列空 — 走原 polling 间隔，不需要 throttle
                            std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                        } else {
                            // 处理了任务 — 让 governor 决定退让时长
                            std::thread::sleep(governor.after_work());
                        }
                    }
                    Err(e) => {
                        log::error!("Queue worker error: {}", e);
                        std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                    }
                }
            }
        })
    }

    /// 停止 worker
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// 检查 worker 是否正在运行
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 处理一批任务，按 task_type 分派，返回处理数量
    fn process_batch(
        store: &Arc<Mutex<Store>>,
        embedding: &Arc<dyn EmbeddingProvider>,
        vectors: &Arc<Mutex<VectorIndex>>,
        fulltext: &Arc<Mutex<FulltextIndex>>,
    ) -> Result<usize> {
        if !embedding.is_available() {
            return Ok(0);
        }

        // 获取一批 pending 任务
        let tasks = {
            let s = store.lock()
                .map_err(|_| VaultError::Crypto("store lock poisoned".into()))?;
            s.dequeue_embeddings(BATCH_SIZE)?
        };

        if tasks.is_empty() {
            return Ok(0);
        }

        // 按 task_type 分区
        let (embed_tasks, other_tasks): (Vec<QueueTask>, Vec<QueueTask>) =
            tasks.into_iter().partition(|t| t.task_type == "embed");

        let mut total = 0;

        if !embed_tasks.is_empty() {
            total +=
                Self::process_embed_batch(store, embedding, vectors, fulltext, embed_tasks)?;
        }

        if !other_tasks.is_empty() {
            // classify 等任务在 core 层无法处理（需要 Classifier / Taxonomy，属于 server 层），
            // 将其重新标记为 pending，留在队列中等待上层消费者处理。
            // 注意：归还任务不计入 total，避免调用方误认为已处理而进入忙等。
            let s = store.lock()
                .map_err(|_| VaultError::Crypto("store lock poisoned".into()))?;
            for task in &other_tasks {
                s.mark_task_pending(task.id)?;
            }
        }

        Ok(total)
    }

    /// 处理一批 embedding 任务（由 process_batch 分派）— 内部走共享 `embed_and_index_batch`。
    fn process_embed_batch(
        store: &Arc<Mutex<Store>>,
        embedding: &Arc<dyn EmbeddingProvider>,
        vectors: &Arc<Mutex<VectorIndex>>,
        fulltext: &Arc<Mutex<FulltextIndex>>,
        tasks: Vec<QueueTask>,
    ) -> Result<usize> {
        let count = tasks.len();

        // 锁顺序：vectors → fulltext → store（与 server::start_queue_worker 一致）
        let mut vecs = vectors.lock()
            .map_err(|_| VaultError::Crypto("vectors lock poisoned".into()))?;
        let ft = fulltext.lock()
            .map_err(|_| VaultError::Crypto("fulltext lock poisoned".into()))?;
        let s = store.lock()
            .map_err(|_| VaultError::Crypto("store lock poisoned".into()))?;

        let result = embed_and_index_batch(&s, embedding.as_ref(), &mut vecs, &ft, &tasks);
        match result {
            Ok(done_ids) => {
                for id in done_ids {
                    s.mark_embedding_done(id)?;
                }
                Ok(count)
            }
            Err(e) => {
                for task in &tasks {
                    let _ = s.mark_embedding_failed(task.id, MAX_ATTEMPTS);
                }
                Err(e)
            }
        }
    }

    /// 同步处理所有 pending 任务（用于测试）
    pub fn process_all(
        store: &Store,
        embedding: &dyn EmbeddingProvider,
        vectors: &mut VectorIndex,
        fulltext: &FulltextIndex,
    ) -> Result<usize> {
        let mut total = 0;
        loop {
            let tasks = store.dequeue_embeddings(BATCH_SIZE)?;
            if tasks.is_empty() {
                break;
            }
            let done = embed_and_index_batch(store, embedding, vectors, fulltext, &tasks)?;
            for id in done {
                store.mark_embedding_done(id)?;
            }
            total += tasks.len();
        }
        Ok(total)
    }
}

/// R23 (2026-05-01): 共享批处理函数 — `attune-core::queue::QueueWorker` 与
/// `attune-server::start_queue_worker` 的统一入口，消除两处重复的
/// "embed → 写 vectors → 写 fulltext (Level 1 only)" 逻辑。
///
/// 调用方负责：
///   - 已加好 `store` / `vectors` / `fulltext` 的锁（外层 Mutex 已 lock）
///   - 拿到返回的成功 task id 列表后调 `store.mark_embedding_done(id)`
///   - flush（vector 持久化、fulltext commit）由调用方按节流策略决定
///
/// 返回成功处理的 task id；批量 embed 失败抛错（调用方决定是否 mark_failed）。
pub fn embed_and_index_batch(
    store: &Store,
    embedding: &dyn EmbeddingProvider,
    vectors: &mut VectorIndex,
    fulltext: &FulltextIndex,
    tasks: &[QueueTask],
) -> Result<Vec<i64>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    let texts: Vec<&str> = tasks.iter().map(|t| t.chunk_text.as_str()).collect();
    let embeddings = embedding.embed(&texts)?;

    // R10 S3 fix (P1): item 存活检查缓存。竞态场景 — embed worker dequeue chunk 任务
    // 时 item 还在，但 embedding 完写向量前 item 已被 delete_item 软删（reindex
    // worker / HTTP delete 并发）。不检查会写 orphan 向量（已删文档仍被搜到）。
    // 同 batch 多 chunk 常属同 item → HashMap 缓存避免重复 COUNT 查询。
    let mut alive_cache: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

    let mut done_ids = Vec::with_capacity(tasks.len());
    for (i, task) in tasks.iter().enumerate() {
        if i >= embeddings.len() {
            break;
        }
        // R10 S3 fix (update 场景，P1): chunk 任务行被 reindex/delete 的
        // purge_embed_queue_for_item DELETE 掉 → 该 chunk 已作废（item 被 PATCH
        // 重切 / 被删）→ 跳过写向量，防 stale 向量（大文档实测必现）。
        // task 行已不在表里，不 push done_id（mark_done 也无行可改）。
        // 行还在 OR 查询失败（保守继续）
        if let Ok(false) = store.embed_task_exists(task.id) { continue }
        let alive = *alive_cache
            .entry(task.item_id.clone())
            // 查询失败时保守视为存活（继续写，宁可暂时 orphan 也不因瞬时 DB
            // 错误丢正常文档的向量；下次 reindex 会纠正）
            .or_insert_with(|| store.item_exists(&task.item_id).unwrap_or(true));
        if !alive {
            // item 已软删 → 跳过写向量防 orphan，但仍 push done_id 让任务出队
            // （重试一个 item 已删的 embedding 任务无意义）
            done_ids.push(task.id);
            continue;
        }
        vectors.add(
            &embeddings[i],
            VectorMeta {
                item_id: task.item_id.clone(),
                chunk_idx: task.chunk_idx as usize,
                level: task.level as u8,
                section_idx: task.section_idx as usize,
            },
        )?;
        if task.level == 1 {
            fulltext.add_document(&task.item_id, "", &task.chunk_text, "file")?;
        }
        done_ids.push(task.id);
    }
    Ok(done_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::NoopProvider;

    #[test]
    fn worker_lifecycle() {
        let worker = QueueWorker::new();
        assert!(!worker.is_running());
    }

    #[test]
    fn process_all_empty_queue() {
        let store = Store::open_memory().unwrap();
        let provider = NoopProvider;
        let mut vectors = VectorIndex::new(1024).unwrap();
        let fulltext = FulltextIndex::open_memory().unwrap();

        // 队列为空时应返回 Ok(0)
        let count = QueueWorker::process_all(&store, &provider, &mut vectors, &fulltext).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn pending_count_tracks_enqueue() {
        let store = Store::open_memory().unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 0);

        // 需要先插入一个 item 以满足外键约束
        let dek = crate::crypto::Key32::generate();
        let item_id = store
            .insert_item(&dek, "test", "content", None, "note", None, None)
            .unwrap();

        store.enqueue_embedding(&item_id, 0, "hello world", 2, 2, 0).unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);

        store.enqueue_embedding(&item_id, 1, "second chunk", 2, 1, 0).unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 2);
    }

    #[test]
    fn dequeue_marks_processing() {
        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let item_id = store
            .insert_item(&dek, "test", "content", None, "note", None, None)
            .unwrap();

        store.enqueue_embedding(&item_id, 0, "chunk text", 2, 2, 0).unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);

        let tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].chunk_text, "chunk text");
        assert_eq!(tasks[0].level, 2);

        // dequeue 后 pending 数量应减少（状态变为 processing）
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
    }

    #[test]
    fn mark_done_and_failed() {
        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let item_id = store
            .insert_item(&dek, "test", "content", None, "note", None, None)
            .unwrap();

        store.enqueue_embedding(&item_id, 0, "chunk a", 2, 2, 0).unwrap();
        store.enqueue_embedding(&item_id, 1, "chunk b", 2, 2, 0).unwrap();

        let tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 2);

        // 标记第一个完成
        store.mark_embedding_done(tasks[0].id).unwrap();

        // 标记第二个失败（未超过 max_attempts 时回到 pending）
        store.mark_embedding_failed(tasks[1].id, 3).unwrap();
        // attempts=1 < max=3, 所以重新变为 pending
        assert_eq!(store.pending_embedding_count().unwrap(), 1);

        // 再 dequeue 处理失败的
        let retry_tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(retry_tasks.len(), 1);

        // 反复失败直到 abandoned
        store.mark_embedding_failed(retry_tasks[0].id, 3).unwrap(); // attempts=2
        let retry2 = store.dequeue_embeddings(10).unwrap();
        assert_eq!(retry2.len(), 1);
        store.mark_embedding_failed(retry2[0].id, 3).unwrap(); // attempts=3 >= max -> abandoned
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
    }
}
