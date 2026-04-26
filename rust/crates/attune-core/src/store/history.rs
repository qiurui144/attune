//! search_history / click_events / feedback — 用户搜索行为追踪。
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    // --- search_history / click_events ---

    /// 记录一次搜索（query 加密存储）
    pub fn log_search(&self, dek: &Key32, query: &str, result_count: usize) -> Result<()> {
        let encrypted = crypto::encrypt(dek, query.as_bytes())?;
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO search_history (query, result_count, created_at) VALUES (?1, ?2, ?3)",
            params![encrypted, result_count as i64, now],
        )?;
        Ok(())
    }

    /// 列出最近搜索历史（解密 query，按时间倒序）
    pub fn recent_searches(&self, dek: &Key32, limit: usize) -> Result<Vec<SearchHistoryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, query, result_count, created_at FROM search_history
             ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            let (id, encrypted_query, count, created_at) = row?;
            let decrypted = match crypto::decrypt(dek, &encrypted_query) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("recent_searches: decrypt failed for row {id}: {e}");
                    continue;
                }
            };
            let query = String::from_utf8(decrypted)
                .map_err(|e| VaultError::Crypto(format!("search history utf8: {e}")))?;
            results.push(SearchHistoryRow {
                id,
                query,
                result_count: count as usize,
                created_at,
            });
        }
        Ok(results)
    }

    /// 记录一次点击（query + item_id）
    pub fn log_click(&self, dek: &Key32, query: &str, item_id: &str) -> Result<()> {
        let encrypted = crypto::encrypt(dek, query.as_bytes())?;
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO click_events (query, item_id, created_at) VALUES (?1, ?2, ?3)",
            params![encrypted, item_id, now],
        )?;
        Ok(())
    }

    /// 统计最常点击的 item_id（降序）
    pub fn popular_items(&self, limit: usize) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT item_id, COUNT(*) as cnt FROM click_events
             GROUP BY item_id ORDER BY cnt DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}
