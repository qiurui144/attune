//! Unified cache contract — L1 in-memory + L2 SQLite encrypted.
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md`
//! §5.1 (Rust trait) and §3 (Cache layers L1 → L2 → vendor prompt-cache).
//!
//! Public surface (frozen at Task M):
//! - [`CacheBackend`] async trait — implementable by Redis / disk / remote.
//! - [`CacheScope`] enum — namespaces `llm` / `embed` / `search` / `all`.
//! - [`CachedValue`] — payload + token metadata.
//! - [`cache_key`] — BLAKE3 32-hex prefix derivation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod key;
pub mod memory;
pub mod sqlite_encrypted;

pub use key::cache_key;

#[cfg(test)]
mod tests;

/// Namespace selector for cache operations. `All` is for cross-scope queries
/// such as `GET /api/v1/cache/all` aggregation and `DELETE /api/v1/cache/all`
/// bulk clear (see spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheScope {
    /// LLM chat / extract response cache.
    Llm,
    /// Embedding vector cache.
    Embed,
    /// Web / knowledge-base search result cache.
    Search,
    /// Union of the three above — query / aggregate / bulk-clear only.
    All,
}

/// Cached payload + metadata sufficient to reconstruct a [`UsageEvent`] entry
/// when a hit occurs (so cache-hit telemetry remains comparable to misses).
///
/// [`UsageEvent`]: crate::usage::UsageEvent
#[derive(Debug, Clone)]
pub struct CachedValue {
    /// Opaque payload. For LLM cache this is the response body (will be
    /// AES-256-GCM encrypted at the L2 SQLite layer per spec §3). For embed
    /// cache it is the f16-quantized vector — already PII-safe.
    pub bytes: Vec<u8>,
    /// Vendor-reported input tokens at the time the value was cached.
    pub tokens_in: u32,
    /// Vendor-reported output tokens at the time the value was cached.
    pub tokens_out: u32,
    /// Model identifier used when this value was produced.
    pub model: String,
}

/// Trait every cache backend must implement.
///
/// All methods are async so future backends (Redis, disk-based, distributed)
/// can integrate without changing the contract. Implementations MUST be
/// `Send + Sync` for sharing across tokio tasks.
#[async_trait]
pub trait CacheBackend: Send + Sync {
    /// Look up `key` in the given `scope`. Returns `None` on miss.
    async fn get(&self, scope: CacheScope, key: &str) -> Option<CachedValue>;

    /// Insert `value` at `key` in `scope`. `ttl_secs = None` means "use the
    /// backend default" (typically `settings.cache.retention_days * 86400`).
    async fn put(
        &self,
        scope: CacheScope,
        key: &str,
        value: CachedValue,
        ttl_secs: Option<u32>,
    );

    /// Remove all entries in `scope`. Returns the number of entries removed.
    /// `CacheScope::All` clears every scope.
    async fn clear(&self, scope: CacheScope) -> usize;

    /// Count entries in `scope`. `CacheScope::All` returns the union total.
    async fn count(&self, scope: CacheScope) -> usize;
}
