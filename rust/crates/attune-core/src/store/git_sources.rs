//! Git 源配置持久化（GitConnector OSS 仓导入）。
//!
//! 与 `webdav_remotes` 同模式：增量同步 worker 要复用 clone 范围 + token + commit
//! 游标 → 此表存每个 `git:<url>#<ref>` bound_dir 的完整配置。
//! `token_ref` 经字段级 AES-256-GCM 加密（dek，与 `items.content` 同模式）落
//! `token_ref_enc` BLOB —— **存的是 env 键名 / token 引用，不是明文 token**（per
//! 全局 §1.4：明文 token 绝不落盘 / 不进日志 / 不回显）。公开仓 token_ref = None。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

/// 写入用的 git 源配置（调用方持有；token_ref 是明文 env 键名，落库前加密）。
#[derive(Debug, Clone)]
pub struct GitSourceInput {
    /// 关联的 bound_dirs.id。
    pub dir_id: String,
    /// 归一后的 clone URL。
    pub url: String,
    /// 主机名（SSRF allowlist / 展示）。
    pub host: String,
    /// 分支 / tag；None = 仓默认分支。
    pub branch: Option<String>,
    /// 子目录（sparse checkout）；None = 整仓。
    pub subdir: Option<String>,
    /// include glob JSON array 字符串；"" = 默认知识类。
    pub include_glob: String,
    /// exclude glob JSON array 字符串。
    pub exclude_glob: String,
    /// 语料领域（写入 RawDocument.corpus_domain）。
    pub corpus_domain: String,
    /// token 引用（env 键名等明文标识），落库前用 dek 加密；None = 公开仓。
    pub token_ref: Option<String>,
    pub max_files: u64,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

/// 从表里读出的 git 源配置（token_ref 已解密回明文键名）。
#[derive(Debug, Clone)]
pub struct GitSourceRow {
    pub dir_id: String,
    pub url: String,
    pub host: String,
    pub branch: Option<String>,
    pub subdir: Option<String>,
    pub include_glob: String,
    pub exclude_glob: String,
    pub corpus_domain: String,
    pub token_ref: Option<String>,
    pub max_files: u64,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub last_commit_sha: Option<String>,
    pub last_synced_at: Option<String>,
}

impl Store {
    /// upsert 一条 git 源配置。`token_ref` 用 dek 加密成 BLOB 落盘。
    /// 同 `dir_id` 已存在则整行替换（幂等），但**保留** `last_commit_sha` 游标
    /// （re-bind 不应重置增量起点 —— 除非显式 `update_git_cursor`）。
    pub fn upsert_git_source(&self, dek: &Key32, input: &GitSourceInput) -> Result<()> {
        let token_ref_enc: Option<Vec<u8>> = match &input.token_ref {
            Some(t) => Some(crypto::encrypt(dek, t.as_bytes())?),
            None => None,
        };
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO git_sources
                (dir_id, url, host, branch, subdir, include_glob, exclude_glob,
                 corpus_domain, token_ref_enc, max_files, max_file_bytes,
                 max_total_bytes, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(dir_id) DO UPDATE SET
                url=excluded.url,
                host=excluded.host,
                branch=excluded.branch,
                subdir=excluded.subdir,
                include_glob=excluded.include_glob,
                exclude_glob=excluded.exclude_glob,
                corpus_domain=excluded.corpus_domain,
                token_ref_enc=excluded.token_ref_enc,
                max_files=excluded.max_files,
                max_file_bytes=excluded.max_file_bytes,
                max_total_bytes=excluded.max_total_bytes,
                updated_at=excluded.updated_at",
            params![
                input.dir_id,
                input.url,
                input.host,
                input.branch,
                input.subdir,
                input.include_glob,
                input.exclude_glob,
                input.corpus_domain,
                token_ref_enc,
                input.max_files as i64,
                input.max_file_bytes as i64,
                input.max_total_bytes as i64,
                now,
            ],
        )?;
        Ok(())
    }

    /// 读单条 git 源配置（token_ref 解密回明文键名）。
    pub fn get_git_source(&self, dek: &Key32, dir_id: &str) -> Result<Option<GitSourceRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, url, host, branch, subdir, include_glob, exclude_glob,
                    corpus_domain, token_ref_enc, max_files, max_file_bytes,
                    max_total_bytes, last_commit_sha, last_synced_at
             FROM git_sources WHERE dir_id = ?1",
        )?;
        let row = stmt
            .query_row(params![dir_id], Self::map_git_row)
            .ok();
        match row {
            None => Ok(None),
            Some(raw) => Ok(Some(Self::decrypt_git_row(dek, raw)?)),
        }
    }

    /// 列出全部 git 源配置（周期 worker 用，token_ref 已解密）。
    pub fn list_git_sources(&self, dek: &Key32) -> Result<Vec<GitSourceRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, url, host, branch, subdir, include_glob, exclude_glob,
                    corpus_domain, token_ref_enc, max_files, max_file_bytes,
                    max_total_bytes, last_commit_sha, last_synced_at
             FROM git_sources ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], Self::map_git_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(Self::decrypt_git_row(dek, row?)?);
        }
        Ok(out)
    }

    /// 推进增量游标（仅在一次同步成功 ingest 后调用）。
    pub fn update_git_cursor(&self, dir_id: &str, commit_sha: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE git_sources SET last_commit_sha = ?1, last_synced_at = ?2 WHERE dir_id = ?3",
            params![commit_sha, now, dir_id],
        )?;
        Ok(())
    }

    /// 记录最近一次同步时间（不推进 commit 游标，用于失败 / not-modified 路径）。
    pub fn touch_git_synced(&self, dir_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE git_sources SET last_synced_at = ?1 WHERE dir_id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    /// 行映射：token_ref_enc 仍是密文 BLOB（解密在 `decrypt_git_row`）。
    #[allow(clippy::type_complexity)]
    fn map_git_row(
        r: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<(
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
        Option<Vec<u8>>,
        i64,
        i64,
        i64,
        Option<String>,
        Option<String>,
    )> {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
            r.get(11)?,
            r.get(12)?,
            r.get(13)?,
        ))
    }

    #[allow(clippy::type_complexity)]
    fn decrypt_git_row(
        dek: &Key32,
        raw: (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
            Option<Vec<u8>>,
            i64,
            i64,
            i64,
            Option<String>,
            Option<String>,
        ),
    ) -> Result<GitSourceRow> {
        let (
            dir_id,
            url,
            host,
            branch,
            subdir,
            include_glob,
            exclude_glob,
            corpus_domain,
            token_ref_enc,
            max_files,
            max_file_bytes,
            max_total_bytes,
            last_commit_sha,
            last_synced_at,
        ) = raw;
        let token_ref = match token_ref_enc {
            Some(blob) => Some(
                String::from_utf8(crypto::decrypt(dek, &blob)?)
                    .map_err(|e| VaultError::Crypto(format!("git token_ref utf8: {e}")))?,
            ),
            None => None,
        };
        Ok(GitSourceRow {
            dir_id,
            url,
            host,
            branch,
            subdir,
            include_glob,
            exclude_glob,
            corpus_domain,
            token_ref,
            max_files: max_files as u64,
            max_file_bytes: max_file_bytes as u64,
            max_total_bytes: max_total_bytes as u64,
            last_commit_sha,
            last_synced_at,
        })
    }

    /// 仅供集成测试用：取 token_ref_enc 原始密文字节（验证不含明文）。
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn debug_raw_git_token_enc(&self, dir_id: &str) -> Result<Vec<u8>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT token_ref_enc FROM git_sources WHERE dir_id = ?1",
                params![dir_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(blob.unwrap_or_default())
    }
}
