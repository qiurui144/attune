//! W4 G2 — 高 engagement 自动 bookmark 候选表（2026-04-27）。
//!
//! per W4 plan G2 + W3 batch B spec §3.G2。
//!
//! ## 设计动机
//!
//! W3 batch B 决策：高 engagement 浏览页"仅计数不创建 item，留 G3 (W5-6) 真正的
//! page content extraction 后再 insert_item"。但 W4 plan 要求 G2 deliverable，矛盾。
//!
//! 折衷：建独立 `auto_bookmarks` staging 表 — 高 engagement 时记录候选行
//! (promoted=0)，G3 worker 抓内容后 promote 到 items 表 (promoted=1, promoted_item_id=...)。
//! 这样：
//! - W4 G2 有可独立测试的 deliverable
//! - W3 batch B "不入主 items 表" 决策保留
//! - G3 实施时 SELECT WHERE promoted = 0 + extract + insert_item + UPDATE 一次完成
//!
//! ## 加密一致性
//!
//! url/title DEK 加密同 browse_signals — 候选状态也是用户隐私。
//! domain_hash 复用 browse_signals 同 pepper（per W4-003 v0.7 升级时同时迁移两表）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::Result;
use super::Store;

/// 出站行（解密后给前端 / G3 worker 用）
#[derive(Debug, Clone)]
pub struct AutoBookmarkRow {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub domain_hash: String,
    pub dwell_ms: u64,
    pub scroll_pct: u32,
    pub copy_count: u32,
    pub visit_count: u32,
    pub created_at_secs: i64,
    pub promoted: bool,
    pub promoted_item_id: Option<String>,
}

impl Store {
    /// 记录一个 high engagement 候选（promoted=0）。
    /// 由 routes::browse_signals::record_batch 在 is_high_engagement() 时调用。
    /// url/title/domain_hash 应已与 record_browse_signal 同源 — 由调用方保证一致性。
    #[allow(clippy::too_many_arguments)]
    pub fn record_auto_bookmark(
        &self,
        dek: &Key32,
        url: &str,
        title: &str,
        domain_hash: &str,
        dwell_ms: u64,
        scroll_pct: u32,
        copy_count: u32,
        visit_count: u32,
        now_secs: i64,
    ) -> Result<i64> {
        let url_enc = crypto::encrypt(dek, url.as_bytes())?;
        let title_enc = crypto::encrypt(dek, title.as_bytes())?;
        self.conn.execute(
            "INSERT INTO auto_bookmarks
             (url_enc, title_enc, domain_hash, dwell_ms, scroll_pct, copy_count, visit_count, created_at_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                url_enc,
                title_enc,
                domain_hash,
                dwell_ms as i64,
                scroll_pct as i64,
                copy_count as i64,
                visit_count as i64,
                now_secs,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 总候选条数（含已 promote）— 诊断用。
    pub fn auto_bookmarks_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM auto_bookmarks", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// 待 promote 候选条数。G3 worker 周期决定是否触发抓取。
    pub fn pending_auto_bookmarks_count(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM auto_bookmarks WHERE promoted = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// 列出最近 N 条候选（解密 url/title）。limit 上限调用方夹紧。
    /// 解密失败的行 silent skip + warn — 与 list_recent_browse_signals 一致。
    pub fn list_recent_auto_bookmarks(
        &self,
        dek: &Key32,
        limit: usize,
        only_pending: bool,
    ) -> Result<Vec<AutoBookmarkRow>> {
        let sql = if only_pending {
            "SELECT id, url_enc, title_enc, domain_hash, dwell_ms, scroll_pct, copy_count,
                    visit_count, created_at_secs, promoted, promoted_item_id
             FROM auto_bookmarks WHERE promoted = 0 ORDER BY created_at_secs DESC LIMIT ?1"
        } else {
            "SELECT id, url_enc, title_enc, domain_hash, dwell_ms, scroll_pct, copy_count,
                    visit_count, created_at_secs, promoted, promoted_item_id
             FROM auto_bookmarks ORDER BY created_at_secs DESC LIMIT ?1"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![limit as i64], |r| {
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
                r.get::<_, i64>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, url_enc, title_enc, domain_hash, dwell, scroll, copy, visit, created, promoted, pid) =
                row?;
            let url_bytes = match crypto::decrypt(dek, &url_enc) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("G2 list_recent_auto_bookmarks decrypt url failed id={id}: {e}");
                    continue;
                }
            };
            let title_bytes = match crypto::decrypt(dek, &title_enc) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("G2 list_recent_auto_bookmarks decrypt title failed id={id}: {e}");
                    continue;
                }
            };
            out.push(AutoBookmarkRow {
                id,
                url: String::from_utf8_lossy(&url_bytes).into_owned(),
                title: String::from_utf8_lossy(&title_bytes).into_owned(),
                domain_hash,
                dwell_ms: dwell as u64,
                scroll_pct: scroll as u32,
                copy_count: copy as u32,
                visit_count: visit as u32,
                created_at_secs: created,
                promoted: promoted != 0,
                promoted_item_id: pid,
            });
        }
        Ok(out)
    }

    /// G3 worker 完成 promote 后调用。幂等（重复调用同 id 无副作用）。
    /// 必须传 item_id 关联真实 items.id（G3 已 insert_item）。
    pub fn mark_auto_bookmark_promoted(&self, id: i64, item_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE auto_bookmarks SET promoted = 1, promoted_item_id = ?1
             WHERE id = ?2 AND promoted = 0",
            params![item_id, id],
        )?;
        Ok(())
    }

    /// 全清候选（G5 隐私面板"清除已捕获" extension 入口可选调用）。返回删除条数。
    pub fn clear_all_auto_bookmarks(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM auto_bookmarks", [])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dek() -> Key32 {
        Key32::generate()
    }

    #[test]
    fn record_then_list_round_trip() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        let id = store
            .record_auto_bookmark(
                &dek,
                "https://github.com/anthropic/attune/issues/42",
                "Issue: long article",
                "abc123",
                240_000, // 4 min
                85,
                3,
                1,
                1700000000,
            )
            .unwrap();
        assert!(id > 0);
        assert_eq!(store.auto_bookmarks_count().unwrap(), 1);
        assert_eq!(store.pending_auto_bookmarks_count().unwrap(), 1);

        let rows = store.list_recent_auto_bookmarks(&dek, 10, true).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://github.com/anthropic/attune/issues/42");
        assert_eq!(rows[0].title, "Issue: long article");
        assert!(!rows[0].promoted);
    }

    #[test]
    fn url_and_title_encrypted_at_rest() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .record_auto_bookmark(
                &dek,
                "https://secret.internal/plan",
                "敏感标题",
                "h",
                300_000,
                90,
                2,
                1,
                1700000000,
            )
            .unwrap();
        let url_blob: Vec<u8> = store
            .conn
            .query_row("SELECT url_enc FROM auto_bookmarks LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let s = String::from_utf8_lossy(&url_blob);
        assert!(!s.contains("secret"), "url 必须加密: {s}");
        assert!(!s.contains("plan"));

        let title_blob: Vec<u8> = store
            .conn
            .query_row("SELECT title_enc FROM auto_bookmarks LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let s = String::from_utf8_lossy(&title_blob);
        assert!(!s.contains("敏感"), "title 必须加密: {s}");
    }

    #[test]
    fn mark_promoted_excludes_from_pending() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        let id = store
            .record_auto_bookmark(&dek, "https://a.com/", "A", "h1", 200_000, 60, 1, 1, 1700000000)
            .unwrap();
        let id2 = store
            .record_auto_bookmark(&dek, "https://b.com/", "B", "h2", 250_000, 70, 2, 1, 1700000001)
            .unwrap();

        store.mark_auto_bookmark_promoted(id, "item-uuid-1").unwrap();
        assert_eq!(store.pending_auto_bookmarks_count().unwrap(), 1);
        assert_eq!(store.auto_bookmarks_count().unwrap(), 2, "总数不变");

        let pending = store.list_recent_auto_bookmarks(&dek, 10, true).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id2);

        let all = store.list_recent_auto_bookmarks(&dek, 10, false).unwrap();
        assert_eq!(all.len(), 2);
        let promoted_row = all.iter().find(|r| r.id == id).unwrap();
        assert!(promoted_row.promoted);
        assert_eq!(promoted_row.promoted_item_id.as_deref(), Some("item-uuid-1"));
    }

    #[test]
    fn mark_promoted_idempotent() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        let id = store
            .record_auto_bookmark(&dek, "https://x.com/", "X", "h", 200_000, 60, 1, 1, 1700000000)
            .unwrap();
        store.mark_auto_bookmark_promoted(id, "item-1").unwrap();
        // 重复 mark 不应改变 promoted_item_id（WHERE promoted = 0 拒）
        store.mark_auto_bookmark_promoted(id, "item-2-WRONG").unwrap();
        let row = store
            .list_recent_auto_bookmarks(&dek, 10, false)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(row.promoted_item_id.as_deref(), Some("item-1"));
    }

    #[test]
    fn list_with_wrong_dek_silently_skips() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .record_auto_bookmark(&dek, "https://a.com/", "A", "h", 200_000, 60, 1, 1, 1700000000)
            .unwrap();
        let wrong = Key32::generate();
        let rows = store.list_recent_auto_bookmarks(&wrong, 10, true).unwrap();
        assert_eq!(rows.len(), 0, "错 dek 不应返回明文");
    }

    #[test]
    fn clear_all_resets_counts() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .record_auto_bookmark(&dek, "https://a.com/", "A", "h", 200_000, 60, 1, 1, 1700000000)
            .unwrap();
        store
            .record_auto_bookmark(&dek, "https://b.com/", "B", "h", 200_000, 60, 1, 1, 1700000001)
            .unwrap();
        let n = store.clear_all_auto_bookmarks().unwrap();
        assert_eq!(n, 2);
        assert_eq!(store.auto_bookmarks_count().unwrap(), 0);
    }
}
