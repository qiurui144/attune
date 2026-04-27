//! C1 Web search local cache（W3 batch A，2026-04-27）。
//!
//! per spec `docs/superpowers/specs/2026-04-27-w3-batch-a-design.md` §3
//! per ACKNOWLEDGMENTS.md C 系列 — 吴师兄 §6 高频 query 缓存模式。
//!
//! 设计：query_hash = SHA-256(query) 作主键；query_text + results JSON 加密；
//! 30 天默认 TTL，过期由查询时过滤（惰性 GC，与 chunk_summaries 一致）。

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;
use crate::web_search::WebSearchResult;

/// 默认 30 天 TTL。
pub const DEFAULT_TTL_SECS: i64 = 30 * 24 * 3600;

fn hash_query(query: &str) -> String {
    let mut h = Sha256::new();
    h.update(query.as_bytes());
    hex::encode(h.finalize())
}

impl Store {
    /// 命中且未过期 → Some(results)；缺失 / 过期 / 解密失败 → None。
    /// 不主动 GC 过期行 — 下次 put 同 query 时 INSERT OR REPLACE 自动覆盖。
    pub fn get_web_search_cached(
        &self,
        dek: &Key32,
        query: &str,
        now_secs: i64,
    ) -> Result<Option<Vec<WebSearchResult>>> {
        let key = hash_query(query);
        let row: Option<(Vec<u8>, i64, i64)> = self
            .conn
            .query_row(
                "SELECT results_json_enc, created_at_secs, ttl_secs \
                 FROM web_search_cache WHERE query_hash = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((enc, created, ttl)) = row else {
            return Ok(None);
        };
        if now_secs - created > ttl {
            return Ok(None); // 过期视同 miss
        }
        let plain = match crypto::decrypt(dek, &enc) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("C1 web cache decrypt failed for key={key}: {e}");
                return Ok(None);
            }
        };
        match serde_json::from_slice::<Vec<WebSearchResult>>(&plain) {
            Ok(results) => Ok(Some(results)),
            Err(e) => {
                log::warn!("C1 web cache deserialize failed: {e}");
                Ok(None)
            }
        }
    }

    /// 写入 / 覆盖缓存（INSERT OR REPLACE）。
    pub fn put_web_search_cached(
        &self,
        dek: &Key32,
        query: &str,
        results: &[WebSearchResult],
        ttl_secs: i64,
        now_secs: i64,
    ) -> Result<()> {
        let key = hash_query(query);
        let json = serde_json::to_vec(results)
            .map_err(|e| crate::error::VaultError::InvalidInput(format!("json: {e}")))?;
        let json_enc = crypto::encrypt(dek, &json)?;
        let query_enc = crypto::encrypt(dek, query.as_bytes())?;
        self.conn.execute(
            "INSERT OR REPLACE INTO web_search_cache \
                (query_hash, query_text_enc, results_json_enc, created_at_secs, ttl_secs) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, query_enc, json_enc, now_secs, ttl_secs],
        )?;
        Ok(())
    }

    /// 总数（不过滤过期）— 诊断用。
    pub fn web_search_cache_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM web_search_cache", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// 显式清空（用户在 Settings 点 "清空 web 缓存"）。返回删除条数。
    pub fn clear_web_search_cache(&self) -> Result<usize> {
        let n = self
            .conn
            .execute("DELETE FROM web_search_cache", [])?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;
    use crate::web_search::WebSearchResult;

    fn sample_results() -> Vec<WebSearchResult> {
        vec![
            WebSearchResult {
                title: "Rust ownership".into(),
                url: "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html".into(),
                snippet: "Ownership is Rust's most unique feature...".into(),
                published_date: None,
            },
            WebSearchResult {
                title: "Borrow checker".into(),
                url: "https://example.com/borrow".into(),
                snippet: "The borrow checker enforces...".into(),
                published_date: Some("2024-01-15".into()),
            },
        ]
    }

    #[test]
    fn miss_returns_none() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let r = store
            .get_web_search_cached(&dek, "未缓存的 query", 1000)
            .unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn put_then_hit_round_trips() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let results = sample_results();
        store
            .put_web_search_cached(&dek, "rust ownership", &results, DEFAULT_TTL_SECS, 1000)
            .unwrap();
        let hit = store
            .get_web_search_cached(&dek, "rust ownership", 2000)
            .unwrap()
            .expect("应命中");
        assert_eq!(hit.len(), 2);
        assert_eq!(hit[0].title, "Rust ownership");
        assert_eq!(hit[1].published_date.as_deref(), Some("2024-01-15"));
    }

    #[test]
    fn expired_returns_none() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let results = sample_results();
        // TTL = 100 秒，put 时刻 t=1000
        store
            .put_web_search_cached(&dek, "expiring query", &results, 100, 1000)
            .unwrap();
        // 查询时刻 t=1099（差 99 秒，未过期）
        assert!(store
            .get_web_search_cached(&dek, "expiring query", 1099)
            .unwrap()
            .is_some());
        // 查询时刻 t=1101（差 101 秒，已过期）
        assert!(store
            .get_web_search_cached(&dek, "expiring query", 1101)
            .unwrap()
            .is_none());
    }

    #[test]
    fn different_queries_dont_collide() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .put_web_search_cached(&dek, "query A", &sample_results(), DEFAULT_TTL_SECS, 1000)
            .unwrap();
        store
            .put_web_search_cached(
                &dek,
                "query B",
                &vec![WebSearchResult {
                    title: "B 专属".into(),
                    url: "https://b.com".into(),
                    snippet: "different".into(),
                    published_date: None,
                }],
                DEFAULT_TTL_SECS,
                1000,
            )
            .unwrap();
        let a = store.get_web_search_cached(&dek, "query A", 1500).unwrap().unwrap();
        let b = store.get_web_search_cached(&dek, "query B", 1500).unwrap().unwrap();
        assert_eq!(a[0].title, "Rust ownership");
        assert_eq!(b[0].title, "B 专属");
        assert_eq!(store.web_search_cache_count().unwrap(), 2);
    }

    #[test]
    fn put_same_query_overwrites() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .put_web_search_cached(&dek, "q", &sample_results(), DEFAULT_TTL_SECS, 1000)
            .unwrap();
        // 二次 put 用新结果
        let new_results = vec![WebSearchResult {
            title: "新结果".into(),
            url: "https://new.com".into(),
            snippet: "覆盖".into(),
            published_date: None,
        }];
        store
            .put_web_search_cached(&dek, "q", &new_results, DEFAULT_TTL_SECS, 2000)
            .unwrap();
        let hit = store.get_web_search_cached(&dek, "q", 3000).unwrap().unwrap();
        assert_eq!(hit.len(), 1, "覆盖后只剩 1 条");
        assert_eq!(hit[0].title, "新结果");
        assert_eq!(store.web_search_cache_count().unwrap(), 1, "PRIMARY KEY 保证不重复");
    }

    #[test]
    fn results_encrypted_at_rest() {
        // 验证 results_json_enc 不是明文 JSON
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .put_web_search_cached(&dek, "q", &sample_results(), DEFAULT_TTL_SECS, 1000)
            .unwrap();
        let raw: Vec<u8> = store
            .conn
            .query_row(
                "SELECT results_json_enc FROM web_search_cache WHERE query_hash = ?1",
                params![hash_query("q")],
                |r| r.get(0),
            )
            .unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(!raw_str.contains("Rust ownership"), "标题应加密，不出现在原始 blob 中");
    }

    #[test]
    fn clear_returns_deleted_count() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store.put_web_search_cached(&dek, "a", &sample_results(), DEFAULT_TTL_SECS, 1000).unwrap();
        store.put_web_search_cached(&dek, "b", &sample_results(), DEFAULT_TTL_SECS, 1000).unwrap();
        store.put_web_search_cached(&dek, "c", &sample_results(), DEFAULT_TTL_SECS, 1000).unwrap();
        let n = store.clear_web_search_cache().unwrap();
        assert_eq!(n, 3);
        assert_eq!(store.web_search_cache_count().unwrap(), 0);
    }
}
