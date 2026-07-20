//! bound_dirs / indexed_files 表 — 目录绑定 + 索引文件追踪。

use rusqlite::{params, Row};

use crate::error::{Result, VaultError};
use crate::store::Store;

#[allow(unused_imports)]
use crate::store::types::*;

impl Store {
    // --- bound_dirs ---

    /// 绑定监控目录，返回 dir_id（默认 corpus_domain='general'）
    pub fn bind_directory(
        &self,
        path: &str,
        recursive: bool,
        file_types: &[&str],
    ) -> Result<String> {
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
        let ft_json = serde_json::to_string(file_types)?;

        // path UNIQUE: 已绑定 → 走 UPDATE 路径而非 REPLACE
        // (REPLACE = DELETE + INSERT, 会让 indexed_files.dir_id FK 失败,
        //  per feedback_reset_vault_incomplete_wipe)
        if let Ok(existing_id) = self.conn.query_row(
            "SELECT id FROM bound_dirs WHERE path = ?1",
            params![path],
            |r| r.get::<_, String>(0),
        ) {
            self.conn.execute(
                "UPDATE bound_dirs SET recursive=?1, file_types=?2, is_active=1, corpus_domain=?3 WHERE id=?4",
                params![recursive as i32, ft_json, corpus_domain, existing_id],
            )?;
            return Ok(existing_id);
        }

        // 新增
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.conn.execute(
            "INSERT INTO bound_dirs (id, path, recursive, file_types, is_active, corpus_domain)
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
            "SELECT id, dir_id, path, file_hash, item_id,
                    file_size, file_mtime_ns, file_ctime_ns, file_inode, file_dev
             FROM indexed_files WHERE path = ?1",
        )?;
        let result = stmt.query_row(params![path], indexed_file_row_from_sql);
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 列出某 bound_dir 下所有已索引文件（path + item_id）。
    /// git 增量同步用：比对本次 fetch 的文件集，找出上游已删除的文件。
    pub fn list_indexed_files_for_dir(&self, dir_id: &str) -> Result<Vec<IndexedFileRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, dir_id, path, file_hash, item_id,
                    file_size, file_mtime_ns, file_ctime_ns, file_inode, file_dev
             FROM indexed_files WHERE dir_id = ?1",
        )?;
        let rows = stmt.query_map(params![dir_id], indexed_file_row_from_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 列出某文件系统路径前缀下所有已索引文件的 item_id（"文件夹一键整理"按目录圈选）。
    /// prefix LIKE 匹配：传一个目录路径 → 命中该目录及其子目录下所有索引文件。
    /// WHY LIKE 而非递归遍历:indexed_files.path 是绝对路径,前缀即可圈定子树,无需触盘。
    /// 顺带返回的是去重后的 item_id（一文件一 item;同 item 多文件极少见,DISTINCT 兜底）。
    pub fn list_item_ids_under_path(&self, prefix: &str) -> Result<Vec<String>> {
        // ESCAPE '\\': 用户路径可能含 LIKE 元字符 % / _,转义后按字面匹配,
        // 避免 "/a_b" 误命中 "/axb"。前缀已转义,再拼通配 '%'。
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{escaped}%");
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT item_id FROM indexed_files WHERE path LIKE ?1 ESCAPE '\\'",
        )?;
        let rows = stmt.query_map(params![pattern], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    }

    /// 删除一条 indexed_files 记录（按 path）。git 增量删除上游消失文件时调。
    pub fn delete_indexed_file(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM indexed_files WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// 插入或更新已索引文件记录（INSERT OR REPLACE 原子操作，消除 check-then-act 竞态）
    pub fn upsert_indexed_file(
        &self,
        dir_id: &str,
        path: &str,
        file_hash: &str,
        item_id: &str,
    ) -> Result<()> {
        self.upsert_indexed_file_with_stat(dir_id, path, file_hash, item_id, None)
    }

    pub fn upsert_indexed_file_with_stat(
        &self,
        dir_id: &str,
        path: &str,
        file_hash: &str,
        item_id: &str,
        stat: Option<IndexedFileStatMarker>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.conn.execute(
            "INSERT INTO indexed_files
                (id, dir_id, path, file_hash, item_id,
                 file_size, file_mtime_ns, file_ctime_ns, file_inode, file_dev, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(path) DO UPDATE SET
               dir_id = excluded.dir_id,
               file_hash = excluded.file_hash,
               item_id = excluded.item_id,
               file_size = excluded.file_size,
               file_mtime_ns = excluded.file_mtime_ns,
               file_ctime_ns = excluded.file_ctime_ns,
               file_inode = excluded.file_inode,
               file_dev = excluded.file_dev,
               indexed_at = excluded.indexed_at",
            params![
                id,
                dir_id,
                path,
                file_hash,
                item_id,
                stat.map(|s| s.size),
                stat.map(|s| s.mtime_ns),
                stat.and_then(|s| s.ctime_ns),
                stat.and_then(|s| s.inode),
                stat.and_then(|s| s.dev),
                now
            ],
        )?;
        Ok(())
    }
}

fn indexed_file_row_from_sql(row: &Row<'_>) -> rusqlite::Result<IndexedFileRow> {
    let file_size: Option<i64> = row.get(5)?;
    let file_mtime_ns: Option<i64> = row.get(6)?;
    let stat = match (file_size, file_mtime_ns) {
        (Some(size), Some(mtime_ns)) => Some(IndexedFileStatMarker {
            size,
            mtime_ns,
            ctime_ns: row.get(7)?,
            inode: row.get(8)?,
            dev: row.get(9)?,
        }),
        _ => None,
    };
    Ok(IndexedFileRow {
        id: row.get(0)?,
        dir_id: row.get(1)?,
        path: row.get(2)?,
        file_hash: row.get(3)?,
        item_id: row.get(4)?,
        stat,
    })
}

#[cfg(test)]
mod tests {
    use crate::crypto::Key32;
    use crate::store::Store;

    /// Bind a dir, insert real items, link them via indexed_files, then verify
    /// `list_item_ids_under_path` selects the subtree (and LIKE metachars in the
    /// prefix match literally, not as wildcards).
    #[test]
    fn list_item_ids_under_path_selects_subtree() {
        let s = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let dir_id = s.bind_directory("/docs", true, &["txt"]).unwrap();

        let mk = |name: &str, path: &str| -> String {
            let id = s
                .insert_item(&dek, name, "body", None, "note", None, None)
                .unwrap();
            s.upsert_indexed_file(&dir_id, path, "h", &id).unwrap();
            id
        };
        let a = mk("a", "/docs/a.txt");
        let b = mk("b", "/docs/sub/b.txt");
        let _c = mk("c", "/other/c.txt"); // outside prefix

        let mut got = s.list_item_ids_under_path("/docs").unwrap();
        got.sort();
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(got, want, "subtree under /docs, excluding /other");
    }

    #[test]
    fn list_item_ids_under_path_escapes_like_metachars() {
        let s = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let dir_id = s.bind_directory("/root", true, &["txt"]).unwrap();

        let id_underscore = s
            .insert_item(&dek, "u", "b", None, "note", None, None)
            .unwrap();
        s.upsert_indexed_file(&dir_id, "/a_b/x.txt", "h", &id_underscore)
            .unwrap();
        let id_decoy = s
            .insert_item(&dek, "d", "b", None, "note", None, None)
            .unwrap();
        s.upsert_indexed_file(&dir_id, "/axb/y.txt", "h", &id_decoy)
            .unwrap();

        // "/a_b" must match only the literal underscore path, NOT "/axb".
        let got = s.list_item_ids_under_path("/a_b").unwrap();
        assert_eq!(
            got,
            vec![id_underscore],
            "underscore is escaped, /axb not matched"
        );
    }

    #[test]
    fn bound_dir_row_parses_json_file_types_for_rescan_workers() {
        let s = Store::open_memory().unwrap();
        s.bind_directory("/docs", true, &["md", "txt"]).unwrap();

        let dirs = s.list_bound_directories().unwrap();

        assert_eq!(dirs[0].file_type_list(), vec!["md", "txt"]);
    }
}
