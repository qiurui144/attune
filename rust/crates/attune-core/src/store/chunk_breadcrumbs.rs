//! F2 Chunk breadcrumb 元数据 sidecar（W3 batch A，2026-04-27）。
//!
//! per spec `docs/superpowers/specs/2026-04-27-w3-batch-a-design.md` §4
//!
//! 关闭 W2 batch 1 留下的 placeholder 状态：让 `Citation.breadcrumb` + `chunk_offset_*`
//! 真正有值。设计取舍：用独立 sidecar 表而非扩 `embed_queue` / `VectorMeta` —
//! 避免老 vault `.encbin` 反序列化破坏 + 4 个 enqueue 调用点的迁移风险。
//!
//! 老 vault 升级时 IF NOT EXISTS 创建空表 → ChatEngine 查不到时返回空 Vec 优雅降级。
//! 新 indexer pipeline（routes/upload.rs / routes/ingest.rs / scanner.rs / scanner_webdav.rs）
//! 在每个 item 入库后调 [`Store::upsert_chunk_breadcrumbs_from_content`] 一行写入。

use rusqlite::{params, OptionalExtension};

use crate::chunker::extract_sections_with_path;
use crate::error::{Result, VaultError};
use crate::store::Store;

impl Store {
    /// 用文档原文跑 [`extract_sections_with_path`] 后批量写入 chunk_breadcrumbs。
    ///
    /// 调用方：indexer pipeline 在 chunk 入 embed_queue 之前 / 同时调用一次。
    /// 同 (item_id, chunk_idx) 二次调用走 INSERT OR REPLACE 覆盖。
    /// 返回写入条数。
    pub fn upsert_chunk_breadcrumbs_from_content(
        &self,
        item_id: &str,
        content: &str,
    ) -> Result<usize> {
        let sections = extract_sections_with_path(content);
        if sections.is_empty() {
            return Ok(0);
        }
        // 计算每个 section 在 content 中的 char-level offset。
        // chunker 的 section 是按行累积的，相对顺序与原文一致；用 cumulative char count
        // 即可（content.chars() 一次扫描足够）。
        let mut cursor: usize = 0;
        let mut written = 0;
        for section in &sections {
            let section_chars = section.content.chars().count();
            let offset_start = cursor;
            let offset_end = cursor + section_chars;
            cursor = offset_end;

            let breadcrumb_json = serde_json::to_string(&section.path)
                .map_err(|e| VaultError::InvalidInput(format!("breadcrumb json: {e}")))?;
            self.conn.execute(
                "INSERT OR REPLACE INTO chunk_breadcrumbs \
                    (item_id, chunk_idx, breadcrumb_json, offset_start, offset_end) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    item_id,
                    section.section_idx as i64,
                    breadcrumb_json,
                    offset_start as i64,
                    offset_end as i64,
                ],
            )?;
            written += 1;
        }
        Ok(written)
    }

    /// 查询单个 chunk 的 (breadcrumb, offset_start, offset_end)。缺失返回 None。
    pub fn get_chunk_breadcrumb(
        &self,
        item_id: &str,
        chunk_idx: usize,
    ) -> Result<Option<(Vec<String>, usize, usize)>> {
        let row: Option<(String, i64, i64)> = self
            .conn
            .query_row(
                "SELECT breadcrumb_json, offset_start, offset_end \
                 FROM chunk_breadcrumbs WHERE item_id = ?1 AND chunk_idx = ?2",
                params![item_id, chunk_idx as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((json, start, end)) = row else {
            return Ok(None);
        };
        let path: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        Ok(Some((path, start as usize, end as usize)))
    }

    /// 查询某 item 的第一个 chunk 的 breadcrumb（启发式：F2 v1 SearchResult 不追踪
    /// 具体 chunk_idx 命中，用第一个 chunk 的路径作"item 的 top-level 路径"）。
    /// W5+ 当 SearchResult 携带 chunk_idx 后切到精确 [`Self::get_chunk_breadcrumb`]。
    pub fn get_first_chunk_breadcrumb(
        &self,
        item_id: &str,
    ) -> Result<Option<(Vec<String>, usize, usize)>> {
        let row: Option<(String, i64, i64)> = self
            .conn
            .query_row(
                "SELECT breadcrumb_json, offset_start, offset_end \
                 FROM chunk_breadcrumbs WHERE item_id = ?1 \
                 ORDER BY chunk_idx ASC LIMIT 1",
                params![item_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((json, start, end)) = row else {
            return Ok(None);
        };
        let path: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
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

    /// 测试辅助：先 insert_item 再返回真 item_id（FK CASCADE 要求 items 存在，per reviewer I3）
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
            .upsert_chunk_breadcrumbs_from_content(&item_id, content)
            .unwrap();
        // 至少 3 段（标题 + 第一章 + 第二章）
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
            .upsert_chunk_breadcrumbs_from_content(&item_id, content)
            .unwrap();
        let r = store.get_chunk_breadcrumb(&item_id, 0).unwrap();
        assert!(r.is_some());
        let (path, start, end) = r.unwrap();
        assert!(!path.is_empty());
        assert!(end > start, "offset 区间合法: {start}..{end}");
    }

    #[test]
    fn lookup_unknown_returns_none() {
        let store = Store::open_memory().unwrap();
        // 用未 insert_item 过的 id 查询 — FK 校验仅 INSERT 时触发，SELECT 无影响
        let r = store.get_chunk_breadcrumb("non-existent", 0).unwrap();
        assert!(r.is_none());
        let r2 = store.get_first_chunk_breadcrumb("non-existent").unwrap();
        assert!(r2.is_none());
    }

    #[test]
    fn upsert_replaces_on_reindex() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let v1 = "# A\n\n旧";
        let item_id = seed_item(&store, &dek, v1);
        store.upsert_chunk_breadcrumbs_from_content(&item_id, v1).unwrap();
        let count_after_v1 = store.chunk_breadcrumbs_count().unwrap();
        let v2 = "# A\n\n新内容";
        let n = store.upsert_chunk_breadcrumbs_from_content(&item_id, v2).unwrap();
        assert_eq!(n, count_after_v1);
    }

    #[test]
    fn first_chunk_returns_lowest_idx() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 文档根\n\n## 第一章\n\nA\n\n## 第二章\n\nB";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&item_id, content).unwrap();
        let first = store.get_first_chunk_breadcrumb(&item_id).unwrap().unwrap();
        assert_eq!(first.1, 0);
    }

    #[test]
    fn empty_content_writes_nothing() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let item_id = seed_item(&store, &dek, "placeholder");
        let n = store.upsert_chunk_breadcrumbs_from_content(&item_id, "").unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn breadcrumb_json_round_trips_unicode() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 中文标题 🎉\n\n## 子节 emoji 😀\n\n内容";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&item_id, content).unwrap();
        let r = store.get_chunk_breadcrumb(&item_id, 0).unwrap().unwrap();
        assert!(r.0.iter().any(|p| p.contains("中文") || p.contains("🎉")));
    }

    #[test]
    fn fk_cascade_deletes_breadcrumbs_on_item_hard_delete() {
        // 验证 reviewer I3 的 FK CASCADE 正确生效
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# T\n\n## A\n\n正文";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&item_id, content).unwrap();
        assert!(store.chunk_breadcrumbs_count().unwrap() > 0);

        // 硬删除 item → CASCADE 触发清理 breadcrumbs
        store.conn.execute("DELETE FROM items WHERE id = ?1", rusqlite::params![item_id]).unwrap();
        assert_eq!(store.chunk_breadcrumbs_count().unwrap(), 0, "CASCADE 应清空 breadcrumbs");
    }

    #[test]
    fn soft_delete_clears_breadcrumbs() {
        // 验证 reviewer R2 P0-1：软删除 item 后 chunk_breadcrumbs 也被清理，
        // 防止 ChatEngine 后续透传 stale breadcrumb 到 Citation
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let content = "# 文档\n\n## 章节\n\n正文";
        let item_id = seed_item(&store, &dek, content);
        store.upsert_chunk_breadcrumbs_from_content(&item_id, content).unwrap();
        let before = store.chunk_breadcrumbs_count().unwrap();
        assert!(before > 0);

        // 走软删除路径（is_deleted=1，item 行不会真删）
        let deleted = store.delete_item(&item_id).unwrap();
        assert!(deleted, "软删除应成功");
        // 验证 breadcrumbs 同时被清
        assert_eq!(store.chunk_breadcrumbs_count().unwrap(), 0, "软删除应连坐 breadcrumbs");
        // 验证 ChatEngine 路径返回 None（优雅降级而非 stale data）
        assert!(store.get_first_chunk_breadcrumb(&item_id).unwrap().is_none());
    }
}
