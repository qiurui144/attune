// npu-vault/crates/vault-core/src/vault.rs

use std::path::Path;
use std::sync::Mutex;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::platform;
use crate::store::Store;

/// Vault 状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultState {
    Sealed,
    Locked,
    Unlocked,
}

/// 解锁后的内存密钥集（lock 时清零）
struct UnlockedKeys {
    master_key: Key32,
    dek_db: Key32,
    dek_idx: Key32,
    dek_vec: Key32,
}

/// 顶层 Vault 引擎
pub struct Vault {
    store: Store,
    config_dir: std::path::PathBuf,
    unlocked: Mutex<Option<UnlockedKeys>>,
}

/// Session token 有效期（秒）
const SESSION_TTL_SECS: i64 = 4 * 3600; // 4 小时

impl Vault {
    /// 打开 vault（使用默认路径）
    pub fn open_default() -> Result<Self> {
        let db_path = platform::db_path();
        Self::open(&db_path, &platform::config_dir())
    }

    /// 打开 vault（自定义路径，用于测试）
    pub fn open(db_path: &Path, config_dir: &Path) -> Result<Self> {
        let store = Store::open(db_path)?;
        Ok(Self {
            store,
            config_dir: config_dir.to_path_buf(),
            unlocked: Mutex::new(None),
        })
    }

    /// 打开内存 vault（测试用）
    pub fn open_memory(config_dir: &Path) -> Result<Self> {
        let store = Store::open_memory()?;
        Ok(Self {
            store,
            config_dir: config_dir.to_path_buf(),
            unlocked: Mutex::new(None),
        })
    }

    /// 当前状态
    pub fn state(&self) -> VaultState {
        if self.unlocked.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return VaultState::Unlocked;
        }
        match self.store.has_meta("salt") {
            Ok(true) => VaultState::Locked,
            _ => VaultState::Sealed,
        }
    }

    /// 首次设置：生成 device secret + DEK，用密码保护
    pub fn setup(&self, password: &str) -> Result<()> {
        let _ = self.setup_with_recovery_key(password)?;
        Ok(())
    }

    /// 首次设置 + 返回一次性恢复密钥（用于忘记主密码时重置，不丢数据）。
    pub fn setup_with_recovery_key(&self, password: &str) -> Result<String> {
        if self.state() != VaultState::Sealed {
            return Err(VaultError::AlreadyInitialized);
        }

        // 生成 device secret 并写入文件
        let device_secret = crypto::generate_device_secret();
        let ds_path = self.config_dir.join("device.key");
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::write(&ds_path, device_secret.as_bytes())?;
        restrict_file_permissions(&ds_path)?;

        // 生成 salt + 派生 MK
        let salt = crypto::generate_salt();
        let mk = crypto::derive_master_key(password.as_bytes(), device_secret.as_ref(), &salt)?;

        // 生成 3 个 DEK
        let dek_db = Key32::generate();
        let dek_idx = Key32::generate();
        let dek_vec = Key32::generate();

        // 生成恢复密钥（仅此处明文返回给调用方，库内不持久化明文）
        let recovery_key = generate_recovery_key();
        let recovery_salt = crypto::generate_salt();
        let recovery_mk = crypto::derive_master_key(
            recovery_key.as_bytes(),
            device_secret.as_ref(),
            &recovery_salt,
        )?;

        // 用 MK 加密 DEK 并存储
        self.store.set_meta("salt", &salt)?;
        self.store.set_meta("encrypted_dek_db", &crypto::encrypt_dek(&mk, &dek_db)?)?;
        self.store.set_meta("encrypted_dek_idx", &crypto::encrypt_dek(&mk, &dek_idx)?)?;
        self.store.set_meta("encrypted_dek_vec", &crypto::encrypt_dek(&mk, &dek_vec)?)?;

        // 额外保存"恢复密钥路径"加密副本：忘记主密码时可用 recovery_key 解 DEK。
        self.store.set_meta("recovery_salt", &recovery_salt)?;
        self.store
            .set_meta("encrypted_dek_db_recovery", &crypto::encrypt_dek(&recovery_mk, &dek_db)?)?;
        self.store
            .set_meta("encrypted_dek_idx_recovery", &crypto::encrypt_dek(&recovery_mk, &dek_idx)?)?;
        self.store
            .set_meta("encrypted_dek_vec_recovery", &crypto::encrypt_dek(&recovery_mk, &dek_vec)?)?;

        // 存储 device secret hash（验证用）
        let ds_hash = sha2_hash(device_secret.as_ref());
        self.store.set_meta("device_secret_hash", &ds_hash)?;

        // 存储 vault 版本
        self.store.set_meta("vault_version", b"1")?;

        // 自动解锁
        *self.unlocked.lock().unwrap_or_else(|e| e.into_inner()) = Some(UnlockedKeys {
            master_key: mk,
            dek_db,
            dek_idx,
            dek_vec,
        });

        Ok(recovery_key)
    }

    /// 使用恢复密钥重置主密码（保留数据，不需要旧密码）。
    pub fn reset_password_with_recovery_key(
        &self,
        recovery_key: &str,
        new_password: &str,
    ) -> Result<()> {
        if new_password.is_empty() {
            return Err(VaultError::InvalidInput("new password must not be empty".into()));
        }
        if self.state() == VaultState::Sealed {
            return Err(VaultError::Sealed);
        }

        let ds_path = self.config_dir.join("device.key");
        let device_secret_bytes = std::fs::read(&ds_path)
            .map_err(|_| VaultError::DeviceSecretMissing(ds_path.display().to_string()))?;

        let recovery_salt = self
            .store
            .get_meta("recovery_salt")?
            .ok_or_else(|| VaultError::InvalidInput("recovery key is not configured for this vault".into()))?;
        let recovery_mk = crypto::derive_master_key(
            recovery_key.as_bytes(),
            &device_secret_bytes,
            &recovery_salt,
        )?;

        // 用恢复密钥路径解出 DEK（若 recovery_key 错误，这里会返回 InvalidPassword）
        let enc_dek_db = self
            .store
            .get_meta("encrypted_dek_db_recovery")?
            .ok_or_else(|| VaultError::InvalidInput("recovery key material missing".into()))?;
        let dek_db = crypto::decrypt_dek(&recovery_mk, &enc_dek_db)?;

        let enc_dek_idx = self
            .store
            .get_meta("encrypted_dek_idx_recovery")?
            .ok_or_else(|| VaultError::InvalidInput("recovery key material missing".into()))?;
        let dek_idx = crypto::decrypt_dek(&recovery_mk, &enc_dek_idx)?;

        let enc_dek_vec = self
            .store
            .get_meta("encrypted_dek_vec_recovery")?
            .ok_or_else(|| VaultError::InvalidInput("recovery key material missing".into()))?;
        let dek_vec = crypto::decrypt_dek(&recovery_mk, &enc_dek_vec)?;

        // 换主密码：只重写主路径 salt + DEK 包装；恢复路径保持不变。
        let new_salt = crypto::generate_salt();
        let new_mk = crypto::derive_master_key(new_password.as_bytes(), &device_secret_bytes, &new_salt)?;
        let new_enc_dek_db = crypto::encrypt_dek(&new_mk, &dek_db)?;
        let new_enc_dek_idx = crypto::encrypt_dek(&new_mk, &dek_idx)?;
        let new_enc_dek_vec = crypto::encrypt_dek(&new_mk, &dek_vec)?;

        self.store.set_meta_batch(&[
            ("salt", new_salt.as_ref()),
            ("encrypted_dek_db", new_enc_dek_db.as_slice()),
            ("encrypted_dek_idx", new_enc_dek_idx.as_slice()),
            ("encrypted_dek_vec", new_enc_dek_vec.as_slice()),
        ])?;

        // 失效旧 token
        let _ = self.store.increment_token_nonce()?;

        // 若当前恰好是 UNLOCKED（例如本地调试误触），同步内存密钥，避免状态漂移。
        let mut guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(keys) = guard.as_mut() {
            keys.master_key = new_mk;
            keys.dek_db = dek_db;
            keys.dek_idx = dek_idx;
            keys.dek_vec = dek_vec;
        }

        Ok(())
    }

    /// 解锁 vault
    ///
    /// 已解锁状态下重复调用：如果密码正确，签发一个全新 session token（同样的 MK，
    /// 内存密钥保持不变）。用于浏览器重启 / sessionStorage 被清 / token 过期等
    /// "服务端 vault 已解锁但客户端没有有效 token" 的场景，避免用户被迫先 lock 再 unlock。
    /// 密码错误时仍抛 AEAD 认证错误，行为与首次 unlock 一致。
    pub fn unlock(&self, password: &str) -> Result<String> {
        match self.state() {
            VaultState::Sealed => return Err(VaultError::Sealed),
            VaultState::Unlocked => return self.reissue_token(password),
            VaultState::Locked => {}
        }

        // 读取 device secret
        let ds_path = self.config_dir.join("device.key");
        let device_secret_bytes = std::fs::read(&ds_path)
            .map_err(|_| VaultError::DeviceSecretMissing(ds_path.display().to_string()))?;
        if device_secret_bytes.len() != 32 {
            return Err(VaultError::DeviceSecretMismatch);
        }

        // 验证 device secret hash
        let stored_hash = self.store.get_meta("device_secret_hash")?
            .ok_or(VaultError::Crypto("missing device_secret_hash".into()))?;
        let actual_hash = sha2_hash(&device_secret_bytes);
        if stored_hash != actual_hash {
            return Err(VaultError::DeviceSecretMismatch);
        }

        // 派生 MK
        let salt = self.store.get_meta("salt")?
            .ok_or(VaultError::Crypto("missing salt".into()))?;
        let mk = crypto::derive_master_key(password.as_bytes(), &device_secret_bytes, &salt)?;

        // 尝试解密 DEK（如果密码错误，这里会失败）
        let enc_dek_db = self.store.get_meta("encrypted_dek_db")?
            .ok_or(VaultError::Crypto("missing dek_db".into()))?;
        let dek_db = crypto::decrypt_dek(&mk, &enc_dek_db)?;

        let enc_dek_idx = self.store.get_meta("encrypted_dek_idx")?
            .ok_or(VaultError::Crypto("missing dek_idx".into()))?;
        let dek_idx = crypto::decrypt_dek(&mk, &enc_dek_idx)?;

        let enc_dek_vec = self.store.get_meta("encrypted_dek_vec")?
            .ok_or(VaultError::Crypto("missing dek_vec".into()))?;
        let dek_vec = crypto::decrypt_dek(&mk, &enc_dek_vec)?;

        // 签发 session token
        let token = self.create_session_token(&mk)?;

        // 存入内存
        *self.unlocked.lock().unwrap_or_else(|e| e.into_inner()) = Some(UnlockedKeys {
            master_key: mk,
            dek_db,
            dek_idx,
            dek_vec,
        });

        Ok(token)
    }

    /// 锁定 vault（清零内存密钥）
    pub fn lock(&self) -> Result<()> {
        // 先递增 nonce，使所有已签发 token 失效
        self.store.increment_token_nonce()?;
        // 再清零内存密钥（UnlockedKeys 内的 Key32 实现了 ZeroizeOnDrop）
        let mut guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
        Ok(())
    }

    /// 更改密码（重新加密 DEK，数据不变）
    pub fn change_password(&self, old_password: &str, new_password: &str) -> Result<()> {
        if new_password.is_empty() {
            return Err(VaultError::Crypto("new password must not be empty".into()));
        }
        if self.state() != VaultState::Unlocked {
            return Err(VaultError::Locked);
        }

        let guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;

        // 验证旧密码（重新派生 MK 比对）
        let ds_path = self.config_dir.join("device.key");
        let device_secret_bytes = std::fs::read(&ds_path)
            .map_err(|_| VaultError::DeviceSecretMissing(ds_path.display().to_string()))?;
        let salt = self.store.get_meta("salt")?
            .ok_or(VaultError::Crypto("missing salt".into()))?;
        let old_mk = crypto::derive_master_key(old_password.as_bytes(), &device_secret_bytes, &salt)?;

        // 验证旧 MK 能解密 dek_db
        let enc_dek_db = self.store.get_meta("encrypted_dek_db")?
            .ok_or(VaultError::Crypto("missing dek_db".into()))?;
        crypto::decrypt_dek(&old_mk, &enc_dek_db)?; // 如果旧密码错误，这里会报 InvalidPassword

        // 生成新 salt + 派生新 MK
        let new_salt = crypto::generate_salt();
        let new_mk = crypto::derive_master_key(new_password.as_bytes(), &device_secret_bytes, &new_salt)?;

        // 预计算新加密 DEK（在事务外计算，避免持锁时 argon2 长耗时）
        let new_enc_dek_db = crypto::encrypt_dek(&new_mk, &keys.dek_db)?;
        let new_enc_dek_idx = crypto::encrypt_dek(&new_mk, &keys.dek_idx)?;
        let new_enc_dek_vec = crypto::encrypt_dek(&new_mk, &keys.dek_vec)?;

        // 在单个 SQLite 事务中原子写入 salt + 3 个 DEK，防止中途失败导致数据不一致
        self.store.set_meta_batch(&[
            ("salt", new_salt.as_ref()),
            ("encrypted_dek_db", new_enc_dek_db.as_slice()),
            ("encrypted_dek_idx", new_enc_dek_idx.as_slice()),
            ("encrypted_dek_vec", new_enc_dek_vec.as_slice()),
        ])?;

        // OSS-S6 fix (2026-05-02): MK 也是 session token 的 HMAC 签名 key（见
        // verify_session 第 317 行），不只是用于解密 DEK。change_password 后必须把
        // 内存里的 MK 同步换成 new_mk，否则 reissue_token 用 new_mk 签 / verify_session
        // 用 old_mk 验 → 全部新 token 都返 401 "session invalid"，直到下次 lock+unlock
        // 才能恢复。原注释 "DEK 不变 MK 不需更新" 漏看了 HMAC 用途。
        drop(guard);
        let mut guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(keys) = guard.as_mut() {
            keys.master_key = new_mk;
        }

        Ok(())
    }

    /// 获取 DEK_db（仅 UNLOCKED 状态）
    pub fn dek_db(&self) -> Result<Key32> {
        let guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(keys.dek_db.clone())
    }

    /// 获取 DEK_idx（仅 UNLOCKED 状态）
    pub fn dek_idx(&self) -> Result<Key32> {
        let guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(keys.dek_idx.clone())
    }

    /// 获取 DEK_vec（仅 UNLOCKED 状态）
    pub fn dek_vec(&self) -> Result<Key32> {
        let guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(keys.dek_vec.clone())
    }

    /// 获取 Store 引用（用于 CRUD 操作）
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 导出 device secret（hex 编码，需 UNLOCKED 状态）
    pub fn export_device_secret(&self) -> Result<String> {
        if self.state() != VaultState::Unlocked {
            return Err(VaultError::Locked);
        }

        let ds_path = self.config_dir.join("device.key");
        let bytes = std::fs::read(&ds_path)
            .map_err(|_| VaultError::DeviceSecretMissing(ds_path.display().to_string()))?;

        if bytes.len() != 32 {
            return Err(VaultError::DeviceSecretMismatch);
        }

        Ok(hex::encode(&bytes))
    }

    /// 导入 device secret (hex 编码)，替换当前设备的 device.key
    /// NOTE: 导入后 vault 仍然 SEALED/LOCKED，需要原密码 unlock
    /// 实际流程: 1) 迁移数据库 2) 导入 device.key 3) 用原密码 unlock
    pub fn import_device_secret(&self, hex_encoded: &str) -> Result<()> {
        let bytes = hex::decode(hex_encoded)
            .map_err(|_| VaultError::Crypto("invalid hex encoding".into()))?;

        if bytes.len() != 32 {
            return Err(VaultError::Crypto("device secret must be 32 bytes".into()));
        }

        let ds_path = self.config_dir.join("device.key");
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::write(&ds_path, &bytes)?;
        restrict_file_permissions(&ds_path)?;

        Ok(())
    }

    /// 验证 session token
    pub fn verify_session(&self, token: &str) -> Result<()> {
        let guard = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;

        // 分离签名与 payload
        let dot_pos = token.rfind('.').ok_or(VaultError::SessionInvalid)?;
        let payload = &token[..dot_pos];
        let sig_hex = &token[dot_pos + 1..];

        let sig = hex::decode(sig_hex).map_err(|_| VaultError::SessionInvalid)?;
        if !crypto::hmac_verify(&keys.master_key, payload.as_bytes(), &sig) {
            return Err(VaultError::SessionInvalid);
        }

        // payload 格式：{session_id}:{expires}:{nonce}
        let parts: Vec<&str> = payload.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(VaultError::SessionInvalid);
        }
        let expires: i64 = parts[1].parse().map_err(|_| VaultError::SessionInvalid)?;
        let token_nonce: u64 = parts[2].parse().map_err(|_| VaultError::SessionInvalid)?;

        // 检查过期时间
        let now = chrono::Utc::now().timestamp();
        if now > expires {
            return Err(VaultError::SessionExpired);
        }

        // 检查 nonce：token 中的 nonce 必须等于当前存储的 nonce
        // 在持有 unlocked guard 期间读取 nonce，消除 TOCTOU 竞态窗口
        let current_nonce = self.store.get_token_nonce()?;
        drop(guard); // 释放 unlocked 锁（nonce 已读取，无 TOCTOU）
        if token_nonce != current_nonce {
            return Err(VaultError::SessionInvalid);
        }

        Ok(())
    }

    /// 已解锁 vault 的 token 重发：用密码再认证 + 签发新 token
    /// 成功条件：派生 MK 能通过 AES-GCM 认证标签解密 dek_db（密码正确）。
    /// 不修改内存中的 UnlockedKeys。
    fn reissue_token(&self, password: &str) -> Result<String> {
        let ds_path = self.config_dir.join("device.key");
        let device_secret_bytes = std::fs::read(&ds_path)
            .map_err(|_| VaultError::DeviceSecretMissing(ds_path.display().to_string()))?;
        let salt = self.store.get_meta("salt")?
            .ok_or(VaultError::Crypto("missing salt".into()))?;
        let mk = crypto::derive_master_key(password.as_bytes(), &device_secret_bytes, &salt)?;
        // AEAD 标签校验提供常数时间密码验证（解密失败 = 密码错）
        let enc_dek_db = self.store.get_meta("encrypted_dek_db")?
            .ok_or(VaultError::Crypto("missing dek_db".into()))?;
        let _ = crypto::decrypt_dek(&mk, &enc_dek_db)?;
        self.create_session_token(&mk)
    }

    fn create_session_token(&self, mk: &Key32) -> Result<String> {
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let expires = chrono::Utc::now().timestamp() + SESSION_TTL_SECS;
        let nonce = self.store.get_token_nonce()?;
        // payload 格式：{session_id}:{expires}:{nonce}
        let payload = format!("{session_id}:{expires}:{nonce}");
        let sig = crypto::hmac_sign(mk, payload.as_bytes());
        Ok(format!("{payload}.{}", hex::encode(sig)))
    }

    /// 忘记密码后的本地重置：清空 vault 数据并回到 SEALED。
    ///
    /// 安全边界：
    /// - 仅允许在 LOCKED/SEALED 状态触发（UNLOCKED 时拒绝，避免误触）。
    /// - 必须携带固定确认串 "RESET"。
    ///
    /// 重置后结果：
    /// - sqlite 所有业务表清空（含 vault_meta，故 state 回到 sealed）
    /// - 删除 device.key（旧密码不可再用于解锁）
    /// - 删除本地全文/向量索引文件（避免残留泄漏）
    pub fn forgot_password_reset(&self, confirmation: &str) -> Result<()> {
        if confirmation != "RESET" {
            return Err(VaultError::InvalidInput(
                "confirmation must be exactly 'RESET'".into(),
            ));
        }
        if self.state() == VaultState::Unlocked {
            return Err(VaultError::InvalidInput(
                "reset requires locked state (lock vault first)".into(),
            ));
        }

        // 保险起见：清零内存密钥
        *self.unlocked.lock().unwrap_or_else(|e| e.into_inner()) = None;

        self.store.wipe_all_user_data()?;

        let ds_path = self.config_dir.join("device.key");
        if ds_path.exists() {
            std::fs::remove_file(&ds_path)?;
        }

        let data_dir = platform::data_dir();
        let tantivy_dir = data_dir.join("tantivy");
        if tantivy_dir.exists() {
            let _ = std::fs::remove_dir_all(&tantivy_dir);
        }
        let vectors = data_dir.join("vectors.encbin");
        if vectors.exists() {
            let _ = std::fs::remove_file(&vectors);
        }

        Ok(())
    }
}

fn sha2_hash(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).to_vec()
}

fn generate_recovery_key() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("ATN-{}-{}", &a[..16], &b[..16]).to_uppercase()
}

/// 跨平台文件权限限制: Unix 设 0600, Windows 设 NTFS ACL 仅当前用户可访问
fn restrict_file_permissions(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(windows)]
    {
        // Windows: 使用 icacls 命令限制文件权限为仅当前用户
        // 等效于: icacls <path> /inheritance:r /grant:r "%USERNAME%:(R,W)"
        let path_str = path.to_string_lossy().to_string();
        let username = std::env::var("USERNAME").unwrap_or_else(|_| "CURRENT_USER".to_string());
        let _ = std::process::Command::new("icacls")
            .args([&path_str, "/inheritance:r", "/grant:r", &format!("{username}:(R,W)")])
            .output(); // 忽略错误: 比不设权限好，但不应阻塞 vault 启动
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_vault() -> (Vault, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let config_dir = tmp.path().join("config");
        let vault = Vault::open(&db_path, &config_dir).unwrap();
        (vault, tmp)
    }

    #[test]
    fn initial_state_is_sealed() {
        let (vault, _tmp) = test_vault();
        assert_eq!(vault.state(), VaultState::Sealed);
    }

    #[test]
    fn setup_transitions_to_unlocked() {
        let (vault, _tmp) = test_vault();
        vault.setup("my-password").unwrap();
        assert_eq!(vault.state(), VaultState::Unlocked);
    }

    #[test]
    fn setup_creates_device_key() {
        let (vault, tmp) = test_vault();
        vault.setup("pw").unwrap();
        let ds_path = tmp.path().join("config/device.key");
        assert!(ds_path.exists());
        assert_eq!(std::fs::read(&ds_path).unwrap().len(), 32);
    }

    #[test]
    fn setup_twice_fails() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        vault.lock().unwrap();
        let result = vault.setup("pw2");
        assert!(matches!(result, Err(VaultError::AlreadyInitialized)));
    }

    #[test]
    fn lock_transitions_to_locked() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        vault.lock().unwrap();
        assert_eq!(vault.state(), VaultState::Locked);
    }

    #[test]
    fn unlock_with_correct_password() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        vault.lock().unwrap();

        let token = vault.unlock("pw").unwrap();
        assert_eq!(vault.state(), VaultState::Unlocked);
        assert!(!token.is_empty());
    }

    #[test]
    fn unlock_with_wrong_password_fails() {
        let (vault, _tmp) = test_vault();
        vault.setup("correct").unwrap();
        vault.lock().unwrap();

        let result = vault.unlock("wrong");
        assert!(result.is_err());
        assert_eq!(vault.state(), VaultState::Locked);
    }

    #[test]
    fn unlock_when_already_unlocked_reissues_token() {
        // 模拟"vault 已解锁但客户端 token 失效"的场景：
        // 同一密码再次 unlock 不应失败，而是签发新 token；MK / 内存密钥保持原样。
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        assert_eq!(vault.state(), VaultState::Unlocked);
        let dek_before = vault.dek_db().unwrap();

        let new_token = vault.unlock("pw").unwrap();
        assert!(!new_token.is_empty(), "new token issued");
        assert_eq!(vault.state(), VaultState::Unlocked, "still unlocked");
        vault.verify_session(&new_token).unwrap();

        // 内存 DEK 未被替换（Drop / Zeroize 不应触发）
        let dek_after = vault.dek_db().unwrap();
        assert_eq!(dek_before.as_bytes(), dek_after.as_bytes());
    }

    #[test]
    fn unlock_when_already_unlocked_wrong_password_fails() {
        // 已解锁状态下用错误密码 unlock 必须失败，且不影响当前会话状态
        let (vault, _tmp) = test_vault();
        vault.setup("correct").unwrap();
        let dek_before = vault.dek_db().unwrap();

        let result = vault.unlock("wrong");
        assert!(result.is_err(), "wrong password rejected");
        assert_eq!(vault.state(), VaultState::Unlocked, "still unlocked");

        // 原 DEK 仍可访问（内存未被破坏）
        let dek_after = vault.dek_db().unwrap();
        assert_eq!(dek_before.as_bytes(), dek_after.as_bytes());
    }

    #[test]
    fn dek_access_requires_unlock() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        vault.lock().unwrap();

        assert!(vault.dek_db().is_err());
        vault.unlock("pw").unwrap();
        assert!(vault.dek_db().is_ok());
    }

    #[test]
    fn dek_consistent_across_lock_unlock() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        let dek1 = vault.dek_db().unwrap();
        vault.lock().unwrap();
        vault.unlock("pw").unwrap();
        let dek2 = vault.dek_db().unwrap();
        assert_eq!(dek1.as_bytes(), dek2.as_bytes(), "DEK should be same after re-unlock");
    }

    #[test]
    fn session_token_valid() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        let token = {
            let guard = vault.unlocked.lock().unwrap();
            let keys = guard.as_ref().unwrap();
            vault.create_session_token(&keys.master_key).unwrap()
        };
        vault.verify_session(&token).unwrap();
    }

    #[test]
    fn session_token_tampered_fails() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();
        let token = {
            let guard = vault.unlocked.lock().unwrap();
            let keys = guard.as_ref().unwrap();
            vault.create_session_token(&keys.master_key).unwrap()
        };
        let tampered = format!("x{token}");
        assert!(vault.verify_session(&tampered).is_err());
    }

    #[test]
    fn change_password_works() {
        let (vault, _tmp) = test_vault();
        vault.setup("old-pw").unwrap();

        // 插入数据
        let dek = vault.dek_db().unwrap();
        let id = vault.store().insert_item(&dek, "Title", "Secret", None, "note", None, None).unwrap();

        // 改密码
        vault.change_password("old-pw", "new-pw").unwrap();
        vault.lock().unwrap();

        // 用旧密码解锁应失败
        assert!(vault.unlock("old-pw").is_err());

        // 用新密码解锁应成功
        vault.unlock("new-pw").unwrap();
        let dek_new = vault.dek_db().unwrap();
        let item = vault.store().get_item(&dek_new, &id).unwrap().unwrap();
        assert_eq!(item.content, "Secret", "Data should survive password change");
    }

    /// Regression test for OSS-S6 (2026-05-02):
    /// change_password 之前不更新内存 MK → reissue_token 签新 MK / verify_session 验旧 MK → 401。
    /// 修复后：change_password 必须同步内存 MK，使后续 reissue → verify 链路正确。
    #[test]
    fn change_password_keeps_session_alive_without_lock_unlock_cycle() {
        let (vault, _tmp) = test_vault();
        vault.setup("old-pw").unwrap();

        // 在 unlocked 状态下改密码（不 lock）
        vault.change_password("old-pw", "new-pw").unwrap();

        // 不做 lock+unlock，直接调 reissue_token（即在 unlocked 状态下 unlock("new-pw")）
        let new_token = vault.unlock("new-pw").expect("reissue_token after change_password");

        // verify_session 必须接受这个 token — 之前会 401 SessionInvalid，因为
        // 内存 MK 还是 old_mk 但 token 用 new_mk 签
        vault.verify_session(&new_token)
            .expect("REGRESSION (OSS-S6): verify_session must accept token issued post-change_password");
    }

    #[test]
    fn export_device_secret_requires_unlocked() {
        let (vault, _tmp) = test_vault();
        // SEALED state
        assert!(vault.export_device_secret().is_err());

        vault.setup("pw").unwrap();
        // UNLOCKED after setup
        let exported = vault.export_device_secret().unwrap();
        assert_eq!(exported.len(), 64); // 32 bytes = 64 hex chars

        vault.lock().unwrap();
        assert!(vault.export_device_secret().is_err());
    }

    #[test]
    fn import_device_secret_writes_file() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();

        let vault = Vault::open(&tmp.path().join("vault.db"), &config_dir).unwrap();
        let hex_secret = "a".repeat(64); // 32 bytes of 0xaa

        vault.import_device_secret(&hex_secret).unwrap();

        let ds_path = config_dir.join("device.key");
        assert!(ds_path.exists());
        let bytes = std::fs::read(&ds_path).unwrap();
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[0], 0xaa);
    }

    #[test]
    fn import_invalid_hex_fails() {
        let tmp = TempDir::new().unwrap();
        let vault = Vault::open(&tmp.path().join("vault.db"), &tmp.path().join("config")).unwrap();
        assert!(vault.import_device_secret("not-hex").is_err());
        assert!(vault.import_device_secret("aa").is_err()); // wrong length
    }

    #[test]
    fn forgot_password_reset_requires_confirmation_and_locked_state() {
        let (vault, _tmp) = test_vault();
        vault.setup("pw").unwrap();

        let err = vault.forgot_password_reset("RESET").unwrap_err();
        assert!(
            matches!(err, VaultError::InvalidInput(_)),
            "unlocked state must be rejected"
        );

        vault.lock().unwrap();
        let err = vault.forgot_password_reset("WRONG").unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[test]
    fn forgot_password_reset_clears_vault_and_returns_to_sealed() {
        let (vault, tmp) = test_vault();
        vault.setup("pw").unwrap();
        let dek = vault.dek_db().unwrap();
        let _ = vault
            .store()
            .insert_item(&dek, "Title", "Secret", None, "note", None, None)
            .unwrap();
        vault.lock().unwrap();

        // 模拟已有索引文件
        vault.forgot_password_reset("RESET").unwrap();

        assert_eq!(vault.state(), VaultState::Sealed);
        assert!(vault.unlock("pw").is_err(), "old password must be unusable");
        assert!(!tmp.path().join("config/device.key").exists());
    }

    #[test]
    fn setup_with_recovery_key_returns_key_and_can_reset_password() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let config_dir = tmp.path().join("config");
        let vault = Vault::open(&db_path, &config_dir).unwrap();

        let recovery_key = vault.setup_with_recovery_key("old-pass").unwrap();
        assert!(recovery_key.starts_with("ATN-"));
        vault.lock().unwrap();

        vault
            .reset_password_with_recovery_key(&recovery_key, "new-pass")
            .unwrap();

        assert!(vault.unlock("old-pass").is_err(), "old password must fail");
        assert!(vault.unlock("new-pass").is_ok(), "new password must work");
    }

    #[test]
    fn reset_password_with_wrong_recovery_key_fails() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let config_dir = tmp.path().join("config");
        let vault = Vault::open(&db_path, &config_dir).unwrap();

        let _recovery_key = vault.setup_with_recovery_key("old-pass").unwrap();
        vault.lock().unwrap();

        let err = vault
            .reset_password_with_recovery_key("ATN-WRONG-KEY", "new-pass")
            .unwrap_err();
        assert!(matches!(err, VaultError::InvalidPassword));
    }

    #[test]
    fn full_lifecycle_encrypted_crud() {
        let (vault, _tmp) = test_vault();
        vault.setup("password123").unwrap();

        let dek = vault.dek_db().unwrap();

        // Insert
        let id = vault.store().insert_item(
            &dek, "My Note", "This is top secret", None, "note", None, None,
        ).unwrap();

        // Get (unlocked)
        let item = vault.store().get_item(&dek, &id).unwrap().unwrap();
        assert_eq!(item.content, "This is top secret");

        // Lock
        vault.lock().unwrap();
        assert!(vault.dek_db().is_err(), "DEK should be inaccessible when locked");

        // Unlock and verify data intact
        vault.unlock("password123").unwrap();
        let dek2 = vault.dek_db().unwrap();
        let item2 = vault.store().get_item(&dek2, &id).unwrap().unwrap();
        assert_eq!(item2.content, "This is top secret");

        // Delete
        assert!(vault.store().delete_item(&id).unwrap());
        assert!(vault.store().get_item(&dek2, &id).unwrap().is_none());
    }

    // ── R15 v0.6.4: vault 并发竞态防御 ────────────────────────────────
    //
    // Vault 内部 store::Store 持有 rusqlite::Connection (含 !Sync 的 RefCell<StatementCache>),
    // 因此 Vault 类型本身 !Sync。生产环境通过 AppState.vault: Mutex<Vault> 串行化访问
    // (rust/crates/attune-server/src/state.rs)。这两个测试模拟该生产模式:
    // Arc<Mutex<Vault>> 多线程争用,验证 lock/unlock 在外部 Mutex 序列化下
    // 保持一致状态 + token 有效。

    #[test]
    fn concurrent_lock_unlock_no_race_via_mutex() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let (vault, _tmp) = test_vault();
        vault.setup("test-password-12345").unwrap();
        let vault = Arc::new(Mutex::new(vault));

        // Note: each unlock invokes argon2id (~1-2s by design); keep iteration count low
        // to keep test under ~30s while still covering races. Threads = 4, ops = 3.
        let mut handles = vec![];
        for thread_id in 0..4 {
            let v = Arc::clone(&vault);
            handles.push(thread::spawn(move || {
                for _ in 0..3 {
                    let g = v.lock().unwrap();
                    if thread_id % 2 == 0 {
                        let _ = g.lock();
                    } else {
                        let _ = g.unlock("test-password-12345");
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("thread didn't panic");
        }

        let g = vault.lock().unwrap();
        let state = g.state();
        assert!(
            matches!(state, VaultState::Locked | VaultState::Unlocked),
            "vault must be in valid state after concurrent ops, got {:?}",
            state
        );
    }

    #[test]
    fn concurrent_unlock_tokens_valid_via_mutex() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let (vault, _tmp) = test_vault();
        vault.setup("test-password-concurrent").unwrap();
        let vault = Arc::new(Mutex::new(vault));
        let tokens: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut handles = vec![];
        for _ in 0..5 {
            let v = Arc::clone(&vault);
            let t = Arc::clone(&tokens);
            handles.push(thread::spawn(move || {
                let g = v.lock().unwrap();
                if let Ok(token) = g.unlock("test-password-concurrent") {
                    t.lock().unwrap().push(token);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let tokens = tokens.lock().unwrap();
        assert!(!tokens.is_empty(), "at least one unlock should succeed");
        for token in tokens.iter() {
            assert!(!token.is_empty(), "token must be non-empty");
            assert!(token.len() >= 32, "token too short: len={}", token.len());
        }
    }
}
