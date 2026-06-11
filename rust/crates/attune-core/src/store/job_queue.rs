//! G5 durable job queue — SQLite-backed CRUD on the `job_queue` table.
//! Mirrors the reindex_queue idiom (store/items.rs) generalized to multi-kind jobs.
//! Spec: docs/superpowers/specs/2026-06-10-k3-g5-durable-job-queue.md

#[cfg(test)]
mod tests {
    use crate::store::Store;

    #[test]
    fn job_queue_table_exists_after_open() {
        let store = Store::open_memory().unwrap();
        let n: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='job_queue'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "job_queue table must be created by SCHEMA_SQL");
    }
}
