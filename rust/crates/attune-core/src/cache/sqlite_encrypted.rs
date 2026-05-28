//! L2 SQLite-backed cache with AES-256-GCM payload encryption (via the vault
//! DEK).
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md`
//! §3 — `llm_cache` table, response BLOB encrypted at rest.
//!
//! Scope semantics:
//! - `Llm`    → encrypted llm_cache table (response may contain user data).
//! - `Embed`  → bypass (plain f16 vectors are not PII; use `Store::embed_cache_*`
//!   directly when you want L2 embed cache).
//! - `Search` → bypass (web_search_cache is its own module with its own
//!   encryption decision).
//! - `All`    → routes to llm only for get/put (those are the only encrypted
//!   entries this backend owns); clear/count delegate to the store
//!   which understands the All semantics.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::cache::{CacheBackend, CacheScope, CachedValue};
use crate::crypto::{self, Key32};
use crate::store::Store;

/// L2 backend persisting LLM cache to SQLite with AES-256-GCM payload
/// encryption keyed by the vault DEK.
///
/// Store access is serialized through a `Mutex` because `rusqlite::Connection`
/// is `Send` but not `Sync` (it holds `RefCell<InnerConnection>` internally).
/// All CacheBackend methods hold the mutex synchronously without crossing an
/// `.await`, so contention is bounded by the time of a single SQL statement.
pub struct SqliteEncryptedCache {
    store: Arc<Mutex<Store>>,
    dek: Key32,
}

impl SqliteEncryptedCache {
    /// Construct a new backend. `dek` is held for the lifetime of the cache;
    /// callers should pass a cloned DEK from the unlocked vault.
    pub fn new(store: Arc<Mutex<Store>>, dek: Key32) -> Self {
        Self { store, dek }
    }
}

#[async_trait]
impl CacheBackend for SqliteEncryptedCache {
    async fn get(&self, scope: CacheScope, key: &str) -> Option<CachedValue> {
        if !matches!(scope, CacheScope::Llm) {
            // Embed cache stores plain f16 vectors (not PII); search cache is
            // its own module. This backend only encrypts LLM responses.
            return None;
        }
        let raw = {
            let g = self.store.lock().ok()?;
            g.llm_cache_get(key).ok().flatten()?
        };
        // Decryption failure → treat as miss + log warning. A corrupt blob
        // should not crash the call path (spec §7.3 graceful degradation).
        let pt = match crypto::decrypt(&self.dek, &raw.bytes) {
            Ok(pt) => pt,
            Err(e) => {
                log::warn!(
                    "L2 cache decryption failed for key={} model={}: {}",
                    key, raw.model, e
                );
                return None;
            }
        };
        Some(CachedValue {
            bytes: pt,
            tokens_in: raw.tokens_in,
            tokens_out: raw.tokens_out,
            model: raw.model,
        })
    }

    async fn put(&self, scope: CacheScope, key: &str, value: CachedValue, _ttl: Option<u32>) {
        if !matches!(scope, CacheScope::Llm) {
            return;
        }
        // Encrypt the payload before persisting. On failure, log + skip put —
        // spec §7.1 cache-encryption-failed is internal telemetry, must not
        // bubble up as an error since the upstream call has already succeeded.
        let ct = match crypto::encrypt(&self.dek, &value.bytes) {
            Ok(ct) => ct,
            Err(e) => {
                log::warn!("L2 cache encryption failed for key={}: {}", key, e);
                return;
            }
        };
        let encrypted = CachedValue {
            bytes: ct,
            ..value
        };
        let Ok(g) = self.store.lock() else {
            log::warn!("L2 cache: store mutex poisoned, skipping put for key={}", key);
            return;
        };
        if let Err(e) = g.llm_cache_put(key, &encrypted) {
            log::warn!("L2 cache persist failed for key={}: {}", key, e);
        }
    }

    async fn clear(&self, scope: CacheScope) -> usize {
        // Delegate to the store — it already understands the All scope (sums
        // llm + embed + search) and we don't want to fork that logic here.
        let Ok(g) = self.store.lock() else { return 0 };
        g.cache_clear_scope(scope).unwrap_or(0)
    }

    async fn count(&self, scope: CacheScope) -> usize {
        let Ok(g) = self.store.lock() else { return 0 };
        g.cache_count(scope).unwrap_or(0)
    }
}
