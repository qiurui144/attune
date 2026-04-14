// npu-vault/crates/vault-core/src/store.rs

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS vault_meta (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    content     BLOB NOT NULL,
    url         TEXT,
    source_type TEXT NOT NULL DEFAULT 'note',
    domain      TEXT,
    tags        BLOB,
    metadata    BLOB,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    is_deleted  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_items_source ON items(source_type);
CREATE INDEX IF NOT EXISTS idx_items_created ON items(created_at);
CREATE INDEX IF NOT EXISTS idx_items_deleted ON items(is_deleted);

CREATE TABLE IF NOT EXISTS embed_queue (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     TEXT NOT NULL REFERENCES items(id),
    chunk_idx   INTEGER NOT NULL,
    chunk_text  BLOB NOT NULL,
    level       INTEGER NOT NULL DEFAULT 2,
    section_idx INTEGER NOT NULL DEFAULT 0,
    priority    INTEGER NOT NULL DEFAULT 2,
    status      TEXT NOT NULL DEFAULT 'pending',
    attempts    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_eq_status ON embed_queue(status, priority, created_at);
CREATE INDEX IF NOT EXISTS idx_eq_item ON embed_queue(item_id);

CREATE TABLE IF NOT EXISTS bound_dirs (
    id         TEXT PRIMARY KEY,
    path       TEXT UNIQUE NOT NULL,
    recursive  INTEGER NOT NULL DEFAULT 1,
    file_types TEXT NOT NULL,
    is_active  INTEGER NOT NULL DEFAULT 1,
    last_scan  TEXT
);

CREATE TABLE IF NOT EXISTS indexed_files (
    id         TEXT PRIMARY KEY,
    dir_id     TEXT NOT NULL REFERENCES bound_dirs(id),
    path       TEXT UNIQUE NOT NULL,
    file_hash  TEXT NOT NULL,
    item_id    TEXT REFERENCES items(id),
    indexed_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_if_dir ON indexed_files(dir_id);

CREATE TABLE IF NOT EXISTS sessions (
    token      TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS search_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query BLOB NOT NULL,
    result_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_history_created ON search_history(created_at);

CREATE TABLE IF NOT EXISTS click_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query BLOB NOT NULL,
    item_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_click_item ON click_events(item_id);
CREATE INDEX IF NOT EXISTS idx_click_created ON click_events(created_at);

CREATE TABLE IF NOT EXISTS feedback (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id      TEXT NOT NULL,
    feedback_type TEXT NOT NULL CHECK(feedback_type IN ('relevant','irrelevant','correction')),
    query        TEXT,
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_feedback_item ON feedback(item_id);
CREATE INDEX IF NOT EXISTS idx_feedback_created ON feedback(created_at);

CREATE TABLE IF NOT EXISTS conversations (
    id          TEXT PRIMARY KEY,
    title       BLOB NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS conversation_messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK(role IN ('user','assistant','system')),
    content         BLOB NOT NULL,
    citations       TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_conv_messages_conv_id
    ON conversation_messages(conversation_id);
"#;

pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开或创建数据库，初始化 schema
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Self::migrate_task_type(&conn)?;
        Ok(Self { conn })
    }

    /// 打开内存数据库（测试用）
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Self::migrate_task_type(&conn)?;
        Ok(Self { conn })
    }

    /// 迁移: embed_queue 新增 task_type 列（幂等）
    fn migrate_task_type(conn: &Connection) -> Result<()> {
        let has_task_type: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('embed_queue') WHERE name = 'task_type'",
            [],
            |row| row.get(0),
        )?;
        if has_task_type == 0 {
            conn.execute(
                "ALTER TABLE embed_queue ADD COLUMN task_type TEXT NOT NULL DEFAULT 'embed'",
                [],
            )?;
        }
        Ok(())
    }

    /// Checkpoint WAL to main DB file (for testing at-rest encryption)
    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    // --- vault_meta ---

    pub fn set_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mut stmt = self.conn.prepare("SELECT value FROM vault_meta WHERE key = ?1")?;
        let result = stmt.query_row(params![key], |row| row.get::<_, Vec<u8>>(0));
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn has_meta(&self, key: &str) -> Result<bool> {
        Ok(self.get_meta(key)?.is_some())
    }

    /// 获取当前 token nonce（不存在时返回 0）
    pub fn get_token_nonce(&self) -> Result<u64> {
        match self.get_meta("token_nonce")? {
            Some(bytes) if bytes.len() == 8 => {
                let arr: [u8; 8] = bytes.as_slice().try_into()
                    .map_err(|_| VaultError::Crypto("token nonce size mismatch".into()))?;
                Ok(u64::from_le_bytes(arr))
            }
            _ => Ok(0u64),
        }
    }

    /// 递增 token nonce（每次 lock 调用）
    pub fn increment_token_nonce(&self) -> Result<u64> {
        let current = self.get_token_nonce()?;
        let next = current.wrapping_add(1);
        self.set_meta("token_nonce", &next.to_le_bytes())?;
        Ok(next)
    }

    /// 在单个事务中批量写入 vault_meta（用于 change_password 原子更新）
    /// 使用 unchecked_transaction 与 dequeue_embeddings/append_conversation_turn 保持一致，
    /// 避免与 rusqlite 内部事务状态机冲突。
    pub fn set_meta_batch(&self, entries: &[(&str, &[u8])]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (key, value) in entries {
            tx.execute(
                "INSERT OR REPLACE INTO vault_meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

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

    // --- bound_dirs ---

    /// 绑定监控目录，返回 dir_id
    pub fn bind_directory(&self, path: &str, recursive: bool, file_types: &[&str]) -> Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let ft_json = serde_json::to_string(file_types)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO bound_dirs (id, path, recursive, file_types, is_active)
             VALUES (?1, ?2, ?3, ?4, 1)",
            params![id, path, recursive as i32, ft_json],
        )?;
        Ok(id)
    }

    /// 解绑监控目录（标记为非活跃）
    pub fn unbind_directory(&self, dir_id: &str) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE bound_dirs SET is_active = 0 WHERE id = ?1 AND is_active = 1",
            params![dir_id],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(format!("bound_dir {dir_id}")));
        }
        Ok(())
    }

    /// 列出所有活跃的绑定目录
    pub fn list_bound_directories(&self) -> Result<Vec<BoundDirRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, recursive, file_types, last_scan FROM bound_dirs WHERE is_active = 1",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BoundDirRow {
                id: row.get(0)?,
                path: row.get(1)?,
                recursive: row.get::<_, i32>(2)? != 0,
                file_types: row.get(3)?,
                last_scan: row.get(4)?,
            })
        })?;
        let mut dirs = Vec::new();
        for row in rows {
            dirs.push(row?);
        }
        Ok(dirs)
    }

    /// 更新目录的 last_scan 时间戳
    pub fn update_dir_last_scan(&self, dir_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE bound_dirs SET last_scan = ?1 WHERE id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    // --- indexed_files ---

    /// 查询已索引文件
    pub fn get_indexed_file(&self, path: &str) -> Result<Option<IndexedFileRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, dir_id, path, file_hash, item_id FROM indexed_files WHERE path = ?1",
        )?;
        let result = stmt.query_row(params![path], |row| {
            Ok(IndexedFileRow {
                id: row.get(0)?,
                dir_id: row.get(1)?,
                path: row.get(2)?,
                file_hash: row.get(3)?,
                item_id: row.get(4)?,
            })
        });
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 插入或更新已索引文件记录（INSERT OR REPLACE 原子操作，消除 check-then-act 竞态）
    pub fn upsert_indexed_file(
        &self,
        dir_id: &str,
        path: &str,
        file_hash: &str,
        item_id: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.conn.execute(
            "INSERT INTO indexed_files (id, dir_id, path, file_hash, item_id, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(path) DO UPDATE SET
               dir_id = excluded.dir_id,
               file_hash = excluded.file_hash,
               item_id = excluded.item_id,
               indexed_at = excluded.indexed_at",
            params![id, dir_id, path, file_hash, item_id, now],
        )?;
        Ok(())
    }

    // --- embed_queue ---

    /// 将文本块加入 embedding 队列
    pub fn enqueue_embedding(
        &self,
        item_id: &str,
        chunk_idx: usize,
        chunk_text: &str,
        priority: i32,
        level: i32,
        section_idx: usize,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO embed_queue (item_id, chunk_idx, chunk_text, level, section_idx, priority, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![item_id, chunk_idx as i64, chunk_text.as_bytes(), level, section_idx as i64, priority, now],
        )?;
        Ok(())
    }

    /// 从队列中取出一批 pending 任务，标记为 processing
    /// SELECT + UPDATE 在同一事务中执行，防止并发 worker 重复拾取同一任务。
    pub fn dequeue_embeddings(&self, batch_size: usize) -> Result<Vec<QueueTask>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut stmt = tx.prepare(
            "SELECT id, item_id, chunk_idx, chunk_text, level, section_idx, priority, attempts, task_type
             FROM embed_queue WHERE status = 'pending'
             ORDER BY priority ASC, created_at ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![batch_size as i64], |row| {
            let chunk_blob: Vec<u8> = row.get(3)?;
            Ok(QueueTask {
                id: row.get(0)?,
                item_id: row.get(1)?,
                chunk_idx: row.get(2)?,
                chunk_text: String::from_utf8_lossy(&chunk_blob).into_owned(),
                level: row.get(4)?,
                section_idx: row.get(5)?,
                priority: row.get(6)?,
                attempts: row.get(7)?,
                task_type: row.get(8)?,
            })
        })?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        drop(stmt);
        // 批量标记为 processing（与 SELECT 在同一事务内，防止并发重复拾取）
        for task in &tasks {
            tx.execute(
                "UPDATE embed_queue SET status = 'processing' WHERE id = ?1",
                params![task.id],
            )?;
        }
        tx.commit()?;
        Ok(tasks)
    }

    /// 标记队列任务为完成
    pub fn mark_embedding_done(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE embed_queue SET status = 'done' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// 标记队列任务为失败，超过最大尝试次数则标记为 abandoned
    pub fn mark_embedding_failed(&self, id: i64, max_attempts: i32) -> Result<()> {
        self.conn.execute(
            "UPDATE embed_queue SET attempts = attempts + 1 WHERE id = ?1",
            params![id],
        )?;
        let attempts: i32 = self.conn.query_row(
            "SELECT attempts FROM embed_queue WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if attempts >= max_attempts {
            self.conn.execute(
                "UPDATE embed_queue SET status = 'abandoned' WHERE id = ?1",
                params![id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE embed_queue SET status = 'pending' WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(())
    }

    /// 查询 pending 状态的队列任务数量
    pub fn pending_embedding_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 按 task_type 查询 pending 状态任务数量（用于进度推送）
    pub fn pending_count_by_type(&self, task_type: &str) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE status = 'pending' AND task_type = ?1",
            [task_type],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// 为 item 入队一个分类任务 (task_type='classify')
    pub fn enqueue_classify(&self, item_id: &str, priority: i32) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO embed_queue (item_id, chunk_idx, chunk_text, level, section_idx, priority, status, created_at, task_type)
             VALUES (?1, 0, ?2, 0, 0, ?3, 'pending', ?4, 'classify')",
            params![item_id, Vec::<u8>::new(), priority, now],
        )?;
        Ok(())
    }

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

    /// 将 processing 任务重新标记为 pending（用于未实现处理时占位）
    pub fn mark_task_pending(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE embed_queue SET status = 'pending' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

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

    // ── Conversation Session CRUD ─────────────────────────────────────────────

    pub fn create_conversation(&self, dek: &Key32, title: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let enc_title = crypto::encrypt(dek, title.as_bytes())?;
        self.conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, enc_title, now],
        )?;
        Ok(id)
    }

    pub fn list_conversations(&self, dek: &Key32, limit: usize, offset: usize) -> Result<Vec<ConversationSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, created_at, updated_at FROM conversations
             ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            let enc_title: Vec<u8> = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                enc_title,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            let (id, enc_title, created_at, updated_at) = row.map_err(VaultError::Database)?;
            let title = String::from_utf8(crypto::decrypt(dek, &enc_title)?)
                .map_err(|e| VaultError::Crypto(format!("conversation title utf8: {e}")))?;
            results.push(ConversationSummary { id, title, created_at, updated_at });
        }
        Ok(results)
    }

    pub fn get_conversation_messages(&self, dek: &Key32, conv_id: &str) -> Result<Vec<ConvMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content, citations, created_at
             FROM conversation_messages
             WHERE conversation_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![conv_id], |row| {
            let enc_content: Vec<u8> = row.get(2)?;
            let citations_json: Option<String> = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                enc_content,
                citations_json,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            let (id, role, enc_content, citations_json, created_at) = row.map_err(VaultError::Database)?;
            let content = String::from_utf8(crypto::decrypt(dek, &enc_content)?)
                .map_err(|e| VaultError::Crypto(format!("conversation message utf8: {e}")))?;
            let citations: Vec<Citation> = citations_json
                .and_then(|j| serde_json::from_str::<Vec<Citation>>(&j).ok())
                .unwrap_or_default();
            results.push(ConvMessage { id, role, content, citations, created_at });
        }
        Ok(results)
    }

    pub fn append_message(
        &self,
        dek: &Key32,
        conv_id: &str,
        role: &str,
        content: &str,
        citations: &[Citation],
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let enc = crypto::encrypt(dek, content.as_bytes())?;
        let citations_json: Option<String> = if citations.is_empty() {
            None
        } else {
            Some(serde_json::to_string(citations)
                .map_err(|e| VaultError::Json(e))?)
        };
        self.conn.execute(
            "INSERT INTO conversation_messages (id, conversation_id, role, content, citations, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, conv_id, role, enc, citations_json, now],
        )?;
        // Update conversation updated_at
        self.conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, conv_id],
        )?;
        Ok(id)
    }

    /// 在单一事务中写入 user + assistant 一对消息，保证原子性。
    /// 若 user 写入成功但 assistant 失败，事务回滚，两条均不写入。
    pub fn append_conversation_turn(
        &self,
        dek: &Key32,
        conv_id: &str,
        user_content: &str,
        assistant_content: &str,
        citations: &[Citation],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        // user message
        let user_enc = crypto::encrypt(dek, user_content.as_bytes())?;
        let user_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO conversation_messages (id, conversation_id, role, content, citations, created_at)
             VALUES (?1, ?2, 'user', ?3, NULL, ?4)",
            params![user_id, conv_id, user_enc, now],
        )?;

        // assistant message
        let asst_enc = crypto::encrypt(dek, assistant_content.as_bytes())?;
        let asst_id = uuid::Uuid::new_v4().to_string();
        let citations_json: Option<String> = if citations.is_empty() {
            None
        } else {
            Some(serde_json::to_string(citations).map_err(VaultError::Json)?)
        };
        tx.execute(
            "INSERT INTO conversation_messages (id, conversation_id, role, content, citations, created_at)
             VALUES (?1, ?2, 'assistant', ?3, ?4, ?5)",
            params![asst_id, conv_id, asst_enc, citations_json, now],
        )?;

        // update conversation timestamp
        tx.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, conv_id],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn delete_conversation(&self, conv_id: &str) -> Result<()> {
        // CASCADE 会自动删 conversation_messages
        self.conn.execute("DELETE FROM conversations WHERE id = ?1", params![conv_id])?;
        Ok(())
    }

    pub fn get_conversation_by_id(&self, dek: &Key32, conv_id: &str) -> Result<Option<ConversationSummary>> {
        use rusqlite::OptionalExtension;
        let row = self.conn
            .query_row(
                "SELECT id, title, created_at, updated_at FROM conversations WHERE id = ?1",
                params![conv_id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                )),
            )
            .optional()
            .map_err(VaultError::Database)?;
        match row {
            Some((id, enc_title, created_at, updated_at)) => {
                let title = String::from_utf8(crypto::decrypt(dek, &enc_title)?)
                .map_err(|e| VaultError::Crypto(format!("conversation title utf8: {e}")))?;
                Ok(Some(ConversationSummary { id, title, created_at, updated_at }))
            }
            None => Ok(None),
        }
    }
}

// --- 数据结构 ---

struct RawItem {
    id: String,
    title: String,
    content: Vec<u8>,
    url: Option<String>,
    source_type: String,
    domain: Option<String>,
    tags: Option<Vec<u8>>,
    created_at: String,
    updated_at: String,
}

impl RawItem {
    fn decrypt(self, dek: &Key32) -> Result<DecryptedItem> {
        let content = String::from_utf8(crypto::decrypt(dek, &self.content)?)
            .map_err(|e| VaultError::Crypto(format!("utf8: {e}")))?;
        let tags: Option<Vec<String>> = match self.tags {
            Some(ref enc) => Some(serde_json::from_slice(&crypto::decrypt(dek, enc)?)?),
            None => None,
        };
        Ok(DecryptedItem {
            id: self.id,
            title: self.title,
            content,
            url: self.url,
            source_type: self.source_type,
            domain: self.domain,
            tags,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DecryptedItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub source_type: String,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemSummary {
    pub id: String,
    pub title: String,
    pub source_type: String,
    pub domain: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StaleItemSummary {
    pub id: String,
    pub title: String,
    pub source_type: String,
    pub updated_at: String,
    pub created_at: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ItemStats {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub chunk_count: i64,
    pub embedding_pending: i64,
    pub embedding_done: i64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FeedbackEntry {
    pub id: i64,
    pub item_id: String,
    pub feedback_type: String,
    pub query: Option<String>,
    pub created_at: String,
}

/// Embedding 队列任务
#[derive(Debug)]
pub struct QueueTask {
    pub id: i64,
    pub item_id: String,
    pub chunk_idx: i32,
    pub chunk_text: String,
    pub level: i32,
    pub section_idx: i32,
    pub priority: i32,
    pub attempts: i32,
    pub task_type: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BoundDirRow {
    pub id: String,
    pub path: String,
    pub recursive: bool,
    pub file_types: String,
    pub last_scan: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHistoryRow {
    pub id: i64,
    pub query: String,
    pub result_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct IndexedFileRow {
    pub id: String,
    pub dir_id: String,
    pub path: String,
    pub file_hash: String,
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub item_id: String,
    pub title: String,
    pub relevance: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub citations: Vec<Citation>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dek() -> Key32 {
        Key32::generate()
    }

    #[test]
    fn open_memory_creates_tables() {
        let store = Store::open_memory().unwrap();
        assert!(store.has_meta("nonexistent").unwrap() == false);
    }

    #[test]
    fn meta_set_get_roundtrip() {
        let store = Store::open_memory().unwrap();
        store.set_meta("salt", b"test-salt-value").unwrap();
        let value = store.get_meta("salt").unwrap().unwrap();
        assert_eq!(value, b"test-salt-value");
    }

    #[test]
    fn meta_overwrite() {
        let store = Store::open_memory().unwrap();
        store.set_meta("key", b"v1").unwrap();
        store.set_meta("key", b"v2").unwrap();
        assert_eq!(store.get_meta("key").unwrap().unwrap(), b"v2");
    }

    #[test]
    fn insert_and_get_item() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store
            .insert_item(
                &dek,
                "Test Title",
                "Secret content",
                Some("https://example.com"),
                "note",
                Some("example.com"),
                Some(&["tag1".into(), "tag2".into()]),
            )
            .unwrap();

        let item = store.get_item(&dek, &id).unwrap().unwrap();
        assert_eq!(item.title, "Test Title");
        assert_eq!(item.content, "Secret content");
        assert_eq!(item.url.as_deref(), Some("https://example.com"));
        assert_eq!(item.source_type, "note");
        assert_eq!(item.tags.unwrap(), vec!["tag1", "tag2"]);
    }

    #[test]
    fn get_item_wrong_dek_fails() {
        let store = Store::open_memory().unwrap();
        let dek1 = test_dek();
        let dek2 = test_dek();

        let id = store
            .insert_item(&dek1, "Title", "Secret", None, "note", None, None)
            .unwrap();
        let result = store.get_item(&dek2, &id);
        assert!(result.is_err(), "Should fail with wrong DEK");
    }

    #[test]
    fn content_stored_encrypted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store
            .insert_item(&dek, "Title", "Plaintext secret", None, "note", None, None)
            .unwrap();

        // 直接读取原始 BLOB，验证不是明文
        let raw: Vec<u8> = store
            .conn
            .query_row("SELECT content FROM items WHERE id = ?1", params![id], |row| {
                row.get(0)
            })
            .unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(
            !raw_str.contains("Plaintext secret"),
            "Content should be encrypted in DB"
        );
    }

    #[test]
    fn list_items_returns_summaries() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        store
            .insert_item(&dek, "Item 1", "content1", None, "note", None, None)
            .unwrap();
        store
            .insert_item(
                &dek,
                "Item 2",
                "content2",
                None,
                "webpage",
                Some("example.com"),
                None,
            )
            .unwrap();

        let items = store.list_items(10, 0).unwrap();
        assert_eq!(items.len(), 2);
        // list_items 不包含 content（不需解密）
        assert!(items.iter().any(|i| i.title == "Item 1"));
        assert!(items.iter().any(|i| i.title == "Item 2"));
    }

    #[test]
    fn delete_item_soft_deletes() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store
            .insert_item(&dek, "To Delete", "secret", None, "note", None, None)
            .unwrap();
        assert_eq!(store.item_count().unwrap(), 1);

        assert!(store.delete_item(&id).unwrap());
        assert_eq!(store.item_count().unwrap(), 0);
        assert!(store.get_item(&dek, &id).unwrap().is_none());
    }

    #[test]
    fn item_count_excludes_deleted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id1 = store
            .insert_item(&dek, "A", "a", None, "note", None, None)
            .unwrap();
        store
            .insert_item(&dek, "B", "b", None, "note", None, None)
            .unwrap();
        assert_eq!(store.item_count().unwrap(), 2);

        store.delete_item(&id1).unwrap();
        assert_eq!(store.item_count().unwrap(), 1);
    }

    #[test]
    fn task_type_column_migration() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let id = store.insert_item(&dek, "T", "C", None, "note", None, None).unwrap();
        store.enqueue_classify(&id, 3).unwrap();
        let tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_type, "classify");
        assert_eq!(tasks[0].item_id, id);
    }

    #[test]
    fn update_and_get_tags() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let id = store.insert_item(&dek, "T", "C", None, "note", None, None).unwrap();
        let tags_json = r#"{"core":{"domain":["技术"]}}"#;
        assert!(store.update_tags(&dek, &id, tags_json).unwrap());
        let retrieved = store.get_tags_json(&dek, &id).unwrap().unwrap();
        assert_eq!(retrieved, tags_json);
    }

    #[test]
    fn log_and_recent_searches() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        store.log_search(&dek, "rust crypto", 5).unwrap();
        store.log_search(&dek, "python script", 3).unwrap();
        store.log_search(&dek, "法律合同", 7).unwrap();

        let history = store.recent_searches(&dek, 10).unwrap();
        assert_eq!(history.len(), 3);
        // 最新的应该在前
        assert_eq!(history[0].query, "法律合同");
        assert_eq!(history[0].result_count, 7);
    }

    #[test]
    fn log_click_and_popular() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        store.log_click(&dek, "q1", "item-a").unwrap();
        store.log_click(&dek, "q2", "item-a").unwrap();
        store.log_click(&dek, "q3", "item-b").unwrap();
        store.log_click(&dek, "q4", "item-a").unwrap();

        let popular = store.popular_items(10).unwrap();
        assert_eq!(popular.len(), 2);
        assert_eq!(popular[0].0, "item-a");
        assert_eq!(popular[0].1, 3);
        assert_eq!(popular[1].0, "item-b");
        assert_eq!(popular[1].1, 1);
    }

    #[test]
    fn search_history_query_encrypted_at_rest() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        store.log_search(&dek, "SECRET_QUERY_XYZ", 0).unwrap();

        // Read raw row
        let raw: Vec<u8> = store.conn.query_row(
            "SELECT query FROM search_history LIMIT 1",
            [],
            |row| row.get(0),
        ).unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(!raw_str.contains("SECRET_QUERY_XYZ"), "Query should be encrypted");
    }

    #[test]
    fn list_all_item_ids_excludes_deleted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let a = store.insert_item(&dek, "A", "c", None, "note", None, None).unwrap();
        store.insert_item(&dek, "B", "c", None, "note", None, None).unwrap();
        let c = store.insert_item(&dek, "C", "c", None, "note", None, None).unwrap();
        store.delete_item(&c).unwrap();
        let ids = store.list_all_item_ids().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a));
    }

    #[test]
    fn list_stale_items_basic() {
        use chrono::{Duration, Utc};
        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let id = store.insert_item(&dek, "New", "content", None, "note", None, None).unwrap();
        let old_ts = (Utc::now() - Duration::days(40)).format("%Y-%m-%dT%H:%M:%S").to_string();
        store.set_updated_at(&id, &old_ts).unwrap();
        let stale = store.list_stale_items(30, 50).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, id);
    }

    #[test]
    fn list_stale_items_empty() {
        let store = Store::open_memory().unwrap();
        let stale = store.list_stale_items(30, 50).unwrap();
        assert!(stale.is_empty());
    }

    #[test]
    fn get_item_stats_basic() {
        let store = Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        let id = store.insert_item(&dek, "Test", "content", None, "note", None, None).unwrap();
        let stats = store.get_item_stats(&id).unwrap().unwrap();
        assert_eq!(stats.id, id);
        assert!(stats.chunk_count >= 0);
        assert_eq!(stats.embedding_pending + stats.embedding_done, stats.chunk_count);
    }

    #[test]
    fn get_item_stats_missing() {
        let store = Store::open_memory().unwrap();
        let result = store.get_item_stats("nonexistent-id").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn insert_feedback_valid() {
        let store = Store::open_memory().unwrap();
        let id = store.insert_feedback("item-1", "relevant", Some("my query")).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn insert_feedback_invalid_type() {
        let store = Store::open_memory().unwrap();
        let result = store.insert_feedback("item-1", "bad_type", None);
        assert!(result.is_err());
    }

    #[test]
    fn insert_feedback_no_query() {
        let store = Store::open_memory().unwrap();
        let id = store.insert_feedback("item-1", "irrelevant", None).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_create_and_list_conversations() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let id1 = store.create_conversation(&dek, "第一个会话").unwrap();
        let _id2 = store.create_conversation(&dek, "第二个会话").unwrap();
        let list = store.list_conversations(&dek, 10, 0).unwrap();
        assert_eq!(list.len(), 2);
        let ids: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&id1.as_str()));
    }

    #[test]
    fn test_append_and_get_messages() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let conv_id = store.create_conversation(&dek, "测试会话").unwrap();
        store.append_message(&dek, &conv_id, "user", "你好", &[]).unwrap();
        store.append_message(&dek, &conv_id, "assistant", "你好！有什么可以帮你的？", &[]).unwrap();
        let msgs = store.get_conversation_messages(&dek, &conv_id).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "你好");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn test_delete_conversation_cascades() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let conv_id = store.create_conversation(&dek, "待删除").unwrap();
        store.append_message(&dek, &conv_id, "user", "消息内容", &[]).unwrap();
        store.delete_conversation(&conv_id).unwrap();
        let msgs = store.get_conversation_messages(&dek, &conv_id).unwrap();
        assert!(msgs.is_empty());
        let list = store.list_conversations(&dek, 10, 0).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_citations_json_roundtrip() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let conv_id = store.create_conversation(&dek, "带引用").unwrap();
        let citations = vec![
            Citation { item_id: "abc".to_string(), title: "文档A".to_string(), relevance: 0.9 },
        ];
        store.append_message(&dek, &conv_id, "assistant", "回答内容", &citations).unwrap();
        let msgs = store.get_conversation_messages(&dek, &conv_id).unwrap();
        assert_eq!(msgs[0].citations.len(), 1);
        assert_eq!(msgs[0].citations[0].item_id, "abc");
    }

    // #12: append_message 外键约束（conv_id 不存在）
    #[test]
    fn test_append_message_nonexistent_conv_fails() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        // 直接向不存在的 conv_id 追加消息，应返回 Err（外键约束失败）
        let result = store.append_message(&dek, "nonexistent-conv-id", "user", "hello", &[]);
        assert!(result.is_err(), "append_message to nonexistent conversation should fail");
    }

    // #13: get_conversation_by_id 不存在返回 None
    #[test]
    fn test_get_conversation_by_id_not_found() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();
        let result = store.get_conversation_by_id(&dek, "does-not-exist").unwrap();
        assert!(result.is_none());
    }
}

#[cfg(test)]
mod tests_dir {
    use super::*;

    fn open_store() -> Store {
        Store::open_memory().unwrap()
    }

    #[test]
    fn test_bind_directory_returns_id() {
        let store = open_store();
        let id = store.bind_directory("/tmp/docs", true, &["md", "txt"]).unwrap();
        assert!(!id.is_empty());
    }

    #[test]
    fn test_list_bound_directories_after_bind() {
        let store = open_store();
        store.bind_directory("/tmp/docs", true, &["md"]).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].path, "/tmp/docs");
    }

    #[test]
    fn test_bind_multiple_directories() {
        let store = open_store();
        store.bind_directory("/tmp/a", false, &["txt"]).unwrap();
        store.bind_directory("/tmp/b", true, &["md"]).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn test_unbind_directory_marks_inactive() {
        let store = open_store();
        let id = store.bind_directory("/tmp/docs", true, &["md"]).unwrap();
        store.unbind_directory(&id).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 0);
    }

    #[test]
    fn test_unbind_nonexistent_returns_err() {
        let store = open_store();
        let result = store.unbind_directory("nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_dir_last_scan() {
        let store = open_store();
        let id = store.bind_directory("/tmp/docs", false, &["md"]).unwrap();
        store.update_dir_last_scan(&id).unwrap();
        let dirs = store.list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].last_scan.is_some());
    }
}

#[cfg(test)]
mod tests_indexed_files {
    use super::*;

    fn open_store() -> Store {
        Store::open_memory().unwrap()
    }

    fn insert_test_item(store: &Store) -> String {
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(&dek, "test title", "test content", None, "note", None, None)
            .unwrap()
    }

    #[test]
    fn test_get_indexed_file_returns_none_for_unknown() {
        let store = open_store();
        let result = store.get_indexed_file("/nonexistent.md").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_upsert_indexed_file_insert() {
        let store = open_store();
        let dir_id = store.bind_directory("/tmp/docs", false, &["md"]).unwrap();
        let item_id = insert_test_item(&store);
        store
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "abc123", &item_id)
            .unwrap();
        let row = store.get_indexed_file("/tmp/docs/note.md").unwrap();
        assert!(row.is_some());
        let row = row.unwrap();
        assert_eq!(row.file_hash, "abc123");
        assert_eq!(row.item_id.as_deref(), Some(item_id.as_str()));
    }

    #[test]
    fn test_upsert_indexed_file_updates_hash() {
        let store = open_store();
        let dir_id = store.bind_directory("/tmp/docs", false, &["md"]).unwrap();
        let item_id = insert_test_item(&store);
        store
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "v1", &item_id)
            .unwrap();
        store
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "v2", &item_id)
            .unwrap();
        let row = store
            .get_indexed_file("/tmp/docs/note.md")
            .unwrap()
            .unwrap();
        assert_eq!(row.file_hash, "v2");
    }
}

#[cfg(test)]
mod tests_embed_queue {
    use super::*;

    fn open_store() -> Store {
        Store::open_memory().unwrap()
    }

    fn insert_test_item(store: &Store) -> String {
        let dek = crate::crypto::Key32::generate();
        store
            .insert_item(&dek, "title", "content", None, "note", None, None)
            .unwrap()
    }

    #[test]
    fn test_enqueue_embedding_adds_to_queue() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk text", 1, 1, 0)
            .unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_dequeue_embeddings_returns_tasks() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk A", 1, 1, 0)
            .unwrap();
        store
            .enqueue_embedding(&item_id, 1, "chunk B", 1, 2, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 2);
        // dequeue 后状态变为 processing，pending 计数应为 0
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
    }

    #[test]
    fn test_dequeue_respects_batch_size() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        for i in 0..5 {
            store
                .enqueue_embedding(&item_id, i, &format!("chunk {i}"), 1, 1, 0)
                .unwrap();
        }
        let tasks = store.dequeue_embeddings(3).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(store.pending_embedding_count().unwrap(), 2);
    }

    #[test]
    fn test_mark_embedding_done_removes_from_active() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        store.mark_embedding_done(tasks[0].id).unwrap();
        // done 状态不再是 pending 或 processing，再次 dequeue 应为空
        let re_tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(re_tasks.len(), 0);
    }

    #[test]
    fn test_mark_embedding_failed_retries_within_max() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        // max_attempts=3，第一次失败后 attempts=1 < 3，应重新变为 pending
        store.mark_embedding_failed(tasks[0].id, 3).unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_mark_embedding_failed_abandons_after_max() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        // 连续失败 3 次（max_attempts=3），第3次后状态变为 abandoned
        for _ in 0..3 {
            let tasks = store.dequeue_embeddings(1).unwrap();
            if tasks.is_empty() {
                break;
            }
            store.mark_embedding_failed(tasks[0].id, 3).unwrap();
        }
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
    }

    #[test]
    fn test_mark_task_pending_restores_processing() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        store
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        // dequeue 后变为 processing，pending 计数为 0
        assert_eq!(store.pending_embedding_count().unwrap(), 0);
        store.mark_task_pending(tasks[0].id).unwrap();
        assert_eq!(store.pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_checkpoint_does_not_error() {
        let store = open_store();
        // open_memory 使用内存数据库，wal_checkpoint 是 no-op 但不应报错
        store.checkpoint().unwrap();
    }

    #[test]
    fn test_enqueue_chunk_text_preserved() {
        let store = open_store();
        let item_id = insert_test_item(&store);
        let text = "Unicode text: 中文 \u{1F511}";
        store
            .enqueue_embedding(&item_id, 0, text, 1, 1, 0)
            .unwrap();
        let tasks = store.dequeue_embeddings(1).unwrap();
        assert_eq!(tasks[0].chunk_text, text);
    }
}
