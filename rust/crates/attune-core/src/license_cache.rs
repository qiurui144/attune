//! Client-side license cache — 持久化 cloud login 后的 license code.
//!
//! 位置: ~/.config/npu-vault/license.json
//! 用途:
//! - attune-cli login 后写入
//! - attune-server 启动时读, 透传给 PluginRegistry::scan_with_key 解密 paid plugin
//! - 客户端 sync-plugins / chat 调 LLM gateway 时鉴权用

use crate::error::{Result, VaultError};
use crate::license::SignedLicense;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseCache {
    /// signed license code (base64)
    pub license_code: String,
    /// 解码后的 claims (冗余, 方便客户端不重复 decode)
    pub claims: crate::license::LicenseClaims,
    /// 缓存写入时间 (Unix epoch)
    pub cached_at: i64,
    /// 关联 cloud accounts URL (回溯用)
    pub cloud_url: String,
}

impl LicenseCache {
    /// 默认存储路径
    pub fn default_path() -> PathBuf {
        crate::platform::config_dir().join("license.json")
    }

    /// 写入 (chmod 600 on Unix — license 是敏感凭证)
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(VaultError::Io)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| VaultError::Crypto(format!("license cache ser: {e}")))?;
        std::fs::write(path, &json).map_err(VaultError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// 从磁盘读取 — 不存在返 Ok(None)
    pub fn load(path: &std::path::Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(path).map_err(VaultError::Io)?;
        let cache: Self = serde_json::from_str(&s)
            .map_err(|e| VaultError::Crypto(format!("license cache parse: {e}")))?;
        Ok(Some(cache))
    }

    /// 从 SignedLicense 构造
    pub fn from_signed(signed: SignedLicense, cloud_url: impl Into<String>) -> Result<Self> {
        let code = signed.to_code()?;
        Ok(Self {
            license_code: code,
            claims: signed.claims,
            cached_at: chrono::Utc::now().timestamp(),
            cloud_url: cloud_url.into(),
        })
    }

    /// license code 字节 (给 scan_with_key + plugin_encryption::decrypt_yaml 用)
    pub fn as_decrypt_key(&self) -> &[u8] {
        self.license_code.as_bytes()
    }

    /// 删除 cache (logout / license 撤销时)
    pub fn remove(path: &std::path::Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_file(path).map_err(VaultError::Io)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{sign_license, LicenseClaims};
    use crate::plugin_sig::generate_signing_key;
    use tempfile::TempDir;

    fn sample_signed() -> SignedLicense {
        let sk = generate_signing_key();
        sign_license(
            LicenseClaims {
                license_id: "lic-1".into(),
                account_id: "acc-1".into(),
                tier: "paid".into(),
                max_devices: 2,
                llm_monthly_quota: 1_000_000,
                issued_at: 1_700_000_000,
                expires_at: 0,
                note: "test".into(),
            },
            &sk,
        )
    }

    #[test]
    fn save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("license.json");
        let cache = LicenseCache::from_signed(sample_signed(), "https://accounts.attune.ai").unwrap();
        cache.save(&path).unwrap();
        assert!(path.exists());
        let loaded = LicenseCache::load(&path).unwrap().expect("loaded");
        assert_eq!(loaded.license_code, cache.license_code);
        assert_eq!(loaded.claims.license_id, "lic-1");
    }

    #[test]
    fn load_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        assert!(LicenseCache::load(&path).unwrap().is_none());
    }

    #[test]
    fn as_decrypt_key_is_license_code_bytes() {
        let cache = LicenseCache::from_signed(sample_signed(), "x").unwrap();
        assert_eq!(cache.as_decrypt_key(), cache.license_code.as_bytes());
    }

    #[test]
    fn remove_idempotent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("license.json");
        // 不存在时 remove 不报错
        LicenseCache::remove(&path).unwrap();
        // 写入后 remove 成功
        let cache = LicenseCache::from_signed(sample_signed(), "x").unwrap();
        cache.save(&path).unwrap();
        assert!(path.exists());
        LicenseCache::remove(&path).unwrap();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_600_perms_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("license.json");
        let cache = LicenseCache::from_signed(sample_signed(), "x").unwrap();
        cache.save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "license cache must be chmod 600");
    }
}
