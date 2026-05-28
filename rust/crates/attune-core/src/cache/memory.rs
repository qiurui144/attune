//! L1 in-memory LRU cache backend.
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md`
//! §3 (Cache layers) — default capacity 512 entries / 64 MB per scope, LRU
//! eviction, process-lifetime only.
//!
//! Capacity is **per scope** (LLM / Embed / Search hold independent LRUs)
//! so a flood of search-result caches cannot evict still-warm LLM responses.

use std::num::NonZeroUsize;
use std::sync::Mutex;

use async_trait::async_trait;
use lru::LruCache;

use crate::cache::{CacheBackend, CacheScope, CachedValue};

/// In-process LRU cache. `cap_per_scope` is enforced separately for each of
/// LLM / Embed / Search; `All` is a query-only logical view.
pub struct MemoryLruCache {
    llm: Mutex<LruCache<String, CachedValue>>,
    embed: Mutex<LruCache<String, CachedValue>>,
    search: Mutex<LruCache<String, CachedValue>>,
}

impl MemoryLruCache {
    /// Create a new LRU. `cap_per_scope` is clamped to at least 1 to satisfy
    /// `NonZeroUsize`.
    pub fn new(cap_per_scope: usize) -> Self {
        let cap = NonZeroUsize::new(cap_per_scope.max(1)).expect("clamped to >= 1");
        Self {
            llm: Mutex::new(LruCache::new(cap)),
            embed: Mutex::new(LruCache::new(cap)),
            search: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Return the per-scope mutex. `CacheScope::All` returns `None` because
    /// it has no single backing store — callers must fan out.
    fn lock(&self, scope: CacheScope) -> Option<&Mutex<LruCache<String, CachedValue>>> {
        match scope {
            CacheScope::Llm => Some(&self.llm),
            CacheScope::Embed => Some(&self.embed),
            CacheScope::Search => Some(&self.search),
            CacheScope::All => None,
        }
    }
}

#[async_trait]
impl CacheBackend for MemoryLruCache {
    async fn get(&self, scope: CacheScope, key: &str) -> Option<CachedValue> {
        let m = self.lock(scope)?;
        // `LruCache::get` is `&mut` (it bumps recency), hence we lock the
        // mutex even on a read path. A poisoned lock means the cache is in
        // an unknown state — treat as miss rather than panic.
        let mut guard = m.lock().ok()?;
        guard.get(key).cloned()
    }

    async fn put(&self, scope: CacheScope, key: &str, value: CachedValue, _ttl: Option<u32>) {
        // TTL is not honored by the L1 LRU (eviction is recency-based only).
        // L2 SqliteEncryptedCache honors TTL via the retention worker.
        if let Some(m) = self.lock(scope) {
            if let Ok(mut g) = m.lock() {
                g.put(key.to_string(), value);
            }
        }
    }

    async fn clear(&self, scope: CacheScope) -> usize {
        match scope {
            CacheScope::All => {
                // Fan out instead of cross-locking three mutexes at once
                // (each lock is brief; the inner blocks are not held across
                // await points by `Send` rules).
                let mut total = 0;
                for s in [CacheScope::Llm, CacheScope::Embed, CacheScope::Search] {
                    if let Some(m) = self.lock(s) {
                        if let Ok(mut g) = m.lock() {
                            total += g.len();
                            g.clear();
                        }
                    }
                }
                total
            }
            other => {
                if let Some(m) = self.lock(other) {
                    if let Ok(mut g) = m.lock() {
                        let n = g.len();
                        g.clear();
                        return n;
                    }
                }
                0
            }
        }
    }

    async fn count(&self, scope: CacheScope) -> usize {
        match scope {
            CacheScope::All => {
                let mut total = 0;
                for s in [CacheScope::Llm, CacheScope::Embed, CacheScope::Search] {
                    if let Some(m) = self.lock(s) {
                        if let Ok(g) = m.lock() {
                            total += g.len();
                        }
                    }
                }
                total
            }
            other => self
                .lock(other)
                .and_then(|m| m.lock().ok())
                .map(|g| g.len())
                .unwrap_or(0),
        }
    }
}
