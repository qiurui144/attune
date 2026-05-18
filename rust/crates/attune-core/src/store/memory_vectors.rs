//! memory_vectors — embedding sidecar for L2/L3 memories.
//!
//! Episodic/semantic summaries must be embeddable so the tier-aware assembler can
//! *rank* them by relevance, not just list newest-N. Mirrors the document vector
//! sidecar pattern.
//!
//! The embedding BLOB is raw little-endian `f32` bytes — the in-memory `usearch`
//! index quantizes to F16 at load time, so no separate quantization crate is needed.
//! `model` + `dim` are stored so a later embedding-model switch can graceful-skip
//! dimension-mismatched rows instead of panicking.

use rusqlite::{params, OptionalExtension};

use crate::error::{Result, VaultError};
use crate::store::Store;

/// One memory's embedding row.
#[derive(Debug, Clone)]
pub struct MemoryVectorRow {
    pub memory_id: String,
    pub embedding: Vec<f32>,
    pub dim: usize,
    pub model: String,
    pub created_at: i64,
}

fn encode_f32(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn decode_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

impl Store {
    /// Upsert a memory's embedding. Re-embedding the same memory (model switch)
    /// replaces the row in place.
    pub fn put_memory_vector(
        &self,
        memory_id: &str,
        embedding: &[f32],
        model: &str,
        now_secs: i64,
    ) -> Result<()> {
        if embedding.is_empty() {
            return Err(VaultError::InvalidInput(
                "memory vector must be non-empty".into(),
            ));
        }
        let blob = encode_f32(embedding);
        self.conn.execute(
            "INSERT OR REPLACE INTO memory_vectors \
                (memory_id, embedding, dim, model, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![memory_id, blob, embedding.len() as i64, model, now_secs],
        )?;
        Ok(())
    }

    /// Fetch one memory's embedding; `None` if not embedded yet.
    pub fn get_memory_vector(&self, memory_id: &str) -> Result<Option<MemoryVectorRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT memory_id, embedding, dim, model, created_at \
                 FROM memory_vectors WHERE memory_id = ?1",
                params![memory_id],
                |r| {
                    let blob: Vec<u8> = r.get(1)?;
                    Ok(MemoryVectorRow {
                        memory_id: r.get(0)?,
                        embedding: decode_f32(&blob),
                        dim: r.get::<_, i64>(2)? as usize,
                        model: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// All memory embeddings — used to build the in-memory MemoryVectorIndex at startup.
    pub fn list_all_memory_vectors(&self) -> Result<Vec<MemoryVectorRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT memory_id, embedding, dim, model, created_at FROM memory_vectors",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let blob: Vec<u8> = r.get(1)?;
                Ok(MemoryVectorRow {
                    memory_id: r.get(0)?,
                    embedding: decode_f32(&blob),
                    dim: r.get::<_, i64>(2)? as usize,
                    model: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Delete a memory's embedding. The `ON DELETE CASCADE` FK already removes the
    /// row when the parent memory is deleted; this is for explicit cleanup paths.
    pub fn delete_memory_vector(&self, memory_id: &str) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM memory_vectors WHERE memory_id = ?1",
            params![memory_id],
        )?;
        Ok(n)
    }

    /// Count — diagnostics / tests.
    pub fn memory_vector_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;

    #[test]
    fn put_and_get_roundtrip() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 0, 100, &["h1".into()], "s", "m", 0)
            .unwrap();
        let mem_id = store.list_recent_memories(&dek, 1).unwrap()[0].id.clone();
        store
            .put_memory_vector(&mem_id, &[0.1, 0.2, 0.3, 0.4], "bge-m3", 999)
            .unwrap();
        let got = store.get_memory_vector(&mem_id).unwrap().unwrap();
        assert_eq!(got.dim, 4);
        assert_eq!(got.model, "bge-m3");
        assert!((got.embedding[1] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn rejects_empty_embedding() {
        let store = Store::open_memory().unwrap();
        let err = store
            .put_memory_vector("any-id", &[], "m", 0)
            .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[test]
    fn put_replaces_on_reembed() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 0, 100, &["h1".into()], "s", "m", 0)
            .unwrap();
        let mem_id = store.list_recent_memories(&dek, 1).unwrap()[0].id.clone();
        store.put_memory_vector(&mem_id, &[1.0, 2.0], "old-model", 1).unwrap();
        store.put_memory_vector(&mem_id, &[9.0, 8.0, 7.0], "new-model", 2).unwrap();
        let got = store.get_memory_vector(&mem_id).unwrap().unwrap();
        assert_eq!(got.dim, 3);
        assert_eq!(got.model, "new-model");
        assert_eq!(store.memory_vector_count().unwrap(), 1);
    }

    #[test]
    fn cascade_delete_on_memory_delete() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 0, 100, &["h1".into()], "s", "m", 0)
            .unwrap();
        let mem_id = store.list_recent_memories(&dek, 1).unwrap()[0].id.clone();
        store.put_memory_vector(&mem_id, &[1.0, 2.0], "m", 0).unwrap();
        assert_eq!(store.memory_vector_count().unwrap(), 1);
        store.delete_memory_by_id(&mem_id).unwrap();
        assert_eq!(
            store.memory_vector_count().unwrap(),
            0,
            "FK ON DELETE CASCADE must remove the memory vector"
        );
    }

    #[test]
    fn list_all_returns_every_row() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        for i in 0..3 {
            store
                .insert_memory(&dek, "episodic", 0, 100, &[format!("h{i}")], "s", "m", 0)
                .unwrap();
        }
        let mems = store.list_recent_memories(&dek, 10).unwrap();
        for m in &mems {
            store.put_memory_vector(&m.id, &[0.5, 0.5], "m", 0).unwrap();
        }
        assert_eq!(store.list_all_memory_vectors().unwrap().len(), 3);
    }
}
