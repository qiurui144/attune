//! Persistence boundary for application settings and their secrets.
//!
//! Non-sensitive settings remain in `app_settings` so form-factor and privacy
//! defaults are readable without decrypting the vault. LLM, embedding, and
//! PluginHub credentials live in separate AES-256-GCM encrypted metadata rows
//! and are injected only into the in-memory settings value while unlocked.

use attune_core::crypto;
use attune_core::error::{Result, VaultError};
use attune_core::llm_settings::SETTINGS_META_KEY;
use attune_core::vault::Vault;
use serde_json::Value;

pub(crate) const LLM_API_KEY_SECRET: &str = "app_secret.llm_api_key.v1";
pub(crate) const EMBEDDING_API_KEY_SECRET: &str = "app_secret.embedding_api_key.v1";
pub(crate) const PLUGINHUB_LICENSE_KEY_SECRET: &str = "app_secret.pluginhub_license_key.v1";
const SECURE_MIGRATION_PENDING: &str = "app_secret.secure_migration_pending.v1";
const DEVICE_BINDING_META_KEY: &str = "device_binding";

#[derive(Clone, Copy)]
struct SecretSpec {
    section: &'static str,
    field: &'static str,
    meta_key: &'static str,
}

const SECRET_SPECS: [SecretSpec; 3] = [
    SecretSpec {
        section: "llm",
        field: "api_key",
        meta_key: LLM_API_KEY_SECRET,
    },
    SecretSpec {
        section: "embedding",
        field: "api_key",
        meta_key: EMBEDDING_API_KEY_SECRET,
    },
    SecretSpec {
        section: "pluginhub",
        field: "license_key",
        meta_key: PLUGINHUB_LICENSE_KEY_SECRET,
    },
];

/// Load settings and inject decrypted credentials when the vault is unlocked.
///
/// Legacy plaintext fields are migrated to encrypted metadata during the first
/// unlocked read. A sealed read never exposes or rewrites secret material.
pub(crate) fn load_settings(vault: &Vault) -> Result<Option<Value>> {
    let raw = vault.store().get_meta(SETTINGS_META_KEY)?;
    let had_settings = raw.is_some();
    let mut stored: Value = match raw {
        Some(raw) => serde_json::from_slice(&raw)?,
        None => Value::Object(serde_json::Map::new()),
    };
    let Ok(dek) = vault.dek_db() else {
        // A legacy database may still contain plaintext fields until its first
        // unlocked migration. A sealed read may return non-sensitive settings,
        // but it must never surface those legacy credentials in memory.
        for spec in SECRET_SPECS {
            remove_secret_field(&mut stored, spec);
        }
        return Ok(had_settings.then_some(stored));
    };

    let mut migrated = false;
    let mut secret_upserts: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let mut secret_deletes: Vec<&'static str> = Vec::new();
    for spec in SECRET_SPECS {
        let legacy = secret_value(&stored, spec).map(str::to_string);
        if let Some(legacy) = legacy {
            if legacy.is_empty() {
                secret_deletes.push(spec.meta_key);
            } else {
                let encrypted = crypto::encrypt(&dek, legacy.as_bytes())?;
                secret_upserts.push((spec.meta_key, encrypted));
            }
            remove_secret_field(&mut stored, spec);
            migrated = true;
        }
    }

    // Older builds stored the membership device token as plaintext JSON. It is
    // not part of app settings, but migrate it in the same one-time secure
    // rewrite so no member credential remains in historical SQLite pages.
    if let Some(device_binding) = vault.store().get_meta(DEVICE_BINDING_META_KEY)? {
        let is_legacy_plaintext = serde_json::from_slice::<Value>(&device_binding)
            .ok()
            .and_then(|value| value.get("device_token").cloned())
            .and_then(|value| value.as_str().map(str::to_string))
            .is_some();
        if is_legacy_plaintext {
            secret_upserts.push((
                DEVICE_BINDING_META_KEY,
                crypto::encrypt(&dek, &device_binding)?,
            ));
            migrated = true;
        }
    }
    if migrated {
        let marker = b"1".as_slice();
        let mut entries: Vec<(&str, &[u8])> = secret_upserts
            .iter()
            .map(|(key, value)| (*key, value.as_slice()))
            .collect();
        let sanitized = had_settings
            .then(|| serde_json::to_vec(&stored))
            .transpose()?;
        if let Some(sanitized) = sanitized.as_ref() {
            entries.push((SETTINGS_META_KEY, sanitized.as_slice()));
        }
        entries.push((SECURE_MIGRATION_PENDING, marker));
        vault.store().mutate_meta_batch(&entries, &secret_deletes)?;
    }

    // Fail closed until the one-time physical rewrite succeeds. The marker is
    // committed atomically with the sanitized rows, so an interrupted VACUUM is
    // retried on the next unlocked read instead of silently claiming migration.
    if vault.store().get_meta(SECURE_MIGRATION_PENDING)?.is_some() {
        vault.store().secure_compact()?;
        vault.store().delete_meta(SECURE_MIGRATION_PENDING)?;
    }

    if !had_settings {
        return Ok(None);
    }

    for spec in SECRET_SPECS {
        let Some(encrypted) = vault.store().get_meta(spec.meta_key)? else {
            continue;
        };
        let plaintext = crypto::decrypt(&dek, &encrypted)?;
        let plaintext = String::from_utf8(plaintext)
            .map_err(|e| VaultError::Crypto(format!("{} is not UTF-8: {e}", spec.meta_key)))?;
        insert_secret_field(&mut stored, spec, plaintext)?;
    }
    Ok(Some(stored))
}

/// Persist settings while extracting credential fields into encrypted metadata.
/// Missing secret fields preserve the existing credential; an explicit empty
/// string removes it.
pub(crate) fn persist_settings(vault: &Vault, mut settings: Value) -> Result<()> {
    let mut secret_upserts: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let mut secret_deletes: Vec<&'static str> = Vec::new();
    for spec in SECRET_SPECS {
        let supplied = secret_value(&settings, spec).map(str::to_string);
        if let Some(supplied) = supplied {
            if supplied.is_empty() {
                secret_deletes.push(spec.meta_key);
            } else {
                // Resolve the DEK only when there is new secret material to
                // encrypt. This lets logout/privacy teardown persist a fully
                // sanitized settings object even while the vault is sealed.
                let dek = vault.dek_db()?;
                let encrypted = crypto::encrypt(&dek, supplied.as_bytes())?;
                secret_upserts.push((spec.meta_key, encrypted));
            }
            remove_secret_field(&mut settings, spec);
        }
    }
    let stored = serde_json::to_vec(&settings)?;
    let mut entries: Vec<(&str, &[u8])> = secret_upserts
        .iter()
        .map(|(key, value)| (*key, value.as_slice()))
        .collect();
    entries.push((SETTINGS_META_KEY, stored.as_slice()));
    vault.store().mutate_meta_batch(&entries, &secret_deletes)
}

pub(crate) fn delete_secret(vault: &Vault, meta_key: &str) -> Result<bool> {
    if !SECRET_SPECS.iter().any(|spec| spec.meta_key == meta_key) {
        return Err(VaultError::InvalidInput(format!(
            "unknown application secret key: {meta_key}"
        )));
    }
    vault.store().delete_meta(meta_key)
}

fn secret_value(settings: &Value, spec: SecretSpec) -> Option<&str> {
    settings
        .get(spec.section)
        .and_then(Value::as_object)
        .and_then(|section| section.get(spec.field))
        .and_then(Value::as_str)
}

fn remove_secret_field(settings: &mut Value, spec: SecretSpec) {
    if let Some(section) = settings
        .get_mut(spec.section)
        .and_then(Value::as_object_mut)
    {
        section.remove(spec.field);
    }
}

fn insert_secret_field(settings: &mut Value, spec: SecretSpec, plaintext: String) -> Result<()> {
    let root = settings.as_object_mut().ok_or_else(|| {
        VaultError::InvalidInput("application settings root must be an object".to_string())
    })?;
    let section = root
        .entry(spec.section.to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            VaultError::InvalidInput(format!(
                "application settings section {} must be an object",
                spec.section
            ))
        })?;
    section.insert(spec.field.to_string(), Value::String(plaintext));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_contains(path: &std::path::Path, needle: &[u8]) -> bool {
        std::fs::read(path)
            .ok()
            .is_some_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
    }

    #[test]
    fn migrates_plaintext_credentials_and_never_writes_them_back() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open_memory(tmp.path()).unwrap();
        vault.setup("P@ss-settings-secret-migration").unwrap();
        let legacy = serde_json::json!({
            "llm": {"endpoint": "https://example.test/v1", "api_key": "llm-secret"},
            "embedding": {"api_key": "embedding-secret"},
            "pluginhub": {"license_key": "plugin-secret"}
        });
        vault
            .store()
            .set_meta(SETTINGS_META_KEY, &serde_json::to_vec(&legacy).unwrap())
            .unwrap();

        let loaded = load_settings(&vault).unwrap().unwrap();
        assert_eq!(loaded["llm"]["api_key"], "llm-secret");
        assert_eq!(loaded["embedding"]["api_key"], "embedding-secret");
        assert_eq!(loaded["pluginhub"]["license_key"], "plugin-secret");

        let raw = vault.store().get_meta(SETTINGS_META_KEY).unwrap().unwrap();
        let raw_text = String::from_utf8_lossy(&raw);
        assert!(!raw_text.contains("llm-secret"));
        assert!(!raw_text.contains("embedding-secret"));
        assert!(!raw_text.contains("plugin-secret"));
        for (key, plaintext) in [
            (LLM_API_KEY_SECRET, b"llm-secret".as_slice()),
            (EMBEDDING_API_KEY_SECRET, b"embedding-secret".as_slice()),
            (PLUGINHUB_LICENSE_KEY_SECRET, b"plugin-secret".as_slice()),
        ] {
            let encrypted = vault.store().get_meta(key).unwrap().unwrap();
            assert!(!encrypted.windows(plaintext.len()).any(|w| w == plaintext));
        }
    }

    #[test]
    fn sealed_legacy_read_never_surfaces_plaintext_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open_memory(tmp.path()).unwrap();
        vault.setup("P@ss-settings-sealed-legacy").unwrap();
        let legacy = serde_json::json!({
            "llm": {"model": "m", "api_key": "legacy-llm-secret"},
            "embedding": {"api_key": "legacy-embedding-secret"},
            "pluginhub": {"license_key": "legacy-plugin-secret"}
        });
        vault
            .store()
            .set_meta(SETTINGS_META_KEY, &serde_json::to_vec(&legacy).unwrap())
            .unwrap();
        vault.lock().unwrap();

        let sealed = load_settings(&vault).unwrap().unwrap();
        for spec in SECRET_SPECS {
            assert!(
                secret_value(&sealed, spec).is_none(),
                "sealed legacy read exposed {}.{}",
                spec.section,
                spec.field
            );
        }
        let still_pending = vault.store().get_meta(SETTINGS_META_KEY).unwrap().unwrap();
        assert!(
            String::from_utf8_lossy(&still_pending).contains("legacy-llm-secret"),
            "sealed read must not pretend the physical migration committed"
        );
    }

    #[test]
    fn missing_secret_preserves_and_empty_secret_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open_memory(tmp.path()).unwrap();
        vault.setup("P@ss-settings-secret-preserve").unwrap();
        persist_settings(
            &vault,
            serde_json::json!({"llm": {"model": "m", "api_key": "secret"}}),
        )
        .unwrap();
        persist_settings(&vault, serde_json::json!({"llm": {"model": "m2"}})).unwrap();
        assert_eq!(
            load_settings(&vault).unwrap().unwrap()["llm"]["api_key"],
            "secret"
        );
        persist_settings(
            &vault,
            serde_json::json!({"llm": {"model": "m2", "api_key": ""}}),
        )
        .unwrap();
        assert!(load_settings(&vault).unwrap().unwrap()["llm"]
            .get("api_key")
            .is_none());
    }

    #[test]
    fn on_disk_migration_removes_plaintext_from_database_and_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();
        vault.setup("P@ss-settings-physical-migration").unwrap();
        let legacy = serde_json::json!({
            "llm": {"endpoint": "https://old.example/v1", "api_key": "physical-secret"}
        });
        vault
            .store()
            .set_meta(SETTINGS_META_KEY, &serde_json::to_vec(&legacy).unwrap())
            .unwrap();
        vault.store().checkpoint().unwrap();
        assert!(file_contains(&db_path, b"physical-secret"));

        let loaded = load_settings(&vault).unwrap().unwrap();
        assert_eq!(loaded["llm"]["api_key"], "physical-secret");
        assert!(!file_contains(&db_path, b"physical-secret"));
        assert!(!file_contains(
            &std::path::PathBuf::from(format!("{}-wal", db_path.display())),
            b"physical-secret"
        ));
        assert!(vault
            .store()
            .get_meta(SECURE_MIGRATION_PENDING)
            .unwrap()
            .is_none());
    }

    #[test]
    fn migrates_legacy_plaintext_device_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open_memory(tmp.path()).unwrap();
        vault.setup("P@ss-device-secret-migration").unwrap();
        vault.store().set_meta(SETTINGS_META_KEY, br#"{}"#).unwrap();
        vault
            .store()
            .set_meta(
                DEVICE_BINDING_META_KEY,
                br#"{"device_token":"legacy-device-token","device_id":"dev-1"}"#,
            )
            .unwrap();

        load_settings(&vault).unwrap();
        let encrypted = vault
            .store()
            .get_meta(DEVICE_BINDING_META_KEY)
            .unwrap()
            .unwrap();
        assert!(!encrypted
            .windows("legacy-device-token".len())
            .any(|window| window == b"legacy-device-token"));
        let plaintext = crypto::decrypt(&vault.dek_db().unwrap(), &encrypted).unwrap();
        let value: Value = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(value["device_token"], "legacy-device-token");
    }

    #[test]
    fn migrates_device_binding_even_when_app_settings_are_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open_memory(tmp.path()).unwrap();
        vault.setup("P@ss-device-only-migration").unwrap();
        vault
            .store()
            .set_meta(
                DEVICE_BINDING_META_KEY,
                br#"{"device_token":"device-only-secret","device_id":"dev-only"}"#,
            )
            .unwrap();

        assert!(load_settings(&vault).unwrap().is_none());
        let encrypted = vault
            .store()
            .get_meta(DEVICE_BINDING_META_KEY)
            .unwrap()
            .unwrap();
        assert!(!encrypted
            .windows("device-only-secret".len())
            .any(|window| window == b"device-only-secret"));
        let plaintext = crypto::decrypt(&vault.dek_db().unwrap(), &encrypted).unwrap();
        let value: Value = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(value["device_token"], "device-only-secret");
        assert!(vault.store().get_meta(SETTINGS_META_KEY).unwrap().is_none());
    }
}
