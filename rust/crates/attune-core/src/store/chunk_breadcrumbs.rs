//! F2 Chunk breadcrumb 元数据 sidecar（W3 batch A，2026-04-27）。
//!
//! per spec `docs/superpowers/specs/2026-04-27-w3-batch-a-design.md` §4
//! per R04 P0-1：breadcrumb 属用户敏感数据（章节标题路径暴露文档结构 + 主题），
//! 必须 DEK 加密落盘。violates "All data encrypted on your own device" 承诺。
//!
//! 关闭 W2 batch 1 留下的 placeholder 状态：让 `Citation.breadcrumb` + `chunk_offset_*`
//! 真正有值。设计取舍：用独立 sidecar 表而非扩 `embed_queue` / `VectorMeta` —
//! 避免老 vault `.encbin` 反序列化破坏 + 4 个 enqueue 调用点的迁移风险。

use rusqlite::{params, OptionalExtension};

use crate::chunker::extract_sections_with_path;
use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

impl Store {
    /// 用文档原文跑 [`extract_sections_with_path`] 后批量写入 chunk_breadcrumbs。
    ///
    /// per R04 P0-1：breadcrumb_json DEK 加密后落盘，参数加 `dek: &Key32`。
    /// 调用方：indexer pipeline 在 chunk 入 embed_queue 之前 / 同时调用一次。
    /// 同 (item_id, chunk_idx) 二次调用走 INSERT OR REPLACE 覆盖。
    /// 返回写入条数。
    pub fn upsert_chunk_breadcrumbs_from_content(
        &self,
        dek: &Key32,
        item_id: &str,
        content: &str,
    ) -> Result<usize> {
        let sections = extract_sections_with_path(content);
        if sections.is_empty() {
            return Ok(0);
        }
        let mut cursor: usize = 0;
        let mut written = 0;
        for section in &sections {
            let section_chars = section.content.chars().count();
            let offset_start = cursor;
            let offset_end = cursor + section_chars;
            cursor = offset_end;

            let breadcrumb_json = serde_json::to_string(&section.path)
                .map_err(|e| VaultError::InvalidInput(format!("breadcrumb json: {e}")))?;
            // P0-1: 加密敏感字段
            let breadcrumb_enc = crypto::encrypt(dek, breadcrumb_json.as_bytes())?;
            self.conn.execute(
                "INSERT OR REPLACE INTO chunk_breadcrumbs \
                    (item_id, chunk_idx, breadcrumb_enc, offset_start, offset_end) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    item_id,
                    section.section_idx as i64,
                    breadcrumb_enc,
                    offset_start as i64,
                    offset_end as i64,
                ],
            )?;
            written += 1;
        }
        Ok(written)
    }

    /// 查询单个 chunk 的 (breadcrumb, offset_start, offset_end)。缺失返回 None。
    /// per P0-1: 解密 breadcrumb_enc。
    pub fn get_chunk_breadcrumb(
        &self,
        dek: &Key32,
        item_id: &str,
        chunk_idx: usize,
    ) -> Result<Option<(Vec<String>, usize, usize)>> {
        let row: Option<(Vec<u8>, i64, i64)> = self
            .conn
            .query_row(
                "SELECT breadcrumb_enc, offset_start, offset_end \
                 FROM chunk_breadcrumbs WHERE item_id = ?1 AND chunk_idx = ?2",
                params![item_id, chunk_idx as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((enc, start, end)) = row else {
            return Ok(None);
        };
        let plain = match crypto::decrypt(dek, &enc) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("F2 P0-1 breadcrumb decrypt failed for {item_id}#{chunk_idx}: {e}");
                return Ok(None);
            }
        };
        let path: Vec<String> = serde_json::from_slice(&plain).unwrap_or_else(|e| {
            log::warn!("F2 breadcrumb json parse failed for {item_id}#{chunk_idx}: {e}");
            Vec::new()
        });
        Ok(Some((path, start as usize, end as usize)))
    }

    /// 查询某 item 的第一个 chunk 的 breadcrumb（启发式：F2 v1 SearchResult 不追踪
    /// 具体 chunk_idx 命中，用第一个 chunk 的路径作"item 的 top-level 路径"）。
    /// W5+ 当 SearchResult 携带 chunk_idx 后切到精确 [`Self::get_chunk_breadcrumb`]。
    pub fn get_first_chunk_breadcrumb(
        &self,
        dek: &Key32,
        item_id: &str,
    ) -> Result<Option<(Vec<String>, usize, usize)>> {
        let row: Option<(Vec<u8>, i64, i64)> = self
            .conn
            .query_row(
                "SELECT breadcrumb_enc, offset_start, offset_end \
                 FROM chunk_breadcrumbs WHERE item_id = ?1 \
                 ORDER BY chunk_idx ASC LIMIT 1",
                params![item_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((enc, start, end)) = row else {
            return Ok(None);
        };
        let plain = match crypto::decrypt(dek, &enc) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("F2 P0-1 breadcrumb decrypt failed for {item_id}#first: {e}");
                return Ok(None);
            }
        };
        let path: Vec<String> = serde_json::from_slice(&plain).unwrap_or_default();
        Ok(Some((path, start as usize, end as usize)))
    }

    /// 总数 — 诊断用。
    pub fn chunk_breadcrumbs_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunk_breadcrumbs", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;

    fn seed_item(store: &Store, dek: &Key32, content: &str) -> String {
        store
            .insert_item(dek, "test-doc", content, None, "file", None, None)
            .unwrap()
    }

    #[test]
    fn upsert_writes_rows_for_nested_markdown() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 公司手册\n\n概述\n\n## 第一章\n\n内容 A\n\n## 第二章\n\n内容 B";
        let item_id = seed_item(&store, &dek, content);
        let n = store
            .upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content)
            .unwrap();
        assert!(n >= 3, "应写入 ≥3 行，得到 {n}");
        assert_eq!(store.chunk_breadcrumbs_count().unwrap(), n);
    }

    #[test]
    fn lookup_returns_path_for_known_chunk() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 文档\n\n## 章节 A\n\n正文";
        let item_id = seed_item(&store, &dek, content);
        store
            .upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content)
            .unwrap();
        let r = store.get_chunk_breadcrumb(&dek, &item_id, 0).unwrap();
        assert!(r.is_some());
        let (path, start, end) = r.unwrap();
        assert!(!path.is_empty());
        assert!(end > start, "offset 区间合法: {start}..{end}");
    }

    #[test]
    fn lookup_unknown_returns_none() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let r = store.get_chunk_breadcrumb(&dek, "non-existent", 0).unwrap();
        assert!(r.is_none());
        let r2 = store.get_first_chunk_breadcrumb(&dek, "non-existent").unwrap();
        assert!(r2.is_none());
    }

    #[test]
    fn upsert_replaces_on_reindex() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let v1 = "# A\n\n旧";
        let item_id = seed_item(&store, &dek, v1);
        store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, v1).unwrap();
        let count_after_v1 = store.chunk_breadcrumbs_count().unwrap();
        let v2 = "# A\n\n新内容";
        let n = store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, v2).unwrap();
        assert_eq!(n, count_after_v1);
    }

    #[test]
    fn first_chunk_returns_lowest_idx() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 文档根\n\n## 第一章\n\nA\n\n## 第二章\n\nB";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content).unwrap();
        let first = store.get_first_chunk_breadcrumb(&dek, &item_id).unwrap().unwrap();
        assert_eq!(first.1, 0);
    }

    #[test]
    fn empty_content_writes_nothing() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let item_id = seed_item(&store, &dek, "placeholder");
        let n = store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, "").unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn breadcrumb_json_round_trips_unicode() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 中文标题 🎉\n\n## 子节 emoji 😀\n\n内容";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content).unwrap();
        let r = store.get_chunk_breadcrumb(&dek, &item_id, 0).unwrap().unwrap();
        assert!(r.0.iter().any(|p| p.contains("中文") || p.contains("🎉")));
    }

    #[test]
    fn fk_cascade_deletes_breadcrumbs_on_item_hard_delete() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# T\n\n## A\n\n正文";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content).unwrap();
        assert!(store.chunk_breadcrumbs_count().unwrap() > 0);
        store.conn.execute("DELETE FROM items WHERE id = ?1", rusqlite::params![item_id]).unwrap();
        assert_eq!(store.chunk_breadcrumbs_count().unwrap(), 0, "CASCADE 应清空 breadcrumbs");
    }

    #[test]
    fn soft_delete_clears_breadcrumbs() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 文档\n\n## 章节\n\n正文";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content).unwrap();
        let before = store.chunk_breadcrumbs_count().unwrap();
        assert!(before > 0);
        let deleted = store.delete_item(&item_id).unwrap();
        assert!(deleted, "软删除应成功");
        assert_eq!(store.chunk_breadcrumbs_count().unwrap(), 0);
        assert!(store.get_first_chunk_breadcrumb(&dek, &item_id).unwrap().is_none());
    }

    #[test]
    fn migrate_breadcrumbs_encrypt_drops_old_plaintext_column() {
        // per R07 P0：模拟 W3 batch A 末老 schema → 升级到 W3 末新 schema
        // 验证 migrate_breadcrumbs_encrypt 触发 DROP + 重建，不让 indexer 写入 SQL error
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        // 1. 跑老 schema（仅 chunk_breadcrumbs 这张表，模拟 W3 batch A 末）
        conn.execute(
            "CREATE TABLE items (id TEXT PRIMARY KEY)", []
        ).unwrap();
        conn.execute(
            "CREATE TABLE chunk_breadcrumbs (\
                item_id TEXT NOT NULL,\
                chunk_idx INTEGER NOT NULL,\
                breadcrumb_json TEXT NOT NULL,\
                offset_start INTEGER NOT NULL,\
                offset_end INTEGER NOT NULL,\
                PRIMARY KEY (item_id, chunk_idx)\
             )",
            [],
        ).unwrap();
        // 写入老明文行
        conn.execute(
            "INSERT INTO items (id) VALUES ('old-item')", []
        ).unwrap();
        conn.execute(
            "INSERT INTO chunk_breadcrumbs (item_id, chunk_idx, breadcrumb_json, offset_start, offset_end) \
             VALUES ('old-item', 0, '[\"老明文\"]', 0, 100)",
            [],
        ).unwrap();
        // 2. 跑 migrate
        Store::migrate_breadcrumbs_encrypt(&conn).unwrap();
        // 3. 验证：老列 breadcrumb_json 不存在；新列 breadcrumb_enc 存在
        let has_old: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('chunk_breadcrumbs') WHERE name = 'breadcrumb_json'",
                [], |r| r.get(0),
            ).unwrap();
        let has_new: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('chunk_breadcrumbs') WHERE name = 'breadcrumb_enc'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(has_old, 0, "老明文列必须被 DROP");
        assert_eq!(has_new, 1, "新加密列必须存在");
        // 老数据已丢（acceptable 因为下次 indexer 重建）
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM chunk_breadcrumbs", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn migrate_breadcrumbs_encrypt_idempotent_on_new_schema() {
        // 全新 schema 已经是 breadcrumb_enc，迁移函数不应做任何事
        let store = Store::open_memory().unwrap();
        let count_before = store.chunk_breadcrumbs_count().unwrap();
        Store::migrate_breadcrumbs_encrypt(&store.conn).unwrap();
        assert_eq!(store.chunk_breadcrumbs_count().unwrap(), count_before, "新 schema 下 migrate 应 no-op");
    }

    #[test]
    fn breadcrumb_encrypted_at_rest() {
        // per R04 P0-1：breadcrumb 落盘必须加密。用通用文档结构（与行业无关）。
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 项目分析\n\n## 重点观察\n\n详情";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&dek, &item_id, content).unwrap();
        let raw: Vec<u8> = store
            .conn
            .query_row(
                "SELECT breadcrumb_enc FROM chunk_breadcrumbs WHERE item_id = ?1 LIMIT 1",
                rusqlite::params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(!raw_str.contains("项目分析"), "breadcrumb 必须加密落盘 (P0-1)");
        assert!(!raw_str.contains("重点观察"), "breadcrumb 必须加密落盘 (P0-1)");
    }
}
