// npu-vault/crates/vault-core/src/store.rs

use rusqlite::{params, Connection};
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
                Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
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
    pub fn set_meta_batch(&self, entries: &[(&str, &[u8])]) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let exec_result: Result<()> = (|| {
            for (key, value) in entries {
                self.conn.execute(
                    "INSERT OR REPLACE INTO vault_meta (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, value],
                )?;
            }
            Ok(())
        })();
        match exec_result {
            Ok(_) => {
                if let Err(e) = self.conn.execute_batch("COMMIT") {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(e.into());
                }
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
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

    /// 插入或更新已索引文件记录
    pub fn upsert_indexed_file(
        &self,
        dir_id: &str,
        path: &str,
        file_hash: &str,
        item_id: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        // 先检查是否已存在
        if let Some(existing) = self.get_indexed_file(path)? {
            self.conn.execute(
                "UPDATE indexed_files SET dir_id = ?1, file_hash = ?2, item_id = ?3, indexed_at = ?4
                 WHERE id = ?5",
                params![dir_id, file_hash, item_id, now, existing.id],
            )?;
        } else {
            let id = uuid::Uuid::new_v4().simple().to_string();
            self.conn.execute(
                "INSERT INTO indexed_files (id, dir_id, path, file_hash, item_id, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, dir_id, path, file_hash, item_id, now],
            )?;
        }
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
    pub fn dequeue_embeddings(&self, batch_size: usize) -> Result<Vec<QueueTask>> {
        let mut stmt = self.conn.prepare(
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
        // 批量标记为 processing
        for task in &tasks {
            self.conn.execute(
                "UPDATE embed_queue SET status = 'processing' WHERE id = ?1",
                params![task.id],
            )?;
        }
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
                Ok(Some(String::from_utf8_lossy(&decrypted).to_string()))
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
            let decrypted = crypto::decrypt(dek, &encrypted_query)
                .unwrap_or_else(|_| Vec::new());
            let query = String::from_utf8_lossy(&decrypted).to_string();
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
}
