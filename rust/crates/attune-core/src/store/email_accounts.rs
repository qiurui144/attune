//! Email IMAP 采集账户持久化。
//!
//! 与 store/webdav_remotes.rs 同模式：周期同步 worker 要对邮箱自动按 UID
//! 增量重扫，必须能读回 IMAP 凭据。此表存每个 email: bound_dir 的完整账户
//! 配置，`password` 经字段级 AES-256-GCM 加密（dek，与 items.content 同模式）
//! 落 password_enc BLOB 列；明文密码绝不落盘。folder UID 增量游标单独存
//! email_folder_uids 表（每账户每文件夹一行）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

/// 写入用的 Email 账户配置（明文，调用方持有）。
#[derive(Debug, Clone)]
pub struct EmailAccountInput {
    /// 关联的 bound_dirs.id。
    pub dir_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 明文密码 / App Password；落库前由 `upsert_email_account` 用 dek 加密。
    pub password: String,
    /// 要同步的 IMAP 文件夹列表（默认 INBOX + Sent）。
    pub folders: Vec<String>,
    /// 语料领域（写入 RawDocument.corpus_domain，驱动 F-Pro 跨域防污染）。
    pub corpus_domain: String,
}

/// 从表里读出的 Email 账户配置（password 已解密回明文）。
#[derive(Debug, Clone)]
pub struct EmailAccountRow {
    pub dir_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub folders: Vec<String>,
    pub corpus_domain: String,
    pub last_sync: Option<String>,
}

/// folders 列表 ⇄ 逗号分隔字符串。空段过滤，避免 "INBOX,," 解析出空 folder。
fn join_folders(folders: &[String]) -> String {
    folders.join(",")
}
fn split_folders(s: &str) -> Vec<String> {
    s.split(',')
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(|f| f.to_string())
        .collect()
}

impl Store {
    /// upsert 一条 Email 账户配置。`password` 用 dek 加密成 BLOB 落盘。
    /// 同 `dir_id` 已存在则整行替换（幂等）。
    pub fn upsert_email_account(&self, dek: &Key32, input: &EmailAccountInput) -> Result<()> {
        let password_enc = crypto::encrypt(dek, input.password.as_bytes())?;
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO email_accounts
                (dir_id, host, port, username, password_enc, folders, corpus_domain, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(dir_id) DO UPDATE SET
                host=excluded.host,
                port=excluded.port,
                username=excluded.username,
                password_enc=excluded.password_enc,
                folders=excluded.folders,
                corpus_domain=excluded.corpus_domain,
                updated_at=excluded.updated_at",
            params![
                input.dir_id,
                input.host,
                input.port as i64,
                input.username,
                password_enc,
                join_folders(&input.folders),
                input.corpus_domain,
                now,
            ],
        )?;
        Ok(())
    }

    /// 读单条 Email 账户配置（password 解密回明文）。
    pub fn get_email_account(&self, dek: &Key32, dir_id: &str) -> Result<Option<EmailAccountRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync
             FROM email_accounts WHERE dir_id = ?1",
        )?;
        let row = stmt
            .query_row(params![dir_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })
            .ok();
        match row {
            None => Ok(None),
            Some((dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync)) => {
                let password = String::from_utf8(crypto::decrypt(dek, &password_enc)?)
                    .map_err(|e| VaultError::Crypto(format!("email password utf8: {e}")))?;
                Ok(Some(EmailAccountRow {
                    dir_id,
                    host,
                    port: port as u16,
                    username,
                    password,
                    folders: split_folders(&folders),
                    corpus_domain,
                    last_sync,
                }))
            }
        }
    }

    /// 列出全部 Email 账户配置（周期 worker 用，password 已解密）。
    pub fn list_email_accounts(&self, dek: &Key32) -> Result<Vec<EmailAccountRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync
             FROM email_accounts ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync) =
                row?;
            let password = String::from_utf8(crypto::decrypt(dek, &password_enc)?)
                .map_err(|e| VaultError::Crypto(format!("email password utf8: {e}")))?;
            out.push(EmailAccountRow {
                dir_id,
                host,
                port: port as u16,
                username,
                password,
                folders: split_folders(&folders),
                corpus_domain,
                last_sync,
            });
        }
        Ok(out)
    }

    /// 删除一条 Email 账户配置（email_folder_uids 经 ON DELETE CASCADE 一并清）。
    pub fn delete_email_account(&self, dir_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM email_accounts WHERE dir_id = ?1", params![dir_id])?;
        Ok(())
    }

    /// 记录某账户最近一次同步时间（周期 worker / 手动同步调用）。
    pub fn touch_email_account_sync(&self, dir_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE email_accounts SET last_sync = ?1 WHERE dir_id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    /// 读某账户某文件夹的 IMAP UID 增量游标（未设置回 0）。
    /// 仅「无此行」回退 0；真实 DB 错误必须上抛 —— 静默回 0 会让增量同步整箱重扫。
    pub fn get_folder_uid(&self, dir_id: &str, folder: &str) -> Result<u32> {
        match self.conn.query_row(
            "SELECT last_uid FROM email_folder_uids WHERE dir_id = ?1 AND folder = ?2",
            params![dir_id, folder],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(uid) => Ok(uid.max(0) as u32),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    /// 写某账户某文件夹的 IMAP UID 增量游标（upsert）。
    pub fn set_folder_uid(&self, dir_id: &str, folder: &str, last_uid: u32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO email_folder_uids (dir_id, folder, last_uid)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(dir_id, folder) DO UPDATE SET last_uid=excluded.last_uid",
            params![dir_id, folder, last_uid as i64],
        )?;
        Ok(())
    }

    /// 仅供集成测试用：取 password_enc 原始密文字节（验证不含明文）。
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn debug_raw_email_password_enc(&self, dir_id: &str) -> Result<Vec<u8>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT password_enc FROM email_accounts WHERE dir_id = ?1",
                params![dir_id],
                |r| r.get(0),
            )
            .ok();
        Ok(blob.unwrap_or_default())
    }
}
