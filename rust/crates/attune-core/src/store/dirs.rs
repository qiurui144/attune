//! bound_dirs / indexed_files 表 — 目录绑定 + 索引文件追踪。

use rusqlite::params;

use crate::error::{Result, VaultError};
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    // --- bound_dirs ---

    /// 绑定监控目录，返回 dir_id（默认 corpus_domain='general'）
    pub fn bind_directory(&self, path: &str, recursive: bool, file_types: &[&str]) -> Result<String> {
        self.bind_directory_with_domain(path, recursive, file_types, "general")
    }

    /// v0.6 Phase B F-Pro：bind 时声明 corpus 领域（legal / tech / medical / patent / general）。
    /// scanner 后续 ingest 时把 items.corpus_domain = 该领域，driving cross-domain penalty。
    pub fn bind_directory_with_domain(
        &self,
        path: &str,
        recursive: bool,
        file_types: &[&str],
        corpus_domain: &str,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let ft_json = serde_json::to_string(file_types)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO bound_dirs (id, path, recursive, file_types, is_active, corpus_domain)
             VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            params![id, path, recursive as i32, ft_json, corpus_domain],
        )?;
        Ok(id)
    }

    /// 读取 bound_dir 的 corpus_domain；找不到回退 'general'
    pub fn get_dir_corpus_domain(&self, dir_id: &str) -> Result<String> {
        let domain: String = self
            .conn
            .query_row(
                "SELECT corpus_domain FROM bound_dirs WHERE id = ?1",
                params![dir_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "general".to_string());
        Ok(domain)
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
}
