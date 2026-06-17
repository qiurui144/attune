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

/// 用户在提案审核面板确认后的一个分组动作。
/// action: "create"(新建项目) | "add-to"(归入既有,需 project_id) | "skip"(跳过)。
/// items: (item_id, role);role 空串表无角色。
pub struct ConfirmedGroup {
    pub action: String,
    pub project_id: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub items: Vec<(String, String)>,
}

/// apply_proposal 结果。already_applied=true 表示该 proposal 此前已 apply(幂等短路)。
#[derive(Debug, Default)]
pub struct ApplyResult {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub filed_count: usize,
    pub skipped_count: usize,
    pub already_applied: bool,
}

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

    /// 轻量状态读取:只 SELECT 明文 status 列,**不需要 dek、不解密**。
    /// apply_proposal 用它判幂等(只关心 'applied' 与否,无需解 proposal_json)。
    pub fn proposal_status(&self, id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT status FROM organization_proposals WHERE id = ?1")?;
        let status = stmt.query_row(params![id], |r| r.get::<_, String>(0)).ok();
        Ok(status)
    }

    /// 用户确认后批量归档:create_project + add_file + timeline + status 翻转,**单事务幂等**。
    ///
    /// 幂等只看明文 status(无需 dek/解密);已 'applied' → 直接返回 already_applied,不重做。
    /// 全程单 unchecked_transaction:中途任一 INSERT 失败 → 整体回滚,绝无半建 Project。
    /// 不涉及加密字段(project/project_file/project_timeline 均明文或 opaque blob),故无 dek 参数。
    pub fn apply_proposal(
        &self,
        proposal_id: &str,
        groups: &[ConfirmedGroup],
    ) -> Result<ApplyResult> {
        if self.proposal_status(proposal_id)?.as_deref() == Some("applied") {
            return Ok(ApplyResult {
                already_applied: true,
                ..Default::default()
            });
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut res = ApplyResult::default();
        for g in groups {
            if g.action == "skip" {
                res.skipped_count += g.items.len();
                continue;
            }
            let now = Utc::now().timestamp();
            let pid = if g.action == "create" {
                let title = g.title.as_deref().unwrap_or("未命名案卷");
                let kind = g.kind.as_deref().unwrap_or("collection");
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO project (id, title, kind, metadata_encrypted, created_at, updated_at, archived) \
                     VALUES (?1, ?2, ?3, NULL, ?4, ?4, 0)",
                    params![id, title, kind, now],
                )?;
                res.created.push(id.clone());
                id
            } else {
                // "add-to"：归入既有项目，必须带 project_id
                let id = g.project_id.clone().ok_or_else(|| {
                    crate::error::VaultError::InvalidInput("add-to requires project_id".into())
                })?;
                res.updated.push(id.clone());
                id
            };
            for (item_id, role) in &g.items {
                tx.execute(
                    "INSERT OR REPLACE INTO project_file (project_id, file_id, role, added_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![pid, item_id, role, now],
                )?;
                res.filed_count += 1;
            }
            tx.execute(
                "INSERT INTO project_timeline (project_id, ts, event_type, payload_encrypted) \
                 VALUES (?1, ?2, 'organized', NULL)",
                params![pid, Utc::now().timestamp_millis()],
            )?;
        }
        tx.execute(
            "UPDATE organization_proposals SET status='applied', applied_at=?2 WHERE id=?1",
            params![proposal_id, Utc::now().timestamp()],
        )?;
        tx.commit()?;
        Ok(res)
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
    fn apply_is_transactional_and_idempotent() {
        let s = store();
        let dek = Key32::generate();
        s.save_proposal(&dek, "p1", "{}", None, "{}").unwrap();
        let groups = vec![ConfirmedGroup {
            action: "create".into(),
            project_id: None,
            title: Some("案卷A".into()),
            kind: Some("collection".into()),
            items: vec![("i1".into(), "证据".into()), ("i2".into(), "".into())],
        }];
        let r1 = s.apply_proposal("p1", &groups).unwrap();
        assert_eq!(r1.created.len(), 1);
        assert_eq!(r1.filed_count, 2);
        assert!(!r1.already_applied);
        // status 已翻转 applied(明文列,proposal_status 无 dek 即可读)
        assert_eq!(s.proposal_status("p1").unwrap().as_deref(), Some("applied"));
        // 文件确实归入新建项目
        let pid = &r1.created[0];
        assert_eq!(s.list_files_for_project(pid).unwrap().len(), 2);

        // 幂等:再 apply 同 proposal_id 返回 already_applied,不重复建项目
        let before = s.list_projects(false).unwrap().len();
        let r2 = s.apply_proposal("p1", &groups).unwrap();
        assert!(r2.already_applied);
        assert_eq!(r2.created.len(), 0);
        assert_eq!(s.list_projects(false).unwrap().len(), before);
    }

    #[test]
    fn apply_add_to_existing_and_skip() {
        let s = store();
        let dek = Key32::generate();
        s.save_proposal(&dek, "p3", "{}", None, "{}").unwrap();
        let existing = s.create_project("Existing", "collection").unwrap();
        let groups = vec![
            ConfirmedGroup {
                action: "add-to".into(),
                project_id: Some(existing.id.clone()),
                title: None,
                kind: None,
                items: vec![("i9".into(), "".into())],
            },
            ConfirmedGroup {
                action: "skip".into(),
                project_id: None,
                title: None,
                kind: None,
                items: vec![("i10".into(), "".into())],
            },
        ];
        let r = s.apply_proposal("p3", &groups).unwrap();
        assert_eq!(r.updated, vec![existing.id.clone()]);
        assert_eq!(r.filed_count, 1);
        assert_eq!(r.skipped_count, 1);
        assert_eq!(s.list_files_for_project(&existing.id).unwrap().len(), 1);
    }

    #[test]
    fn apply_add_to_without_project_id_errors() {
        let s = store();
        let dek = Key32::generate();
        s.save_proposal(&dek, "p4", "{}", None, "{}").unwrap();
        let groups = vec![ConfirmedGroup {
            action: "add-to".into(),
            project_id: None,
            title: None,
            kind: None,
            items: vec![("i1".into(), "".into())],
        }];
        assert!(s.apply_proposal("p4", &groups).is_err());
        // 事务回滚:status 仍是 draft,无半建项目
        assert_eq!(s.proposal_status("p4").unwrap().as_deref(), Some("draft"));
        assert_eq!(s.list_projects(false).unwrap().len(), 0);
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
