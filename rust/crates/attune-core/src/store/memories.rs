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
                    summary_encrypted, model, created_at, topic_key, cold, superseded_by \
             FROM memories \
             ORDER BY created_at DESC \
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| Self::row_to_memory(r))?
            .filter_map(|r| r.ok());
        let mut out = Vec::new();
        for raw in rows {
            out.push(raw.decrypt(dek));
        }
        Ok(out)
    }

    /// 列出 live（未 superseded、可选排除 cold）的指定 kind memory。
    /// 多层记忆检索的入口 — assembler 经 memory_vectors 排序后用此解密正文。
    pub fn list_live_memories(
        &self,
        dek: &Key32,
        kind: &str,
        include_cold: bool,
    ) -> Result<Vec<MemoryRow>> {
        let sql = if include_cold {
            "SELECT id, kind, window_start, window_end, source_chunk_hashes, \
                    summary_encrypted, model, created_at, topic_key, cold, superseded_by \
             FROM memories \
             WHERE kind = ?1 AND superseded_by IS NULL \
             ORDER BY created_at DESC"
        } else {
            "SELECT id, kind, window_start, window_end, source_chunk_hashes, \
                    summary_encrypted, model, created_at, topic_key, cold, superseded_by \
             FROM memories \
             WHERE kind = ?1 AND superseded_by IS NULL AND cold = 0 \
             ORDER BY created_at DESC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(params![kind], |r| Self::row_to_memory(r))?
            .filter_map(|r| r.ok());
        let mut out = Vec::new();
        for raw in rows {
            out.push(raw.decrypt(dek));
        }
        Ok(out)
    }

    /// 写入一条 semantic (L3) memory。幂等键 = (kind='semantic', topic_key)：
    /// `uq_memories_topic` 部分唯一索引拒绝重复 → 返回 (id, 0) 表示已存在。
    /// 返回 (新行 id, 受影响行数)。
    pub fn insert_semantic_memory(
        &self,
        dek: &Key32,
        topic_key: &str,
        sorted_member_hashes: &[String],
        summary: &str,
        model: &str,
        window_start: i64,
        window_end: i64,
        now_secs: i64,
    ) -> Result<(String, usize)> {
        if sorted_member_hashes.is_empty() {
            return Err(VaultError::InvalidInput(
                "semantic memory must reference at least 1 member".into(),
            ));
        }
        let hashes_json = serde_json::to_string(sorted_member_hashes)
            .map_err(|e| VaultError::InvalidInput(format!("hashes serialize: {e}")))?;
        let summary_enc = crypto::encrypt(dek, summary.as_bytes())?;
        let id = Uuid::new_v4().to_string();
        let affected = self.conn.execute(
            "INSERT OR IGNORE INTO memories \
                (id, kind, window_start, window_end, source_chunk_hashes, source_chunk_count, \
                 summary_encrypted, model, created_at, topic_key, cold, superseded_by) \
             VALUES (?1, 'semantic', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, NULL)",
            params![
                id,
                window_start,
                window_end,
                hashes_json,
                sorted_member_hashes.len() as i64,
                summary_enc,
                model,
                now_secs,
                topic_key,
            ],
        )?;
        Ok((id, affected))
    }

    /// 把旧 semantic 行标记为被 `new_id` 取代（L3 refresh）。live 检索随即排除旧行。
    pub fn mark_memory_superseded(&self, old_id: &str, new_id: &str) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE memories SET superseded_by = ?2 WHERE id = ?1 AND superseded_by IS NULL",
            params![old_id, new_id],
        )?;
        Ok(n)
    }

    /// 把"足够老 + 已被某 L3 行覆盖"的 episodic 行降级为 cold（纯 SQL，零 LLM）。
    ///
    /// 覆盖判定：存在一条 live 的 semantic memory，其 created_at 晚于该 episodic 行的
    /// window_end —— 即该时间段的内容已被卷入更晚生成的语义层。
    /// 返回本次新降级的行数。
    pub fn demote_cold_memories(&self, now_secs: i64, cold_age_secs: i64) -> Result<usize> {
        let cutoff = now_secs - cold_age_secs;
        let n = self.conn.execute(
            "UPDATE memories SET cold = 1 \
             WHERE kind = 'episodic' AND cold = 0 AND window_end < ?1 \
               AND EXISTS ( \
                   SELECT 1 FROM memories s \
                   WHERE s.kind = 'semantic' AND s.superseded_by IS NULL \
                     AND s.created_at >= memories.window_end \
               )",
            params![cutoff],
        )?;
        Ok(n)
    }

    /// 显式删除一条 memory（同时级联删除 memory_vectors）。
    pub fn delete_memory_by_id(&self, memory_id: &str) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM memories WHERE id = ?1",
            params![memory_id],
        )?;
        Ok(n)
    }

    /// 总数 — 测试 / 诊断用。
    pub fn memory_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// 按 kind 统计 live（非 superseded）行数 — `attune --diag` 显示分层计数。
    pub fn memory_count_by_kind(&self, kind: &str, include_cold: bool) -> Result<usize> {
        let sql = if include_cold {
            "SELECT COUNT(*) FROM memories WHERE kind = ?1 AND superseded_by IS NULL"
        } else {
            "SELECT COUNT(*) FROM memories WHERE kind = ?1 AND superseded_by IS NULL AND cold = 0"
        };
        let n: i64 = self.conn.query_row(sql, params![kind], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// 把一行 SQL 结果映射到 [`RawMemory`]（仍是密文，解密由调用方在锁内做）。
    fn row_to_memory(r: &rusqlite::Row) -> rusqlite::Result<RawMemory> {
        Ok(RawMemory {
            id: r.get(0)?,
            kind: r.get(1)?,
            window_start: r.get(2)?,
            window_end: r.get(3)?,
            hashes_json: r.get(4)?,
            summary_enc: r.get(5)?,
            model: r.get(6)?,
            created_at: r.get(7)?,
            topic_key: r.get(8)?,
            cold: r.get::<_, i64>(9)? != 0,
            superseded_by: r.get(10)?,
        })
    }
}

/// memories 表一行的密文投影 — 解密在 [`RawMemory::decrypt`] 内完成。
struct RawMemory {
    id: String,
    kind: String,
    window_start: i64,
    window_end: i64,
    hashes_json: String,
    summary_enc: Vec<u8>,
    model: String,
    created_at: i64,
    topic_key: Option<String>,
    cold: bool,
    superseded_by: Option<String>,
}

impl RawMemory {
    fn decrypt(self, dek: &Key32) -> MemoryRow {
        let summary = crypto::decrypt(dek, &self.summary_enc)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        let source_chunk_hashes: Vec<String> =
            serde_json::from_str(&self.hashes_json).unwrap_or_default();
        MemoryRow {
            id: self.id,
            kind: self.kind,
            window_start: self.window_start,
            window_end: self.window_end,
            source_chunk_hashes,
            summary,
            model: self.model,
            created_at: self.created_at,
            topic_key: self.topic_key,
            cold: self.cold,
            superseded_by: self.superseded_by,
        }
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

    #[test]
    fn list_recent_memories_carries_multilayer_fields() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 1, 2, &["h".into()], "s", "m", 100)
            .unwrap();
        let row = &store.list_recent_memories(&dek, 1).unwrap()[0];
        assert_eq!(row.topic_key, None);
        assert!(!row.cold);
        assert_eq!(row.superseded_by, None);
    }

    #[test]
    fn insert_semantic_memory_idempotent_on_topic_key() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let members = vec!["m1".to_string(), "m2".to_string()];
        let (_id1, n1) = store
            .insert_semantic_memory(&dek, "topic-abc", &members, "first", "m", 0, 100, 1000)
            .unwrap();
        assert_eq!(n1, 1);
        // 相同 topic_key → INSERT OR IGNORE 返回 0
        let (_id2, n2) = store
            .insert_semantic_memory(&dek, "topic-abc", &members, "second", "m", 0, 100, 2000)
            .unwrap();
        assert_eq!(n2, 0, "duplicate topic_key must be ignored");
        assert_eq!(store.memory_count_by_kind("semantic", false).unwrap(), 1);
    }

    #[test]
    fn insert_semantic_rejects_empty_members() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let err = store
            .insert_semantic_memory(&dek, "t", &[], "s", "m", 0, 1, 0)
            .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[test]
    fn mark_superseded_excludes_old_from_live() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let (old_id, _) = store
            .insert_semantic_memory(&dek, "t-old", &["m1".into()], "old", "m", 0, 100, 1000)
            .unwrap();
        let (new_id, _) = store
            .insert_semantic_memory(&dek, "t-new", &["m1".into(), "m2".into()], "new", "m", 0, 200, 2000)
            .unwrap();
        let n = store.mark_memory_superseded(&old_id, &new_id).unwrap();
        assert_eq!(n, 1);
        let live = store.list_live_memories(&dek, "semantic", false).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, new_id);
        // 二次标记同一行 → 0（已 superseded）
        assert_eq!(store.mark_memory_superseded(&old_id, &new_id).unwrap(), 0);
    }

    #[test]
    fn demote_cold_only_touches_old_and_covered() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let day = 24 * 3600;
        // 老 episodic（window_end 很早），有 semantic 覆盖
        store
            .insert_memory(&dek, "episodic", 0, day, &["old".into()], "s", "m", day)
            .unwrap();
        // 新 episodic（window_end 接近 now），不该被降级
        let now = 400 * day;
        store
            .insert_memory(&dek, "episodic", now - day, now, &["fresh".into()], "s", "m", now)
            .unwrap();
        // 一条 live semantic（created_at 晚于老 episodic 的 window_end）
        store
            .insert_semantic_memory(&dek, "t", &["old".into()], "sem", "m", 0, day, 10 * day)
            .unwrap();
        let cold_age = 180 * day;
        let demoted = store.demote_cold_memories(now, cold_age).unwrap();
        assert_eq!(demoted, 1, "only the old+covered episodic should go cold");
        // hot 检索仅剩新 episodic
        let hot = store.list_live_memories(&dek, "episodic", false).unwrap();
        assert_eq!(hot.len(), 1);
        assert_eq!(hot[0].source_chunk_hashes, vec!["fresh"]);
        // include_cold 看得到全部
        assert_eq!(store.list_live_memories(&dek, "episodic", true).unwrap().len(), 2);
        // 二次跑幂等：无新增降级
        assert_eq!(store.demote_cold_memories(now, cold_age).unwrap(), 0);
    }

    #[test]
    fn demote_skips_when_no_semantic_coverage() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let day = 24 * 3600;
        store
            .insert_memory(&dek, "episodic", 0, day, &["old".into()], "s", "m", day)
            .unwrap();
        // 无 semantic memory → 不该降级（即便足够老）
        let demoted = store.demote_cold_memories(400 * day, 180 * day).unwrap();
        assert_eq!(demoted, 0);
    }
}
