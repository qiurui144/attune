//! Persistent cloud-session storage shared by the CLI, server, and verifier.
//!
//! The session cookie is still needed before the vault is unlocked, so it lives
//! in the platform config directory rather than vault metadata. Writes are
//! atomic and the resulting file is owner-only on Unix.
//!
//! Account switches use a two-phase local transaction. A sibling suppression
//! marker makes a newly written session invisible to readers until the caller
//! commits it. The same marker also makes rollback fail closed: even if the
//! cookie file cannot be deleted, lazy restore and restart will not load it.

use crate::error::{Result, VaultError};
use fs4::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedCloudSession {
    pub cloud_url: String,
    /// Full cookie pair (`session=...`) or the raw token accepted by the cloud.
    pub session: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCloudSession {
    cloud_url: String,
    session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transaction_id: Option<String>,
}

impl StoredCloudSession {
    fn public(&self) -> PersistedCloudSession {
        PersistedCloudSession {
            cloud_url: self.cloud_url.clone(),
            session: self.session.clone(),
        }
    }
}

/// A path-bound cloud-session store.
///
/// Capturing the path before dispatching filesystem work to another thread
/// keeps account transactions deterministic and makes tests independent of
/// process-global HOME/config environment changes.
#[derive(Debug, Clone)]
pub struct CloudSessionStore {
    path: PathBuf,
    pending_transaction: Arc<Mutex<Option<String>>>,
}

/// Cross-process account-transition fence for one cloud-session path.
///
/// The lock file remains exclusively locked for this value's entire lifetime.
/// Server membership transitions retain it from proof/stage through local
/// cleanup and runtime publication; convenience store operations acquire a
/// short-lived transaction automatically.
#[derive(Debug)]
pub struct CloudSessionTransaction {
    store: CloudSessionStore,
    _lock_file: File,
}

fn default_store_transactions() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<Option<String>>>>> {
    static TRANSACTIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Option<String>>>>>> =
        OnceLock::new();
    TRANSACTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

impl Default for CloudSessionStore {
    fn default() -> Self {
        let path = cloud_session_path();
        // The legacy free functions stage and commit in separate calls. Keep
        // their nonce state path-scoped within this process while explicit
        // `new(path)` stores retain independent transaction identities.
        let pending_transaction = default_store_transactions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(path.clone())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone();
        Self {
            path,
            pending_transaction,
        }
    }
}

impl CloudSessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            pending_transaction: Arc::new(Mutex::new(None)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<PersistedCloudSession>> {
        self.transaction()?.load()
    }

    pub fn persist(&self, cloud_url: &str, session: &str) -> Result<PathBuf> {
        let transaction = self.transaction()?;
        transaction.stage(cloud_url, session)?;
        if !transaction.commit()? {
            return Err(VaultError::InvalidInput(
                "cloud session transaction is no longer current".to_string(),
            ));
        }
        Ok(self.path.clone())
    }

    pub fn stage(&self, cloud_url: &str, session: &str) -> Result<PathBuf> {
        self.transaction()?.stage(cloud_url, session)
    }

    pub fn commit(&self) -> Result<bool> {
        self.transaction()?.commit()
    }

    pub fn suppress_restore(&self) -> Result<PathBuf> {
        self.transaction()?.suppress_restore()
    }

    pub fn remove(&self) -> Result<bool> {
        self.transaction()?.remove()
    }

    /// Stable, credential-free identity of the currently visible/suppressed
    /// session state. Servers retain this after publishing member runtime and
    /// compare it on later requests so a sequential CLI account switch cannot
    /// leave memory on account A while disk points at account B.
    pub fn epoch(&self) -> Result<String> {
        self.transaction()?.epoch()
    }

    pub fn transaction(&self) -> Result<CloudSessionTransaction> {
        let lock_path = transition_lock_path(&self.path)?;
        let parent = lock_path.parent().ok_or_else(|| {
            VaultError::InvalidInput("cloud session lock path has no parent".to_string())
        })?;
        std::fs::create_dir_all(parent)?;
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            lock_file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        lock_file.lock_exclusive()?;
        Ok(CloudSessionTransaction {
            store: self.clone(),
            _lock_file: lock_file,
        })
    }
}

impl CloudSessionTransaction {
    pub fn load(&self) -> Result<Option<PersistedCloudSession>> {
        load_cloud_session_at(&self.store.path)
    }

    pub fn stage(&self, cloud_url: &str, session: &str) -> Result<PathBuf> {
        let value = validated_session(cloud_url, session)?;
        let transaction_id = uuid::Uuid::new_v4().to_string();
        stage_cloud_session_at(&self.store.path, &value, &transaction_id)?;
        *self
            .store
            .pending_transaction
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(transaction_id);
        Ok(self.store.path.clone())
    }

    pub fn suppress_restore(&self) -> Result<PathBuf> {
        let transaction_id = uuid::Uuid::new_v4().to_string();
        let existing = load_stored_cloud_session_unchecked_at(&self.store.path)?;
        suppress_cloud_session_restore_at(&self.store.path, &transaction_id)?;
        if let Some(existing) = existing {
            write_stored_cloud_session_at(&self.store.path, &existing.public(), &transaction_id)?;
        }
        *self
            .store
            .pending_transaction
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(transaction_id);
        restore_suppression_path(&self.store.path)
    }

    pub fn commit(&self) -> Result<bool> {
        let expected = self
            .store
            .pending_transaction
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        commit_staged_cloud_session_at(&self.store.path, expected.as_deref())
    }

    pub fn remove(&self) -> Result<bool> {
        *self
            .store
            .pending_transaction
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        remove_cloud_session_at(&self.store.path)
    }

    pub fn epoch(&self) -> Result<String> {
        cloud_session_epoch_at(&self.store.path)
    }
}

pub fn cloud_session_path() -> PathBuf {
    crate::platform::config_dir().join("cloud-session.json")
}

pub fn load_cloud_session() -> Result<Option<PersistedCloudSession>> {
    CloudSessionStore::default().load()
}

pub fn persist_cloud_session(cloud_url: &str, session: &str) -> Result<PathBuf> {
    CloudSessionStore::default().persist(cloud_url, session)
}

/// Atomically write a session while keeping it invisible to all readers.
///
/// Call [`commit_staged_cloud_session`] only after the account switch's other
/// durable cleanup has succeeded. On every error the suppression marker is
/// deliberately retained.
pub fn stage_cloud_session(cloud_url: &str, session: &str) -> Result<PathBuf> {
    CloudSessionStore::default().stage(cloud_url, session)
}

/// Publish the session written by [`stage_cloud_session`].
///
/// The cookie is parsed and validated before the suppression marker is removed,
/// so a corrupt/incomplete staged file can never be made active.
pub fn commit_staged_cloud_session() -> Result<bool> {
    CloudSessionStore::default().commit()
}

/// Hide the current session from readers without deleting it.
///
/// This is useful when an already-authoritative session must not race a local
/// account cleanup. Call [`commit_staged_cloud_session`] after cleanup succeeds.
pub fn suppress_cloud_session_restore() -> Result<PathBuf> {
    CloudSessionStore::default().suppress_restore()
}

pub fn remove_cloud_session() -> Result<bool> {
    CloudSessionStore::default().remove()
}

fn load_cloud_session_at(path: &Path) -> Result<Option<PersistedCloudSession>> {
    if restore_is_suppressed_at(path)? {
        return Ok(None);
    }
    load_cloud_session_unchecked_at(path)
}

fn load_cloud_session_unchecked_at(path: &Path) -> Result<Option<PersistedCloudSession>> {
    Ok(load_stored_cloud_session_unchecked_at(path)?.map(|stored| stored.public()))
}

fn load_stored_cloud_session_unchecked_at(path: &Path) -> Result<Option<StoredCloudSession>> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let value: StoredCloudSession = serde_json::from_slice(&data)?;
    if value.cloud_url.trim().is_empty() || value.session.trim().is_empty() {
        return Err(VaultError::InvalidInput(
            "persisted cloud session is incomplete".to_string(),
        ));
    }
    Ok(Some(value))
}

fn validated_session(cloud_url: &str, session: &str) -> Result<PersistedCloudSession> {
    let value = PersistedCloudSession {
        cloud_url: cloud_url.trim_end_matches('/').to_string(),
        session: session.trim().to_string(),
    };
    if value.cloud_url.is_empty() || value.session.is_empty() {
        return Err(VaultError::InvalidInput(
            "cloud session URL and token must be non-empty".to_string(),
        ));
    }
    Ok(value)
}

fn stage_cloud_session_at(
    path: &Path,
    value: &PersistedCloudSession,
    transaction_id: &str,
) -> Result<()> {
    suppress_cloud_session_restore_at(path, transaction_id)?;
    write_stored_cloud_session_at(path, value, transaction_id)
}

fn write_stored_cloud_session_at(
    path: &Path,
    value: &PersistedCloudSession,
    transaction_id: &str,
) -> Result<()> {
    let stored = StoredCloudSession {
        cloud_url: value.cloud_url.clone(),
        session: value.session.clone(),
        transaction_id: Some(transaction_id.to_string()),
    };
    let mut data = serde_json::to_vec_pretty(&stored)?;
    data.push(b'\n');
    atomic_write_owner_only(path, ".cloud-session-", &data)
}

fn atomic_write_owner_only(path: &Path, prefix: &str, data: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        VaultError::InvalidInput("cloud session path has no parent directory".to_string())
    })?;
    std::fs::create_dir_all(parent)?;

    let mut temp = tempfile::Builder::new()
        .prefix(prefix)
        .tempfile_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temp.as_file_mut().write_all(data)?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path).map_err(|e| VaultError::Io(e.error))?;
    sync_parent_dir(path)?;
    Ok(())
}

fn remove_cloud_session_at(path: &Path) -> Result<bool> {
    // The marker is the authoritative rollback. Create it before attempting
    // deletion so a sharing violation/permissions error cannot expose the file
    // to restart or lazy member-state restore.
    let transaction_id = uuid::Uuid::new_v4().to_string();
    suppress_cloud_session_restore_at(path, &transaction_id)?;
    match std::fs::remove_file(path) {
        Ok(()) => {
            sync_parent_dir(path)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn restore_suppression_path(path: &Path) -> Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        VaultError::InvalidInput("cloud session path has no file name".to_string())
    })?;
    let mut marker_name = file_name.to_os_string();
    marker_name.push(".restore-suppressed");
    Ok(path.with_file_name(marker_name))
}

fn transition_lock_path(path: &Path) -> Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        VaultError::InvalidInput("cloud session path has no file name".to_string())
    })?;
    let mut lock_name = file_name.to_os_string();
    lock_name.push(".transition.lock");
    Ok(path.with_file_name(lock_name))
}

fn restore_is_suppressed_at(path: &Path) -> Result<bool> {
    let marker = restore_suppression_path(path)?;
    match std::fs::metadata(marker) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn cloud_session_epoch_at(path: &Path) -> Result<String> {
    if let Some(transaction_id) = read_suppression_transaction_at(path)? {
        return Ok(format!("suppressed:{transaction_id}"));
    }
    let Some(stored) = load_stored_cloud_session_unchecked_at(path)? else {
        return Ok("absent".to_string());
    };
    if let Some(transaction_id) = stored.transaction_id {
        return Ok(format!("active:{transaction_id}"));
    }
    // Legacy files predate transaction ids. Hash both fields so the epoch is
    // stable without ever exposing the bearer credential in logs or state.
    let mut digest = Sha256::new();
    digest.update(stored.cloud_url.as_bytes());
    digest.update([0]);
    digest.update(stored.session.as_bytes());
    Ok(format!("active-legacy:{:x}", digest.finalize()))
}

fn suppress_cloud_session_restore_at(path: &Path, transaction_id: &str) -> Result<()> {
    let marker = restore_suppression_path(path)?;
    let mut marker_data = transaction_id.as_bytes().to_vec();
    marker_data.push(b'\n');
    atomic_write_owner_only(&marker, ".cloud-session-restore-suppressed-", &marker_data)
}

fn read_suppression_transaction_at(path: &Path) -> Result<Option<String>> {
    let marker = restore_suppression_path(path)?;
    let value = match std::fs::read_to_string(marker) {
        Ok(value) => value,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(VaultError::InvalidInput(
            "cloud session suppression marker is incomplete".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

fn commit_staged_cloud_session_at(path: &Path, expected: Option<&str>) -> Result<bool> {
    let Some(expected) = expected.filter(|value| !value.trim().is_empty()) else {
        return Ok(false);
    };
    if read_suppression_transaction_at(path)?.as_deref() != Some(expected) {
        return Ok(false);
    }
    let Some(stored) = load_stored_cloud_session_unchecked_at(path)? else {
        return Err(VaultError::InvalidInput(
            "cannot commit a missing cloud session".to_string(),
        ));
    };
    if stored.transaction_id.as_deref() != Some(expected) {
        return Ok(false);
    }
    let marker = restore_suppression_path(path)?;
    match std::fs::remove_file(marker) {
        Ok(()) => {
            sync_parent_dir(path)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Persist directory-entry changes across power loss on Unix. Opening a
/// directory as a regular file is not portable to Windows, where the atomic
/// rename/remove operations above remain the best available compatible path.
fn sync_parent_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path.parent().ok_or_else(|| {
            VaultError::InvalidInput("cloud session path has no parent directory".to_string())
        })?;
        std::fs::File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_is_atomic_and_removable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud-session.json");
        let store = CloudSessionStore::new(path.clone());
        let expected = PersistedCloudSession {
            cloud_url: "https://accounts.example.test".into(),
            session: "session=secret".into(),
        };
        store.stage(&expected.cloud_url, &expected.session).unwrap();
        assert_eq!(store.load().unwrap(), None);
        assert!(store.commit().unwrap());
        assert_eq!(store.load().unwrap(), Some(expected));
        assert!(store.remove().unwrap());
        assert!(!store.remove().unwrap());
        assert_eq!(store.load().unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn failed_staged_session_rollback_stays_invisible() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud-session.json");
        let staged = PersistedCloudSession {
            cloud_url: "https://new-account.example.test".into(),
            session: "session=new-account".into(),
        };
        let store = CloudSessionStore::new(path.clone());
        store.stage(&staged.cloud_url, &staged.session).unwrap();
        assert!(
            path.is_file(),
            "precondition: staged cookie remains on disk"
        );

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let rollback = store.remove();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            rollback.is_err(),
            "read-only parent must reject cookie deletion"
        );
        assert!(
            path.is_file(),
            "failed rollback deliberately leaves raw cookie"
        );
        assert_eq!(
            store.load().unwrap(),
            None,
            "the durable suppression marker must fail closed even when deletion fails"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persisted_session_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud-session.json");
        let store = CloudSessionStore::new(path.clone());
        store
            .stage("https://accounts.example.test", "session=secret")
            .unwrap();
        let marker = restore_suppression_path(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(marker).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(transition_lock_path(&path).unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn stale_store_cannot_commit_another_staged_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud-session.json");
        let first = CloudSessionStore::new(path.clone());
        let second = CloudSessionStore::new(path);

        first.stage("https://first.test", "session=first").unwrap();
        second
            .stage("https://second.test", "session=second")
            .unwrap();

        assert!(!first.commit().unwrap());
        assert!(second.commit().unwrap());
        assert_eq!(
            second.load().unwrap(),
            Some(PersistedCloudSession {
                cloud_url: "https://second.test".into(),
                session: "session=second".into(),
            })
        );
    }

    #[test]
    fn transaction_fences_other_store_until_publication() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud-session.json");
        let first = CloudSessionStore::new(path.clone());
        let second = CloudSessionStore::new(path);
        let transaction = first.transaction().unwrap();
        transaction
            .stage("https://first.test", "session=first")
            .unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = second.persist("https://second.test", "session=second");
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());

        assert!(transaction.commit().unwrap());
        drop(transaction);
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(
            first.load().unwrap(),
            Some(PersistedCloudSession {
                cloud_url: "https://second.test".into(),
                session: "session=second".into(),
            })
        );
    }
}
