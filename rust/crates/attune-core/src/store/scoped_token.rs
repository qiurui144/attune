//! G2 — Scoped token 元数据持久化。
//!
//! Scoped token = 外部 agent host(经 MCP)调 attune 私有知识库的最小权限授权。
//! 三权封闭集:`search` / `chat` / `ingest`。
//!
//! ## 安全设计
//! - **token 明文不落库**:只落 `token_id`(payload 里的 random id)+ 权限位 + label +
//!   过期 + 吊销标志。token 明文经 HMAC(over vault master_key)签发,签发时**仅返回一次**。
//! - **校验 = HMAC 重算 + DB 查 token_id**:必须 (a) HMAC 有效 (b) DB 有该 token_id 行
//!   (c) 未吊销 (d) 未过期。任一不满足 → 拒。吊销 = DB 行 `revoked=1`,即时生效(不缓存)。
//! - **独立命名空间**:payload 前缀 `scoped:` 与 vault session token 区分,
//!   防 session token 当 scoped 用、反之亦然。
//!
//! 表 `scoped_tokens` 在 `crate::store::SCHEMA_SQL` 声明(`CREATE TABLE IF NOT EXISTS`,additive)。

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::Store;

/// scoped token 的权限位 + 元数据(明文 token 不在此结构)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedTokenMeta {
    /// payload 内的随机 id(= DB 主键),非 token 明文。
    pub token_id: String,
    /// 人类可读标签(审计 agent_source 用)。
    pub label: String,
    /// 逗号分隔的权限位,子集 ⊆ {search,chat,ingest}。
    pub scopes: Vec<String>,
    /// 过期 epoch 秒。
    pub expires_at: i64,
    /// 是否已吊销。
    pub revoked: bool,
    /// 签发 epoch 秒。
    pub created_at: i64,
}

/// 合法权限位封闭集(三权)。新增权限是架构决策,需新 spec。
pub const VALID_SCOPES: &[&str] = &["search", "chat", "ingest"];

/// 校验 scopes 全部 ∈ VALID_SCOPES 且非空。
pub fn validate_scopes(scopes: &[String]) -> std::result::Result<(), String> {
    if scopes.is_empty() {
        return Err("scopes must not be empty".into());
    }
    for s in scopes {
        if !VALID_SCOPES.contains(&s.as_str()) {
            return Err(format!(
                "invalid scope '{s}' (allowed: {})",
                VALID_SCOPES.join(",")
            ));
        }
    }
    Ok(())
}

impl Store {
    /// 插入一条 scoped token 元数据(签发时调用)。token 明文不落,只落元数据。
    pub fn insert_scoped_token(&self, meta: &ScopedTokenMeta) -> Result<()> {
        self.conn.execute(
            "INSERT INTO scoped_tokens (token_id, label, scopes, expires_at, revoked, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![
                &meta.token_id,
                &meta.label,
                meta.scopes.join(","),
                meta.expires_at,
                meta.created_at,
            ],
        )?;
        Ok(())
    }

    /// 按 token_id 查元数据(校验路径用)。返回 None = 该 token_id 不存在(伪造/已删)。
    pub fn get_scoped_token(&self, token_id: &str) -> Result<Option<ScopedTokenMeta>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT token_id, label, scopes, expires_at, revoked, created_at
             FROM scoped_tokens WHERE token_id = ?1",
        )?;
        let row = stmt
            .query_row(params![token_id], |r| {
                let scopes_csv: String = r.get(2)?;
                Ok(ScopedTokenMeta {
                    token_id: r.get(0)?,
                    label: r.get(1)?,
                    scopes: scopes_csv
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect(),
                    expires_at: r.get(3)?,
                    revoked: r.get::<_, i64>(4)? != 0,
                    created_at: r.get(5)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    /// 列出所有 scoped token 元数据(settings 面板用,**不含 token 明文**)。
    pub fn list_scoped_tokens(&self) -> Result<Vec<ScopedTokenMeta>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT token_id, label, scopes, expires_at, revoked, created_at
             FROM scoped_tokens ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let scopes_csv: String = r.get(2)?;
            Ok(ScopedTokenMeta {
                token_id: r.get(0)?,
                label: r.get(1)?,
                scopes: scopes_csv
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
                expires_at: r.get(3)?,
                revoked: r.get::<_, i64>(4)? != 0,
                created_at: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 吊销一个 scoped token(set revoked=1)。返回是否改动了行(false = 不存在)。
    /// 吊销即时生效(校验路径每次查 DB,不缓存)。
    pub fn revoke_scoped_token(&self, token_id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE scoped_tokens SET revoked = 1 WHERE token_id = ?1 AND revoked = 0",
            params![token_id],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> ScopedTokenMeta {
        ScopedTokenMeta {
            token_id: id.into(),
            label: "test-agent".into(),
            scopes: vec!["search".into(), "chat".into()],
            expires_at: 9_999_999_999,
            revoked: false,
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn insert_get_list_revoke() {
        let store = Store::open_memory().unwrap();
        store.insert_scoped_token(&sample("tok1")).unwrap();

        let got = store.get_scoped_token("tok1").unwrap().unwrap();
        assert_eq!(got.label, "test-agent");
        assert_eq!(got.scopes, vec!["search", "chat"]);
        assert!(!got.revoked);

        assert!(store.get_scoped_token("nope").unwrap().is_none());

        let list = store.list_scoped_tokens().unwrap();
        assert_eq!(list.len(), 1);

        // revoke 改动一次,二次无改动
        assert!(store.revoke_scoped_token("tok1").unwrap());
        assert!(!store.revoke_scoped_token("tok1").unwrap());
        assert!(store.get_scoped_token("tok1").unwrap().unwrap().revoked);
    }

    #[test]
    fn validate_scopes_rejects_unknown_and_empty() {
        assert!(validate_scopes(&[]).is_err());
        assert!(validate_scopes(&["search".into()]).is_ok());
        assert!(validate_scopes(&["delete".into()]).is_err());
        assert!(validate_scopes(&["search".into(), "export".into()]).is_err());
    }
}
