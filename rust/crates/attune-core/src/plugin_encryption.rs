//! Plugin yaml 加密 — Argon2id 派生密钥 + AES-256-GCM 对称加密.
//!
//! 用于付费 plugin 的 plugin.yaml 加密分发. 装载时由 plugin_loader 解密.
//! 复用 vault.rs 的 Argon2id 参数 (与用户主密钥派生兼容).

use crate::error::{Result, VaultError};
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const MAGIC: &[u8] = b"ATTPKGE1"; // attune-pkg encrypted v1

/// 加密 plugin.yaml 明文 → 字节流 (含 magic + salt + nonce + ciphertext).
///
/// `password`: 通常是 plugin 装载时验证的设备绑定密钥 (跨 device 不通用),
/// 或商业分发的 plugin license token.
pub fn encrypt_yaml(plaintext: &[u8], password: &[u8]) -> Result<Vec<u8>> {
    // OsRng (crypto-secure, 系统熵源) — 不用 thread_rng (非密码学保证).
    // salt + nonce 都是加密关键参数, 必须用 OsRng.
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce_bytes);

    let key = derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| VaultError::Crypto(format!("aes-gcm encrypt: {e}")))?;

    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 解密
pub fn decrypt_yaml(encrypted: &[u8], password: &[u8]) -> Result<Vec<u8>> {
    if encrypted.len() < MAGIC.len() + SALT_LEN + NONCE_LEN {
        return Err(VaultError::Crypto("ciphertext too short".into()));
    }
    if &encrypted[..MAGIC.len()] != MAGIC {
        return Err(VaultError::Crypto(
            "bad magic — not an attune encrypted plugin yaml".into(),
        ));
    }
    let salt = &encrypted[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce_bytes = &encrypted[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
    let ciphertext = &encrypted[MAGIC.len() + SALT_LEN + NONCE_LEN..];

    let key = derive_key(password, salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| VaultError::Crypto(format!("aes-gcm decrypt: {e}")))
}

/// 派生 32 字节 AES key, Argon2id 参数与 vault 一致.
fn derive_key(password: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(19_456, 2, 1, Some(KEY_LEN))
        .map_err(|e| VaultError::Crypto(format!("argon2 params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(password, salt, &mut key)
        .map_err(|e| VaultError::Crypto(format!("argon2 derive: {e}")))?;
    Ok(key)
}

/// 检查 paid/trial plugin 必须 Official/ThirdParty(Trusted) trust 级别.
/// 联动: pricing.tier == "paid" || "trial" → trust ∈ {Official, ThirdParty}.
///
/// T2 (G2): trust 由 `&str` 魔法串改为类型安全的 [`crate::plugin_sig::Trust`] enum —
/// 调用方再也无法传任意字符串("Official"/"Trusted")绕过验证。
pub fn validate_trust_for_pricing(
    trust: crate::plugin_sig::Trust,
    pricing_tier: &str,
) -> Result<()> {
    use crate::plugin_sig::Trust;
    let is_paid = matches!(pricing_tier, "paid" | "trial");
    if is_paid && !matches!(trust, Trust::Official | Trust::ThirdParty) {
        return Err(VaultError::Crypto(format!(
            "paid/trial plugin must be Official or Trusted, got '{}'",
            trust.as_str()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"id: law-pro\npricing:\n  tier: paid\n";
        let password = b"device-secret-1234567890";
        let ciphertext = encrypt_yaml(plaintext, password).expect("encrypt");
        assert_ne!(&ciphertext[..], &plaintext[..]);
        assert!(ciphertext.starts_with(MAGIC));
        let recovered = decrypt_yaml(&ciphertext, password).expect("decrypt");
        assert_eq!(&recovered[..], &plaintext[..]);
    }

    #[test]
    fn wrong_password_fails() {
        let plaintext = b"secret content";
        let ciphertext = encrypt_yaml(plaintext, b"correct-pw").expect("encrypt");
        let result = decrypt_yaml(&ciphertext, b"wrong-pw");
        assert!(result.is_err());
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bad = vec![b'X'; 64];
        bad[0..4].copy_from_slice(b"FAKE");
        let result = decrypt_yaml(&bad, b"any-pw");
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("magic"));
    }

    #[test]
    fn short_ciphertext_rejected() {
        let result = decrypt_yaml(b"short", b"pw");
        assert!(result.is_err());
    }

    #[test]
    fn each_encryption_uses_fresh_salt_and_nonce() {
        let pt = b"same plaintext";
        let pw = b"same password";
        let c1 = encrypt_yaml(pt, pw).expect("e1");
        let c2 = encrypt_yaml(pt, pw).expect("e2");
        assert_ne!(c1, c2, "ciphertexts must differ even for same input");
    }

    use crate::plugin_sig::Trust;

    #[test]
    fn validate_trust_for_pricing_passes_paid_with_official() {
        assert!(validate_trust_for_pricing(Trust::Official, "paid").is_ok());
        assert!(validate_trust_for_pricing(Trust::ThirdParty, "trial").is_ok());
    }

    #[test]
    fn validate_trust_for_pricing_rejects_paid_unsigned() {
        let err = validate_trust_for_pricing(Trust::Unsigned, "paid").unwrap_err();
        let s = format!("{err:?}");
        assert!(s.contains("paid/trial"));
    }

    #[test]
    fn validate_trust_for_pricing_allows_free_unsigned() {
        // free plugin 可任意 trust 级别
        assert!(validate_trust_for_pricing(Trust::Unsigned, "free").is_ok());
        assert!(validate_trust_for_pricing(Trust::ThirdParty, "free").is_ok());
    }
}
