//! G1 浏览状态信号 store CRUD（W3 batch B，2026-04-27）。
//!
//! per spec `docs/superpowers/specs/2026-04-27-w3-batch-b-design.md` §3。
//! 隐私：url + title 用 DEK 加密；domain_hash = SHA-256(domain) 明文（便于
//! per-domain 聚合 / 删除而不暴露域名）；engagement 数值明文便于查询。
//! 入站请求来自 Chrome 扩展 background worker 周期 flush（30s 一次）。

use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;

/// 高 engagement 阈值（per spec §3.G2）— 触发 auto_bookmark
pub const HIGH_ENGAGEMENT_DWELL_MS: u64 = 3 * 60 * 1000; // 3 min
pub const HIGH_ENGAGEMENT_SCROLL_PCT: u32 = 50;
pub const HIGH_ENGAGEMENT_COPY_COUNT: u32 = 1;

/// 字段长度上限（per reviewer I3：防恶意页面 document.title=1MB 拖慢加密 + 写盘）
pub const MAX_URL_LEN: usize = 2048;
pub const MAX_TITLE_LEN: usize = 512;

/// 入站信号（来自 Chrome 扩展 POST 上报）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseSignalInput {
    pub url: String,
    pub title: String,
    pub dwell_ms: u64,
    pub scroll_pct: u32,
    pub copy_count: u32,
    pub visit_count: u32,
}

impl BrowseSignalInput {
    /// per spec §3.G2 高 engagement 评分 — 触发 auto-bookmark
    pub fn is_high_engagement(&self) -> bool {
        self.dwell_ms >= HIGH_ENGAGEMENT_DWELL_MS
            && self.scroll_pct >= HIGH_ENGAGEMENT_SCROLL_PCT
            && self.copy_count >= HIGH_ENGAGEMENT_COPY_COUNT
    }

    /// 提取 domain 部分（去掉 protocol / port / path / query / fragment）
    pub fn domain(&self) -> String {
        url_to_domain(&self.url)
    }

    /// per reviewer I3：截断超长字段。route / store 调用前必走，防恶意页面拖慢加密。
    /// 超长按 char boundary 截断（不破坏 UTF-8）。
    pub fn truncate_to_limits(&mut self) {
        if self.url.chars().count() > MAX_URL_LEN {
            self.url = self.url.chars().take(MAX_URL_LEN).collect();
        }
        if self.title.chars().count() > MAX_TITLE_LEN {
            self.title = self.title.chars().take(MAX_TITLE_LEN).collect();
        }
    }
}

/// 出站行（解密后给前端 / 诊断用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseSignalRow {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub domain_hash: String,
    pub dwell_ms: u64,
    pub scroll_pct: u32,
    pub copy_count: u32,
    pub visit_count: u32,
    pub created_at_secs: i64,
}

fn url_to_domain(url: &str) -> String {
    // 简化解析：取 :// 之后到下一个 / 之间，去掉 :port
    let after_proto = url.split("://").nth(1).unwrap_or(url);
    let host = after_proto.split('/').next().unwrap_or("");
    host.split(':').next().unwrap_or(host).to_lowercase()
}

/// per reviewer I1：domain hash 加 pepper，防止 SHA-256(常见域名) 彩虹表反推。
/// 用 HMAC-SHA256(pepper, domain)。pepper 是编译期常量（per-installation 不同更好，
/// 但 attune 单二进制无每用户配置；此处接受弱 pepper 比裸 SHA-256 强很多）。
/// 真正的 vault-scoped salt 需要 dek 透传，会让 hash_domain 签名变重 — 留 W4 评估。
const DOMAIN_HASH_PEPPER: &[u8] = b"attune.browse_signals.v1.2026";

fn hash_domain(domain: &str) -> String {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(DOMAIN_HASH_PEPPER)
        .expect("HMAC accepts any key length");
    mac.update(domain.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

impl Store {
    /// 写入一条浏览信号，返回新 row id。
    pub fn record_browse_signal(
        &self,
        dek: &Key32,
        signal: &BrowseSignalInput,
        now_secs: i64,
    ) -> Result<i64> {
        let url_enc = crypto::encrypt(dek, signal.url.as_bytes())?;
        let title_enc = crypto::encrypt(dek, signal.title.as_bytes())?;
        let domain_hash = hash_domain(&signal.domain());
        self.conn.execute(
            "INSERT INTO browse_signals \
                (url_enc, title_enc, domain_hash, dwell_ms, scroll_pct, copy_count, \
                 visit_count, created_at_secs) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                url_enc,
                title_enc,
                domain_hash,
                signal.dwell_ms as i64,
                signal.scroll_pct as i64,
                signal.copy_count as i64,
                signal.visit_count as i64,
                now_secs,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 拉最近 N 条信号（解密 url/title）。
    pub fn list_recent_browse_signals(
        &self,
        dek: &Key32,
        limit: usize,
    ) -> Result<Vec<BrowseSignalRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url_enc, title_enc, domain_hash, dwell_ms, scroll_pct, \
                    copy_count, visit_count, created_at_secs \
             FROM browse_signals \
             ORDER BY created_at_secs DESC \
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })?
            .filter_map(|r| r.ok());
        let mut out = Vec::new();
        for (id, url_enc, title_enc, domain_hash, dwell, scroll, copy, visit, ts) in rows {
            let url = crypto::decrypt(dek, &url_enc)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            let title = crypto::decrypt(dek, &title_enc)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            out.push(BrowseSignalRow {
                id,
                url,
                title,
                domain_hash,
                dwell_ms: dwell as u64,
                scroll_pct: scroll as u32,
                copy_count: copy as u32,
                visit_count: visit as u32,
                created_at_secs: ts,
            });
        }
        Ok(out)
    }

    pub fn browse_signals_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM browse_signals", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// 按 domain（明文 domain，自动 hash 后查）批量删除。
    pub fn clear_browse_signals_for_domain(&self, domain: &str) -> Result<usize> {
        let h = hash_domain(domain);
        let n = self
            .conn
            .execute("DELETE FROM browse_signals WHERE domain_hash = ?1", params![h])?;
        Ok(n)
    }

    pub fn clear_all_browse_signals(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM browse_signals", [])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_signal(url: &str, dwell_min: u64, scroll: u32, copies: u32) -> BrowseSignalInput {
        BrowseSignalInput {
            url: url.into(),
            title: format!("Title for {url}"),
            dwell_ms: dwell_min * 60 * 1000,
            scroll_pct: scroll,
            copy_count: copies,
            visit_count: 1,
        }
    }

    #[test]
    fn url_to_domain_extracts_host() {
        assert_eq!(url_to_domain("https://github.com/foo/bar"), "github.com");
        assert_eq!(url_to_domain("http://example.com:8080/path"), "example.com");
        assert_eq!(url_to_domain("https://sub.x.y.com/?q=1"), "sub.x.y.com");
        // 边界：无 protocol
        assert_eq!(url_to_domain("github.com/foo"), "github.com");
    }

    #[test]
    fn high_engagement_thresholds() {
        // 全满足 → true
        assert!(sample_signal("x", 5, 80, 2).is_high_engagement());
        // dwell 不够（2 分钟 < 3） → false
        assert!(!sample_signal("x", 2, 80, 2).is_high_engagement());
        // scroll 不够（30 < 50） → false
        assert!(!sample_signal("x", 5, 30, 2).is_high_engagement());
        // copy 不够（0 < 1） → false
        assert!(!sample_signal("x", 5, 80, 0).is_high_engagement());
        // 边界：恰好满足（3 分钟 + 50% + 1 copy） → true
        assert!(sample_signal("x", 3, 50, 1).is_high_engagement());
    }

    #[test]
    fn record_then_list_round_trips() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let id = store
            .record_browse_signal(&dek, &sample_signal("https://github.com/x/y", 5, 80, 2), 1000)
            .unwrap();
        assert!(id > 0);
        let rows = store.list_recent_browse_signals(&dek, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://github.com/x/y");
        assert_eq!(rows[0].title, "Title for https://github.com/x/y");
        assert_eq!(rows[0].dwell_ms, 5 * 60 * 1000);
        assert_eq!(rows[0].scroll_pct, 80);
    }

    #[test]
    fn url_and_title_encrypted_at_rest() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .record_browse_signal(
                &dek,
                &sample_signal("https://secret.com/private", 3, 50, 1),
                1000,
            )
            .unwrap();
        let raw_url: Vec<u8> = store
            .conn
            .query_row(
                "SELECT url_enc FROM browse_signals LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let raw_str = String::from_utf8_lossy(&raw_url);
        assert!(!raw_str.contains("secret.com"), "url 应加密落盘");
    }

    #[test]
    fn clear_for_domain_filters() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store.record_browse_signal(&dek, &sample_signal("https://a.com/x", 5, 80, 2), 1000).unwrap();
        store.record_browse_signal(&dek, &sample_signal("https://a.com/y", 5, 80, 2), 1001).unwrap();
        store.record_browse_signal(&dek, &sample_signal("https://b.com/x", 5, 80, 2), 1002).unwrap();
        assert_eq!(store.browse_signals_count().unwrap(), 3);
        let n = store.clear_browse_signals_for_domain("a.com").unwrap();
        assert_eq!(n, 2);
        assert_eq!(store.browse_signals_count().unwrap(), 1);
    }

    #[test]
    fn clear_all_returns_deleted_count() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        for i in 0..5 {
            store.record_browse_signal(
                &dek, &sample_signal(&format!("https://x{i}.com"), 5, 80, 2), 1000 + i,
            ).unwrap();
        }
        assert_eq!(store.clear_all_browse_signals().unwrap(), 5);
        assert_eq!(store.browse_signals_count().unwrap(), 0);
    }

    #[test]
    fn list_returns_descending_by_time() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store.record_browse_signal(&dek, &sample_signal("https://old.com", 5, 80, 2), 1000).unwrap();
        store.record_browse_signal(&dek, &sample_signal("https://new.com", 5, 80, 2), 2000).unwrap();
        let rows = store.list_recent_browse_signals(&dek, 10).unwrap();
        assert_eq!(rows[0].url, "https://new.com");
        assert_eq!(rows[1].url, "https://old.com");
    }
}
