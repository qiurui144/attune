//! agent_state — ACP-6 versioned, plugin-scoped, encrypted agent learned/user state.
//!
//! Schema lives in [`crate::store::SCHEMA_SQL`] (`CREATE TABLE IF NOT EXISTS
//! agent_state ...`). All methods are inherent `impl Store` — rustc merges
//! cross-file inherent impls automatically (same pattern as
//! `crate::store::skill_expansions`).
//!
//! Why a versioned, plugin-scoped table when `skill_expansions` already exists?
//!
//! The D audit (`2026-05-29-self-iteration-preservation-audit.md`) found:
//! - learned state survives plugin upgrade only **by lucky architecture**
//!   (vault DB vs plugin dir never touch), not by a tested contract;
//! - `skill_expansions` is **global, unscoped** — terms learned while a plugin
//!   was installed persist with no owner tag, so uninstall/upgrade cannot GC
//!   or migrate that plugin's accumulated state deliberately;
//! - there is **no per-row schema version**, so a key-format change across
//!   plugin versions silently orphans rows.
//!
//! `agent_state` closes all three: every row carries `(agent_id, plugin_id,
//! state_kind)` as its key plus a `schema_version`, and `payload` is a
//! DEK-encrypted BLOB (same field-level AES-256-GCM model as `items.content`).
//! This is the substrate the spec §5.4 contract describes.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;

/// Default plugin scope for state owned by the OSS base (no third-party plugin).
/// Matches the spec §10 backfill target for legacy global rows.
pub const OSS_CORE_PLUGIN: &str = "oss-core";

/// Kind of learned/user state a row holds. Bounded set per spec §5.4 —
/// `skill_expansion | preference | ratchet_watermark`. Reflected in the
/// `state_kind` TEXT column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStateKind {
    /// Per-agent learned query expansions (the scoped successor to the global
    /// `skill_expansions` table).
    SkillExpansion,
    /// User/agent preference blob (followup rank / verbosity / form defaults /
    /// retrieval weights / prompt hints — the spec §4.1 surface).
    Preference,
    /// Quality "water level" the agent has reached at the user's machine
    /// (ratchet watermark). Persisting it makes ratchet monotonicity checkable
    /// at the user's installed copy, not only in CI.
    RatchetWatermark,
}

impl AgentStateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStateKind::SkillExpansion => "skill_expansion",
            AgentStateKind::Preference => "preference",
            AgentStateKind::RatchetWatermark => "ratchet_watermark",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "skill_expansion" => Some(AgentStateKind::SkillExpansion),
            "preference" => Some(AgentStateKind::Preference),
            "ratchet_watermark" => Some(AgentStateKind::RatchetWatermark),
            _ => None,
        }
    }
}

/// One decrypted row of `agent_state`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStateRow {
    pub agent_id: String,
    pub plugin_id: String,
    pub state_kind: AgentStateKind,
    pub schema_version: i64,
    /// Decrypted payload bytes (caller owns the serialization format).
    pub payload: Vec<u8>,
    pub created_at: String,
    pub updated_at: String,
}

impl Store {
    /// Upsert one agent-state row. The `(agent_id, plugin_id, state_kind)`
    /// triple is the primary key; an existing row is overwritten in place
    /// (payload re-encrypted, `schema_version` updated, `updated_at` bumped).
    ///
    /// `payload` is encrypted with the vault DEK before storage — `agent_state`
    /// never holds plaintext user state.
    pub fn upsert_agent_state(
        &self,
        dek: &Key32,
        agent_id: &str,
        plugin_id: &str,
        state_kind: AgentStateKind,
        schema_version: i64,
        payload: &[u8],
    ) -> Result<()> {
        let enc = crypto::encrypt(dek, payload)?;
        self.conn.execute(
            "INSERT INTO agent_state \
                 (agent_id, plugin_id, state_kind, schema_version, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(agent_id, plugin_id, state_kind) DO UPDATE SET \
                 schema_version = excluded.schema_version, \
                 payload = excluded.payload, \
                 updated_at = datetime('now')",
            params![agent_id, plugin_id, state_kind.as_str(), schema_version, enc],
        )?;
        Ok(())
    }

    /// Fetch a single row by its full `(agent_id, plugin_id, state_kind)` key,
    /// decrypting the payload with the vault DEK.
    pub fn get_agent_state(
        &self,
        dek: &Key32,
        agent_id: &str,
        plugin_id: &str,
        state_kind: AgentStateKind,
    ) -> Result<Option<AgentStateRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT agent_id, plugin_id, state_kind, schema_version, payload, created_at, updated_at \
                 FROM agent_state WHERE agent_id = ?1 AND plugin_id = ?2 AND state_kind = ?3",
                params![agent_id, plugin_id, state_kind.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;

        let Some((aid, pid, kind_s, ver, enc, created, updated)) = row else {
            return Ok(None);
        };
        let Some(kind) = AgentStateKind::parse(&kind_s) else {
            return Ok(None);
        };
        let payload = crypto::decrypt(dek, &enc)?;
        Ok(Some(AgentStateRow {
            agent_id: aid,
            plugin_id: pid,
            state_kind: kind,
            schema_version: ver,
            payload,
            created_at: created,
            updated_at: updated,
        }))
    }

    /// List all rows owned by a given plugin (decrypted), newest first. Drives
    /// deliberate per-plugin GC/migration on uninstall/upgrade.
    pub fn list_agent_state_for_plugin(
        &self,
        dek: &Key32,
        plugin_id: &str,
    ) -> Result<Vec<AgentStateRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT agent_id, plugin_id, state_kind, schema_version, payload, created_at, updated_at \
             FROM agent_state WHERE plugin_id = ?1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![plugin_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (aid, pid, kind_s, ver, enc, created, updated) = r?;
            let Some(kind) = AgentStateKind::parse(&kind_s) else {
                continue;
            };
            let payload = crypto::decrypt(dek, &enc)?;
            out.push(AgentStateRow {
                agent_id: aid,
                plugin_id: pid,
                state_kind: kind,
                schema_version: ver,
                payload,
                created_at: created,
                updated_at: updated,
            });
        }
        Ok(out)
    }

    /// Delete a single row by key. Returns true if a row was removed.
    pub fn delete_agent_state(
        &self,
        agent_id: &str,
        plugin_id: &str,
        state_kind: AgentStateKind,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM agent_state WHERE agent_id = ?1 AND plugin_id = ?2 AND state_kind = ?3",
            params![agent_id, plugin_id, state_kind.as_str()],
        )?;
        Ok(n > 0)
    }

    /// Total row count (cheap governance stat).
    pub fn count_agent_state(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM agent_state", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dek() -> Key32 {
        Key32::generate()
    }

    #[test]
    fn upsert_and_get_roundtrip_encrypted() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(
                &dek,
                "defamation_extractor",
                "law-pro",
                AgentStateKind::Preference,
                1,
                b"{\"verbosity\":\"terse\"}",
            )
            .unwrap();
        let row = store
            .get_agent_state(&dek, "defamation_extractor", "law-pro", AgentStateKind::Preference)
            .unwrap()
            .unwrap();
        assert_eq!(row.payload, b"{\"verbosity\":\"terse\"}");
        assert_eq!(row.schema_version, 1);
        assert_eq!(row.plugin_id, "law-pro");
        assert_eq!(row.state_kind, AgentStateKind::Preference);
    }

    #[test]
    fn payload_is_encrypted_at_rest() {
        // The raw BLOB on disk must not contain the plaintext — proves DEK
        // encryption actually happened (not a plaintext column).
        let store = Store::open_memory().unwrap();
        let dek = dek();
        let secret = b"top-secret-learned-preference";
        store
            .upsert_agent_state(&dek, "a", "oss-core", AgentStateKind::Preference, 1, secret)
            .unwrap();
        let raw: Vec<u8> = store
            .raw_connection_for_test()
            .query_row("SELECT payload FROM agent_state", [], |r| r.get(0))
            .unwrap();
        assert_ne!(raw, secret, "payload must be ciphertext, not plaintext");
        // And the wrong DEK must fail to decrypt.
        let wrong = dek2_distinct(&dek);
        let err = store.get_agent_state(&wrong, "a", "oss-core", AgentStateKind::Preference);
        assert!(err.is_err(), "decrypt with wrong DEK must error, not return garbage");
    }

    // Generate a DEK guaranteed different from `other` (Key32::generate is random
    // but we assert distinctness so the test cannot flake on an astronomically
    // unlikely collision).
    fn dek2_distinct(other: &Key32) -> Key32 {
        loop {
            let k = Key32::generate();
            if k.as_bytes() != other.as_bytes() {
                return k;
            }
        }
    }

    #[test]
    fn plugin_id_isolation() {
        // Same agent_id + state_kind but different plugin_id are distinct rows —
        // the plugin scope is part of the identity (D audit fix).
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "shared_agent", "law-pro", AgentStateKind::SkillExpansion, 1, b"law")
            .unwrap();
        store
            .upsert_agent_state(&dek, "shared_agent", "tech-pro", AgentStateKind::SkillExpansion, 1, b"tech")
            .unwrap();

        let law = store
            .get_agent_state(&dek, "shared_agent", "law-pro", AgentStateKind::SkillExpansion)
            .unwrap()
            .unwrap();
        let tech = store
            .get_agent_state(&dek, "shared_agent", "tech-pro", AgentStateKind::SkillExpansion)
            .unwrap()
            .unwrap();
        assert_eq!(law.payload, b"law");
        assert_eq!(tech.payload, b"tech");

        // Listing by plugin returns only that plugin's rows.
        let law_rows = store.list_agent_state_for_plugin(&dek, "law-pro").unwrap();
        assert_eq!(law_rows.len(), 1);
        assert_eq!(law_rows[0].plugin_id, "law-pro");
        assert_eq!(store.count_agent_state().unwrap(), 2);
    }

    #[test]
    fn upsert_overwrites_in_place() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "a", "oss-core", AgentStateKind::RatchetWatermark, 1, b"0.80")
            .unwrap();
        store
            .upsert_agent_state(&dek, "a", "oss-core", AgentStateKind::RatchetWatermark, 2, b"0.85")
            .unwrap();
        assert_eq!(store.count_agent_state().unwrap(), 1, "same key must not duplicate");
        let row = store
            .get_agent_state(&dek, "a", "oss-core", AgentStateKind::RatchetWatermark)
            .unwrap()
            .unwrap();
        assert_eq!(row.payload, b"0.85");
        assert_eq!(row.schema_version, 2);
    }

    #[test]
    fn delete_removes_row() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "a", "oss-core", AgentStateKind::Preference, 1, b"x")
            .unwrap();
        assert!(store
            .delete_agent_state("a", "oss-core", AgentStateKind::Preference)
            .unwrap());
        assert!(store
            .get_agent_state(&dek, "a", "oss-core", AgentStateKind::Preference)
            .unwrap()
            .is_none());
        // Deleting a missing row returns false.
        assert!(!store
            .delete_agent_state("a", "oss-core", AgentStateKind::Preference)
            .unwrap());
    }

    #[test]
    fn get_missing_returns_none() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        assert!(store
            .get_agent_state(&dek, "nope", "oss-core", AgentStateKind::Preference)
            .unwrap()
            .is_none());
    }

    #[test]
    fn state_kind_roundtrips_via_str() {
        for k in [
            AgentStateKind::SkillExpansion,
            AgentStateKind::Preference,
            AgentStateKind::RatchetWatermark,
        ] {
            assert_eq!(AgentStateKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(AgentStateKind::parse("bogus"), None);
    }

    #[test]
    fn empty_payload_roundtrips() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "a", "oss-core", AgentStateKind::Preference, 1, b"")
            .unwrap();
        let row = store
            .get_agent_state(&dek, "a", "oss-core", AgentStateKind::Preference)
            .unwrap()
            .unwrap();
        assert_eq!(row.payload, b"");
    }
}
