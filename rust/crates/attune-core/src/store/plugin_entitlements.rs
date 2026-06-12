//! 客户端 entitlement 缓存持久化(spec §3.2 / §5.2;T4)。
//!
//! 付费插件(law-pro 等)的本地授权态缓存,落 vault DB 新表 `plugin_entitlements`,
//! 随 vault 字段级加密 + 锁(ACP-6 边界:**不进** `plugins/<id>/`,插件升级 wholesale
//! 替换不触碰此表 —— 属 user-accumulated 类)。
//!
//! ## 加密边界
//!
//! `license_id` 是敏感 license 标识 → 字段级 AES-256-GCM 加密(dek,与
//! `git_sources.token_ref_enc` / `items.content` 同模式)落 `license_id_enc` BLOB。
//! `plugin_id` / `tier` / `status` / 时间戳是非敏感(plugin_id 已是 plugins/ 目录名),
//! 明文存以支持 PRIMARY KEY 查询 + O(1) hydrate(spec §3.3 dispatch 热点)。
//!
//! ## migration
//!
//! 走 `CREATE TABLE IF NOT EXISTS`(additive,与 `git_sources` / `signals` 同惯例),
//! **不 bump `SCHEMA_VERSION`**(per `store/mod.rs` 注释:纯追加表不 bump)。老 vault
//! 下次 open 自动建空表。

use rusqlite::OptionalExtension;

use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;

/// 一条 entitlement 缓存行(写入 / 读出共用;`license_id` 是明文,落库前加密)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitlementRow {
    pub plugin_id: String,
    pub license_id: String,
    /// free | trial | paid
    pub tier: String,
    /// active | suspended | revoked
    pub status: String,
    /// RFC3339 | None(trial 到期时间)
    pub trial_expires: Option<String>,
    pub signing_pubkey_hex: String,
    /// RFC3339,时钟回拨检测 + freshness 单调基准(SEC-2)。
    pub last_verified_at: String,
    /// RFC3339 | None(None = 非宽限态)。
    pub grace_started_at: Option<String>,
    pub updated_at: String,
}

/// 行的加密中间态(license_id 仍是密文 BLOB)—— 避免在 query closure 里返回
/// 巨型 tuple(clippy::type_complexity)。`decrypt` 后转 [`EntitlementRow`]。
struct EncRow {
    plugin_id: String,
    license_id_enc: Vec<u8>,
    tier: String,
    status: String,
    trial_expires: Option<String>,
    signing_pubkey_hex: String,
    last_verified_at: String,
    grace_started_at: Option<String>,
    updated_at: String,
}

impl EncRow {
    fn from_sql_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(EncRow {
            plugin_id: r.get(0)?,
            license_id_enc: r.get(1)?,
            tier: r.get(2)?,
            status: r.get(3)?,
            trial_expires: r.get(4)?,
            signing_pubkey_hex: r.get(5)?,
            last_verified_at: r.get(6)?,
            grace_started_at: r.get(7)?,
            updated_at: r.get(8)?,
        })
    }

    fn decrypt(self, dek: &Key32) -> Result<EntitlementRow> {
        let license_id = String::from_utf8(crypto::decrypt(dek, &self.license_id_enc)?)
            .map_err(|e| crate::error::VaultError::Crypto(format!("license_id utf8: {e}")))?;
        Ok(EntitlementRow {
            plugin_id: self.plugin_id,
            license_id,
            tier: self.tier,
            status: self.status,
            trial_expires: self.trial_expires,
            signing_pubkey_hex: self.signing_pubkey_hex,
            last_verified_at: self.last_verified_at,
            grace_started_at: self.grace_started_at,
            updated_at: self.updated_at,
        })
    }
}

/// status 优先级:active > trial(其他更高) > suspended/revoked。
/// 用于 multi-license 同 plugin 取最优(spec §7.2)。数字越大越优。
fn status_rank(status: &str) -> u8 {
    match status {
        "active" => 3,
        "trial" => 2,
        "suspended" => 1,
        _ => 0, // revoked / unknown
    }
}

impl Store {
    /// upsert 一条 entitlement。`license_id` 用 dek 加密成 BLOB 落盘。
    ///
    /// **multi-license 同 plugin 归并(PERF-5)**:`plugin_id` 是 PRIMARY KEY,同一
    /// plugin 只保留**最优 status** 的一条 —— upsert 时若已有行且现有 status 优于
    /// 入参 status,则保留现有(不降级);否则覆盖。这样 `get_entitlement` 是 O(1)
    /// 直接返最优态,dispatch 热点无需运行期遍历多 license。
    pub fn upsert_entitlement(&self, dek: &Key32, row: &EntitlementRow) -> Result<()> {
        // 归并:若已存在且现有 status rank >= 入参 rank,保留现有(取最优,不降级)。
        if let Some(existing) = self.get_entitlement(dek, &row.plugin_id)? {
            if status_rank(&existing.status) > status_rank(&row.status) {
                return Ok(());
            }
        }
        let license_id_enc = crypto::encrypt(dek, row.license_id.as_bytes())?;
        self.conn.execute(
            "INSERT INTO plugin_entitlements
                (plugin_id, license_id_enc, tier, status, trial_expires,
                 signing_pubkey_hex, last_verified_at, grace_started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(plugin_id) DO UPDATE SET
                license_id_enc=excluded.license_id_enc,
                tier=excluded.tier,
                status=excluded.status,
                trial_expires=excluded.trial_expires,
                signing_pubkey_hex=excluded.signing_pubkey_hex,
                last_verified_at=excluded.last_verified_at,
                grace_started_at=excluded.grace_started_at,
                updated_at=excluded.updated_at",
            rusqlite::params![
                row.plugin_id,
                license_id_enc,
                row.tier,
                row.status,
                row.trial_expires,
                row.signing_pubkey_hex,
                row.last_verified_at,
                row.grace_started_at,
                row.updated_at,
            ],
        )?;
        Ok(())
    }

    /// 显式降级落盘 —— **唯一**能把更优态降到更劣态的持久化入口(镜像内存层
    /// [`crate::entitlement::EntitlementCache::set_status`])。re-verify worker 收到
    /// **验签通过的** revoked/suspended 时调用,**绕过** [`Self::upsert_entitlement`]
    /// 的 PERF-5 反降级归并 —— 否则一条已存的 `active` 行会拒绝吊销写盘,导致吊销
    /// 在重启 / re-unlock 后复活(REVIEW Critical-1)。
    ///
    /// 直接 `UPDATE ... WHERE plugin_id=?`,**无 rank guard**。行不存在 → 影响 0 行
    /// (无错):付费插件吊销时该行必由 install 时写入存在;免费插件无行 → is_entitled
    /// 本就 Allow,无需吊销。
    pub fn set_entitlement_status(
        &self,
        plugin_id: &str,
        status: &str,
        last_verified_at: &str,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "UPDATE plugin_entitlements
                SET status = ?1, last_verified_at = ?2, updated_at = ?2
              WHERE plugin_id = ?3",
        )?;
        stmt.execute(rusqlite::params![status, last_verified_at, plugin_id])?;
        Ok(())
    }

    /// 读一条 entitlement(license_id 解密回明文)。
    pub fn get_entitlement(&self, dek: &Key32, plugin_id: &str) -> Result<Option<EntitlementRow>> {
        let enc_row: Option<EncRow> = self
            .conn
            .query_row(
                "SELECT plugin_id, license_id_enc, tier, status, trial_expires, signing_pubkey_hex,
                        last_verified_at, grace_started_at, updated_at
                 FROM plugin_entitlements WHERE plugin_id = ?1",
                rusqlite::params![plugin_id],
                EncRow::from_sql_row,
            )
            .optional()?;
        match enc_row {
            None => Ok(None),
            Some(e) => Ok(Some(e.decrypt(dek)?)),
        }
    }

    /// 列出全部 entitlement(启动 hydrate 用)。
    pub fn list_entitlements(&self, dek: &Key32) -> Result<Vec<EntitlementRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT plugin_id, license_id_enc, tier, status, trial_expires,
                    signing_pubkey_hex, last_verified_at, grace_started_at, updated_at
             FROM plugin_entitlements ORDER BY plugin_id",
        )?;
        let rows = stmt.query_map([], EncRow::from_sql_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.decrypt(dek)?);
        }
        Ok(out)
    }

    /// 删一条 entitlement(uninstall / 用户清理)。
    pub fn delete_entitlement(&self, plugin_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM plugin_entitlements WHERE plugin_id = ?1", rusqlite::params![plugin_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(plugin_id: &str, status: &str) -> EntitlementRow {
        EntitlementRow {
            plugin_id: plugin_id.to_string(),
            license_id: "lic-secret-12345".to_string(),
            tier: "paid".to_string(),
            status: status.to_string(),
            trial_expires: Some("2026-12-31T00:00:00+00:00".to_string()),
            signing_pubkey_hex: "8866ae9b8f0026aaa99902a34fa06223b5e88d5a8f933c7f084342cb9953bcac"
                .to_string(),
            last_verified_at: "2026-06-12T00:00:00+00:00".to_string(),
            grace_started_at: None,
            updated_at: "2026-06-12T00:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn upsert_then_get_roundtrip() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let r = row("law-pro", "active");
        store.upsert_entitlement(&dek, &r).unwrap();
        let got = store.get_entitlement(&dek, "law-pro").unwrap().unwrap();
        assert_eq!(got, r, "roundtrip must preserve all fields (incl decrypted license_id)");
        // license_id was encrypted at rest — confirm the BLOB is not the plaintext.
        let raw_blob: Vec<u8> = store
            .raw_connection_for_test()
            .query_row(
                "SELECT license_id_enc FROM plugin_entitlements WHERE plugin_id='law-pro'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(raw_blob, r.license_id.as_bytes(), "license_id must be encrypted at rest");
    }

    #[test]
    fn old_vault_auto_creates_table() {
        // open_memory runs SCHEMA_SQL which includes the CREATE TABLE IF NOT EXISTS.
        let store = Store::open_memory().unwrap();
        // SCHEMA_VERSION must NOT be bumped by an additive table.
        assert_eq!(store.schema_version().unwrap(), crate::store::SCHEMA_VERSION);
        // Table exists and is queryable (empty).
        let dek = Key32::generate();
        assert!(store.list_entitlements(&dek).unwrap().is_empty());
    }

    #[test]
    fn multi_license_same_plugin_picks_best_status() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // First write a suspended row, then an active row for the SAME plugin.
        store.upsert_entitlement(&dek, &row("law-pro", "suspended")).unwrap();
        store.upsert_entitlement(&dek, &row("law-pro", "active")).unwrap();
        let got = store.get_entitlement(&dek, "law-pro").unwrap().unwrap();
        // PERF-5: best status (active > trial > others) is resolved AT upsert time;
        // get returns the merged-best directly with no runtime scan of multiple rows.
        assert_eq!(got.status, "active", "active must win over suspended");

        // Now a lower-priority status must NOT downgrade the stored active.
        store.upsert_entitlement(&dek, &row("law-pro", "revoked")).unwrap();
        // Exception: revoke is a deliberate downgrade and IS allowed via explicit path,
        // but the generic upsert keeps best-status (revoke goes through re-verify, see T5/T8).
        let after = store.get_entitlement(&dek, "law-pro").unwrap().unwrap();
        assert_eq!(after.status, "active", "generic upsert never silently downgrades active");
    }

    #[test]
    fn list_all_for_hydrate() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store.upsert_entitlement(&dek, &row("law-pro", "active")).unwrap();
        store.upsert_entitlement(&dek, &row("med-pro", "trial")).unwrap();
        let all = store.list_entitlements(&dek).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].plugin_id, "law-pro");
        assert_eq!(all[1].plugin_id, "med-pro");
    }

    #[test]
    fn delete_removes_row() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store.upsert_entitlement(&dek, &row("law-pro", "active")).unwrap();
        store.delete_entitlement("law-pro").unwrap();
        assert!(store.get_entitlement(&dek, "law-pro").unwrap().is_none());
    }

    #[test]
    fn persisted_revoke_survives_reopen() {
        // REVIEW Critical-1: an AUTHORITATIVE VERIFIED revoke must reach disk and
        // survive restart / re-unlock — even though an existing `active` row would
        // make the anti-downgrade merge (upsert_entitlement) refuse it.
        use crate::entitlement::{EntitlementCache, EntitlementDecision};
        use chrono::Utc;

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let dek = Key32::generate();
        {
            let store = Store::open(&path).unwrap();
            // Paid plugin installed → active row on disk.
            store.upsert_entitlement(&dek, &row("law-pro", "active")).unwrap();
            // Cloud revokes; re-verify worker persists the VERIFIED deny via the
            // explicit-downgrade path (NOT upsert, which would be eaten by the merge).
            store
                .set_entitlement_status("law-pro", "revoked", "2026-06-12T10:00:00+00:00")
                .unwrap();
            // Sanity within the same Store: the downgrade landed despite prior active.
            let same = store.get_entitlement(&dek, "law-pro").unwrap().unwrap();
            assert_eq!(same.status, "revoked", "explicit downgrade must overwrite active");
        }

        // (1) Durable: reopen a FRESH Store on the same DB path — revoke survived.
        let store2 = Store::open(&path).unwrap();
        let got = store2.get_entitlement(&dek, "law-pro").unwrap().unwrap();
        assert_eq!(
            got.status, "revoked",
            "verified revoke must survive restart/re-unlock (not revive as active)"
        );

        // (2) Rehydrate path: a fresh EntitlementCache built from list_entitlements
        // must dispatch-Reject the revoked plugin (license-revoked), not Allow.
        let cache = EntitlementCache::new();
        cache.hydrate_from_rows(store2.list_entitlements(&dek).unwrap());
        let now = Utc::now();
        assert_eq!(
            cache.is_entitled("law-pro", &now),
            EntitlementDecision::Reject("license-revoked"),
            "rehydrated cache must reject a revoked plugin, not revive it to Allow"
        );
    }

    #[test]
    fn upsert_persists_through_open_path() {
        // open(path) parity with open_memory (acceptance: both paths build the table).
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let dek = Key32::generate();
        {
            let store = Store::open(&path).unwrap();
            store.upsert_entitlement(&dek, &row("law-pro", "active")).unwrap();
        }
        // Re-open: the row survives (persisted, not memory-only).
        let store2 = Store::open(&path).unwrap();
        let got = store2.get_entitlement(&dek, "law-pro").unwrap().unwrap();
        assert_eq!(got.status, "active");
        assert_eq!(store2.schema_version().unwrap(), crate::store::SCHEMA_VERSION);
    }
}
