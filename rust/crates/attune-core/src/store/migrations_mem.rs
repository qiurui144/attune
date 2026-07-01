//! memory_migrations — progress ledger for embedding-model reindex runs.
//!
//! When the active embedding model changes, every memory vector tagged with the
//! old model key becomes stale and must be re-embedded. `list_stale_memory_ids`
//! drives the reindex worker; the `memory_migrations` table records a run's
//! from/to model, total, and live `done` counter so the UI can show progress and
//! the run survives a restart.

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

/// One reindex run's progress row.
#[derive(Debug, Clone)]
pub struct MemMigration {
    pub id: i64,
    pub from_model: String,
    pub to_model: String,
    pub total: i64,
    pub done: i64,
    pub status: String,
}

impl Store {
    /// memory_ids whose stored vector model != the current model key (stale → reindex).
    pub fn list_stale_memory_ids(&self, cur_model: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT memory_id FROM memory_vectors WHERE model != ?1")?;
        let rows = stmt.query_map(params![cur_model], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn start_memory_migration(&self, from: &str, to: &str, total: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO memory_migrations (from_model, to_model, total, done, status, started_at) \
             VALUES (?1, ?2, ?3, 0, 'running', ?4)",
            params![from, to, total, chrono::Utc::now().timestamp()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn bump_memory_migration_done(&self, id: i64, n: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE memory_migrations SET done = done + ?2 WHERE id = ?1",
            params![id, n],
        )?;
        Ok(())
    }

    pub fn finish_memory_migration(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE memory_migrations SET status = 'done' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn get_memory_migration(&self, id: i64) -> Result<Option<MemMigration>> {
        let r = self
            .conn
            .query_row(
                "SELECT id, from_model, to_model, total, done, status \
                 FROM memory_migrations WHERE id = ?1",
                params![id],
                |r| {
                    Ok(MemMigration {
                        id: r.get(0)?,
                        from_model: r.get(1)?,
                        to_model: r.get(2)?,
                        total: r.get(3)?,
                        done: r.get(4)?,
                        status: r.get(5)?,
                    })
                },
            )
            .ok();
        Ok(r)
    }
}

impl Store {
    /// Create the `memory_migrations` progress table. Idempotent; runs in both the
    /// on-disk (`open`) and in-memory (`open_memory`) migration sequences.
    pub(crate) fn migrate_memory_migrations(conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memory_migrations (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                from_model TEXT, \
                to_model TEXT, \
                total INTEGER NOT NULL, \
                done INTEGER NOT NULL DEFAULT 0, \
                status TEXT NOT NULL DEFAULT 'running', \
                started_at INTEGER NOT NULL);",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;

    /// Insert a real memory row and return its id — memory_vectors.memory_id has a
    /// FK to memories(id) (foreign_keys=ON in open_memory), so bare ids would be
    /// rejected. Mirrors the memory_vectors test helper pattern.
    fn seed_memory(store: &Store, dek: &Key32, hash: &str, created_at: i64) -> String {
        store
            .insert_memory(
                dek,
                "episodic",
                0,
                100,
                &[hash.into()],
                "summary",
                "m",
                created_at,
            )
            .unwrap();
        // most-recent memory (ORDER BY created_at DESC) is the one we just inserted;
        // distinct created_at keeps the ordering deterministic across seeds.
        store.list_recent_memories(dek, 1).unwrap()[0].id.clone()
    }

    #[test]
    fn stale_ids_and_progress_roundtrip() {
        let s = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let m1 = seed_memory(&s, &dek, "h1", 1);
        let m2 = seed_memory(&s, &dek, "h2", 2);
        s.put_memory_vector(&m1, &[1.0, 2.0], "old-model", 1)
            .unwrap();
        s.put_memory_vector(&m2, &[1.0, 2.0, 3.0], "bge-m3", 1)
            .unwrap();

        let stale = s.list_stale_memory_ids("bge-m3").unwrap();
        assert_eq!(stale, vec![m1.clone()]); // m2 already on current model

        let id = s.start_memory_migration("old-model", "bge-m3", 1).unwrap();
        s.bump_memory_migration_done(id, 1).unwrap();
        let row = s.get_memory_migration(id).unwrap().unwrap();
        assert_eq!(row.done, 1);
        assert_eq!(row.total, 1);
        assert_eq!(row.status, "running");

        s.finish_memory_migration(id).unwrap();
        assert_eq!(s.get_memory_migration(id).unwrap().unwrap().status, "done");
    }
}
