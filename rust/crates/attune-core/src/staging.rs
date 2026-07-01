//! Locked-mode ingest staging (K3 G3①).
//!
//! When the vault is LOCKED there is no vault DEK in memory, so inbound documents
//! cannot be parsed/encrypted into the vault. Instead of dropping them (the old
//! behavior — a 403 that loses the file), they are written here: the raw bytes are
//! AES-256-GCM encrypted at rest with a *staging key derived from the device secret*
//! (which lives on disk independent of vault lock state), and a small plaintext
//! sidecar holds non-sensitive routing metadata.
//!
//! On unlock a drain worker decrypts each staged item and runs it through the normal
//! `ingest_document` pipeline, then deletes the staging files. The drain is idempotent:
//! a file present == pending; deleting it == done. A mid-drain crash leaves the
//! remaining files for the next drain, and the pipeline's `content_hash` short-circuit
//! prevents duplicate inserts for anything that was ingested-but-not-yet-deleted.
//!
//! Security invariant: the on-disk `.stg` blob is ciphertext only — it never contains
//! the document plaintext. The staging key shares the device-secret trust boundary with
//! the vault DEK wrapping (a physical-access attacker who can read `device.key` is the
//! same attacker who could unwrap the vault); staging is a short-lived transit area
//! (cleared on unlock), so its exposure window is far smaller than the long-lived vault.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};

/// Domain-separation context for deriving the staging key from the device secret.
/// Stable forever — changing it would orphan existing staged blobs.
const STAGING_KEY_CONTEXT: &[u8] = b"attune-staging-key-v1";

/// Hard cap on the number of pending staged items. Beyond this, `stage` rejects with
/// `StagingFull` so an unattended LOCKED device cannot have its disk filled by inbound
/// pushes. Drain on unlock clears the backlog.
pub const STAGING_MAX_PENDING: usize = 5_000;

/// Non-sensitive routing metadata for a staged inbound document.
///
/// This is written as a plaintext JSON sidecar. It deliberately holds NO document
/// content — only enough to route the (separately encrypted) bytes through the ingest
/// pipeline on unlock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StagedMeta {
    /// Resource URI (e.g. `upload://report.pdf`).
    pub uri: String,
    /// Source-provided title (may be empty; parser fills in on ingest).
    pub title: String,
    /// MIME hint, if known.
    pub mime_hint: Option<String>,
    /// Source kind, serialized as a stable lowercase tag.
    pub source_kind: String,
    /// Origin domain (Chrome-extension ingest), if any.
    pub domain: Option<String>,
    /// User tags, if any.
    pub tags: Option<Vec<String>>,
    /// Corpus domain (legal/tech/...), if any.
    pub corpus_domain: Option<String>,
    /// Unix epoch seconds when staged (for stable ordering + observability).
    pub created_at: i64,
}

/// A pending staged item: its id plus decrypted content + routing metadata.
pub struct StagedItem {
    pub id: String,
    pub meta: StagedMeta,
    pub content: Vec<u8>,
}

/// File-system staging area rooted at `<data_dir>/staging/`.
pub struct IngestStaging {
    dir: PathBuf,
    device_secret_path: PathBuf,
}

impl IngestStaging {
    /// Construct a staging area. `data_dir` is the app data dir; `config_dir` is where
    /// `device.key` lives. Neither requires the vault to be unlocked.
    pub fn new(data_dir: &Path, config_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("staging"),
            device_secret_path: config_dir.join("device.key"),
        }
    }

    /// Production constructor using the platform default paths.
    pub fn open_default() -> Self {
        Self::new(&crate::platform::data_dir(), &crate::platform::config_dir())
    }

    /// The staging directory path (for tests / observability).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Derive the staging key from the device secret. Fast (single HMAC), unlike the
    /// argon2 master-key derivation — staging key is not password-gated, it is bound to
    /// the device secret only. Fails if `device.key` is missing (SEALED brand-new vault).
    fn staging_key(&self) -> Result<Key32> {
        let bytes = std::fs::read(&self.device_secret_path).map_err(|_| {
            VaultError::DeviceSecretMissing(self.device_secret_path.display().to_string())
        })?;
        if bytes.len() != 32 {
            return Err(VaultError::DeviceSecretMismatch);
        }
        let ds = Key32::from_bytes({
            let mut a = [0u8; 32];
            a.copy_from_slice(&bytes);
            a
        });
        let mac = crypto::hmac_sign(&ds, STAGING_KEY_CONTEXT); // 32-byte HMAC-SHA256
        let mut k = [0u8; 32];
        k.copy_from_slice(&mac[..32]);
        Ok(Key32::from_bytes(k))
    }

    /// Count pending staged items (`.stg` files). Does NOT require vault unlock.
    pub fn count(&self) -> usize {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        rd.filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "stg").unwrap_or(false))
            .count()
    }

    /// Stage an inbound document. Encrypts `content` at rest with the staging key and
    /// writes a plaintext routing sidecar. Returns the staging id (a uuid).
    ///
    /// Rejects with `StagingFull` when the pending count is at the hard cap, and with
    /// `DeviceSecretMissing` when there is no device key to derive the staging key from.
    pub fn stage(&self, meta: &StagedMeta, content: &[u8]) -> Result<String> {
        if self.count() >= STAGING_MAX_PENDING {
            return Err(VaultError::StagingFull);
        }
        let key = self.staging_key()?;
        std::fs::create_dir_all(&self.dir)?;

        let id = uuid::Uuid::new_v4().simple().to_string();
        let ciphertext = crypto::encrypt(&key, content)?;
        let meta_json =
            serde_json::to_vec(meta).map_err(|e| VaultError::Crypto(format!("meta json: {e}")))?;

        // Write meta sidecar first, then ciphertext. The `.stg` file is the commit point:
        // drain keys off `.stg` presence, so a crash after the meta write but before the
        // `.stg` write just leaves an orphan meta (cleaned on next stage of same id =
        // impossible since id is a fresh uuid; harmless leftover, swept by remove on its
        // own id never happening). To keep it tidy we write `.stg` last and, on `.stg`
        // failure, remove the orphan meta.
        let meta_path = self.meta_path(&id);
        let stg_path = self.stg_path(&id);
        std::fs::write(&meta_path, &meta_json)?;
        if let Err(e) = std::fs::write(&stg_path, &ciphertext) {
            let _ = std::fs::remove_file(&meta_path);
            return Err(e.into());
        }
        Ok(id)
    }

    /// List pending staged item ids, sorted by `created_at` then id for stable,
    /// deterministic drain order. Skips (but retains) corrupt entries.
    pub fn list_pending(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut entries: Vec<(i64, String)> = Vec::new();
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().map(|x| x == "stg").unwrap_or(false) {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    let id = stem.to_string();
                    // created_at is best-effort for ordering; corrupt meta sorts first (0).
                    let ts = self.load_meta(&id).map(|m| m.created_at).unwrap_or(0);
                    entries.push((ts, id));
                }
            }
        }
        entries.sort();
        entries.into_iter().map(|(_, id)| id).collect()
    }

    /// Load + decrypt a single staged item. Returns `Err` (and leaves files in place) if
    /// the ciphertext or meta is corrupt, so the caller can skip-and-retain.
    pub fn load(&self, id: &str) -> Result<StagedItem> {
        let key = self.staging_key()?;
        let meta = self.load_meta(id)?;
        let ciphertext = std::fs::read(self.stg_path(id))?;
        let content = crypto::decrypt(&key, &ciphertext)?;
        Ok(StagedItem {
            id: id.to_string(),
            meta,
            content,
        })
    }

    /// Remove a staged item (the idempotent "done" point). Best-effort on both files.
    pub fn remove(&self, id: &str) -> Result<()> {
        let _ = std::fs::remove_file(self.stg_path(id));
        let _ = std::fs::remove_file(self.meta_path(id));
        Ok(())
    }

    fn load_meta(&self, id: &str) -> Result<StagedMeta> {
        let bytes = std::fs::read(self.meta_path(id))?;
        serde_json::from_slice(&bytes).map_err(|e| VaultError::Crypto(format!("meta parse: {e}")))
    }

    fn stg_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.stg"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.meta.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a staging area with a real device.key so the staging key can be derived.
    fn test_staging() -> (IngestStaging, TempDir) {
        let tmp = TempDir::new().unwrap();
        let data = tmp.path().join("data");
        let config = tmp.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        // 32-byte device secret
        std::fs::write(config.join("device.key"), [7u8; 32]).unwrap();
        (IngestStaging::new(&data, &config), tmp)
    }

    fn meta(uri: &str) -> StagedMeta {
        StagedMeta {
            uri: uri.into(),
            title: String::new(),
            mime_hint: Some("text/plain".into()),
            source_kind: "localfolder".into(),
            domain: None,
            tags: None,
            corpus_domain: None,
            created_at: 1000,
        }
    }

    // ── happy ────────────────────────────────────────────────────────────
    #[test]
    fn stage_then_load_roundtrip() {
        let (s, _t) = test_staging();
        let id = s
            .stage(&meta("upload://a.txt"), b"hello secret world")
            .unwrap();
        assert_eq!(s.count(), 1);
        let item = s.load(&id).unwrap();
        assert_eq!(item.content, b"hello secret world");
        assert_eq!(item.meta.uri, "upload://a.txt");
    }

    #[test]
    fn remove_is_idempotent_done_point() {
        let (s, _t) = test_staging();
        let id = s.stage(&meta("upload://a.txt"), b"data").unwrap();
        assert_eq!(s.count(), 1);
        s.remove(&id).unwrap();
        assert_eq!(s.count(), 0);
        // remove again = no-op, no error (idempotent)
        s.remove(&id).unwrap();
        assert_eq!(s.count(), 0);
    }

    // ── edge ─────────────────────────────────────────────────────────────
    #[test]
    fn list_pending_empty_is_noop() {
        let (s, _t) = test_staging();
        assert!(s.list_pending().is_empty());
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn zero_byte_content_roundtrips() {
        let (s, _t) = test_staging();
        let id = s.stage(&meta("upload://empty"), b"").unwrap();
        assert_eq!(s.load(&id).unwrap().content, b"");
    }

    #[test]
    fn unicode_uri_roundtrips() {
        let (s, _t) = test_staging();
        let id = s
            .stage(&meta("upload://报告_2026.pdf"), "内容".as_bytes())
            .unwrap();
        let item = s.load(&id).unwrap();
        assert_eq!(item.meta.uri, "upload://报告_2026.pdf");
        assert_eq!(item.content, "内容".as_bytes());
    }

    #[test]
    fn list_pending_sorted_by_created_at() {
        let (s, _t) = test_staging();
        let mut m_late = meta("upload://late");
        m_late.created_at = 2000;
        let mut m_early = meta("upload://early");
        m_early.created_at = 1000;
        let id_late = s.stage(&m_late, b"l").unwrap();
        let id_early = s.stage(&m_early, b"e").unwrap();
        let order = s.list_pending();
        assert_eq!(order, vec![id_early, id_late]);
    }

    // ── error ────────────────────────────────────────────────────────────
    #[test]
    fn stage_without_device_key_fails() {
        let tmp = TempDir::new().unwrap();
        let s = IngestStaging::new(&tmp.path().join("data"), &tmp.path().join("config"));
        let err = s.stage(&meta("upload://a"), b"x").unwrap_err();
        assert!(matches!(err, VaultError::DeviceSecretMissing(_)));
    }

    #[test]
    fn corrupt_ciphertext_load_fails_and_retains() {
        let (s, _t) = test_staging();
        let id = s.stage(&meta("upload://a"), b"data").unwrap();
        // Corrupt the .stg blob
        std::fs::write(s.dir().join(format!("{id}.stg")), b"garbage").unwrap();
        assert!(s.load(&id).is_err());
        // file retained for manual inspection
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn staging_full_rejects() {
        // Use a tiny cap via direct file creation to avoid staging 5000 real items.
        let (s, _t) = test_staging();
        std::fs::create_dir_all(s.dir()).unwrap();
        for i in 0..super::STAGING_MAX_PENDING {
            std::fs::write(s.dir().join(format!("fill{i}.stg")), b"x").unwrap();
        }
        let err = s.stage(&meta("upload://over"), b"x").unwrap_err();
        assert!(matches!(err, VaultError::StagingFull));
    }

    // ── adversarial: no plaintext at rest ────────────────────────────────
    #[test]
    fn staged_blob_contains_no_plaintext() {
        let (s, _t) = test_staging();
        let secret = b"TOP-SECRET-MARKER-7f3a";
        let id = s.stage(&meta("upload://a"), secret).unwrap();
        let blob = std::fs::read(s.dir().join(format!("{id}.stg"))).unwrap();
        // The ciphertext must NOT contain the plaintext marker anywhere.
        assert!(
            !blob.windows(secret.len()).any(|w| w == secret),
            "staging .stg blob leaked plaintext"
        );
        // And the meta sidecar must not contain content either (only routing info).
        let meta_bytes = std::fs::read(s.dir().join(format!("{id}.meta.json"))).unwrap();
        assert!(
            !meta_bytes.windows(secret.len()).any(|w| w == secret),
            "staging meta sidecar leaked plaintext"
        );
    }

    #[test]
    fn wrong_device_secret_cannot_decrypt() {
        let (s, t) = test_staging();
        let id = s.stage(&meta("upload://a"), b"data").unwrap();
        // Swap device.key for a different one → staging key changes → decrypt fails.
        std::fs::write(t.path().join("config/device.key"), [9u8; 32]).unwrap();
        assert!(s.load(&id).is_err());
    }

    // ── concurrent ───────────────────────────────────────────────────────
    #[test]
    fn concurrent_stage_unique_ids_no_collision() {
        use std::sync::Arc;
        use std::thread;
        let (s, _t) = test_staging();
        std::fs::create_dir_all(s.dir()).unwrap();
        let s = Arc::new(s);
        let mut handles = vec![];
        for i in 0..8 {
            let s = Arc::clone(&s);
            handles.push(thread::spawn(move || {
                s.stage(
                    &meta(&format!("upload://c{i}")),
                    format!("data{i}").as_bytes(),
                )
                .unwrap()
            }));
        }
        let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let mut uniq = ids.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "uuid collision in concurrent stage");
        assert_eq!(s.count(), 8);
    }

    // ── idempotency / mid-drain crash simulation ─────────────────────────
    #[test]
    fn mid_drain_crash_does_not_lose_or_double() {
        let (s, _t) = test_staging();
        let mut ids = vec![];
        for i in 0..4 {
            let mut m = meta(&format!("upload://d{i}"));
            m.created_at = 1000 + i as i64;
            ids.push(s.stage(&m, format!("d{i}").as_bytes()).unwrap());
        }
        // Simulate draining the first two then "crashing".
        let pending = s.list_pending();
        assert_eq!(pending.len(), 4);
        s.remove(&pending[0]).unwrap();
        s.remove(&pending[1]).unwrap();
        // Re-list: exactly the remaining two, no re-processing of removed ones.
        let after = s.list_pending();
        assert_eq!(after.len(), 2);
        assert_eq!(after, vec![pending[2].clone(), pending[3].clone()]);
    }
}
