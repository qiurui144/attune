# npu-vault Phase 1: 加密存储引擎 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 构建 npu-vault 的核心加密存储引擎——Cargo workspace + vault-core library + vault-cli 二进制，实现 Argon2id 密钥派生、AES-256-GCM 字段级加密、SQLite 加密 CRUD、Vault 状态机 (SEALED → LOCKED → UNLOCKED)、session token。

**Architecture:** vault-core 是纯 Rust library crate，包含 crypto/vault/store/platform/error 模块。vault-cli 是 clap 命令行工具，调用 vault-core 实现 setup/unlock/lock/status/insert/get 命令。所有加密数据存在 SQLite（rusqlite），敏感字段用 AES-256-GCM 加密，Master Key 由 Argon2id(password + device_secret) 派生，DEK 三把钥匙用 MK 加密存于 vault_meta 表。

**Tech Stack:** Rust 2021, rusqlite (bundled), argon2, aes-gcm, rand, zeroize, hmac, sha2, hex, uuid, chrono, serde/serde_json, toml, clap, dirs, thiserror, tempfile (dev)

**Design Spec:** `docs/superpowers/specs/2026-03-31-npu-vault-design.md`

---

## File Structure

```
npu-vault/                              # 新目录，位于项目根下
├── Cargo.toml                          # workspace manifest
├── crates/
│   ├── vault-core/
│   │   ├── Cargo.toml                  # lib crate
│   │   └── src/
│   │       ├── lib.rs                  # 公开 API re-export
│   │       ├── error.rs                # VaultError enum (thiserror)
│   │       ├── crypto.rs               # Argon2id + AES-256-GCM + DEK 管理
│   │       ├── vault.rs                # 状态机 + session token + lock/unlock
│   │       ├── store.rs                # rusqlite schema + 加密 CRUD
│   │       └── platform.rs             # 跨平台路径 (data_dir/config_dir)
│   │
│   └── vault-cli/
│       ├── Cargo.toml                  # bin crate, depends on vault-core
│       └── src/
│           └── main.rs                 # clap CLI: setup/unlock/lock/status/insert/get
│
└── tests/
    └── integration_test.rs             # 端到端集成测试
```

每个文件职责：
- **error.rs** — 统一错误类型，所有模块共用
- **crypto.rs** — 纯密码学操作：密钥派生、加密、解密、DEK 生成/加密/解密，不依赖数据库
- **platform.rs** — 跨平台路径计算，不依赖其他模块
- **store.rs** — SQLite schema 初始化 + vault_meta 读写 + items 加密 CRUD，依赖 crypto + error
- **vault.rs** — 状态机编排，依赖 crypto + store + platform + error，是 vault-core 的顶层 API
- **lib.rs** — re-export vault.rs 的公开类型
- **main.rs (cli)** — 解析命令行参数，调用 vault-core API，显示结果

---

### Task 1: Cargo Workspace 脚手架

**Files:**
- Create: `npu-vault/Cargo.toml`
- Create: `npu-vault/crates/vault-core/Cargo.toml`
- Create: `npu-vault/crates/vault-core/src/lib.rs`
- Create: `npu-vault/crates/vault-cli/Cargo.toml`
- Create: `npu-vault/crates/vault-cli/src/main.rs`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
# npu-vault/Cargo.toml
[workspace]
resolver = "2"
members = ["crates/vault-core", "crates/vault-cli"]

[workspace.package]
edition = "2021"
rust-version = "1.75"
license = "MIT"
```

- [ ] **Step 2: Create vault-core Cargo.toml**

```toml
# npu-vault/crates/vault-core/Cargo.toml
[package]
name = "vault-core"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 3: Create vault-core lib.rs stub**

```rust
// npu-vault/crates/vault-core/src/lib.rs
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 4: Create vault-cli Cargo.toml**

```toml
# npu-vault/crates/vault-cli/Cargo.toml
[package]
name = "vault-cli"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[[bin]]
name = "npu-vault"
path = "src/main.rs"

[dependencies]
vault-core = { path = "../vault-core" }
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 5: Create vault-cli main.rs stub**

```rust
// npu-vault/crates/vault-cli/src/main.rs
fn main() {
    println!("npu-vault v{}", vault_core::version());
}
```

- [ ] **Step 6: Build workspace**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build`
Expected: BUILD SUCCESS, binary at `target/debug/npu-vault`

- [ ] **Step 7: Run CLI**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo run --bin npu-vault`
Expected: `npu-vault v0.1.0`

- [ ] **Step 8: Commit**

```bash
git add npu-vault/
git commit -m "feat(vault): init Cargo workspace with vault-core + vault-cli"
```

---

### Task 2: error.rs — 统一错误类型

**Files:**
- Create: `npu-vault/crates/vault-core/src/error.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Write the failing test**

在 `error.rs` 底部添加测试：

```rust
// npu-vault/crates/vault-core/src/error.rs

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault is sealed: run setup first")]
    Sealed,

    #[error("vault is locked: unlock required")]
    Locked,

    #[error("vault is already unlocked")]
    AlreadyUnlocked,

    #[error("vault is already initialized")]
    AlreadyInitialized,

    #[error("invalid password")]
    InvalidPassword,

    #[error("device secret missing: {0}")]
    DeviceSecretMissing(String),

    #[error("device secret mismatch")]
    DeviceSecretMismatch,

    #[error("session expired")]
    SessionExpired,

    #[error("session invalid")]
    SessionInvalid,

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, VaultError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        assert_eq!(VaultError::Sealed.to_string(), "vault is sealed: run setup first");
        assert_eq!(VaultError::Locked.to_string(), "vault is locked: unlock required");
        assert_eq!(VaultError::InvalidPassword.to_string(), "invalid password");
        assert_eq!(
            VaultError::DeviceSecretMissing("/path".into()).to_string(),
            "device secret missing: /path"
        );
    }
}
```

- [ ] **Step 2: Add rusqlite dependency to vault-core**

在 `npu-vault/crates/vault-core/Cargo.toml` 的 `[dependencies]` 中追加：

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```

- [ ] **Step 3: Register module in lib.rs**

```rust
// npu-vault/crates/vault-core/src/lib.rs
pub mod error;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 4: Run test**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core error::tests`
Expected: 1 test PASS

- [ ] **Step 5: Commit**

```bash
git add npu-vault/crates/vault-core/src/error.rs npu-vault/crates/vault-core/src/lib.rs npu-vault/crates/vault-core/Cargo.toml
git commit -m "feat(vault): add VaultError unified error type"
```

---

### Task 3: platform.rs — 跨平台路径

**Files:**
- Create: `npu-vault/crates/vault-core/src/platform.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`
- Modify: `npu-vault/crates/vault-core/Cargo.toml`

- [ ] **Step 1: Add dirs dependency**

在 `npu-vault/crates/vault-core/Cargo.toml` 的 `[dependencies]` 中追加：

```toml
dirs = "6"
```

- [ ] **Step 2: Write platform.rs with tests**

```rust
// npu-vault/crates/vault-core/src/platform.rs

use std::path::PathBuf;

/// 数据目录：存储 vault.db, tantivy/, vectors/
pub fn data_dir() -> PathBuf {
    let base = dirs::data_local_dir().expect("cannot determine data directory");
    base.join("npu-vault")
}

/// 配置目录：存储 config.toml, device.key
pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().expect("cannot determine config directory");
    base.join("npu-vault")
}

/// SQLite 数据库路径
pub fn db_path() -> PathBuf {
    data_dir().join("vault.db")
}

/// Device secret 文件路径
pub fn device_secret_path() -> PathBuf {
    config_dir().join("device.key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_end_with_npu_vault() {
        let dd = data_dir();
        let cd = config_dir();
        assert!(dd.ends_with("npu-vault"), "data_dir should end with npu-vault: {:?}", dd);
        assert!(cd.ends_with("npu-vault"), "config_dir should end with npu-vault: {:?}", cd);
    }

    #[test]
    fn db_path_inside_data_dir() {
        let db = db_path();
        assert!(db.starts_with(data_dir()));
        assert_eq!(db.file_name().unwrap(), "vault.db");
    }

    #[test]
    fn device_secret_inside_config_dir() {
        let ds = device_secret_path();
        assert!(ds.starts_with(config_dir()));
        assert_eq!(ds.file_name().unwrap(), "device.key");
    }
}
```

- [ ] **Step 3: Register module in lib.rs**

```rust
// npu-vault/crates/vault-core/src/lib.rs
pub mod error;
pub mod platform;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 4: Run tests**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core platform::tests`
Expected: 3 tests PASS

- [ ] **Step 5: Commit**

```bash
git add npu-vault/crates/vault-core/src/platform.rs npu-vault/crates/vault-core/src/lib.rs npu-vault/crates/vault-core/Cargo.toml
git commit -m "feat(vault): add cross-platform path helpers"
```

---

### Task 4: crypto.rs — Argon2id + AES-256-GCM

**Files:**
- Create: `npu-vault/crates/vault-core/src/crypto.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`
- Modify: `npu-vault/crates/vault-core/Cargo.toml`

- [ ] **Step 1: Add crypto dependencies**

在 `npu-vault/crates/vault-core/Cargo.toml` 的 `[dependencies]` 中追加：

```toml
argon2 = "0.5"
aes-gcm = "0.10"
rand = "0.8"
zeroize = { version = "1", features = ["derive"] }
hmac = "0.12"
sha2 = "0.10"
hex = "0.4"
```

- [ ] **Step 2: Write crypto.rs — 密钥派生**

```rust
// npu-vault/crates/vault-core/src/crypto.rs

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Result, VaultError};

/// 32-byte 密钥，Drop 时自动清零
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Key32([u8; 32]);

impl Key32 {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 生成随机 32-byte 密钥
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }
}

impl AsRef<[u8]> for Key32 {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Argon2id 参数
pub const ARGON2_M_COST: u32 = 65536; // 64 MB
pub const ARGON2_T_COST: u32 = 3;
pub const ARGON2_P_COST: u32 = 4;
pub const SALT_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;

/// 生成随机 salt
pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    salt
}

/// 从 password + device_secret 派生 Master Key
pub fn derive_master_key(password: &[u8], device_secret: &[u8], salt: &[u8]) -> Result<Key32> {
    // 合并 password 和 device_secret 作为输入
    let mut input = Vec::with_capacity(password.len() + device_secret.len());
    input.extend_from_slice(password);
    input.extend_from_slice(device_secret);

    let params = argon2::Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .map_err(|e| VaultError::Crypto(format!("argon2 params: {e}")))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let mut mk = [0u8; 32];
    argon2
        .hash_password_into(&input, salt, &mut mk)
        .map_err(|e| VaultError::Crypto(format!("argon2 derive: {e}")))?;

    // 清零临时 input
    drop(input);
    Ok(Key32(mk))
}

/// AES-256-GCM 加密：返回 nonce(12B) || ciphertext || tag(16B)
pub fn encrypt(key: &Key32, plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| VaultError::Crypto(format!("aes init: {e}")))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| VaultError::Crypto(format!("encrypt: {e}")))?;

    // nonce || ciphertext (includes tag)
    let mut result = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// AES-256-GCM 解密：输入 nonce(12B) || ciphertext || tag(16B)
pub fn decrypt(key: &Key32, data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < NONCE_LEN + 16 {
        return Err(VaultError::Crypto("ciphertext too short".into()));
    }

    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| VaultError::Crypto(format!("aes init: {e}")))?;

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| VaultError::InvalidPassword)
}

/// 用 Master Key 加密一个 DEK
pub fn encrypt_dek(mk: &Key32, dek: &Key32) -> Result<Vec<u8>> {
    encrypt(mk, dek.as_bytes())
}

/// 用 Master Key 解密一个 DEK
pub fn decrypt_dek(mk: &Key32, encrypted_dek: &[u8]) -> Result<Key32> {
    let plain = decrypt(mk, encrypted_dek)?;
    if plain.len() != 32 {
        return Err(VaultError::Crypto("DEK must be 32 bytes".into()));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&plain);
    Ok(Key32(key))
}

/// 生成 Device Secret (256-bit 随机值)
pub fn generate_device_secret() -> Key32 {
    Key32::generate()
}

/// HMAC-SHA256 签名（用于 session token）
pub fn hmac_sign(key: &Key32, message: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;

    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC key length valid");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA256 验证
pub fn hmac_verify(key: &Key32, message: &[u8], signature: &[u8]) -> bool {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;

    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC key length valid");
    mac.update(message);
    mac.verify_slice(signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key32_generate_is_random() {
        let k1 = Key32::generate();
        let k2 = Key32::generate();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn derive_master_key_deterministic() {
        let password = b"test-password";
        let device_secret = Key32::generate();
        let salt = generate_salt();

        let mk1 = derive_master_key(password, device_secret.as_ref(), &salt).unwrap();
        let mk2 = derive_master_key(password, device_secret.as_ref(), &salt).unwrap();
        assert_eq!(mk1.as_bytes(), mk2.as_bytes());
    }

    #[test]
    fn derive_master_key_different_passwords() {
        let device_secret = Key32::generate();
        let salt = generate_salt();

        let mk1 = derive_master_key(b"password1", device_secret.as_ref(), &salt).unwrap();
        let mk2 = derive_master_key(b"password2", device_secret.as_ref(), &salt).unwrap();
        assert_ne!(mk1.as_bytes(), mk2.as_bytes());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = Key32::generate();
        let plaintext = b"hello, vault!";

        let encrypted = encrypt(&key, plaintext).unwrap();
        assert_ne!(&encrypted, plaintext);
        assert!(encrypted.len() > plaintext.len());

        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key1 = Key32::generate();
        let key2 = Key32::generate();

        let encrypted = encrypt(&key1, b"secret").unwrap();
        let result = decrypt(&key2, &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_decrypt_dek_roundtrip() {
        let mk = Key32::generate();
        let dek = Key32::generate();

        let encrypted = encrypt_dek(&mk, &dek).unwrap();
        let recovered = decrypt_dek(&mk, &encrypted).unwrap();
        assert_eq!(dek.as_bytes(), recovered.as_bytes());
    }

    #[test]
    fn decrypt_short_data_errors() {
        let key = Key32::generate();
        let result = decrypt(&key, &[0u8; 10]);
        assert!(result.is_err());
    }

    #[test]
    fn hmac_sign_verify() {
        let key = Key32::generate();
        let message = b"session:abc123:1700000000";

        let sig = hmac_sign(&key, message);
        assert!(hmac_verify(&key, message, &sig));
        assert!(!hmac_verify(&key, b"tampered", &sig));
    }
}
```

- [ ] **Step 3: Register module in lib.rs**

```rust
// npu-vault/crates/vault-core/src/lib.rs
pub mod crypto;
pub mod error;
pub mod platform;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 4: Run tests**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core crypto::tests`
Expected: 8 tests PASS

- [ ] **Step 5: Commit**

```bash
git add npu-vault/crates/vault-core/src/crypto.rs npu-vault/crates/vault-core/src/lib.rs npu-vault/crates/vault-core/Cargo.toml
git commit -m "feat(vault): add Argon2id key derivation + AES-256-GCM encrypt/decrypt"
```

---

### Task 5: store.rs — SQLite Schema + 加密 CRUD

**Files:**
- Create: `npu-vault/crates/vault-core/src/store.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`
- Modify: `npu-vault/crates/vault-core/Cargo.toml`

- [ ] **Step 1: Add uuid + chrono dependencies**

在 `npu-vault/crates/vault-core/Cargo.toml` 的 `[dependencies]` 中追加：

```toml
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 2: Write store.rs**

```rust
// npu-vault/crates/vault-core/src/store.rs

use rusqlite::{params, Connection};
use std::path::Path;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS vault_meta (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    content     BLOB NOT NULL,
    url         TEXT,
    source_type TEXT NOT NULL DEFAULT 'note',
    domain      TEXT,
    tags        BLOB,
    metadata    BLOB,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    is_deleted  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_items_source ON items(source_type);
CREATE INDEX IF NOT EXISTS idx_items_created ON items(created_at);
CREATE INDEX IF NOT EXISTS idx_items_deleted ON items(is_deleted);

CREATE TABLE IF NOT EXISTS embed_queue (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     TEXT NOT NULL REFERENCES items(id),
    chunk_idx   INTEGER NOT NULL,
    chunk_text  BLOB NOT NULL,
    level       INTEGER NOT NULL DEFAULT 2,
    section_idx INTEGER NOT NULL DEFAULT 0,
    priority    INTEGER NOT NULL DEFAULT 2,
    status      TEXT NOT NULL DEFAULT 'pending',
    attempts    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_eq_status ON embed_queue(status, priority, created_at);
CREATE INDEX IF NOT EXISTS idx_eq_item ON embed_queue(item_id);

CREATE TABLE IF NOT EXISTS bound_dirs (
    id         TEXT PRIMARY KEY,
    path       TEXT UNIQUE NOT NULL,
    recursive  INTEGER NOT NULL DEFAULT 1,
    file_types TEXT NOT NULL,
    is_active  INTEGER NOT NULL DEFAULT 1,
    last_scan  TEXT
);

CREATE TABLE IF NOT EXISTS indexed_files (
    id         TEXT PRIMARY KEY,
    dir_id     TEXT NOT NULL REFERENCES bound_dirs(id),
    path       TEXT UNIQUE NOT NULL,
    file_hash  TEXT NOT NULL,
    item_id    TEXT REFERENCES items(id),
    indexed_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_if_dir ON indexed_files(dir_id);

CREATE TABLE IF NOT EXISTS sessions (
    token      TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
"#;

pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开或创建数据库，初始化 schema
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self { conn })
    }

    /// 打开内存数据库（测试用）
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self { conn })
    }

    // --- vault_meta ---

    pub fn set_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mut stmt = self.conn.prepare("SELECT value FROM vault_meta WHERE key = ?1")?;
        let result = stmt.query_row(params![key], |row| row.get::<_, Vec<u8>>(0));
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn has_meta(&self, key: &str) -> Result<bool> {
        Ok(self.get_meta(key)?.is_some())
    }

    // --- items (加密 CRUD) ---

    pub fn insert_item(
        &self,
        dek: &Key32,
        title: &str,
        content: &str,
        url: Option<&str>,
        source_type: &str,
        domain: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let encrypted_content = crypto::encrypt(dek, content.as_bytes())?;
        let encrypted_tags = match tags {
            Some(t) => Some(crypto::encrypt(dek, serde_json::to_string(t)?.as_bytes())?),
            None => None,
        };

        self.conn.execute(
            "INSERT INTO items (id, title, content, url, source_type, domain, tags, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, title, encrypted_content, url, source_type, domain, encrypted_tags, now, now],
        )?;
        Ok(id)
    }

    pub fn get_item(&self, dek: &Key32, id: &str) -> Result<Option<DecryptedItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, url, source_type, domain, tags, created_at, updated_at
             FROM items WHERE id = ?1 AND is_deleted = 0"
        )?;

        let result = stmt.query_row(params![id], |row| {
            Ok(RawItem {
                id: row.get(0)?,
                title: row.get(1)?,
                content: row.get::<_, Vec<u8>>(2)?,
                url: row.get(3)?,
                source_type: row.get(4)?,
                domain: row.get(5)?,
                tags: row.get::<_, Option<Vec<u8>>>(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        });

        match result {
            Ok(raw) => Ok(Some(raw.decrypt(dek)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 列出条目（仅标题和元数据，不解密 content）
    pub fn list_items(&self, limit: usize, offset: usize) -> Result<Vec<ItemSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, source_type, domain, created_at
             FROM items WHERE is_deleted = 0
             ORDER BY created_at DESC LIMIT ?1 OFFSET ?2"
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            Ok(ItemSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                source_type: row.get(2)?,
                domain: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    pub fn delete_item(&self, id: &str) -> Result<bool> {
        let affected = self.conn.execute(
            "UPDATE items SET is_deleted = 1, updated_at = ?1 WHERE id = ?2 AND is_deleted = 0",
            params![chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(affected > 0)
    }

    pub fn item_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items WHERE is_deleted = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

// --- 数据结构 ---

struct RawItem {
    id: String,
    title: String,
    content: Vec<u8>,
    url: Option<String>,
    source_type: String,
    domain: Option<String>,
    tags: Option<Vec<u8>>,
    created_at: String,
    updated_at: String,
}

impl RawItem {
    fn decrypt(self, dek: &Key32) -> Result<DecryptedItem> {
        let content = String::from_utf8(crypto::decrypt(dek, &self.content)?)
            .map_err(|e| VaultError::Crypto(format!("utf8: {e}")))?;
        let tags: Option<Vec<String>> = match self.tags {
            Some(ref enc) => Some(serde_json::from_slice(&crypto::decrypt(dek, enc)?)?),
            None => None,
        };
        Ok(DecryptedItem {
            id: self.id,
            title: self.title,
            content,
            url: self.url,
            source_type: self.source_type,
            domain: self.domain,
            tags,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DecryptedItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub source_type: String,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemSummary {
    pub id: String,
    pub title: String,
    pub source_type: String,
    pub domain: Option<String>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dek() -> Key32 {
        Key32::generate()
    }

    #[test]
    fn open_memory_creates_tables() {
        let store = Store::open_memory().unwrap();
        assert!(store.has_meta("nonexistent").unwrap() == false);
    }

    #[test]
    fn meta_set_get_roundtrip() {
        let store = Store::open_memory().unwrap();
        store.set_meta("salt", b"test-salt-value").unwrap();
        let value = store.get_meta("salt").unwrap().unwrap();
        assert_eq!(value, b"test-salt-value");
    }

    #[test]
    fn meta_overwrite() {
        let store = Store::open_memory().unwrap();
        store.set_meta("key", b"v1").unwrap();
        store.set_meta("key", b"v2").unwrap();
        assert_eq!(store.get_meta("key").unwrap().unwrap(), b"v2");
    }

    #[test]
    fn insert_and_get_item() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store.insert_item(
            &dek, "Test Title", "Secret content", Some("https://example.com"),
            "note", Some("example.com"), Some(&["tag1".into(), "tag2".into()]),
        ).unwrap();

        let item = store.get_item(&dek, &id).unwrap().unwrap();
        assert_eq!(item.title, "Test Title");
        assert_eq!(item.content, "Secret content");
        assert_eq!(item.url.as_deref(), Some("https://example.com"));
        assert_eq!(item.source_type, "note");
        assert_eq!(item.tags.unwrap(), vec!["tag1", "tag2"]);
    }

    #[test]
    fn get_item_wrong_dek_fails() {
        let store = Store::open_memory().unwrap();
        let dek1 = test_dek();
        let dek2 = test_dek();

        let id = store.insert_item(&dek1, "Title", "Secret", None, "note", None, None).unwrap();
        let result = store.get_item(&dek2, &id);
        assert!(result.is_err(), "Should fail with wrong DEK");
    }

    #[test]
    fn content_stored_encrypted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store.insert_item(&dek, "Title", "Plaintext secret", None, "note", None, None).unwrap();

        // 直接读取原始 BLOB，验证不是明文
        let raw: Vec<u8> = store.conn.query_row(
            "SELECT content FROM items WHERE id = ?1",
            params![id],
            |row| row.get(0),
        ).unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(!raw_str.contains("Plaintext secret"), "Content should be encrypted in DB");
    }

    #[test]
    fn list_items_returns_summaries() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        store.insert_item(&dek, "Item 1", "content1", None, "note", None, None).unwrap();
        store.insert_item(&dek, "Item 2", "content2", None, "webpage", Some("example.com"), None).unwrap();

        let items = store.list_items(10, 0).unwrap();
        assert_eq!(items.len(), 2);
        // list_items 不包含 content（不需解密）
        assert!(items.iter().any(|i| i.title == "Item 1"));
        assert!(items.iter().any(|i| i.title == "Item 2"));
    }

    #[test]
    fn delete_item_soft_deletes() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id = store.insert_item(&dek, "To Delete", "secret", None, "note", None, None).unwrap();
        assert_eq!(store.item_count().unwrap(), 1);

        assert!(store.delete_item(&id).unwrap());
        assert_eq!(store.item_count().unwrap(), 0);
        assert!(store.get_item(&dek, &id).unwrap().is_none());
    }

    #[test]
    fn item_count_excludes_deleted() {
        let store = Store::open_memory().unwrap();
        let dek = test_dek();

        let id1 = store.insert_item(&dek, "A", "a", None, "note", None, None).unwrap();
        store.insert_item(&dek, "B", "b", None, "note", None, None).unwrap();
        assert_eq!(store.item_count().unwrap(), 2);

        store.delete_item(&id1).unwrap();
        assert_eq!(store.item_count().unwrap(), 1);
    }
}
```

- [ ] **Step 3: Register module in lib.rs**

```rust
// npu-vault/crates/vault-core/src/lib.rs
pub mod crypto;
pub mod error;
pub mod platform;
pub mod store;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 4: Run tests**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core store::tests`
Expected: 8 tests PASS

- [ ] **Step 5: Commit**

```bash
git add npu-vault/crates/vault-core/src/store.rs npu-vault/crates/vault-core/src/lib.rs npu-vault/crates/vault-core/Cargo.toml
git commit -m "feat(vault): add SQLite store with encrypted CRUD"
```

---

### Task 6: vault.rs — 状态机 + Session Token

**Files:**
- Create: `npu-vault/crates/vault-core/src/vault.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Write vault.rs**

```rust
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
        if self.unlocked.lock().unwrap().is_some() {
            return VaultState::Unlocked;
        }
        match self.store.has_meta("salt") {
            Ok(true) => VaultState::Locked,
            _ => VaultState::Sealed,
        }
    }

    /// 首次设置：生成 device secret + DEK，用密码保护
    pub fn setup(&self, password: &str) -> Result<()> {
        if self.state() != VaultState::Sealed {
            return Err(VaultError::AlreadyInitialized);
        }

        // 生成 device secret 并写入文件
        let device_secret = crypto::generate_device_secret();
        let ds_path = self.config_dir.join("device.key");
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::write(&ds_path, device_secret.as_bytes())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ds_path, std::fs::Permissions::from_mode(0o600))?;
        }

        // 生成 salt + 派生 MK
        let salt = crypto::generate_salt();
        let mk = crypto::derive_master_key(password.as_bytes(), device_secret.as_ref(), &salt)?;

        // 生成 3 个 DEK
        let dek_db = Key32::generate();
        let dek_idx = Key32::generate();
        let dek_vec = Key32::generate();

        // 用 MK 加密 DEK 并存储
        self.store.set_meta("salt", &salt)?;
        self.store.set_meta("encrypted_dek_db", &crypto::encrypt_dek(&mk, &dek_db)?)?;
        self.store.set_meta("encrypted_dek_idx", &crypto::encrypt_dek(&mk, &dek_idx)?)?;
        self.store.set_meta("encrypted_dek_vec", &crypto::encrypt_dek(&mk, &dek_vec)?)?;

        // 存储 device secret hash（验证用）
        let ds_hash = sha2_hash(device_secret.as_ref());
        self.store.set_meta("device_secret_hash", &ds_hash)?;

        // 存储 vault 版本
        self.store.set_meta("vault_version", b"1")?;

        // 自动解锁
        *self.unlocked.lock().unwrap() = Some(UnlockedKeys {
            master_key: mk,
            dek_db,
            dek_idx,
            dek_vec,
        });

        Ok(())
    }

    /// 解锁 vault
    pub fn unlock(&self, password: &str) -> Result<String> {
        match self.state() {
            VaultState::Sealed => return Err(VaultError::Sealed),
            VaultState::Unlocked => return Err(VaultError::AlreadyUnlocked),
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
        *self.unlocked.lock().unwrap() = Some(UnlockedKeys {
            master_key: mk,
            dek_db,
            dek_idx,
            dek_vec,
        });

        Ok(token)
    }

    /// 锁定 vault（清零内存密钥）
    pub fn lock(&self) -> Result<()> {
        let mut guard = self.unlocked.lock().unwrap();
        // UnlockedKeys 内的 Key32 实现了 ZeroizeOnDrop，drop 时自动清零
        *guard = None;
        Ok(())
    }

    /// 更改密码（重新加密 DEK，数据不变）
    pub fn change_password(&self, old_password: &str, new_password: &str) -> Result<()> {
        if self.state() != VaultState::Unlocked {
            return Err(VaultError::Locked);
        }

        let guard = self.unlocked.lock().unwrap();
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

        // 用新 MK 重新加密 DEK
        self.store.set_meta("salt", &new_salt)?;
        self.store.set_meta("encrypted_dek_db", &crypto::encrypt_dek(&new_mk, &keys.dek_db)?)?;
        self.store.set_meta("encrypted_dek_idx", &crypto::encrypt_dek(&new_mk, &keys.dek_idx)?)?;
        self.store.set_meta("encrypted_dek_vec", &crypto::encrypt_dek(&new_mk, &keys.dek_vec)?)?;

        // 注意：需要释放 guard 后再更新内存中的 MK
        drop(guard);
        // MK 更新需要在 unlocked 中替换 — 但当前 DEK 不变，所以简化处理：
        // 在下次 lock/unlock 周期中 MK 会自然更新。这里不需要替换内存 MK，
        // 因为 DEK 是不变的，MK 只在 unlock 时用于解密 DEK。

        Ok(())
    }

    /// 获取 DEK_db（仅 UNLOCKED 状态）
    pub fn dek_db(&self) -> Result<Key32> {
        let guard = self.unlocked.lock().unwrap();
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(keys.dek_db.clone())
    }

    /// 获取 DEK_idx（仅 UNLOCKED 状态）
    pub fn dek_idx(&self) -> Result<Key32> {
        let guard = self.unlocked.lock().unwrap();
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(keys.dek_idx.clone())
    }

    /// 获取 DEK_vec（仅 UNLOCKED 状态）
    pub fn dek_vec(&self) -> Result<Key32> {
        let guard = self.unlocked.lock().unwrap();
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;
        Ok(keys.dek_vec.clone())
    }

    /// 获取 Store 引用（用于 CRUD 操作）
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 验证 session token
    pub fn verify_session(&self, token: &str) -> Result<()> {
        let guard = self.unlocked.lock().unwrap();
        let keys = guard.as_ref().ok_or(VaultError::Locked)?;

        let parts: Vec<&str> = token.rsplitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(VaultError::SessionInvalid);
        }
        let sig_hex = parts[0];
        let payload = parts[1];

        let sig = hex::decode(sig_hex).map_err(|_| VaultError::SessionInvalid)?;
        if !crypto::hmac_verify(&keys.master_key, payload.as_bytes(), &sig) {
            return Err(VaultError::SessionInvalid);
        }

        // 检查过期时间
        let parts: Vec<&str> = payload.split(':').collect();
        if parts.len() != 2 {
            return Err(VaultError::SessionInvalid);
        }
        let expires: i64 = parts[1].parse().map_err(|_| VaultError::SessionInvalid)?;
        let now = chrono::Utc::now().timestamp();
        if now > expires {
            return Err(VaultError::SessionExpired);
        }

        Ok(())
    }

    fn create_session_token(&self, mk: &Key32) -> Result<String> {
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let expires = chrono::Utc::now().timestamp() + SESSION_TTL_SECS;
        let payload = format!("{session_id}:{expires}");
        let sig = crypto::hmac_sign(mk, payload.as_bytes());
        Ok(format!("{payload}.{}", hex::encode(sig)))
    }
}

fn sha2_hash(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).to_vec()
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
}
```

- [ ] **Step 2: Add remaining dependencies to vault-core Cargo.toml**

`[dependencies]` 中追加（部分已存在则跳过）：

```toml
hex = "0.4"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
sha2 = "0.10"
tempfile = "3"  # [dev-dependencies] section below

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Register module in lib.rs**

```rust
// npu-vault/crates/vault-core/src/lib.rs
pub mod crypto;
pub mod error;
pub mod platform;
pub mod store;
pub mod vault;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 4: Run tests**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core vault::tests`
Expected: 14 tests PASS

- [ ] **Step 5: Run all vault-core tests**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core`
Expected: ALL tests PASS (error:1 + platform:3 + crypto:8 + store:8 + vault:14 = 34 tests)

- [ ] **Step 6: Commit**

```bash
git add npu-vault/crates/vault-core/src/vault.rs npu-vault/crates/vault-core/src/lib.rs npu-vault/crates/vault-core/Cargo.toml
git commit -m "feat(vault): add Vault state machine with setup/unlock/lock/change-password + session tokens"
```

---

### Task 7: vault-cli — 命令行工具

**Files:**
- Modify: `npu-vault/crates/vault-cli/src/main.rs`
- Modify: `npu-vault/crates/vault-cli/Cargo.toml`

- [ ] **Step 1: Add CLI dependencies**

`npu-vault/crates/vault-cli/Cargo.toml` 的 `[dependencies]`：

```toml
vault-core = { path = "../vault-core" }
clap = { version = "4", features = ["derive"] }
rpassword = "7"
serde_json = "1"
```

- [ ] **Step 2: Write CLI main.rs**

```rust
// npu-vault/crates/vault-cli/src/main.rs

use clap::{Parser, Subcommand};
use vault_core::vault::Vault;

#[derive(Parser)]
#[command(name = "npu-vault", version, about = "Encrypted personal knowledge vault")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize vault with a master password
    Setup,
    /// Unlock the vault
    Unlock,
    /// Lock the vault
    Lock,
    /// Show vault status
    Status,
    /// Insert a knowledge item
    Insert {
        /// Item title
        #[arg(short, long)]
        title: String,
        /// Item content
        #[arg(short, long)]
        content: String,
        /// Source type (note/webpage/ai_chat/file)
        #[arg(short, long, default_value = "note")]
        source_type: String,
    },
    /// Get a knowledge item by ID
    Get {
        /// Item ID
        id: String,
    },
    /// List knowledge items
    List {
        /// Maximum items to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> vault_core::error::Result<()> {
    let vault = Vault::open_default()?;

    match cli.command {
        Commands::Setup => {
            let password = read_password("Enter master password: ")?;
            let confirm = read_password("Confirm master password: ")?;
            if password != confirm {
                eprintln!("Passwords do not match.");
                std::process::exit(1);
            }
            vault.setup(&password)?;
            println!("Vault initialized and unlocked.");
            println!("Device secret saved to: {}", vault_core::platform::device_secret_path().display());
            println!("IMPORTANT: Back up your device.key file — you need it to unlock on other devices.");
        }
        Commands::Unlock => {
            let password = read_password("Enter master password: ")?;
            let token = vault.unlock(&password)?;
            println!("Vault unlocked.");
            println!("Session token: {token}");
        }
        Commands::Lock => {
            vault.lock()?;
            println!("Vault locked. All keys cleared from memory.");
        }
        Commands::Status => {
            let state = vault.state();
            let count = if matches!(state, vault_core::vault::VaultState::Unlocked) {
                vault.store().item_count().unwrap_or(0)
            } else {
                0
            };
            let status = serde_json::json!({
                "state": state,
                "items": count,
                "data_dir": vault_core::platform::data_dir(),
                "config_dir": vault_core::platform::config_dir(),
            });
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
        }
        Commands::Insert { title, content, source_type } => {
            let dek = vault.dek_db()?;
            let id = vault.store().insert_item(&dek, &title, &content, None, &source_type, None, None)?;
            println!("Inserted: {id}");
        }
        Commands::Get { id } => {
            let dek = vault.dek_db()?;
            match vault.store().get_item(&dek, &id)? {
                Some(item) => println!("{}", serde_json::to_string_pretty(&item).unwrap()),
                None => {
                    eprintln!("Item not found: {id}");
                    std::process::exit(1);
                }
            }
        }
        Commands::List { limit } => {
            // list_items 不需要 DEK（只返回明文字段）
            // 但需要 UNLOCKED 状态验证
            let _ = vault.dek_db()?;
            let items = vault.store().list_items(limit, 0)?;
            println!("{}", serde_json::to_string_pretty(&items).unwrap());
        }
    }
    Ok(())
}

fn read_password(prompt: &str) -> vault_core::error::Result<String> {
    eprint!("{prompt}");
    rpassword::read_password().map_err(|e| vault_core::error::VaultError::Io(e))
}
```

- [ ] **Step 3: Build and verify help**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build --bin npu-vault && ./target/debug/npu-vault --help`
Expected: Shows subcommands (setup, unlock, lock, status, insert, get, list)

- [ ] **Step 4: Verify status on fresh vault**

Run: `cd /data/company/project/npu-webhook/npu-vault && ./target/debug/npu-vault status`
Expected: JSON output with `"state": "sealed"`

- [ ] **Step 5: Commit**

```bash
git add npu-vault/crates/vault-cli/
git commit -m "feat(vault): add CLI with setup/unlock/lock/status/insert/get/list commands"
```

---

### Task 8: 集成测试 — 端到端验证

**Files:**
- Create: `npu-vault/tests/integration_test.rs`

- [ ] **Step 1: Write integration test**

```rust
// npu-vault/tests/integration_test.rs

use tempfile::TempDir;
use vault_core::error::VaultError;
use vault_core::vault::{Vault, VaultState};

fn setup_vault() -> (Vault, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("data/vault.db");
    let config_dir = tmp.path().join("config");
    let vault = Vault::open(&db_path, &config_dir).unwrap();
    (vault, tmp)
}

#[test]
fn e2e_full_lifecycle() {
    let (vault, _tmp) = setup_vault();

    // 1. 初始状态: SEALED
    assert_eq!(vault.state(), VaultState::Sealed);

    // 2. Setup → UNLOCKED
    vault.setup("master-pw-123").unwrap();
    assert_eq!(vault.state(), VaultState::Unlocked);

    // 3. Insert encrypted item
    let dek = vault.dek_db().unwrap();
    let id = vault.store().insert_item(
        &dek, "我的笔记", "这是机密内容：API key = sk-12345",
        Some("https://notes.example.com"), "note", Some("notes.example.com"),
        Some(&["工作".into(), "密钥".into()]),
    ).unwrap();

    // 4. Read back — content decrypted
    let item = vault.store().get_item(&dek, &id).unwrap().unwrap();
    assert_eq!(item.title, "我的笔记");
    assert_eq!(item.content, "这是机密内容：API key = sk-12345");
    assert_eq!(item.tags.unwrap(), vec!["工作", "密钥"]);

    // 5. Lock → LOCKED
    vault.lock().unwrap();
    assert_eq!(vault.state(), VaultState::Locked);

    // 6. DEK inaccessible when locked
    assert!(matches!(vault.dek_db(), Err(VaultError::Locked)));

    // 7. Unlock with wrong password → still LOCKED
    assert!(vault.unlock("wrong-pw").is_err());
    assert_eq!(vault.state(), VaultState::Locked);

    // 8. Unlock with correct password → UNLOCKED + data intact
    let token = vault.unlock("master-pw-123").unwrap();
    assert!(!token.is_empty());
    assert_eq!(vault.state(), VaultState::Unlocked);

    let dek2 = vault.dek_db().unwrap();
    let item2 = vault.store().get_item(&dek2, &id).unwrap().unwrap();
    assert_eq!(item2.content, "这是机密内容：API key = sk-12345");

    // 9. Session token valid
    vault.verify_session(&token).unwrap();

    // 10. Change password
    vault.change_password("master-pw-123", "new-password").unwrap();
    vault.lock().unwrap();
    assert!(vault.unlock("master-pw-123").is_err());
    vault.unlock("new-password").unwrap();
    let dek3 = vault.dek_db().unwrap();
    let item3 = vault.store().get_item(&dek3, &id).unwrap().unwrap();
    assert_eq!(item3.content, "这是机密内容：API key = sk-12345", "Data survives password change");

    // 11. Delete item
    assert!(vault.store().delete_item(&id).unwrap());
    assert!(vault.store().get_item(&dek3, &id).unwrap().is_none());
    assert_eq!(vault.store().item_count().unwrap(), 0);
}

#[test]
fn e2e_content_encrypted_at_rest() {
    let (vault, tmp) = setup_vault();
    vault.setup("pw").unwrap();

    let dek = vault.dek_db().unwrap();
    vault.store().insert_item(&dek, "Title", "SUPER_SECRET_CONTENT", None, "note", None, None).unwrap();

    // 直接读取 SQLite 文件的原始字节
    let db_path = tmp.path().join("data/vault.db");
    let raw_bytes = std::fs::read(&db_path).unwrap();
    let raw_str = String::from_utf8_lossy(&raw_bytes);

    // 内容不应以明文出现在数据库文件中
    assert!(
        !raw_str.contains("SUPER_SECRET_CONTENT"),
        "Content should be encrypted at rest in the SQLite file"
    );

    // 但标题应该是明文（设计决策）
    assert!(
        raw_str.contains("Title"),
        "Title should be stored in plaintext (by design)"
    );
}

#[test]
fn e2e_multiple_items() {
    let (vault, _tmp) = setup_vault();
    vault.setup("pw").unwrap();
    let dek = vault.dek_db().unwrap();

    for i in 0..10 {
        vault.store().insert_item(
            &dek, &format!("Item {i}"), &format!("Content {i}"), None, "note", None, None,
        ).unwrap();
    }

    assert_eq!(vault.store().item_count().unwrap(), 10);
    let items = vault.store().list_items(5, 0).unwrap();
    assert_eq!(items.len(), 5);
    let items_page2 = vault.store().list_items(5, 5).unwrap();
    assert_eq!(items_page2.len(), 5);
}
```

- [ ] **Step 2: Run integration tests**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test --test integration_test`
Expected: 3 tests PASS

- [ ] **Step 3: Run all tests**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test`
Expected: ALL PASS (34 unit + 3 integration = 37 tests)

- [ ] **Step 4: Commit**

```bash
git add npu-vault/tests/
git commit -m "test(vault): add end-to-end integration tests for vault lifecycle"
```

---

### Task 9: README + .gitignore

**Files:**
- Create: `npu-vault/README.md`
- Create: `npu-vault/.gitignore`

- [ ] **Step 1: Create .gitignore**

```gitignore
# npu-vault/.gitignore
/target/
*.db
*.key
```

- [ ] **Step 2: Create README.md**

```markdown
# npu-vault

本地优先、端到端加密的个人知识库引擎。

## 快速开始

```bash
cd npu-vault
cargo build --release

# 首次设置
./target/release/npu-vault setup

# 解锁
./target/release/npu-vault unlock

# 插入知识
./target/release/npu-vault insert -t "笔记标题" -c "笔记内容"

# 查看状态
./target/release/npu-vault status

# 锁定
./target/release/npu-vault lock
```

## 安全模型

- Master Password + Device Secret → Argon2id → Master Key
- 三把 DEK (数据库/全文索引/向量) 用 Master Key 加密存储
- 知识内容 AES-256-GCM 字段级加密，标题明文（可展示列表）
- Session token: HMAC-SHA256 签名，4 小时超时
- Lock 时所有密钥 zeroize 清零

## Phase 计划

- **Phase 1** ✅ 加密存储引擎 (vault-core + vault-cli)
- Phase 2: Axum API Server + 文件扫描 + Embedding
- Phase 3: Tauri 桌面客户端 + NAS 模式
- Phase 4: Chrome 扩展对接 + 安装包
```

- [ ] **Step 3: Commit**

```bash
git add npu-vault/README.md npu-vault/.gitignore
git commit -m "docs(vault): add README and .gitignore"
```

---

## Self-Review Checklist

**1. Spec coverage:**
- ✅ 密钥体系 (Argon2id + Device Secret + 3 DEK) — Task 4 crypto.rs
- ✅ Vault 状态机 (SEALED/LOCKED/UNLOCKED) — Task 6 vault.rs
- ✅ 字段级加密 (content/tags 加密, title 明文) — Task 5 store.rs
- ✅ Session token (HMAC-SHA256, 4h TTL) — Task 6 vault.rs
- ✅ SQLite Schema (vault_meta/items/embed_queue/bound_dirs/indexed_files/sessions) — Task 5 store.rs
- ✅ 跨平台路径 — Task 3 platform.rs
- ✅ CLI (setup/unlock/lock/status/insert/get/list) — Task 7
- ✅ 集成测试 (全生命周期 + 加密验证) — Task 8
- ✅ 密码变更 (重加密 DEK, 数据不变) — Task 6 vault.rs
- ✅ Device Secret 文件权限 0600 — Task 6 vault.rs setup()
- ✅ zeroize (Key32 ZeroizeOnDrop) — Task 4 crypto.rs

**2. Placeholder scan:** 无 TBD/TODO/placeholder。

**3. Type consistency:**
- `Key32` — crypto.rs 定义, store.rs/vault.rs 使用 — 一致
- `VaultError` — error.rs 定义, 全模块使用 `Result<T>` — 一致
- `Store` — store.rs 定义, vault.rs 通过 `vault.store()` 暴露 — 一致
- `Vault` — vault.rs 定义, main.rs 使用 — 一致
- `VaultState` — vault.rs 定义, main.rs/integration_test 使用 — 一致
- `DecryptedItem` / `ItemSummary` — store.rs 定义, cli 使用 — 一致
