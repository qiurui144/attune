//! Memory retrieval — vector search over L2/L3 memory summaries.
//!
//! [`MemoryVectorIndex`] is a small `usearch` index dedicated to memories (hundreds
//! of rows, not millions — kept separate from the document index so memory ranking
//! never pollutes document search, per plan §8 decision 5). [`search_memories`]
//! embeds a query and ranks live (non-cold) memories of a given kind.

use std::collections::HashMap;

use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

use crate::crypto::Key32;
use crate::embed::EmbeddingProvider;
use crate::error::{Result, VaultError};
use crate::search::TimeFilter;
use crate::store::MemoryRow;
use crate::store::Store;

/// One ranked memory retrieval result.
#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub memory: MemoryRow,
    /// cosine similarity in [0, 1].
    pub score: f32,
}

/// Small dedicated usearch index mapping `memory_id` ↔ embedding.
///
/// Unlike the document `VectorIndex` (keyed by item+chunk), this index's unit is a
/// whole memory. Rebuilt cheaply at startup from `memory_vectors`.
pub struct MemoryVectorIndex {
    index: usearch::Index,
    /// usearch integer key → memory_id.
    id_by_key: HashMap<u64, String>,
    /// memory_id → usearch key (for removal / dedup).
    key_by_id: HashMap<String, u64>,
    next_key: u64,
    dims: usize,
}

impl MemoryVectorIndex {
    pub fn new(dims: usize) -> Result<Self> {
        let options = IndexOptions {
            dimensions: dims.max(1),
            metric: MetricKind::Cos,
            quantization: ScalarKind::F16,
            ..Default::default()
        };
        let index = usearch::new_index(&options)
            .map_err(|e| VaultError::Crypto(format!("memory usearch init: {e}")))?;
        index
            .reserve(1024)
            .map_err(|e| VaultError::Crypto(format!("memory usearch reserve: {e}")))?;
        Ok(Self {
            index,
            id_by_key: HashMap::new(),
            key_by_id: HashMap::new(),
            next_key: 0,
            dims,
        })
    }

    pub fn dims(&self) -> usize {
        self.dims
    }

    pub fn len(&self) -> usize {
        self.index.size()
    }

    pub fn is_empty(&self) -> bool {
        self.index.size() == 0
    }

    /// Insert or replace a memory's embedding. Dimension-mismatched vectors are
    /// skipped (graceful degrade on embedding-model switch, per plan §8 decision 6).
    pub fn upsert(&mut self, memory_id: &str, vector: &[f32]) -> Result<bool> {
        if vector.len() != self.dims {
            // Skip silently — the memory is simply not vector-retrievable until
            // re-embedded with the active model.
            return Ok(false);
        }
        if self.index.capacity() <= self.index.size() {
            let new_cap = (self.index.capacity() * 2).max(1024);
            self.index
                .reserve(new_cap)
                .map_err(|e| VaultError::Crypto(format!("memory usearch reserve: {e}")))?;
        }
        if let Some(&old_key) = self.key_by_id.get(memory_id) {
            let _ = self.index.remove(old_key);
            self.id_by_key.remove(&old_key);
        }
        let key = self.next_key;
        self.next_key += 1;
        self.index
            .add(key, vector)
            .map_err(|e| VaultError::Crypto(format!("memory usearch add: {e}")))?;
        self.id_by_key.insert(key, memory_id.to_string());
        self.key_by_id.insert(memory_id.to_string(), key);
        Ok(true)
    }

    /// Remove a memory's embedding (memory deleted / re-clustered away).
    pub fn remove(&mut self, memory_id: &str) -> Result<bool> {
        if let Some(key) = self.key_by_id.remove(memory_id) {
            let _ = self.index.remove(key);
            self.id_by_key.remove(&key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Rank memory ids by cosine similarity to `query_vec`. Returns `(memory_id, score)`.
    pub fn search(&self, query_vec: &[f32], top_k: usize) -> Result<Vec<(String, f32)>> {
        if self.index.size() == 0 || query_vec.len() != self.dims {
            return Ok(vec![]);
        }
        let results = self
            .index
            .search(query_vec, top_k)
            .map_err(|e| VaultError::Crypto(format!("memory usearch search: {e}")))?;
        let mut out = Vec::new();
        for i in 0..results.keys.len() {
            if let Some(id) = self.id_by_key.get(&results.keys[i]) {
                let score = 1.0 - results.distances[i];
                out.push((id.clone(), score));
            }
        }
        Ok(out)
    }

    /// Rebuild from all rows in `memory_vectors`. Mismatched-dimension rows skip.
    pub fn build_from_store(store: &Store, dims: usize) -> Result<Self> {
        let mut idx = Self::new(dims)?;
        for row in store.list_all_memory_vectors()? {
            let _ = idx.upsert(&row.memory_id, &row.embedding);
        }
        Ok(idx)
    }
}

/// Embed `query`, vector-rank live memories of `kind`, optionally filter by time window.
///
/// `kind` is `"episodic"` (L2) or `"semantic"` (L3). Cold memories are always excluded
/// — they remain queryable only via explicit time-travel search elsewhere.
/// Returns at most `top_k` hits sorted by descending score.
#[allow(clippy::too_many_arguments)]
pub fn search_memories(
    store: &Store,
    dek: &Key32,
    index: &MemoryVectorIndex,
    embedder: &dyn EmbeddingProvider,
    query: &str,
    kind: &str,
    time_filter: Option<TimeFilter>,
    top_k: usize,
) -> Result<Vec<MemoryHit>> {
    if index.is_empty() || query.trim().is_empty() {
        return Ok(vec![]);
    }
    let (vecs, _usage) = embedder.embed(&[query])?;
    let query_vec = vecs
        .into_iter()
        .next()
        .ok_or_else(|| VaultError::Crypto("embedder returned no vector".into()))?;

    // Over-fetch: vector hits may include other-kind / out-of-window memories that
    // get filtered out below, so pull a wider candidate set first.
    let raw = index.search(&query_vec, top_k.saturating_mul(4).max(top_k))?;
    if raw.is_empty() {
        return Ok(vec![]);
    }

    // Live (non-cold, non-superseded) memories of the requested kind, by id.
    let live: HashMap<String, MemoryRow> = store
        .list_live_memories(dek, kind, false)?
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();

    let mut hits: Vec<MemoryHit> = Vec::new();
    for (id, score) in raw {
        let Some(mem) = live.get(&id) else { continue };
        if let Some(tf) = time_filter {
            // Episodic windows overlapping the filter range qualify.
            let overlaps = mem.window_start < tf.end_unix && mem.window_end > tf.start_unix;
            if !overlaps {
                continue;
            }
        }
        hits.push(MemoryHit {
            memory: mem.clone(),
            score,
        });
        if hits.len() >= top_k {
            break;
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbeddingProvider;

    fn seed_episodic(store: &Store, dek: &Key32, hash: &str, summary: &str, win: i64) -> String {
        store
            .insert_memory(dek, "episodic", win, win + 86400, &[hash.into()], summary, "m", win)
            .unwrap();
        store
            .list_recent_memories(dek, 100)
            .unwrap()
            .into_iter()
            .find(|m| m.source_chunk_hashes == vec![hash])
            .unwrap()
            .id
    }

    #[test]
    fn empty_index_returns_empty() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let idx = MemoryVectorIndex::new(64).unwrap();
        let emb = MockEmbeddingProvider::new(64);
        let r = search_memories(&store, &dek, &idx, &emb, "anything", "episodic", None, 5).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn search_ranks_by_relevance() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let emb = MockEmbeddingProvider::new(128);
        let mut idx = MemoryVectorIndex::new(128).unwrap();

        let id_rust = seed_episodic(&store, &dek, "h1", "用户研究了 Rust 所有权与借用", 1000);
        let id_cook = seed_episodic(&store, &dek, "h2", "用户学习了川菜的烹饪技巧", 2000);
        let id_async = seed_episodic(&store, &dek, "h3", "用户研究了 Rust async 运行时", 3000);
        for (id, txt) in [
            (&id_rust, "用户研究了 Rust 所有权与借用"),
            (&id_cook, "用户学习了川菜的烹饪技巧"),
            (&id_async, "用户研究了 Rust async 运行时"),
        ] {
            let v = emb.embed(&[txt]).unwrap().0.pop().unwrap();
            idx.upsert(id, &v).unwrap();
        }

        let hits = search_memories(&store, &dek, &idx, &emb, "Rust 研究", "episodic", None, 3).unwrap();
        assert!(!hits.is_empty());
        // 烹饪记忆不该排第一
        assert_ne!(hits[0].memory.id, id_cook);
    }

    #[test]
    fn time_filter_excludes_out_of_window() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let emb = MockEmbeddingProvider::new(64);
        let mut idx = MemoryVectorIndex::new(64).unwrap();
        let id = seed_episodic(&store, &dek, "h1", "用户研究了 Rust", 1000);
        let v = emb.embed(&["用户研究了 Rust"]).unwrap().0.pop().unwrap();
        idx.upsert(&id, &v).unwrap();

        // 窗口完全在 episodic window (1000..87400) 之外
        let tf = TimeFilter { start_unix: 500_000, end_unix: 600_000 };
        let hits = search_memories(&store, &dek, &idx, &emb, "Rust", "episodic", Some(tf), 5).unwrap();
        assert!(hits.is_empty(), "out-of-window memory must be excluded");
    }

    #[test]
    fn cold_memory_excluded() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let emb = MockEmbeddingProvider::new(64);
        let mut idx = MemoryVectorIndex::new(64).unwrap();
        let day = 86400;
        let id = seed_episodic(&store, &dek, "old", "用户研究了 Rust", day);
        let v = emb.embed(&["用户研究了 Rust"]).unwrap().0.pop().unwrap();
        idx.upsert(&id, &v).unwrap();
        // 加一条 semantic 覆盖 + demote
        store
            .insert_semantic_memory(&dek, "t", &["old".into()], "sem", "m", 0, day, 10 * day)
            .unwrap();
        store.demote_cold_memories(400 * day, 180 * day).unwrap();

        let hits = search_memories(&store, &dek, &idx, &emb, "Rust", "episodic", None, 5).unwrap();
        assert!(hits.is_empty(), "cold memory must be excluded from default retrieval");
    }

    #[test]
    fn dimension_mismatch_skips_silently() {
        let mut idx = MemoryVectorIndex::new(64).unwrap();
        // 维度不符 → upsert 返回 false，不 panic
        let added = idx.upsert("m1", &[1.0, 2.0, 3.0]).unwrap();
        assert!(!added);
        assert!(idx.is_empty());
    }

    #[test]
    fn upsert_replaces_existing() {
        let mut idx = MemoryVectorIndex::new(4).unwrap();
        idx.upsert("m1", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("m1", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        assert_eq!(idx.len(), 1, "re-embedding same memory must not duplicate");
    }
}
