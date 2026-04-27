//! memories — A1 周期总结的情景记忆（episodic memory）。
//!
//! 见设计稿 `docs/superpowers/specs/2026-04-27-memory-consolidation-design.md`。
//! 幂等性由唯一索引 `uq_memories_source(kind, source_chunk_hashes)` 保证。

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::types::MemoryRow;
use crate::store::Store;

/// 用作 chunk_summaries 的"已 consolidated"扫描所需的最小投影。
pub struct ChunkSummaryHead {
    pub chunk_hash: String,
    pub item_id: String,
    pub created_at_secs: i64,
    pub summary_encrypted: Vec<u8>,
}

impl Store {
    /// 列出 chunk_summaries 表中 (created_at >= since_secs) 的所有 economical 摘要，
    /// 按 created_at 升序。供 consolidation prepare 阶段消费。
    pub fn list_chunk_summaries_for_consolidation(
        &self,
        since_secs: i64,
        limit: usize,
    ) -> Result<Vec<ChunkSummaryHead>> {
        let mut stmt = self.conn.prepare(
            "SELECT chunk_hash, item_id, summary, strftime('%s', created_at) AS ts \
             FROM chunk_summaries \
             WHERE strategy = 'economical' \
               AND CAST(strftime('%s', created_at) AS INTEGER) >= ?1 \
             ORDER BY ts ASC \
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![since_secs, limit as i64], |r| {
                let ts_str: String = r.get(3)?;
                let ts = ts_str.parse::<i64>().unwrap_or(0);
                Ok(ChunkSummaryHead {
                    chunk_hash: r.get(0)?,
                    item_id: r.get(1)?,
                    summary_encrypted: r.get(2)?,
                    created_at_secs: ts,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// 查 (kind, sorted_chunk_hashes) 是否已存在 memory（幂等检查）。
    /// 直接用 unique index 避免误算。
    pub fn memory_exists(&self, kind: &str, sorted_hashes_json: &str) -> Result<bool> {
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM memories WHERE kind = ?1 AND source_chunk_hashes = ?2 LIMIT 1",
                params![kind, sorted_hashes_json],
                |r| r.get(0),
            )
            .optional()?;
        Ok(exists.is_some())
    }

    /// 写入一条 memory。`source_chunk_hashes` 必须**升序排序**（调用方保证）；
    /// 唯一索引会拒绝重复 (kind, hashes_json) 组合 → 返回 0 表示已存在。
    /// 返回 1 = 新增，0 = 已存在跳过。
    #[allow(clippy::too_many_arguments)]
    pub fn insert_memory(
        &self,
        dek: &Key32,
        kind: &str,
        window_start: i64,
        window_end: i64,
        sorted_chunk_hashes: &[String],
        summary: &str,
        model: &str,
        now_secs: i64,
    ) -> Result<usize> {
        if sorted_chunk_hashes.is_empty() {
            return Err(VaultError::InvalidInput(
                "memory must reference at least 1 chunk".into(),
            ));
        }
        let hashes_json = serde_json::to_string(sorted_chunk_hashes)
            .map_err(|e| VaultError::InvalidInput(format!("hashes serialize: {e}")))?;
        let summary_enc = crypto::encrypt(dek, summary.as_bytes())?;
        let id = Uuid::new_v4().to_string();
        let affected = self.conn.execute(
            "INSERT OR IGNORE INTO memories \
                (id, kind, window_start, window_end, source_chunk_hashes, source_chunk_count, \
                 summary_encrypted, model, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                kind,
                window_start,
                window_end,
                hashes_json,
                sorted_chunk_hashes.len() as i64,
                summary_enc,
                model,
                now_secs,
            ],
        )?;
        Ok(affected)
    }

    /// 列出最近 N 条 memory（用于 H5 attune --diag / 未来 chat 检索预览）。
    pub fn list_recent_memories(&self, dek: &Key32, limit: usize) -> Result<Vec<MemoryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, window_start, window_end, source_chunk_hashes, \
                    summary_encrypted, model, created_at \
             FROM memories \
             ORDER BY created_at DESC \
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                let hashes_json: String = r.get(4)?;
                let summary_enc: Vec<u8> = r.get(5)?;
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    hashes_json,
                    summary_enc,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            })?
            .filter_map(|r| r.ok());
        let mut out = Vec::new();
        for (id, kind, window_start, window_end, hashes_json, summary_enc, model, created_at) in rows {
            let summary = crypto::decrypt(dek, &summary_enc)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            let source_chunk_hashes: Vec<String> =
                serde_json::from_str(&hashes_json).unwrap_or_default();
            out.push(MemoryRow {
                id,
                kind,
                window_start,
                window_end,
                source_chunk_hashes,
                summary,
                model,
                created_at,
            });
        }
        Ok(out)
    }

    /// 总数 — 测试 / 诊断用。
    pub fn memory_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;

    #[test]
    fn insert_memory_returns_one_for_new_row() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let hashes = vec!["aaa".to_string(), "bbb".to_string()];
        let n = store
            .insert_memory(&dek, "episodic", 1000, 2000, &hashes, "summary text", "qwen2.5:3b", 5000)
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(store.memory_count().unwrap(), 1);
    }

    #[test]
    fn insert_memory_is_idempotent_on_same_hash_set() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let hashes = vec!["aaa".to_string(), "bbb".to_string()];
        let _ = store
            .insert_memory(&dek, "episodic", 1000, 2000, &hashes, "first", "model", 5000)
            .unwrap();
        // 二次插入相同 (kind, hashes) → INSERT OR IGNORE 返回 0
        let n = store
            .insert_memory(&dek, "episodic", 1000, 2000, &hashes, "second attempt", "model", 9999)
            .unwrap();
        assert_eq!(n, 0, "duplicate insert must be ignored");
        assert_eq!(store.memory_count().unwrap(), 1);
    }

    #[test]
    fn insert_memory_different_hash_set_creates_new_row() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 1000, 2000, &["a".into()], "s1", "m", 100)
            .unwrap();
        store
            .insert_memory(&dek, "episodic", 1000, 2000, &["b".into()], "s2", "m", 200)
            .unwrap();
        assert_eq!(store.memory_count().unwrap(), 2);
    }

    #[test]
    fn insert_memory_rejects_empty_hashes() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let err = store
            .insert_memory(&dek, "episodic", 1000, 2000, &[], "x", "m", 0)
            .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[test]
    fn list_recent_memories_decrypts_correctly() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 1000, 2000, &["h1".into()], "the answer is 42", "qwen2.5:3b", 100)
            .unwrap();
        let rows = store.list_recent_memories(&dek, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].summary, "the answer is 42");
        assert_eq!(rows[0].source_chunk_hashes, vec!["h1"]);
        assert_eq!(rows[0].kind, "episodic");
        assert_eq!(rows[0].model, "qwen2.5:3b");
    }

    #[test]
    fn memory_exists_finds_existing() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let hashes = vec!["x".to_string(), "y".to_string()];
        let json = serde_json::to_string(&hashes).unwrap();
        assert!(!store.memory_exists("episodic", &json).unwrap());
        store
            .insert_memory(&dek, "episodic", 1, 2, &hashes, "s", "m", 100)
            .unwrap();
        assert!(store.memory_exists("episodic", &json).unwrap());
    }
}
