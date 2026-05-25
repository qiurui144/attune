//! v1.0.1 C4 — Pre-upgrade backup + rollback support.
//!
//! Backups live in `<data_dir>/backups/` and follow the naming convention
//! `vault.db.bak.YYYYMMDD-HHMM` (sortable lex == sortable by time).
//!
//! Retention policy: keep newest 5 backups; older ones are pruned when a new
//! one is created. The rollback path is a plain `std::fs::copy` — vault.db is
//! a single encrypted SQLite file, so byte-for-byte copy is correct.
//!
//! **Safety rules**:
//! - Refuses to operate while vault.db is in use (best-effort: caller should
//!   ensure attune-server is stopped). We do not hold a file lock; SQLite WAL
//!   files are not copied (they are reconstructible on next open).
//! - Before restore, the current vault.db is renamed to
//!   `vault.db.before-rollback.<unix-ts>` so a double-failure can still recover
//!   the pre-restore state.
//! - SHA256 of every backup is recorded next to the file as
//!   `vault.db.bak.<stamp>.sha256` for tamper detection.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Result, VaultError};
use crate::platform;

/// How many backups to keep after pruning.
pub const RETENTION_COUNT: usize = 5;

/// Backup naming prefix.
const BACKUP_PREFIX: &str = "vault.db.bak.";

/// One discovered backup entry.
#[derive(Debug, Clone)]
pub struct BackupEntry {
    /// Full path on disk.
    pub path: PathBuf,
    /// Filename only, e.g. `vault.db.bak.20260525-1430`.
    pub filename: String,
    /// Timestamp portion `YYYYMMDD-HHMM` (used for sort).
    pub stamp: String,
    /// Size in bytes.
    pub size: u64,
}

/// Return the backup directory (creates it if missing).
pub fn backup_dir() -> Result<PathBuf> {
    let dir = platform::data_dir().join("backups");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(VaultError::Io)?;
    }
    Ok(dir)
}

/// List backups, newest first.
pub fn list_backups() -> Result<Vec<BackupEntry>> {
    let dir = backup_dir()?;
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(VaultError::Io(e)),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let filename = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        // Skip companion files (.sha256), only collect the .bak.<stamp> primary file.
        if !filename.starts_with(BACKUP_PREFIX) || filename.ends_with(".sha256") {
            continue;
        }
        let stamp = filename[BACKUP_PREFIX.len()..].to_string();
        // stamp must match YYYYMMDD-HHMM (13 chars, digits + 1 dash)
        if !is_valid_stamp(&stamp) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(BackupEntry {
            path: path.clone(),
            filename,
            stamp,
            size,
        });
    }
    // Newest first — lex sort on stamp gives time order, reversed.
    out.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    Ok(out)
}

fn is_valid_stamp(s: &str) -> bool {
    // YYYYMMDD-HHMM = 8 digits + '-' + 4 digits = 13 chars
    if s.len() != 13 {
        return false;
    }
    let bytes = s.as_bytes();
    bytes[..8].iter().all(|b| b.is_ascii_digit())
        && bytes[8] == b'-'
        && bytes[9..].iter().all(|b| b.is_ascii_digit())
}

/// Compose `YYYYMMDD-HHMM` from current UTC time, fallback to a deterministic
/// epoch-derived string if system clock is broken (tests / sandboxes).
fn current_stamp() -> String {
    use chrono::{TimeZone, Utc};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Utc.timestamp_opt(secs, 0)
        .single()
        .map(|dt| dt.format("%Y%m%d-%H%M").to_string())
        .unwrap_or_else(|| format!("epoch-{secs}"))
}

/// Compute SHA256 hex of a file (streaming).
fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(VaultError::Io)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(VaultError::Io)?;
    Ok(hex::encode(hasher.finalize()))
}

/// Create a backup of the current vault.db.
///
/// Returns the BackupEntry of the new file. If vault.db does not exist, returns
/// `VaultError::NotFound` (caller should `attune setup` first).
///
/// Side effects:
/// - Writes `<backup_dir>/vault.db.bak.<stamp>`
/// - Writes `<backup_dir>/vault.db.bak.<stamp>.sha256`
/// - Prunes oldest backups to keep at most `RETENTION_COUNT` (default 5).
pub fn create_pre_upgrade_backup() -> Result<BackupEntry> {
    let vault_db = platform::db_path();
    if !vault_db.exists() {
        return Err(VaultError::NotFound(format!(
            "vault.db not found at {} — run `attune setup` first",
            vault_db.display()
        )));
    }
    let dir = backup_dir()?;
    let stamp = current_stamp();
    let filename = format!("{BACKUP_PREFIX}{stamp}");
    let dest = dir.join(&filename);
    if dest.exists() {
        // Same-minute double-tap — disambiguate by appending a unix-ts suffix.
        let alt_stamp = format!(
            "{stamp}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() % 60)
                .unwrap_or(0)
        );
        let alt_filename = format!("{BACKUP_PREFIX}{alt_stamp}");
        return create_backup_at(&vault_db, &dir, &alt_stamp, &alt_filename);
    }
    create_backup_at(&vault_db, &dir, &stamp, &filename)
}

fn create_backup_at(
    src: &Path,
    dir: &Path,
    stamp: &str,
    filename: &str,
) -> Result<BackupEntry> {
    let dest = dir.join(filename);
    std::fs::copy(src, &dest).map_err(VaultError::Io)?;
    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    // Companion .sha256 for tamper detection (text format: "<hex>  <filename>\n")
    let sha = sha256_file(&dest)?;
    let sha_path = dir.join(format!("{filename}.sha256"));
    std::fs::write(&sha_path, format!("{sha}  {filename}\n")).map_err(VaultError::Io)?;
    // Retention prune (skip if the just-created file ends up being pruned — shouldn't happen
    // since list_backups() sorts newest-first and we keep top-5).
    prune_old_backups(dir, RETENTION_COUNT)?;
    Ok(BackupEntry {
        path: dest,
        filename: filename.to_string(),
        stamp: stamp.to_string(),
        size,
    })
}

/// Keep the newest `keep` backups, remove the rest (plus their .sha256 companions).
pub fn prune_old_backups(dir: &Path, keep: usize) -> Result<usize> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(VaultError::Io)?
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?.to_string();
            if name.starts_with(BACKUP_PREFIX) && !name.ends_with(".sha256") {
                let stamp = name[BACKUP_PREFIX.len()..].to_string();
                if is_valid_stamp(&stamp) || stamp.len() > 13 {
                    return Some((stamp, path));
                }
            }
            None
        })
        .collect();
    // Newest first (lex desc on stamp).
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    let mut removed = 0;
    for (_, path) in entries.into_iter().skip(keep) {
        let sha_path = path.with_extension(
            format!("{}.sha256", path.extension().and_then(|s| s.to_str()).unwrap_or("")),
        );
        // The companion file naming is `vault.db.bak.<stamp>.sha256` — derive it more robustly:
        let mut companion = path.clone();
        let fname = companion
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        companion.set_file_name(format!("{fname}.sha256"));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&companion);
        let _ = std::fs::remove_file(&sha_path); // safety fallback
        removed += 1;
    }
    Ok(removed)
}

/// Verify a backup's SHA256 against its companion file (if present).
/// Returns Ok(true) when matched, Ok(false) when companion is missing,
/// Err on hash mismatch.
pub fn verify_backup(entry: &BackupEntry) -> Result<bool> {
    let companion = entry.path.with_file_name(format!("{}.sha256", entry.filename));
    if !companion.exists() {
        return Ok(false);
    }
    let expected = std::fs::read_to_string(&companion).map_err(VaultError::Io)?;
    let expected_hex = expected.split_whitespace().next().unwrap_or("");
    let actual = sha256_file(&entry.path)?;
    if expected_hex == actual {
        Ok(true)
    } else {
        Err(VaultError::InvalidInput(format!(
            "SHA256 mismatch for {}: expected {expected_hex}, got {actual}",
            entry.filename
        )))
    }
}

/// Restore from a specific backup by 1-based index (1 = newest).
///
/// Steps:
/// 1. List backups; bail if `index` out of range.
/// 2. Verify SHA256 (best-effort — missing companion is non-fatal, mismatch is fatal).
/// 3. Move current vault.db → `vault.db.before-rollback.<unix-ts>` (safety net).
/// 4. Copy backup → vault.db.
/// 5. Best-effort delete vault.db-wal / vault.db-shm (SQLite will rebuild on open).
///
/// Returns the BackupEntry restored.
pub fn restore_from_index(index: usize) -> Result<BackupEntry> {
    if index == 0 {
        return Err(VaultError::InvalidInput(
            "rollback index is 1-based; pass --index 1 for newest".into(),
        ));
    }
    let backups = list_backups()?;
    if backups.is_empty() {
        return Err(VaultError::NotFound(format!(
            "no backups in {}",
            backup_dir()?.display()
        )));
    }
    let entry = backups.get(index - 1).ok_or_else(|| {
        VaultError::InvalidInput(format!(
            "--index {index} out of range (only {} backup(s) available)",
            backups.len()
        ))
    })?;
    // Verify SHA256 (warns if companion missing, errors on mismatch).
    let _ = verify_backup(entry)?;

    let vault_db = platform::db_path();
    if vault_db.exists() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let safety = vault_db.with_file_name(format!("vault.db.before-rollback.{ts}"));
        std::fs::rename(&vault_db, &safety).map_err(VaultError::Io)?;
    }
    std::fs::copy(&entry.path, &vault_db).map_err(VaultError::Io)?;
    // Best-effort: remove WAL/SHM so SQLite reconstructs cleanly on next open.
    let data = platform::data_dir();
    let _ = std::fs::remove_file(data.join("vault.db-wal"));
    let _ = std::fs::remove_file(data.join("vault.db-shm"));
    Ok(entry.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Test isolation: backup_dir() reads platform::data_dir() which is process-global via XDG.
    // We serialize tests that touch the real backup_dir and use HOME override to isolate.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home<F: FnOnce(&std::path::Path)>(f: F) {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let td = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("HOME");
        let prev_xdg = std::env::var_os("XDG_DATA_HOME");
        let prev_xdg_cfg = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: tests are serialized via TEST_LOCK; mutating env is safe within the lock.
        unsafe {
            std::env::set_var("HOME", td.path());
            std::env::set_var("XDG_DATA_HOME", td.path().join(".local/share"));
            std::env::set_var("XDG_CONFIG_HOME", td.path().join(".config"));
        }
        // Ensure data_dir exists so vault.db writes succeed.
        let data = platform::data_dir();
        std::fs::create_dir_all(&data).expect("mkdir data");
        f(td.path());
        // Restore env to avoid cross-test bleed even though we hold the lock.
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
            match prev_xdg_cfg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[test]
    fn stamp_validation() {
        assert!(is_valid_stamp("20260525-1430"));
        assert!(!is_valid_stamp("2026-05-25"));
        assert!(!is_valid_stamp("20260525_1430"));
        assert!(!is_valid_stamp(""));
    }

    #[test]
    fn list_backups_empty_when_no_dir() {
        with_temp_home(|_| {
            let entries = list_backups().expect("list ok");
            assert!(entries.is_empty(), "fresh home has no backups");
        });
    }

    #[test]
    fn create_backup_requires_vault_db() {
        with_temp_home(|_| {
            let err = create_pre_upgrade_backup().unwrap_err();
            assert!(
                matches!(err, VaultError::NotFound(_)),
                "expected NotFound, got {err:?}"
            );
        });
    }

    #[test]
    fn create_and_list_backup_roundtrip() {
        with_temp_home(|_| {
            // Seed a fake vault.db
            let vault = platform::db_path();
            std::fs::write(&vault, b"fake-encrypted-sqlite-bytes").unwrap();
            let entry = create_pre_upgrade_backup().expect("backup ok");
            assert!(entry.path.exists(), "backup file written");
            assert!(entry.size > 0);
            let list = list_backups().expect("list ok");
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].filename, entry.filename);
            // Companion .sha256 created
            let companion = entry
                .path
                .with_file_name(format!("{}.sha256", entry.filename));
            assert!(companion.exists(), "companion sha256 written");
        });
    }

    #[test]
    fn retention_keeps_5_newest() {
        with_temp_home(|_| {
            let vault = platform::db_path();
            std::fs::write(&vault, b"v0").unwrap();
            let dir = backup_dir().unwrap();
            // Manually seed 7 backups with monotonic stamps so retention triggers.
            for i in 0..7 {
                let stamp = format!("2026052{}-1000", i % 10);
                let f = dir.join(format!("{BACKUP_PREFIX}{stamp}"));
                std::fs::write(&f, format!("backup-{i}")).unwrap();
            }
            // Trigger prune via a real create call (writes its own + prunes).
            std::fs::write(&vault, b"v1").unwrap();
            let _ = create_pre_upgrade_backup().expect("create ok");
            let list = list_backups().expect("list");
            assert!(
                list.len() <= RETENTION_COUNT,
                "kept {} > {}",
                list.len(),
                RETENTION_COUNT
            );
        });
    }

    #[test]
    fn restore_round_trip() {
        with_temp_home(|_| {
            let vault = platform::db_path();
            std::fs::write(&vault, b"original-v1.0.0").unwrap();
            let entry = create_pre_upgrade_backup().expect("backup");
            // Modify vault.db (simulate upgrade migration that we want to undo).
            std::fs::write(&vault, b"corrupted-v1.0.1").unwrap();
            let restored = restore_from_index(1).expect("restore");
            assert_eq!(restored.filename, entry.filename);
            let after = std::fs::read(&vault).unwrap();
            assert_eq!(after, b"original-v1.0.0", "vault.db restored to backup contents");
            // Safety: pre-restore vault.db moved aside
            let safety_files: Vec<_> = std::fs::read_dir(platform::data_dir())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("vault.db.before-rollback."))
                .collect();
            assert!(!safety_files.is_empty(), "before-rollback safety file present");
        });
    }

    #[test]
    fn restore_out_of_range_errors() {
        with_temp_home(|_| {
            let vault = platform::db_path();
            std::fs::write(&vault, b"v1").unwrap();
            create_pre_upgrade_backup().expect("backup");
            let err = restore_from_index(99).unwrap_err();
            assert!(
                matches!(err, VaultError::InvalidInput(_)),
                "expected InvalidInput, got {err:?}"
            );
        });
    }

    #[test]
    fn restore_zero_index_errors() {
        with_temp_home(|_| {
            let err = restore_from_index(0).unwrap_err();
            assert!(matches!(err, VaultError::InvalidInput(_)));
        });
    }

    #[test]
    fn restore_no_backups_errors() {
        with_temp_home(|_| {
            let err = restore_from_index(1).unwrap_err();
            assert!(matches!(err, VaultError::NotFound(_)));
        });
    }

    #[test]
    fn verify_backup_detects_tamper() {
        with_temp_home(|_| {
            let vault = platform::db_path();
            std::fs::write(&vault, b"original").unwrap();
            let entry = create_pre_upgrade_backup().expect("backup");
            // Tamper with the backup file
            std::fs::write(&entry.path, b"tampered").unwrap();
            let res = verify_backup(&entry);
            assert!(res.is_err(), "expected SHA256 mismatch error");
        });
    }
}
