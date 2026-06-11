//! state_migration — ACP-6 Task 3: learned-state migration + orphan quarantine.
//!
//! D audit head risk (R3): a plugin upgrade that changes an agent-id / key
//! format across versions silently orphans the user's accumulated learned
//! state — keys stop matching, rows become unreachable, no migration, no
//! detection. Spec §2.3 red line: **migration NEVER silently drops user
//! learned state; orphans must be detectable and recoverable.**
//!
//! This module turns the incidental guarantee into a tested one:
//!
//! 1. **Backup first** — before any migration, `VACUUM INTO` a backup file so a
//!    failed/buggy migration is recoverable (R3 "migration 前 backup").
//! 2. **Registered migrators** — each [`MigrationStep`] declares the version it
//!    advances a row from→to and a pure transform (`migrate_row`) that may also
//!    re-key the row (agent-id / plugin-id / state-kind rename).
//! 3. **Orphan quarantine** — any row at a stale `schema_version` that no
//!    registered step can advance is **copied** to `agent_state_orphans`
//!    (payload preserved, still DEK-encrypted) and flagged — it is **never
//!    deleted**. A later plugin version shipping a migrator can re-claim it.
//!
//! Why payload migration is a post-unlock step (not at `open()`): payload is
//! DEK-encrypted and the DEK is only available after vault unlock. The
//! `open()`-time [`crate::store::Store::ensure_schema_version`] handles only the
//! structural version stamp; payload-transforming migration runs here, after
//! unlock, when the caller holds the DEK.

use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::agent_state::{AgentStateKind, AgentStateRow};
use crate::store::Store;

/// A row produced by a migrator — may re-key (rename agent/plugin/kind) the
/// original row. The new `schema_version` is taken from the step's `to_version`.
#[derive(Debug, Clone, PartialEq)]
pub struct MigratedRow {
    pub agent_id: String,
    pub plugin_id: String,
    pub state_kind: AgentStateKind,
    pub payload: Vec<u8>,
}

/// One registered migration step. `migrate_row` is a pure function: given a
/// decrypted source row, it returns the migrated row, or `None` if this step
/// does not claim the row (it may then be claimed by another step, or, if no
/// step claims it, quarantined as an orphan).
#[derive(Clone, Copy)]
pub struct MigrationStep {
    pub from_version: i64,
    pub to_version: i64,
    pub migrate_row: fn(&AgentStateRow) -> Option<MigratedRow>,
}

/// A raw `agent_state` row with the `state_kind` left as its on-disk string and
/// the payload decrypted. Lets the migrator quarantine unknown-kind rows
/// (§2.3) instead of dropping them when the enum cannot parse.
struct RawStateRow {
    agent_id: String,
    plugin_id: String,
    state_kind: String,
    schema_version: i64,
    payload: Vec<u8>,
    created_at: String,
    updated_at: String,
}

/// A quarantined orphan row (decrypted view for inspection / recovery UI).
#[derive(Debug, Clone, PartialEq)]
pub struct OrphanRow {
    pub agent_id: String,
    pub plugin_id: String,
    pub state_kind: String,
    pub schema_version: i64,
    pub payload: Vec<u8>,
    pub reason: String,
    pub detected_at: String,
}

/// Outcome of a migration pass.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MigrationReport {
    /// Rows successfully advanced to the target version.
    pub migrated: usize,
    /// Rows quarantined to `agent_state_orphans` (no migrator could advance them).
    pub orphaned: usize,
    /// Rows already at/above target (left untouched).
    pub already_current: usize,
    /// Backup file written before migration (if a backup dir was requested).
    pub backup_path: Option<PathBuf>,
}

impl Store {
    /// Back up the entire vault DB to `dir` via SQLite `VACUUM INTO`, returning
    /// the backup file path. R3: a migration must be preceded by a real backup.
    ///
    /// `VACUUM INTO` writes a fully consistent snapshot (not a raw file copy),
    /// safe even while the DB is open. The filename embeds a UTC timestamp.
    pub fn backup_vault_to(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
        let path = dir.join(format!("vault-backup-{ts}.db"));
        // VACUUM INTO does not accept a bound parameter for the path; quote-escape.
        let escaped = path.to_string_lossy().replace('\'', "''");
        self.conn
            .execute_batch(&format!("VACUUM INTO '{escaped}';"))?;
        Ok(path)
    }

    /// Migrate `agent_state` rows below `target_version` using the registered
    /// `steps`, quarantining any row no step can advance.
    ///
    /// - `dek`: vault DEK (payloads are decrypted to feed migrators, re-encrypted
    ///   on write).
    /// - `backup_dir`: if `Some`, a backup is taken before any mutation (R3).
    /// - `steps`: registered migrators. At the v1 baseline the production caller
    ///   passes `&[]`; tests inject real steps. Steps are applied in a chain
    ///   (a row at v1 with steps v1→v2 and v2→v3 reaches v3).
    ///
    /// §2.3 red line: rows the chain cannot advance are **copied** to
    /// `agent_state_orphans` and left in place — never silently dropped.
    pub fn migrate_agent_state(
        &self,
        dek: &Key32,
        target_version: i64,
        steps: &[MigrationStep],
        backup_dir: Option<&Path>,
    ) -> Result<MigrationReport> {
        let mut report = MigrationReport::default();

        // Backup BEFORE touching anything.
        if let Some(dir) = backup_dir {
            report.backup_path = Some(self.backup_vault_to(dir)?);
        }

        // Snapshot all rows below target (decrypt payloads up front so the
        // migrators see plaintext; we don't hold a statement open while mutating).
        // Rows with an UNKNOWN state_kind (e.g. written by a newer attune) are
        // kept as raw rows so they can be quarantined, never silently skipped.
        let stale = self.collect_rows_below(dek, target_version)?;

        for raw in stale {
            // Unparseable kind → cannot be fed to a migrator; §2.3 says quarantine,
            // not drop. Use the raw kind string so nothing is lost.
            let Some(kind) = AgentStateKind::parse(&raw.state_kind) else {
                let reason = format!(
                    "unknown state_kind '{}' at v{} (agent_id={}, plugin_id={})",
                    raw.state_kind, raw.schema_version, raw.agent_id, raw.plugin_id
                );
                self.quarantine_orphan_raw(dek, &raw, &reason)?;
                report.orphaned += 1;
                continue;
            };
            let row = AgentStateRow {
                agent_id: raw.agent_id,
                plugin_id: raw.plugin_id,
                state_kind: kind,
                schema_version: raw.schema_version,
                payload: raw.payload,
                created_at: raw.created_at,
                updated_at: raw.updated_at,
            };
            match Self::advance_row(&row, target_version, steps) {
                Some(migrated) => {
                    self.apply_migrated_row(dek, &row, &migrated, target_version)?;
                    report.migrated += 1;
                }
                None => {
                    // §2.3: no migrator → quarantine, do NOT delete.
                    let reason = format!(
                        "no migrator for v{}->v{} (agent_id={}, plugin_id={})",
                        row.schema_version, target_version, row.agent_id, row.plugin_id
                    );
                    self.quarantine_orphan(dek, &row, &reason)?;
                    report.orphaned += 1;
                }
            }
        }

        Ok(report)
    }

    /// List quarantined orphans (decrypted) for a recovery / diagnostics UI.
    pub fn list_agent_state_orphans(&self, dek: &Key32) -> Result<Vec<OrphanRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT agent_id, plugin_id, state_kind, schema_version, payload, reason, detected_at \
             FROM agent_state_orphans ORDER BY detected_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (aid, pid, kind, ver, enc, reason, detected) = r?;
            let payload = crypto::decrypt(dek, &enc)?;
            out.push(OrphanRow {
                agent_id: aid,
                plugin_id: pid,
                state_kind: kind,
                schema_version: ver,
                payload,
                reason,
                detected_at: detected,
            });
        }
        Ok(out)
    }

    pub fn count_agent_state_orphans(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM agent_state_orphans", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    // ── internals ────────────────────────────────────────────────────────

    /// Decrypt all `agent_state` rows with `schema_version < target`, keeping
    /// the `state_kind` as its **raw string** so the caller can quarantine rows
    /// with an unknown kind rather than silently dropping them (§2.3).
    fn collect_rows_below(&self, dek: &Key32, target: i64) -> Result<Vec<RawStateRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT agent_id, plugin_id, state_kind, schema_version, payload, created_at, updated_at \
             FROM agent_state WHERE schema_version < ?1",
        )?;
        let rows = stmt.query_map(params![target], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (aid, pid, kind_s, ver, enc, created, updated) = r?;
            let payload = crypto::decrypt(dek, &enc)?;
            out.push(RawStateRow {
                agent_id: aid,
                plugin_id: pid,
                state_kind: kind_s,
                schema_version: ver,
                payload,
                created_at: created,
                updated_at: updated,
            });
        }
        Ok(out)
    }

    /// Walk the step chain to advance a row from its version up to `target`.
    /// Returns the final migrated row, or `None` if any link in the chain is
    /// missing or a migrator declines the row.
    fn advance_row(
        row: &AgentStateRow,
        target: i64,
        steps: &[MigrationStep],
    ) -> Option<MigratedRow> {
        let mut cur_version = row.schema_version;
        // Carry a mutable working row across chained steps.
        let mut working = AgentStateRow {
            agent_id: row.agent_id.clone(),
            plugin_id: row.plugin_id.clone(),
            state_kind: row.state_kind,
            schema_version: cur_version,
            payload: row.payload.clone(),
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        };
        let mut last: Option<MigratedRow> = None;
        while cur_version < target {
            let step = steps
                .iter()
                .find(|s| s.from_version == cur_version && s.to_version <= target)?;
            let out = (step.migrate_row)(&working)?;
            cur_version = step.to_version;
            working = AgentStateRow {
                agent_id: out.agent_id.clone(),
                plugin_id: out.plugin_id.clone(),
                state_kind: out.state_kind,
                schema_version: cur_version,
                payload: out.payload.clone(),
                created_at: working.created_at,
                updated_at: working.updated_at,
            };
            last = Some(out);
        }
        last
    }

    /// Persist a migrated row at `target` version. If the migrator re-keyed the
    /// row, the old key is deleted and the new key inserted (atomic via tx).
    fn apply_migrated_row(
        &self,
        dek: &Key32,
        original: &AgentStateRow,
        migrated: &MigratedRow,
        target: i64,
    ) -> Result<()> {
        let enc = crypto::encrypt(dek, &migrated.payload)?;
        let tx = self.conn.unchecked_transaction()?;
        let rekeyed = migrated.agent_id != original.agent_id
            || migrated.plugin_id != original.plugin_id
            || migrated.state_kind != original.state_kind;
        if rekeyed {
            tx.execute(
                "DELETE FROM agent_state WHERE agent_id = ?1 AND plugin_id = ?2 AND state_kind = ?3",
                params![
                    original.agent_id,
                    original.plugin_id,
                    original.state_kind.as_str()
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO agent_state \
                 (agent_id, plugin_id, state_kind, schema_version, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(agent_id, plugin_id, state_kind) DO UPDATE SET \
                 schema_version = excluded.schema_version, \
                 payload = excluded.payload, \
                 updated_at = datetime('now')",
            params![
                migrated.agent_id,
                migrated.plugin_id,
                migrated.state_kind.as_str(),
                target,
                enc
            ],
        )?;
        tx.commit().map_err(VaultError::from)?;
        Ok(())
    }

    /// Copy an un-advanceable (but parseable-kind) row into the orphan
    /// quarantine, preserving the ciphertext. §2.3: the original row in
    /// `agent_state` is **left in place** (not deleted) so nothing is lost even
    /// if quarantine readout later fails.
    fn quarantine_orphan(&self, dek: &Key32, row: &AgentStateRow, reason: &str) -> Result<()> {
        self.quarantine_orphan_inner(
            dek,
            &row.agent_id,
            &row.plugin_id,
            row.state_kind.as_str(),
            row.schema_version,
            &row.payload,
            reason,
        )
    }

    /// Same as [`Self::quarantine_orphan`] but for a row whose `state_kind`
    /// could not be parsed into [`AgentStateKind`] (e.g. written by a newer
    /// attune). The raw kind string is preserved verbatim so the row remains
    /// recoverable. §2.3: still never deletes the original.
    fn quarantine_orphan_raw(&self, dek: &Key32, raw: &RawStateRow, reason: &str) -> Result<()> {
        self.quarantine_orphan_inner(
            dek,
            &raw.agent_id,
            &raw.plugin_id,
            &raw.state_kind,
            raw.schema_version,
            &raw.payload,
            reason,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn quarantine_orphan_inner(
        &self,
        dek: &Key32,
        agent_id: &str,
        plugin_id: &str,
        state_kind: &str,
        schema_version: i64,
        payload: &[u8],
        reason: &str,
    ) -> Result<()> {
        let enc = crypto::encrypt(dek, payload)?;
        self.conn.execute(
            "INSERT INTO agent_state_orphans \
                 (agent_id, plugin_id, state_kind, schema_version, payload, reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(agent_id, plugin_id, state_kind, schema_version) DO UPDATE SET \
                 payload = excluded.payload, reason = excluded.reason, \
                 detected_at = datetime('now')",
            params![agent_id, plugin_id, state_kind, schema_version, enc, reason],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dek() -> Key32 {
        Key32::generate()
    }

    // A v1->v2 migrator that re-keys agent_id "old_name" -> "new_name" and
    // uppercases the payload (a stand-in for a real format change).
    fn rename_migrator(row: &AgentStateRow) -> Option<MigratedRow> {
        if row.agent_id == "old_name" {
            Some(MigratedRow {
                agent_id: "new_name".to_string(),
                plugin_id: row.plugin_id.clone(),
                state_kind: row.state_kind,
                payload: row.payload.to_ascii_uppercase(),
            })
        } else {
            None
        }
    }

    fn step_v1_to_v2() -> MigrationStep {
        MigrationStep {
            from_version: 1,
            to_version: 2,
            migrate_row: rename_migrator,
        }
    }

    #[test]
    fn migration_v1_to_v2_rekeys_and_transforms() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "old_name", "law-pro", AgentStateKind::Preference, 1, b"hello")
            .unwrap();

        let report = store
            .migrate_agent_state(&dek, 2, &[step_v1_to_v2()], None)
            .unwrap();
        assert_eq!(report.migrated, 1);
        assert_eq!(report.orphaned, 0);

        // Old key is gone, new key holds transformed payload at v2.
        assert!(store
            .get_agent_state(&dek, "old_name", "law-pro", AgentStateKind::Preference)
            .unwrap()
            .is_none());
        let new = store
            .get_agent_state(&dek, "new_name", "law-pro", AgentStateKind::Preference)
            .unwrap()
            .unwrap();
        assert_eq!(new.payload, b"HELLO");
        assert_eq!(new.schema_version, 2);
    }

    #[test]
    fn no_migrator_quarantines_orphan_never_deletes() {
        // §2.3 RED LINE: a row no migrator can advance must be detectable and
        // recoverable — NOT silently dropped.
        let store = Store::open_memory().unwrap();
        let dek = dek();
        // This row's agent_id is NOT "old_name", so rename_migrator declines it.
        store
            .upsert_agent_state(&dek, "unclaimed", "law-pro", AgentStateKind::SkillExpansion, 1, b"precious")
            .unwrap();

        let report = store
            .migrate_agent_state(&dek, 2, &[step_v1_to_v2()], None)
            .unwrap();
        assert_eq!(report.migrated, 0);
        assert_eq!(report.orphaned, 1, "un-advanceable row must be quarantined");

        // The original row is STILL in agent_state (not deleted) — nothing lost.
        let still = store
            .get_agent_state(&dek, "unclaimed", "law-pro", AgentStateKind::SkillExpansion)
            .unwrap();
        assert!(still.is_some(), "§2.3: original learned-state row must NOT be deleted");

        // And it is recoverable from the orphan quarantine, payload intact.
        assert_eq!(store.count_agent_state_orphans().unwrap(), 1);
        let orphans = store.list_agent_state_orphans(&dek).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].payload, b"precious");
        assert_eq!(orphans[0].agent_id, "unclaimed");
        assert!(orphans[0].reason.contains("no migrator"));
    }

    #[test]
    fn no_migrators_at_all_orphans_everything_but_loses_nothing() {
        // Empty step list (the v1-baseline production caller). Every stale row
        // is quarantined, the originals preserved.
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "a", "law-pro", AgentStateKind::Preference, 1, b"x")
            .unwrap();
        store
            .upsert_agent_state(&dek, "b", "tech-pro", AgentStateKind::Preference, 1, b"y")
            .unwrap();

        let report = store.migrate_agent_state(&dek, 2, &[], None).unwrap();
        assert_eq!(report.orphaned, 2);
        assert_eq!(report.migrated, 0);
        // Originals untouched.
        assert_eq!(store.count_agent_state().unwrap(), 2);
        assert_eq!(store.count_agent_state_orphans().unwrap(), 2);
    }

    #[test]
    fn backup_is_really_generated_before_migration() {
        // Migration must be preceded by a real backup file on disk.
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "a", "law-pro", AgentStateKind::Preference, 1, b"x")
            .unwrap();
        let tmp = tempfile::tempdir().unwrap();

        let report = store
            .migrate_agent_state(&dek, 2, &[step_v1_to_v2()], Some(tmp.path()))
            .unwrap();
        let backup = report.backup_path.expect("backup_path must be set");
        assert!(backup.exists(), "backup file must really exist on disk");
        let meta = std::fs::metadata(&backup).unwrap();
        assert!(meta.len() > 0, "backup must be non-empty");

        // And the backup must be a valid, openable SQLite DB carrying the data.
        let bconn = rusqlite::Connection::open(&backup).unwrap();
        let n: i64 = bconn
            .query_row("SELECT COUNT(*) FROM agent_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "backup must contain the pre-migration row");
    }

    #[test]
    fn rows_at_or_above_target_are_untouched() {
        let store = Store::open_memory().unwrap();
        let dek = dek();
        // Already at v2 — below-target query (< 2) must not even see it.
        store
            .upsert_agent_state(&dek, "future", "law-pro", AgentStateKind::Preference, 2, b"keep")
            .unwrap();
        let report = store.migrate_agent_state(&dek, 2, &[], None).unwrap();
        assert_eq!(report.migrated, 0);
        assert_eq!(report.orphaned, 0);
        let row = store
            .get_agent_state(&dek, "future", "law-pro", AgentStateKind::Preference)
            .unwrap()
            .unwrap();
        assert_eq!(row.payload, b"keep");
        assert_eq!(row.schema_version, 2);
    }

    #[test]
    fn chained_migration_v1_to_v3() {
        // Two registered steps chain a v1 row up to v3.
        fn v1_v2(row: &AgentStateRow) -> Option<MigratedRow> {
            Some(MigratedRow {
                agent_id: row.agent_id.clone(),
                plugin_id: row.plugin_id.clone(),
                state_kind: row.state_kind,
                payload: [row.payload.as_slice(), b"-v2"].concat(),
            })
        }
        fn v2_v3(row: &AgentStateRow) -> Option<MigratedRow> {
            Some(MigratedRow {
                agent_id: row.agent_id.clone(),
                plugin_id: row.plugin_id.clone(),
                state_kind: row.state_kind,
                payload: [row.payload.as_slice(), b"-v3"].concat(),
            })
        }
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "a", "law-pro", AgentStateKind::Preference, 1, b"base")
            .unwrap();
        let steps = [
            MigrationStep { from_version: 1, to_version: 2, migrate_row: v1_v2 },
            MigrationStep { from_version: 2, to_version: 3, migrate_row: v2_v3 },
        ];
        let report = store.migrate_agent_state(&dek, 3, &steps, None).unwrap();
        assert_eq!(report.migrated, 1);
        let row = store
            .get_agent_state(&dek, "a", "law-pro", AgentStateKind::Preference)
            .unwrap()
            .unwrap();
        assert_eq!(row.payload, b"base-v2-v3");
        assert_eq!(row.schema_version, 3);
    }

    #[test]
    fn unknown_state_kind_is_quarantined_not_dropped() {
        // §2.3: a row with a state_kind this build cannot parse (e.g. written by
        // a newer attune) must be quarantined, never silently skipped/dropped.
        let store = Store::open_memory().unwrap();
        let dek = dek();
        // Insert directly with a bogus kind + encrypted payload.
        let enc = crypto::encrypt(&dek, b"future-state").unwrap();
        store
            .raw_connection_for_test()
            .execute(
                "INSERT INTO agent_state (agent_id, plugin_id, state_kind, schema_version, payload) \
                 VALUES ('a', 'newplugin', 'future_kind_v9', 1, ?1)",
                rusqlite::params![enc],
            )
            .unwrap();

        let report = store.migrate_agent_state(&dek, 2, &[], None).unwrap();
        assert_eq!(report.orphaned, 1, "unknown-kind row must be quarantined");
        assert_eq!(report.migrated, 0);
        // Original still present (not dropped).
        assert_eq!(store.count_agent_state().unwrap(), 1);
        // Recoverable from quarantine with the raw kind string + payload intact.
        let orphans = store.list_agent_state_orphans(&dek).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].state_kind, "future_kind_v9");
        assert_eq!(orphans[0].payload, b"future-state");
        assert!(orphans[0].reason.contains("unknown state_kind"));
    }

    #[test]
    fn broken_chain_orphans_not_loses() {
        // Only a v2->v3 step is registered; a v1 row cannot start the chain, so
        // it must be quarantined, not lost.
        fn v2_v3(row: &AgentStateRow) -> Option<MigratedRow> {
            Some(MigratedRow {
                agent_id: row.agent_id.clone(),
                plugin_id: row.plugin_id.clone(),
                state_kind: row.state_kind,
                payload: row.payload.clone(),
            })
        }
        let store = Store::open_memory().unwrap();
        let dek = dek();
        store
            .upsert_agent_state(&dek, "a", "law-pro", AgentStateKind::Preference, 1, b"x")
            .unwrap();
        let steps = [MigrationStep { from_version: 2, to_version: 3, migrate_row: v2_v3 }];
        let report = store.migrate_agent_state(&dek, 3, &steps, None).unwrap();
        assert_eq!(report.orphaned, 1);
        assert_eq!(report.migrated, 0);
        assert!(store
            .get_agent_state(&dek, "a", "law-pro", AgentStateKind::Preference)
            .unwrap()
            .is_some());
    }
}
