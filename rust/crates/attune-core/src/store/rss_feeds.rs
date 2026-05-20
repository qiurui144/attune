//! RSS / Atom 订阅持久化。
//!
//! 与 store/webdav_remotes.rs / store/email_accounts.rs 同模式：周期同步 worker
//! 要对订阅做 HTTP 条件请求（ETag + If-Modified-Since）增量重扫，必须能读回
//! 全部订阅配置。URL 经字段级 AES-256-GCM 加密（dek，与 items.content 同模式）
//! 落 `url_enc` BLOB 列；明文 URL 绝不落盘。
//!
//! 设计说明（与 webdav/email 的差异）：
//! - RSS 订阅由 `id` (uuid v4) 作主键，不挂在 `bound_dirs` 上 —— bound_dirs 是
//!   "目录 / 账户" 概念，单个 RSS feed 不是目录，更近似一行书签。后续 UI 可统一
//!   `/sources` 看板列出 webdav / email / rss，不必伪造 bound_dirs 行。
//! - 增量标记三件套：`etag`（HTTP ETag）+ `last_modified`（HTTP Last-Modified
//!   原始字符串）+ `last_entry_guid`（最后一次成功 ingest 的 entry guid/link，
//!   用于 entry 级去重防止 server 不支持条件 GET 时整箱回灌）。
//! - 每 feed 独立 `poll_interval_minutes` —— 高频订阅可设 5 min、低频可设 24h，
//!   worker 跑一遍但只调真正到期的 feed。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

/// 默认轮询间隔（分钟）。多数主流 RSS 站点（LWN / lkml / GitHub releases / 博客）
/// 不希望被高频拉取，60 min 是个保守默认。用户可在 add_feed 时显式覆盖。
pub const DEFAULT_POLL_INTERVAL_MINUTES: u32 = 60;

/// 写入用的 RSS 订阅配置（明文，调用方持有）。
#[derive(Debug, Clone)]
pub struct RssFeedInput {
    /// 用户给的展示名（"LWN 周报" / "rust-lang blog"）。可空字符串。
    pub name: String,
    /// Feed URL 明文；落库前由 `add_feed` 用 dek 加密。
    pub url: String,
    /// 轮询周期（分钟）。`None` 用 `DEFAULT_POLL_INTERVAL_MINUTES`。
    pub poll_interval_minutes: Option<u32>,
}

/// 从表里读出的 RSS 订阅配置（url 已解密回明文）。
#[derive(Debug, Clone)]
pub struct RssFeedRow {
    pub id: String,
    pub name: String,
    pub url: String,
    /// 最近一次成功 ingest 的 entry guid（或 link 兜底），entry 级去重用。
    pub last_entry_guid: Option<String>,
    /// 服务器返回的 ETag —— 下次条件 GET 走 `If-None-Match: <etag>`。
    pub etag: Option<String>,
    /// 服务器返回的 Last-Modified 原始字符串 —— 下次条件 GET 走 `If-Modified-Since`。
    pub last_modified: Option<String>,
    /// 上一次成功 poll 的时间戳（RFC3339）。worker 据此判断是否到期。
    pub last_polled_at: Option<String>,
    /// 轮询周期（分钟）。
    pub poll_interval_minutes: u32,
    /// 是否启用（disabled 的 feed worker 跳过，但配置保留）。
    pub enabled: bool,
}

impl Store {
    /// 新增一条 RSS 订阅。url 用 dek 加密成 BLOB 落盘；id 由调用方在写入前
    /// 用 uuid::Uuid::new_v4() 生成。同 url 不去重（用户可有意订阅多份相同源
    /// 用不同 name 分组），由 caller 自行 dedupe。
    pub fn add_rss_feed(&self, dek: &Key32, input: &RssFeedInput) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let url_enc = crypto::encrypt(dek, input.url.as_bytes())?;
        let now = chrono::Utc::now().to_rfc3339();
        let interval = input
            .poll_interval_minutes
            .unwrap_or(DEFAULT_POLL_INTERVAL_MINUTES);
        self.conn.execute(
            "INSERT INTO rss_feeds
                (id, name, url_enc, poll_interval_minutes, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
            params![id, input.name, url_enc, interval as i64, now],
        )?;
        Ok(id)
    }

    /// 列出全部订阅（url 已解密）。worker 用，按 created_at 排序保持稳定枚举顺序。
    pub fn list_rss_feeds(&self, dek: &Key32) -> Result<Vec<RssFeedRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, url_enc, last_entry_guid, etag, last_modified,
                    last_polled_at, poll_interval_minutes, enabled
             FROM rss_feeds ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                name,
                url_enc,
                last_entry_guid,
                etag,
                last_modified,
                last_polled_at,
                interval,
                enabled,
            ) = row?;
            let url = String::from_utf8(crypto::decrypt(dek, &url_enc)?)
                .map_err(|e| VaultError::Crypto(format!("rss url utf8: {e}")))?;
            out.push(RssFeedRow {
                id,
                name,
                url,
                last_entry_guid,
                etag,
                last_modified,
                last_polled_at,
                poll_interval_minutes: interval.max(1) as u32,
                enabled: enabled != 0,
            });
        }
        Ok(out)
    }

    /// 读单条订阅（url 已解密）。手动 poll-now route 用。
    pub fn get_rss_feed(&self, dek: &Key32, id: &str) -> Result<Option<RssFeedRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, url_enc, last_entry_guid, etag, last_modified,
                    last_polled_at, poll_interval_minutes, enabled
             FROM rss_feeds WHERE id = ?1",
        )?;
        let row = stmt
            .query_row(params![id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })
            .ok();
        match row {
            None => Ok(None),
            Some((
                id,
                name,
                url_enc,
                last_entry_guid,
                etag,
                last_modified,
                last_polled_at,
                interval,
                enabled,
            )) => {
                let url = String::from_utf8(crypto::decrypt(dek, &url_enc)?)
                    .map_err(|e| VaultError::Crypto(format!("rss url utf8: {e}")))?;
                Ok(Some(RssFeedRow {
                    id,
                    name,
                    url,
                    last_entry_guid,
                    etag,
                    last_modified,
                    last_polled_at,
                    poll_interval_minutes: interval.max(1) as u32,
                    enabled: enabled != 0,
                }))
            }
        }
    }

    /// 删除一条订阅。entries 已落 `items` 表的不会被回收 —— 与 email 删账户语义一致。
    pub fn delete_rss_feed(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM rss_feeds WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 写回 HTTP 条件 GET 三件套（ETag + Last-Modified）+ touch last_polled_at。
    /// 用于 200 OK 响应路径：服务器返回新内容并附带 ETag/Last-Modified 头时调用。
    /// 任一字段为 None 即清空对应列（防止 server 撤销 ETag 后我们继续发陈旧的）。
    pub fn update_rss_etag_lastmod(
        &self,
        id: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE rss_feeds
             SET etag = ?1, last_modified = ?2, last_polled_at = ?3, updated_at = ?3
             WHERE id = ?4",
            params![etag, last_modified, now, id],
        )?;
        Ok(())
    }

    /// 仅 touch `last_polled_at` —— 304 Not Modified 或 fetch 出错时调用，
    /// 防止 worker 在下个 tick 立即重试同一 broken feed（tight loop 保护）。
    pub fn touch_rss_polled_at(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE rss_feeds SET last_polled_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// 推进 last_entry_guid（最新一次成功 ingest 的 entry guid/link）。
    /// 同时 touch `last_polled_at`，因为这只会在 poll 成功路径里调。
    pub fn update_rss_last_entry(&self, id: &str, guid: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE rss_feeds
             SET last_entry_guid = ?1, last_polled_at = ?2, updated_at = ?2
             WHERE id = ?3",
            params![guid, now, id],
        )?;
        Ok(())
    }

    /// 改启用状态 / 轮询周期。任一为 None 即保持原值。
    pub fn update_rss_feed_settings(
        &self,
        id: &str,
        enabled: Option<bool>,
        poll_interval_minutes: Option<u32>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(en) = enabled {
            self.conn.execute(
                "UPDATE rss_feeds SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                params![if en { 1 } else { 0 }, now, id],
            )?;
        }
        if let Some(iv) = poll_interval_minutes {
            self.conn.execute(
                "UPDATE rss_feeds SET poll_interval_minutes = ?1, updated_at = ?2 WHERE id = ?3",
                params![iv.max(1) as i64, now, id],
            )?;
        }
        Ok(())
    }

    /// 仅供集成测试用：取 url_enc 原始密文字节（验证不含明文）。
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn debug_raw_rss_url_enc(&self, id: &str) -> Result<Vec<u8>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT url_enc FROM rss_feeds WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok();
        Ok(blob.unwrap_or_default())
    }
}
