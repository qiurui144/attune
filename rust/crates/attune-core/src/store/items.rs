//! Items 表 CRUD（attune-core 主资产 — 文件/笔记内容 + 加密）
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
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
}
