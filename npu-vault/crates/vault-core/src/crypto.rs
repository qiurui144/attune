use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{Result, VaultError};

/// 32-byte key, auto-zeroed on Drop
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Key32([u8; 32]);

impl Key32 {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

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

pub const ARGON2_M_COST: u32 = 65536; // 64 MB
pub const ARGON2_T_COST: u32 = 3;
pub const ARGON2_P_COST: u32 = 4;
pub const SALT_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;

pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    salt
}

pub fn derive_master_key(password: &[u8], device_secret: &[u8], salt: &[u8]) -> Result<Key32> {
    let mut input: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(password.len() + device_secret.len()));
    input.extend_from_slice(password);
    input.extend_from_slice(device_secret);

    let params = argon2::Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .map_err(|e| VaultError::Crypto(format!("argon2 params: {e}")))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let mut mk = [0u8; 32];
    argon2
        .hash_password_into(&input, salt, &mut mk)
        .map_err(|e| VaultError::Crypto(format!("argon2 derive: {e}")))?;

    drop(input);
    Ok(Key32(mk))
}

pub fn encrypt(key: &Key32, plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| VaultError::Crypto(format!("aes init: {e}")))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| VaultError::Crypto(format!("encrypt: {e}")))?;

    let mut result = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

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

pub fn encrypt_dek(mk: &Key32, dek: &Key32) -> Result<Vec<u8>> {
    encrypt(mk, dek.as_bytes())
}

pub fn decrypt_dek(mk: &Key32, encrypted_dek: &[u8]) -> Result<Key32> {
    let plain = decrypt(mk, encrypted_dek)?;
    if plain.len() != 32 {
        return Err(VaultError::Crypto("DEK must be 32 bytes".into()));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&plain);
    Ok(Key32(key))
}

pub fn generate_device_secret() -> Key32 {
    Key32::generate()
}

pub fn hmac_sign(key: &Key32, message: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;

    let mut mac = <HmacSha256 as Mac>::new_from_slice(key.as_bytes()).expect("HMAC key length valid");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

pub fn hmac_verify(key: &Key32, message: &[u8], signature: &[u8]) -> bool {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;

    let mut mac = <HmacSha256 as Mac>::new_from_slice(key.as_bytes()).expect("HMAC key length valid");
    mac.update(message);
    mac.verify_slice(signature).is_ok()
}

/// Save bytes to an encrypted file
pub fn save_encrypted_file(key: &Key32, path: &std::path::Path, plaintext: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let encrypted = encrypt(key, plaintext)?;
    std::fs::write(path, &encrypted)?;
    Ok(())
}

/// Load and decrypt bytes from an encrypted file
pub fn load_encrypted_file(key: &Key32, path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read(path)?;
    let decrypted = decrypt(key, &data)?;
    Ok(Some(decrypted))
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
    fn save_and_load_encrypted_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("secret.enc");
        let key = Key32::generate();
        let plaintext = b"secret data";

        save_encrypted_file(&key, &path, plaintext).unwrap();
        assert!(path.exists());

        let loaded = load_encrypted_file(&key, &path).unwrap().unwrap();
        assert_eq!(loaded, plaintext);

        // wrong key fails
        let wrong = Key32::generate();
        assert!(load_encrypted_file(&wrong, &path).is_err());
    }

    #[test]
    fn load_missing_file_returns_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.enc");
        let key = Key32::generate();
        let result = load_encrypted_file(&key, &path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn hmac_sign_verify() {
        let key = Key32::generate();
        let message = b"session:abc123:1700000000";

        let sig = hmac_sign(&key, message);
        assert!(hmac_verify(&key, message, &sig));
        assert!(!hmac_verify(&key, b"tampered", &sig));
    }

    #[test]
    fn derive_master_key_compiles_with_zeroizing() {
        let password = b"test_password_123";
        let device_secret = [0u8; 32];
        let salt = [0u8; 32];
        // 确认函数可正常调用，中间 Vec 使用 Zeroizing 不影响结果
        let result = derive_master_key(password, &device_secret, &salt);
        assert!(result.is_ok());
    }
}
