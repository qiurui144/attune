// npu-vault/crates/vault-core/src/queue.rs

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use crate::embed::EmbeddingProvider;
use crate::error::Result;
use crate::index::FulltextIndex;
use crate::store::{QueueTask, Store};
use crate::vectors::{VectorIndex, VectorMeta};

const BATCH_SIZE: usize = 10;
const POLL_INTERVAL_MS: u64 = 2000;
const MAX_ATTEMPTS: i32 = 3;

/// Embedding 队列 Worker
pub struct QueueWorker {
    running: Arc<AtomicBool>,
}

impl QueueWorker {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 启动 worker（在后台线程运行）
    pub fn start(
        &self,
        store: Arc<Mutex<Store>>,
        embedding: Arc<dyn EmbeddingProvider>,
        vectors: Arc<Mutex<VectorIndex>>,
        fulltext: Arc<Mutex<FulltextIndex>>,
    ) -> std::thread::JoinHandle<()> {
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

        std::thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                match Self::process_batch(&store, &embedding, &vectors, &fulltext) {
                    Ok(processed) => {
                        if processed == 0 {
                            std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
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
            let s = store.lock().unwrap();
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
            let s = store.lock().unwrap();
            for task in &other_tasks {
                s.mark_task_pending(task.id)?;
            }
            total += other_tasks.len();
        }

        Ok(total)
    }

    /// 处理一批 embedding 任务（由 process_batch 分派）
    fn process_embed_batch(
        store: &Arc<Mutex<Store>>,
        embedding: &Arc<dyn EmbeddingProvider>,
        vectors: &Arc<Mutex<VectorIndex>>,
        fulltext: &Arc<Mutex<FulltextIndex>>,
        tasks: Vec<QueueTask>,
    ) -> Result<usize> {
        let texts: Vec<&str> = tasks.iter().map(|t| t.chunk_text.as_str()).collect();

        // 批量 embedding
        let embeddings = match embedding.embed(&texts) {
            Ok(embs) => embs,
            Err(e) => {
                // 标记为失败
                let s = store.lock().unwrap();
                for task in &tasks {
                    let _ = s.mark_embedding_failed(task.id, MAX_ATTEMPTS);
                }
                return Err(e);
            }
        };

        // 存储结果
        let count = tasks.len();
        for (i, task) in tasks.iter().enumerate() {
            if i >= embeddings.len() {
                break;
            }

            // 添加到向量索引
            {
                let mut vecs = vectors.lock().unwrap();
                vecs.add(
                    &embeddings[i],
                    VectorMeta {
                        item_id: task.item_id.clone(),
                        chunk_idx: task.chunk_idx as usize,
                        level: task.level as u8,
                        section_idx: task.section_idx as usize,
                    },
                )?;
            }

            // 添加到全文索引（仅 Level 1 章节加入全文）
            if task.level == 1 {
                let ft = fulltext.lock().unwrap();
                ft.add_document(&task.item_id, "", &task.chunk_text, "file")?;
            }

            // 标记完成
            let s = store.lock().unwrap();
            s.mark_embedding_done(task.id)?;
        }

        Ok(count)
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
            let texts: Vec<&str> = tasks.iter().map(|t| t.chunk_text.as_str()).collect();
            let embeddings = embedding.embed(&texts)?;

            for (i, task) in tasks.iter().enumerate() {
                if i >= embeddings.len() {
                    break;
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
                store.mark_embedding_done(task.id)?;
            }
            total += tasks.len();
        }
        Ok(total)
    }
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
