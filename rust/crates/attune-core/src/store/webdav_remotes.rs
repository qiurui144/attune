//! WebDAV remote 配置持久化。
//!
//! 决策 4：周期同步 worker 要对认证源（Nextcloud / 坚果云等）自动增量重扫，
//! 必须能读回凭据。此表存每个 webdav: bound_dir 的完整连接配置，`password`
//! 经字段级 AES-256-GCM 加密（dek，与 `items.content` 同模式）落 `password_enc`
//! BLOB 列；明文密码绝不落盘。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

/// 写入用的 WebDAV remote 配置（明文，调用方持有）。
#[derive(Debug, Clone)]
pub struct WebDavRemoteInput {
    /// 关联的 bound_dirs.id。
    pub dir_id: String,
    pub url: String,
    pub username: Option<String>,
    /// 明文密码；落库前由 `upsert_webdav_remote` 用 dek 加密。
    pub password: Option<String>,
    pub depth: u32,
    /// 语料领域（写入 RawDocument.corpus_domain，驱动 F-Pro 跨域防污染）。
    pub corpus_domain: String,
}

/// 从表里读出的 WebDAV remote 配置（password 已解密回明文）。
#[derive(Debug, Clone)]
pub struct WebDavRemoteRow {
    pub dir_id: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub depth: u32,
    pub corpus_domain: String,
    pub last_etag_sync: Option<String>,
}

impl Store {
    /// upsert 一条 WebDAV remote 配置。`password` 用 dek 加密成 BLOB 落盘。
    /// 同 `dir_id` 已存在则整行替换（幂等）。
    pub fn upsert_webdav_remote(&self, dek: &Key32, input: &WebDavRemoteInput) -> Result<()> {
        let password_enc: Option<Vec<u8>> = match &input.password {
            Some(p) => Some(crypto::encrypt(dek, p.as_bytes())?),
            None => None,
        };
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO webdav_remotes
                (dir_id, url, username, password_enc, depth, corpus_domain, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(dir_id) DO UPDATE SET
                url=excluded.url,
                username=excluded.username,
                password_enc=excluded.password_enc,
                depth=excluded.depth,
                corpus_domain=excluded.corpus_domain,
                updated_at=excluded.updated_at",
            params![
                input.dir_id,
                input.url,
                input.username,
                password_enc,
                input.depth as i64,
                input.corpus_domain,
                now,
            ],
        )?;
        Ok(())
    }

    /// 读单条 WebDAV remote 配置（password 解密回明文）。
    pub fn get_webdav_remote(&self, dek: &Key32, dir_id: &str) -> Result<Option<WebDavRemoteRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync
             FROM webdav_remotes WHERE dir_id = ?1",
        )?;
        let row = stmt
            .query_row(params![dir_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            })
            .ok();
        match row {
            None => Ok(None),
            Some((dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync)) => {
                let password = match password_enc {
                    Some(blob) => Some(
                        String::from_utf8(crypto::decrypt(dek, &blob)?)
                            .map_err(|e| VaultError::Crypto(format!("webdav password utf8: {e}")))?,
                    ),
                    None => None,
                };
                Ok(Some(WebDavRemoteRow {
                    dir_id,
                    url,
                    username,
                    password,
                    depth: depth as u32,
                    corpus_domain,
                    last_etag_sync,
                }))
            }
        }
    }

    /// 列出全部 WebDAV remote 配置（周期 worker 用，password 已解密）。
    pub fn list_webdav_remotes(&self, dek: &Key32) -> Result<Vec<WebDavRemoteRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync
             FROM webdav_remotes ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<Vec<u8>>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync) = row?;
            let password = match password_enc {
                Some(blob) => Some(
                    String::from_utf8(crypto::decrypt(dek, &blob)?)
                        .map_err(|e| VaultError::Crypto(format!("webdav password utf8: {e}")))?,
                ),
                None => None,
            };
            out.push(WebDavRemoteRow {
                dir_id,
                url,
                username,
                password,
                depth: depth as u32,
                corpus_domain,
                last_etag_sync,
            });
        }
        Ok(out)
    }

    /// 记录某 remote 最近一次 ETag 增量同步时间（Task 11 周期 worker 调用）。
    pub fn touch_webdav_remote_sync(&self, dir_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE webdav_remotes SET last_etag_sync = ?1 WHERE dir_id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    /// 仅供集成测试用：取 password_enc 原始密文字节（验证不含明文）。
    ///
    /// **Feature-gated**：仅在启用 `test-utils` feature 时编译，生产二进制不暴露。
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn debug_raw_webdav_password_enc(&self, dir_id: &str) -> Result<Vec<u8>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT password_enc FROM webdav_remotes WHERE dir_id = ?1",
                params![dir_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(blob.unwrap_or_default())
    }
}
