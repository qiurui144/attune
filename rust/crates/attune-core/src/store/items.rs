//! Items 表 CRUD（attune-core 主资产 — 文件/笔记内容 + 加密）
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::audit::PrivacyTier;
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    // --- items (加密 CRUD) ---

    pub fn insert_item(
        &self,
        dek: &Key32,
        title: &str,
        content: &str,
        url: Option<&str>,
        source_type: &str,
        domain: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let encrypted_content = crypto::encrypt(dek, content.as_bytes())?;
        let encrypted_tags = match tags {
            Some(t) => Some(crypto::encrypt(dek, serde_json::to_string(t)?.as_bytes())?),
            None => None,
        };

        self.conn.execute(
            "INSERT INTO items (id, title, content, url, source_type, domain, tags, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, title, encrypted_content, url, source_type, domain, encrypted_tags, now, now],
        )?;
        Ok(id)
    }

    /// 廉价的存在性检查（不解密 content），用于外键前置校验，给出比 SQL 错误更清晰的 404
    pub fn item_exists(&self, id: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items WHERE id = ?1 AND is_deleted = 0",
            params![id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn get_item(&self, dek: &Key32, id: &str) -> Result<Option<DecryptedItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, url, source_type, domain, tags, created_at, updated_at
             FROM items WHERE id = ?1 AND is_deleted = 0",
        )?;

        let result = stmt.query_row(params![id], |row| {
            Ok(RawItem {
                id: row.get(0)?,
                title: row.get(1)?,
                content: row.get::<_, Vec<u8>>(2)?,
                url: row.get(3)?,
                source_type: row.get(4)?,
                domain: row.get(5)?,
                tags: row.get::<_, Option<Vec<u8>>>(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        });

        match result {
            Ok(raw) => Ok(Some(raw.decrypt(dek)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 列出条目（仅标题和元数据，不解密 content）
    pub fn list_items(&self, limit: usize, offset: usize) -> Result<Vec<ItemSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, source_type, domain, created_at
             FROM items WHERE is_deleted = 0
             ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            Ok(ItemSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                source_type: row.get(2)?,
                domain: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// 列出长时间未更新的条目（stale items）
    pub fn list_stale_items(&self, days: i64, limit: i64) -> Result<Vec<StaleItemSummary>> {
        let cutoff = chrono::Utc::now()
            .checked_sub_signed(chrono::Duration::days(days))
            .unwrap_or_else(chrono::Utc::now)
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();

        let mut stmt = self.conn.prepare(
            "SELECT id, title, source_type, updated_at, created_at
             FROM items
             WHERE is_deleted = 0 AND updated_at < ?1
             ORDER BY updated_at ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![cutoff, limit], |row| {
            Ok(StaleItemSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                source_type: row.get(2)?,
                updated_at: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    pub fn get_item_stats(&self, id: &str) -> Result<Option<ItemStats>> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items WHERE id = ?1 AND is_deleted = 0",
            params![id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Ok(None);
        }

        let (created_at, updated_at): (String, String) = self.conn.query_row(
            "SELECT created_at, updated_at FROM items WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let chunk_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE item_id = ?1",
            params![id],
            |row| row.get(0),
        )?;

        let embedding_pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE item_id = ?1 AND status = 'pending'",
            params![id],
            |row| row.get(0),
        )?;

        let embedding_done: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE item_id = ?1 AND status = 'done'",
            params![id],
            |row| row.get(0),
        )?;

        Ok(Some(ItemStats {
            id: id.to_string(),
            created_at,
            updated_at,
            chunk_count,
            embedding_pending,
            embedding_done,
        }))
    }

    pub fn insert_feedback(
        &self,
        item_id: &str,
        feedback_type: &str,
        query: Option<&str>,
    ) -> Result<i64> {
        let valid_types = ["relevant", "irrelevant", "correction"];
        if !valid_types.contains(&feedback_type) {
            return Err(VaultError::InvalidInput(format!(
                "invalid feedback_type: {feedback_type}"
            )));
        }
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        self.conn.execute(
            "INSERT INTO feedback (item_id, feedback_type, query, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![item_id, feedback_type, query, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 测试辅助：直接设置 updated_at 时间戳
    #[cfg(test)]
    pub fn set_updated_at(&self, id: &str, ts: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET updated_at = ?1 WHERE id = ?2",
            params![ts, id],
        )?;
        Ok(())
    }

    pub fn update_item(&self, dek: &Key32, id: &str, title: Option<&str>, content: Option<&str>) -> Result<bool> {
        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) FROM items WHERE id = ?1 AND is_deleted = 0",
            params![id],
            |row| row.get::<_, i64>(0),
        )? > 0;

        if !exists {
            return Ok(false);
        }

        let now = chrono::Utc::now().to_rfc3339();

        if let Some(title) = title {
            self.conn.execute(
                "UPDATE items SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title, now, id],
            )?;
        }

        if let Some(content) = content {
            let encrypted = crypto::encrypt(dek, content.as_bytes())?;
            self.conn.execute(
                "UPDATE items SET content = ?1, updated_at = ?2 WHERE id = ?3",
                params![encrypted, now, id],
            )?;
        }

        Ok(true)
    }

    pub fn delete_item(&self, id: &str) -> Result<bool> {
        let affected = self.conn.execute(
            "UPDATE items SET is_deleted = 1, updated_at = ?1 WHERE id = ?2 AND is_deleted = 0",
            params![chrono::Utc::now().to_rfc3339(), id],
        )?;
        // 软删除语义：用户"忘记这条知识"同时要忘记其批注 + 摘要缓存。items 是 soft-delete，
        // ON DELETE CASCADE 永远不会触发，所以这里显式连坐。
        // 注意：这是硬删除 —— 批注/摘要不可恢复。与 item 软删除不对称，但与"忘记"语义一致。
        if affected > 0 {
            self.conn.execute(
                "DELETE FROM annotations WHERE item_id = ?1",
                params![id],
            )?;
            self.conn.execute(
                "DELETE FROM chunk_summaries WHERE item_id = ?1",
                params![id],
            )?;
            // F2 (W3 batch A, per reviewer R2 P0-1)：与 annotations / chunk_summaries
            // 对称清理 chunk_breadcrumbs。否则用户软删除 item 后 ChatEngine 仍可能
            // 透传 stale breadcrumb 到 Citation — "引用已忘记的文档"漏洞。
            // FK CASCADE 仅在硬删除时触发，软删除路径必须显式处理。
            self.conn.execute(
                "DELETE FROM chunk_breadcrumbs WHERE item_id = ?1",
                params![id],
            )?;
        }
        Ok(affected > 0)
    }

    pub fn item_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items WHERE is_deleted = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 按 URL 查找未删除 item，用于入库前去重（例如专利记录重复检查）。
    pub fn find_item_by_url(&self, url: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM items WHERE url = ?1 AND is_deleted = 0 LIMIT 1",
        )?;
        let result = stmt.query_row(params![url], |row| row.get::<_, String>(0));
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // --- items.tags (加密) ---

    /// 更新条目的 tags 字段（加密存储）
    pub fn update_tags(&self, dek: &Key32, item_id: &str, tags_json: &str) -> Result<bool> {
        let encrypted = crypto::encrypt(dek, tags_json.as_bytes())?;
        let now = chrono::Utc::now().to_rfc3339();
        let affected = self.conn.execute(
            "UPDATE items SET tags = ?1, updated_at = ?2 WHERE id = ?3 AND is_deleted = 0",
            params![encrypted, now, item_id],
        )?;
        Ok(affected > 0)
    }

    /// 读取并解密 item 的 tags JSON (返回 None 表示未分类)
    pub fn get_tags_json(&self, dek: &Key32, item_id: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        let tags: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT tags FROM items WHERE id = ?1 AND is_deleted = 0",
                params![item_id],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten();

        match tags {
            None => Ok(None),
            Some(blob) if blob.is_empty() => Ok(None),
            Some(blob) => {
                let decrypted = crypto::decrypt(dek, &blob)?;
                Ok(Some(String::from_utf8(decrypted)
                    .map_err(|e| VaultError::Crypto(format!("tags utf8: {e}")))?))
            }
        }
    }

    /// W4 F1: 按 source_type 聚合（饼图 / 主题分布）。
    /// 返回 (source_type, count) 数组按 count DESC 排序。is_deleted 行排除。
    pub fn aggregate_items_by_source_type(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_type, COUNT(*) FROM items
             WHERE is_deleted = 0
             GROUP BY source_type
             ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 列出所有未删除 item 的 id（用于 TagIndex 构建和 reclassify_all）
    pub fn list_all_item_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM items WHERE is_deleted = 0 ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    // ============================================================
    // v0.6 Phase B F-Pro — corpus domain
    // ============================================================

    /// 设置 item 的 corpus_domain（legal / tech / medical / patent / general）。
    /// search 阶段按 query intent 跨域降权防止"反洗钱"被 cs-notes 顶占。
    pub fn set_item_corpus_domain(&self, item_id: &str, corpus_domain: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE items SET corpus_domain = ?1, updated_at = ?2 WHERE id = ?3 AND is_deleted = 0",
            params![corpus_domain, chrono::Utc::now().to_rfc3339(), item_id],
        )?;
        if n == 0 {
            return Err(VaultError::NotFound(format!("item {item_id}")));
        }
        Ok(())
    }

    /// 读取 item 的 corpus_domain。item 不存在返回 NotFound。
    pub fn get_item_corpus_domain(&self, item_id: &str) -> Result<String> {
        let s: String = self
            .conn
            .query_row(
                "SELECT corpus_domain FROM items WHERE id = ?1 AND is_deleted = 0",
                params![item_id],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    VaultError::NotFound(format!("item {item_id}"))
                }
                other => VaultError::Database(other),
            })?;
        Ok(s)
    }

    // ============================================================
    // v0.6 Phase A.5.4 — per-file 隐私分级
    // ============================================================

    /// 设置文件的隐私级别。
    /// L0 = 🔒 强制本地（chunk 永不出现在云端 LLM context）
    /// L1 = 默认（脱敏后 → 云）
    /// L3 = 高敏感（LLM 脱敏后 → 云，仅 Tier T3+/T4+/K3 启用）
    pub fn set_item_privacy_tier(&self, item_id: &str, tier: PrivacyTier) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE items SET privacy_tier = ?1, updated_at = ?2 WHERE id = ?3 AND is_deleted = 0",
            params![tier_str(tier), chrono::Utc::now().to_rfc3339(), item_id],
        )?;
        if n == 0 {
            return Err(VaultError::NotFound(format!("item {item_id}")));
        }
        Ok(())
    }

    /// 读取文件隐私级别。item 不存在返回 NotFound。
    pub fn get_item_privacy_tier(&self, item_id: &str) -> Result<PrivacyTier> {
        let s: String = self
            .conn
            .query_row(
                "SELECT privacy_tier FROM items WHERE id = ?1 AND is_deleted = 0",
                params![item_id],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    VaultError::NotFound(format!("item {item_id}"))
                }
                other => VaultError::Database(other),
            })?;
        Ok(parse_tier(&s))
    }

    /// chat retrieval hook：从候选 item_ids 列表剔除 L0（🔒 标记）的文件。
    /// 用于 chat.rs 在组装 LLM context 前的二次校验，防止 L0 文件 chunk 被发送到云 LLM。
    /// 如果用户要求"全本地 LLM 模式"（PrivacyTier::L0 全局），此过滤函数无意义，可由调用方跳过。
    pub fn filter_out_l0_items(&self, item_ids: &[String]) -> Result<Vec<String>> {
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        // 以 placeholder 拼 IN 子句（数量动态，rusqlite 不直接支持 Vec<&str> 参数绑定）
        let placeholders = item_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id FROM items WHERE id IN ({placeholders}) \
             AND is_deleted = 0 AND privacy_tier != 'L0'"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params_iter: Vec<&dyn rusqlite::ToSql> =
            item_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_iter.as_slice(), |r| r.get::<_, String>(0))?;
        let mut keep = Vec::with_capacity(item_ids.len());
        for r in rows {
            keep.push(r?);
        }
        Ok(keep)
    }

    /// 列出当前所有标记为 L0 的 item id（Settings UI "受保护文件" 列表）
    pub fn list_l0_item_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM items WHERE is_deleted = 0 AND privacy_tier = 'L0' \
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

fn tier_str(t: PrivacyTier) -> &'static str {
    match t {
        PrivacyTier::L0 => "L0",
        PrivacyTier::L1 => "L1",
        PrivacyTier::L3 => "L3",
    }
}

fn parse_tier(s: &str) -> PrivacyTier {
    match s {
        "L0" => PrivacyTier::L0,
        "L3" => PrivacyTier::L3,
        _ => PrivacyTier::L1,
    }
}

#[cfg(test)]
mod privacy_tier_tests {
    use super::*;
    use crate::store::Store;

    fn open_with_dummy_item() -> (Store, String) {
        let store = Store::open_memory().expect("open memory");
        // 直接 insert 一行（避开 dek 加密路径）
        let id = "test-item-1";
        store
            .conn
            .execute(
                "INSERT INTO items (id, title, content, source_type, created_at, updated_at) \
                 VALUES (?1, 'T', X'00', 'note', '2026-01-01', '2026-01-01')",
                params![id],
            )
            .unwrap();
        (store, id.to_string())
    }

    #[test]
    fn default_tier_is_l1() {
        let (s, id) = open_with_dummy_item();
        assert_eq!(s.get_item_privacy_tier(&id).unwrap(), PrivacyTier::L1);
    }

    #[test]
    fn set_and_get_l0() {
        let (s, id) = open_with_dummy_item();
        s.set_item_privacy_tier(&id, PrivacyTier::L0).unwrap();
        assert_eq!(s.get_item_privacy_tier(&id).unwrap(), PrivacyTier::L0);
    }

    #[test]
    fn set_and_get_l3() {
        let (s, id) = open_with_dummy_item();
        s.set_item_privacy_tier(&id, PrivacyTier::L3).unwrap();
        assert_eq!(s.get_item_privacy_tier(&id).unwrap(), PrivacyTier::L3);
    }

    #[test]
    fn set_unknown_item_errors() {
        let s = Store::open_memory().unwrap();
        let result = s.set_item_privacy_tier("nope", PrivacyTier::L0);
        assert!(matches!(result, Err(VaultError::NotFound(_))));
    }

    #[test]
    fn filter_out_l0_excludes_l0_only() {
        let s = Store::open_memory().unwrap();
        for id in ["a", "b", "c"] {
            s.conn
                .execute(
                    "INSERT INTO items (id, title, content, source_type, created_at, updated_at) \
                     VALUES (?1, 'T', X'00', 'note', '2026-01-01', '2026-01-01')",
                    params![id],
                )
                .unwrap();
        }
        s.set_item_privacy_tier("b", PrivacyTier::L0).unwrap();

        let kept = s
            .filter_out_l0_items(&["a".into(), "b".into(), "c".into()])
            .unwrap();
        assert_eq!(kept.len(), 2);
        assert!(kept.contains(&"a".to_string()));
        assert!(kept.contains(&"c".to_string()));
        assert!(!kept.contains(&"b".to_string()));
    }

    #[test]
    fn filter_empty_returns_empty() {
        let s = Store::open_memory().unwrap();
        let kept = s.filter_out_l0_items(&[]).unwrap();
        assert!(kept.is_empty());
    }

    #[test]
    fn list_l0_item_ids_only_returns_l0() {
        let s = Store::open_memory().unwrap();
        for id in ["a", "b", "c"] {
            s.conn
                .execute(
                    "INSERT INTO items (id, title, content, source_type, created_at, updated_at) \
                     VALUES (?1, 'T', X'00', 'note', '2026-01-01', '2026-01-01')",
                    params![id],
                )
                .unwrap();
        }
        s.set_item_privacy_tier("a", PrivacyTier::L0).unwrap();
        s.set_item_privacy_tier("c", PrivacyTier::L0).unwrap();

        let l0 = s.list_l0_item_ids().unwrap();
        assert_eq!(l0.len(), 2);
        assert!(l0.contains(&"a".to_string()));
        assert!(l0.contains(&"c".to_string()));
    }
}
