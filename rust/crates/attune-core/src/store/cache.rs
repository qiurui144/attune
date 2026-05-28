//! Cache CRUD — backing the `/api/v1/cache/*` REST surface.
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md`
//! §3 (DB schema) + §5.2 (REST endpoints).
//!
//! Encryption of LLM cache payload is the responsibility of the
//! `cache::sqlite_encrypted` backend (Task F); this layer stores raw bytes.

use rusqlite::params;

use crate::cache::{CacheScope, CachedValue};
use crate::error::Result;
use crate::store::Store;

impl Store {
    /// Look up an LLM cache entry. On hit, also bumps `last_hit_ts` and
    /// `hit_count` (best-effort — errors during bump are swallowed so a stale
    /// counter never breaks a hit).
    pub fn llm_cache_get(&self, key: &str) -> Result<Option<CachedValue>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT model, response, tokens_in, tokens_out FROM llm_cache WHERE key = ?1",
        )?;
        let row = stmt
            .query_row(params![key], |r| {
                Ok(CachedValue {
                    model: r.get(0)?,
                    bytes: r.get(1)?,
                    tokens_in: r.get::<_, i64>(2)? as u32,
                    tokens_out: r.get::<_, i64>(3)? as u32,
                })
            })
            .ok();
        if row.is_some() {
            let now = chrono::Utc::now().timestamp_millis();
            // Best-effort LRU bookkeeping — never fail a cache hit on bump errors.
            let _ = self.conn.execute(
                "UPDATE llm_cache SET last_hit_ts=?1, hit_count=hit_count+1 WHERE key=?2",
                params![now, key],
            );
        }
        Ok(row)
    }

    /// Insert or replace an LLM cache entry.
    pub fn llm_cache_put(&self, key: &str, value: &CachedValue) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        self.conn.execute(
            "INSERT OR REPLACE INTO llm_cache
             (key, model, response, tokens_in, tokens_out, created_ts, last_hit_ts, hit_count)
             VALUES (?1,?2,?3,?4,?5,?6,?7,0)",
            params![key, value.model, value.bytes, value.tokens_in, value.tokens_out, now, now],
        )?;
        Ok(())
    }

    /// Look up an embedding cache entry. Vectors are not PII so stored plain
    /// (no decryption layer needed).
    pub fn embed_cache_get(&self, key: &str) -> Result<Option<CachedValue>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT model, vector FROM embed_cache WHERE key = ?1",
        )?;
        let row = stmt
            .query_row(params![key], |r| {
                Ok(CachedValue {
                    model: r.get(0)?,
                    bytes: r.get(1)?,
                    tokens_in: 0,
                    tokens_out: 0,
                })
            })
            .ok();
        if row.is_some() {
            let now = chrono::Utc::now().timestamp_millis();
            let _ = self.conn.execute(
                "UPDATE embed_cache SET last_hit_ts=?1 WHERE key=?2",
                params![now, key],
            );
        }
        Ok(row)
    }

    /// Insert or replace an embedding cache entry. `dim` is derived from
    /// `value.bytes.len() / 2` for f16 quantization.
    pub fn embed_cache_put(&self, key: &str, value: &CachedValue, dim: u32) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        self.conn.execute(
            "INSERT OR REPLACE INTO embed_cache
             (key, model, vector, dim, created_ts, last_hit_ts)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![key, value.model, value.bytes, dim, now, now],
        )?;
        Ok(())
    }

    /// Count entries in the given scope. `All` returns the sum across
    /// llm / embed / search.
    pub fn cache_count(&self, scope: CacheScope) -> Result<usize> {
        match scope {
            CacheScope::Llm => Self::count_table(&self.conn, "llm_cache"),
            CacheScope::Embed => Self::count_table(&self.conn, "embed_cache"),
            CacheScope::Search => Self::count_table(&self.conn, "web_search_cache"),
            CacheScope::All => Ok(self.cache_count(CacheScope::Llm)?
                + self.cache_count(CacheScope::Embed)?
                + self.cache_count(CacheScope::Search)?),
        }
    }

    fn count_table(conn: &rusqlite::Connection, table: &str) -> Result<usize> {
        let sql = format!("SELECT count(*) FROM {table}");
        let n: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Delete all entries in the given scope. `All` clears llm / embed / search
    /// together. Used by `DELETE /api/v1/cache/{scope}`.
    pub fn cache_clear_scope(&self, scope: CacheScope) -> Result<usize> {
        match scope {
            CacheScope::Llm => Ok(self.conn.execute("DELETE FROM llm_cache", [])?),
            CacheScope::Embed => Ok(self.conn.execute("DELETE FROM embed_cache", [])?),
            CacheScope::Search => Ok(self.conn.execute("DELETE FROM web_search_cache", [])?),
            CacheScope::All => Ok(self.cache_clear_scope(CacheScope::Llm)?
                + self.cache_clear_scope(CacheScope::Embed)?
                + self.cache_clear_scope(CacheScope::Search)?),
        }
    }
}

#[cfg(test)]
mod cache_test {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fresh_vault_has_llm_cache_table() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("v.db")).unwrap();
        let conn = store.raw_connection_for_test();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='llm_cache'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn fresh_vault_has_embed_cache_table() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("v.db")).unwrap();
        let conn = store.raw_connection_for_test();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='embed_cache'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn llm_cache_get_miss_returns_none() {
        let store = Store::open_memory().unwrap();
        assert!(store.llm_cache_get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn llm_cache_put_then_get_round_trip() {
        let store = Store::open_memory().unwrap();
        let v = CachedValue {
            bytes: b"cached response".to_vec(),
            tokens_in: 42,
            tokens_out: 13,
            model: "gpt-4o-mini".into(),
        };
        store.llm_cache_put("k1", &v).unwrap();
        let got = store.llm_cache_get("k1").unwrap().expect("hit");
        assert_eq!(got.bytes, b"cached response");
        assert_eq!(got.tokens_in, 42);
        assert_eq!(got.tokens_out, 13);
        assert_eq!(got.model, "gpt-4o-mini");
    }

    #[test]
    fn llm_cache_get_bumps_hit_count() {
        let store = Store::open_memory().unwrap();
        let v = CachedValue {
            bytes: b"x".to_vec(),
            tokens_in: 1,
            tokens_out: 1,
            model: "m".into(),
        };
        store.llm_cache_put("k", &v).unwrap();
        store.llm_cache_get("k").unwrap();
        store.llm_cache_get("k").unwrap();
        store.llm_cache_get("k").unwrap();

        let conn = store.raw_connection_for_test();
        let hits: i64 = conn
            .query_row("SELECT hit_count FROM llm_cache WHERE key='k'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hits, 3);
    }

    #[test]
    fn llm_cache_put_replaces_existing() {
        let store = Store::open_memory().unwrap();
        let v1 = CachedValue {
            bytes: b"old".to_vec(),
            tokens_in: 1,
            tokens_out: 1,
            model: "m".into(),
        };
        let v2 = CachedValue {
            bytes: b"new".to_vec(),
            tokens_in: 9,
            tokens_out: 9,
            model: "m".into(),
        };
        store.llm_cache_put("k", &v1).unwrap();
        store.llm_cache_put("k", &v2).unwrap();
        let got = store.llm_cache_get("k").unwrap().unwrap();
        assert_eq!(got.bytes, b"new");
        assert_eq!(got.tokens_in, 9);
    }

    #[test]
    fn embed_cache_round_trip() {
        let store = Store::open_memory().unwrap();
        let v = CachedValue {
            bytes: vec![0u8; 1536], // 768-dim f16
            tokens_in: 0,
            tokens_out: 0,
            model: "bge-m3".into(),
        };
        store.embed_cache_put("e1", &v, 768).unwrap();
        let got = store.embed_cache_get("e1").unwrap().expect("hit");
        assert_eq!(got.bytes.len(), 1536);
        assert_eq!(got.model, "bge-m3");
    }

    #[test]
    fn cache_count_per_scope() {
        let store = Store::open_memory().unwrap();
        let v = CachedValue {
            bytes: b"x".to_vec(),
            tokens_in: 1,
            tokens_out: 1,
            model: "m".into(),
        };
        store.llm_cache_put("a", &v).unwrap();
        store.llm_cache_put("b", &v).unwrap();
        store.embed_cache_put("e1", &v, 64).unwrap();

        assert_eq!(store.cache_count(CacheScope::Llm).unwrap(), 2);
        assert_eq!(store.cache_count(CacheScope::Embed).unwrap(), 1);
        // search may be 0 — web_search_cache table is independent
        let search_count = store.cache_count(CacheScope::Search).unwrap();
        assert_eq!(
            store.cache_count(CacheScope::All).unwrap(),
            3 + search_count,
            "All must sum llm + embed + search"
        );
    }

    #[test]
    fn cache_clear_scope_llm_only() {
        let store = Store::open_memory().unwrap();
        let v = CachedValue {
            bytes: b"x".to_vec(),
            tokens_in: 1,
            tokens_out: 1,
            model: "m".into(),
        };
        store.llm_cache_put("a", &v).unwrap();
        store.llm_cache_put("b", &v).unwrap();
        store.embed_cache_put("e", &v, 64).unwrap();

        let removed = store.cache_clear_scope(CacheScope::Llm).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(store.cache_count(CacheScope::Llm).unwrap(), 0);
        assert_eq!(
            store.cache_count(CacheScope::Embed).unwrap(),
            1,
            "embed cache should be untouched"
        );
    }

    #[test]
    fn cache_clear_scope_all_clears_everything() {
        let store = Store::open_memory().unwrap();
        let v = CachedValue {
            bytes: b"x".to_vec(),
            tokens_in: 1,
            tokens_out: 1,
            model: "m".into(),
        };
        store.llm_cache_put("a", &v).unwrap();
        store.embed_cache_put("e", &v, 64).unwrap();

        store.cache_clear_scope(CacheScope::All).unwrap();
        assert_eq!(store.cache_count(CacheScope::Llm).unwrap(), 0);
        assert_eq!(store.cache_count(CacheScope::Embed).unwrap(), 0);
    }
}
