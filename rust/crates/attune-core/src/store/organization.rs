//! organization_proposals — 文件夹一键整理提案缓存 CRUD + TTL 清理。
//!
//! 提案内含敏感文件标题/摘要,故 scope_json / proposal_json 字段级 AES-256-GCM 加密
//! (与 items.content 同模式,经 crypto::encrypt(dek, ..))。Store 不持 DEK —— 与既有
//! 加密 CRUD(annotations / auto_bookmarks 等)一致,dek 由调用方(已解锁 vault)逐次传入。
//!
//! 生命周期:draft → applied | discarded。draft 7 天后由 cleanup_stale_proposals 清理,
//! 避免明文敏感缓存(解密后)无限累积。

use chrono::Utc;
use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;

impl Store {
    /// 缓存一份整理提案。scope_json / proposal_json 加密存储(含敏感标题/摘要)。
    /// 重复 id → INSERT OR REPLACE(重新 analyze 同 scope 覆盖旧 draft)。
    pub fn save_proposal(
        &self,
        dek: &Key32,
        id: &str,
        scope_json: &str,
        corpus_domain: Option<&str>,
        proposal_json: &str,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        let scope_enc = crypto::encrypt(dek, scope_json.as_bytes())?;
        let prop_enc = crypto::encrypt(dek, proposal_json.as_bytes())?;
        self.conn.execute(
            "INSERT OR REPLACE INTO organization_proposals \
             (id, scope_encrypted, corpus_domain, proposal_encrypted, status, created_at, applied_at) \
             VALUES (?1, ?2, ?3, ?4, 'draft', ?5, NULL)",
            params![id, scope_enc, corpus_domain, prop_enc, now],
        )?;
        Ok(())
    }

    /// 返回 (status, proposal_json 明文)。不存在 → None。
    pub fn get_proposal(&self, dek: &Key32, id: &str) -> Result<Option<(String, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT status, proposal_encrypted FROM organization_proposals WHERE id = ?1",
        )?;
        let row = stmt
            .query_row(params![id], |r| {
                let status: String = r.get(0)?;
                let blob: Vec<u8> = r.get(1)?;
                Ok((status, blob))
            })
            .ok();
        match row {
            Some((status, blob)) => {
                let json = String::from_utf8(crypto::decrypt(dek, &blob)?).unwrap_or_default();
                Ok(Some((status, json)))
            }
            None => Ok(None),
        }
    }

    pub fn mark_proposal_applied(&self, id: &str) -> Result<()> {
        let now = Utc::now().timestamp();
        self.conn.execute(
            "UPDATE organization_proposals SET status='applied', applied_at=?2 WHERE id=?1",
            params![id, now],
        )?;
        Ok(())
    }

    pub fn discard_proposal(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE organization_proposals SET status='discarded' WHERE id=?1",
            params![id],
        )?;
        Ok(())
    }

    /// 列出提案元数据 (id, status, created_at)。status=None 列全部。明文标题/摘要在
    /// proposal_encrypted 内,列表不解密(列表视图不需要内容,降解密成本 + 缩小明文暴露面)。
    pub fn list_proposals(
        &self,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, status, created_at FROM organization_proposals \
             WHERE (?1 IS NULL OR status = ?1) ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![status, limit, offset], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 清理 7 天前未 apply 的 draft —— 解密后的敏感缓存不应无限累积。
    /// 返回删除行数。applied / discarded 保留(审计 + 幂等查询需要)。
    pub fn cleanup_stale_proposals(&self) -> Result<usize> {
        let cutoff = Utc::now().timestamp() - 7 * 24 * 3600;
        let n = self.conn.execute(
            "DELETE FROM organization_proposals WHERE status='draft' AND created_at < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }

    /// organization_proposals 表迁移(幂等)。新 vault 直接建表;老 vault 升级也走此路径。
    pub(crate) fn migrate_organization_proposals(conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS organization_proposals (\
               id TEXT PRIMARY KEY, scope_encrypted BLOB, corpus_domain TEXT, \
               proposal_encrypted BLOB, status TEXT NOT NULL DEFAULT 'draft', \
               created_at INTEGER NOT NULL, applied_at INTEGER);\
             CREATE INDEX IF NOT EXISTS idx_orgprop_status ON organization_proposals(status, created_at);",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open_memory().unwrap()
    }

    #[test]
    fn save_get_roundtrip_and_status() {
        let s = store();
        let dek = Key32::generate();
        let json = r#"{"proposal_id":"p1","groups":[],"noise_items":[],"cost":{"tier":2,"est_tokens":0,"est_usd":0.0,"model":""},"corpus_domain":null,"dimension_mismatch_count":0}"#;
        s.save_proposal(&dek, "p1", "{}", None, json).unwrap();
        let got = s.get_proposal(&dek, "p1").unwrap().unwrap();
        assert_eq!(got.0, "draft");
        assert_eq!(got.1, json); // 解密回明文一致
        s.mark_proposal_applied("p1").unwrap();
        assert_eq!(s.get_proposal(&dek, "p1").unwrap().unwrap().0, "applied");
    }

    #[test]
    fn get_missing_returns_none() {
        let s = store();
        let dek = Key32::generate();
        assert!(s.get_proposal(&dek, "nope").unwrap().is_none());
    }

    #[test]
    fn discard_sets_status() {
        let s = store();
        let dek = Key32::generate();
        s.save_proposal(&dek, "p2", "{}", Some("legal"), "{}").unwrap();
        s.discard_proposal("p2").unwrap();
        assert_eq!(s.get_proposal(&dek, "p2").unwrap().unwrap().0, "discarded");
    }

    #[test]
    fn list_filters_by_status_and_orders_desc() {
        let s = store();
        let dek = Key32::generate();
        s.save_proposal(&dek, "a", "{}", None, "{}").unwrap();
        s.save_proposal(&dek, "b", "{}", None, "{}").unwrap();
        s.mark_proposal_applied("b").unwrap();
        let drafts = s.list_proposals(Some("draft"), 10, 0).unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].0, "a");
        let all = s.list_proposals(None, 10, 0).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn cleanup_removes_only_stale_drafts() {
        let s = store();
        let dek = Key32::generate();
        s.save_proposal(&dek, "fresh", "{}", None, "{}").unwrap();
        s.save_proposal(&dek, "stale", "{}", None, "{}").unwrap();
        // backdate "stale" 到 8 天前
        let old = Utc::now().timestamp() - 8 * 24 * 3600;
        s.conn
            .execute(
                "UPDATE organization_proposals SET created_at=?1 WHERE id='stale'",
                params![old],
            )
            .unwrap();
        let removed = s.cleanup_stale_proposals().unwrap();
        assert_eq!(removed, 1);
        assert!(s.get_proposal(&dek, "stale").unwrap().is_none());
        assert!(s.get_proposal(&dek, "fresh").unwrap().is_some());
    }
}
