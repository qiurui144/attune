// store queue 状态转换测试 — mark_embedding_failed 原子性校验

#[cfg(test)]
mod tests {
    use attune_core::crypto::Key32;
    use attune_core::store::Store;

    /// 向 embed_queue 写入一条任务并取出，返回 task.id
    fn insert_and_dequeue(store: &Store) -> i64 {
        let dek = Key32::generate();
        let item_id = store
            .insert_item(&dek, "Title", "Content", None, "note", None, None)
            .unwrap();
        store.enqueue_embedding(&item_id, 0, "chunk text", 2, 2, 0).unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        assert_eq!(tasks.len(), 1, "queue must have exactly one task after enqueue+dequeue");
        tasks[0].id
    }

    #[test]
    fn mark_failed_below_max_stays_pending() {
        let store = Store::open_memory().unwrap();
        let task_id = insert_and_dequeue(&store);

        // max_attempts=3，第一次失败后 attempts=1，应保持 pending
        store.mark_embedding_failed(task_id, 3).unwrap();

        let tasks = store.dequeue_embeddings(1).unwrap();
        assert!(!tasks.is_empty(), "task must remain pending after first failure (below max)");
    }

    #[test]
    fn mark_failed_at_max_transitions_to_abandoned() {
        let store = Store::open_memory().unwrap();
        let task_id = insert_and_dequeue(&store);

        // max_attempts=1：第一次失败即达到上限，应转为 abandoned
        store.mark_embedding_failed(task_id, 1).unwrap();

        // abandoned 任务不应被 dequeue 再次返回
        let tasks = store.dequeue_embeddings(1).unwrap();
        assert!(tasks.is_empty(), "abandoned task must not be returned by dequeue");
    }

    #[test]
    fn mark_failed_multiple_times_accumulates_correctly() {
        let store = Store::open_memory().unwrap();
        let task_id = insert_and_dequeue(&store);

        // 3 次失败，max=3；前两次保持 pending，第三次应变 abandoned
        store.mark_embedding_failed(task_id, 3).unwrap();
        let t1 = store.dequeue_embeddings(1).unwrap();
        assert!(!t1.is_empty(), "after 1st failure (max=3) must still be pending");

        store.mark_embedding_failed(t1[0].id, 3).unwrap();
        let t2 = store.dequeue_embeddings(1).unwrap();
        assert!(!t2.is_empty(), "after 2nd failure (max=3) must still be pending");

        store.mark_embedding_failed(t2[0].id, 3).unwrap();
        let t3 = store.dequeue_embeddings(1).unwrap();
        assert!(t3.is_empty(), "after 3rd failure (max=3) must be abandoned");
    }
}
