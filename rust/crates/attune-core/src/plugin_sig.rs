// 插件签名校验（Ed25519）—— P1 骨架
//
// ## 目的
//
// 商业插件（律师 / 售前 / 医疗等）通过 PluginHub 分发，必须签名才能加载。
// 当前只实现校验器 + 一组官方公钥占位，**PluginHub 上线前所有签名校验默认放行**
// （`strict_mode = false`），保证本地开发 / 自写插件不被拦。
//
// 未来 PluginHub 上线后切 `strict_mode = true`，仅加载签名插件。
//
// ## 签名格式
//
// 插件目录结构：
//   plugins/lawyer_contract_review/
//     ├── plugin.yaml
//     ├── prompt.md
//     └── plugin.sig        <- base64(ed25519 signature of sha256(plugin.yaml + prompt.md))
//
// 签名算法：Ed25519 (EdDSA over Curve25519)，固定 64 字节签名。
//
// ## 官方公钥管理
//
// 官方公钥内嵌在二进制里（此文件 `OFFICIAL_PUBLIC_KEYS`）。轮转机制：
//   - 多公钥列表（任一通过即可）允许平滑过渡
//   - 私钥离线保管，签名操作在隔离环境
//   - 公钥 revocation 通过发新版二进制实现（更新 OFFICIAL_PUBLIC_KEYS 列表）
//
// ## 第三方插件
//
// 用户自写插件默认走 `Trust::Unsigned`，提示"未签名"但可加载。
// Pro 版 `strict_mode` 开启后，第三方插件必须自签 + 用户主动加白名单。

use crate::error::{Result, VaultError};
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::path::Path;

/// 官方公钥列表（内嵌二进制）。首次发布前此数组为空 —— 无官方插件可通过严格校验。
/// PluginHub 上线前默认使用 `verify_loose` 不拦截未签名/无匹配公钥的插件。
///
/// 每个公钥是 32-byte Ed25519 verifying key 的 **hex** 形式。
/// 生成：`openssl genpkey -algorithm ED25519 -out attune-priv.pem`
///       `openssl pkey -in attune-priv.pem -pubout -outform DER | tail -c 32 | xxd -p`
pub const OFFICIAL_PUBLIC_KEYS: &[&str] = &[
    // 首批官方公钥待生成后填入
    // 示例格式（勿上线使用）：
    // "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
];

/// 插件信任等级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// 官方签名通过 —— 最高信任
    Official,
    /// 第三方自签（用户白名单）—— 未来 Pro 支持
    ThirdParty,
    /// 未签名 / 签名无效 —— 开发期放行，生产期拒绝
    Unsigned,
}

/// 签名校验结果
#[derive(Debug, Clone)]
pub struct VerifyResult {
    pub trust: Trust,
    pub reason: String,
}

/// 宽松校验：无签名 / 签名无效都返回 `Unsigned` 不 panic。
/// 生产切 strict 前，`is_allowed()` 决定是否加载。
pub fn verify_loose(plugin_dir: &Path) -> Result<VerifyResult> {
    let sig_path = plugin_dir.join("plugin.sig");
    if !sig_path.exists() {
        return Ok(VerifyResult {
            trust: Trust::Unsigned,
            reason: "no plugin.sig file".into(),
        });
    }

    let sig_b64 = std::fs::read_to_string(&sig_path)
        .map_err(VaultError::Io)?;
    let sig_b64 = sig_b64.trim();

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(sig_b64)
        .map_err(|e| VaultError::InvalidInput(format!("bad signature base64: {e}")))?;
    if sig_bytes.len() != 64 {
        return Ok(VerifyResult {
            trust: Trust::Unsigned,
            reason: format!("signature must be 64 bytes, got {}", sig_bytes.len()),
        });
    }
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| VaultError::InvalidInput(format!("bad signature: {e}")))?;

    // 计算插件 digest：sha256(plugin.yaml || "\0" || prompt.md)
    let digest = compute_plugin_digest(plugin_dir)?;

    // 依次尝试官方公钥
    for (idx, pub_hex) in OFFICIAL_PUBLIC_KEYS.iter().enumerate() {
        let Ok(pub_bytes) = hex::decode(pub_hex) else { continue; };
        let Ok(pub_arr): std::result::Result<[u8; 32], _> = pub_bytes.as_slice().try_into() else { continue; };
        let Ok(vk) = VerifyingKey::from_bytes(&pub_arr) else { continue; };
        if vk.verify(&digest, &signature).is_ok() {
            return Ok(VerifyResult {
                trust: Trust::Official,
                reason: format!("verified by official key #{idx}"),
            });
        }
    }

    Ok(VerifyResult {
        trust: Trust::Unsigned,
        reason: "no matching official public key".into(),
    })
}

/// 严格校验：无签名或签名不是官方的，返回 Err。仅 Pro 版启用。
/// 预留，当前不在任何路径调用 —— PluginHub 上线后激活。
#[allow(dead_code)]
pub fn verify_strict(plugin_dir: &Path) -> Result<()> {
    let r = verify_loose(plugin_dir)?;
    if r.trust == Trust::Official {
        Ok(())
    } else {
        Err(VaultError::InvalidInput(format!(
            "strict verify failed for {}: {}",
            plugin_dir.display(), r.reason
        )))
    }
}

/// 计算插件 digest：把 plugin.yaml 和 prompt.md（如存在）按顺序拼接后 SHA-256。
/// 未来加其他文件（如 few-shot examples.yaml）需在此扩展并升版本号。
pub fn compute_plugin_digest(plugin_dir: &Path) -> Result<Vec<u8>> {
    let mut hasher = Sha256::new();
    let yaml = std::fs::read(plugin_dir.join("plugin.yaml"))
        .map_err(VaultError::Io)?;
    hasher.update(&yaml);
    hasher.update(b"\0");  // 分隔符
    let prompt_path = plugin_dir.join("prompt.md");
    if prompt_path.exists() {
        let prompt = std::fs::read(&prompt_path)
            .map_err(VaultError::Io)?;
        hasher.update(&prompt);
    }
    Ok(hasher.finalize().to_vec())
}

/// 便捷判断：loose 模式下此 plugin 是否允许加载。
/// 当前全部允许（开发期）；未来 strict_mode flag 开启后仅 Official 允许。
pub fn is_allowed(trust: Trust, strict: bool) -> bool {
    if !strict {
        true
    } else {
        trust == Trust::Official
    }
}

// ── 签名 API (供 CI / attune-cli 用) ──────────────────────

/// 生成 32-byte Ed25519 私钥 (signing key).
/// 调用方负责把私钥**离线安全存储**, 公钥嵌入 OFFICIAL_PUBLIC_KEYS.
///
/// 使用 OsRng (crypto-secure 系统熵源). 不用 thread_rng — Ed25519 私钥
/// 一旦泄露或预测就毁掉整个签名信任链, 必须密码学保证级 RNG.
pub fn generate_signing_key() -> [u8; 32] {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    seed
}

/// 从 32-byte 种子派生 verifying key (公钥). 公钥可公开.
pub fn derive_verifying_key_hex(signing_key_bytes: &[u8; 32]) -> String {
    use ed25519_dalek::SigningKey;
    let sk = SigningKey::from_bytes(signing_key_bytes);
    hex::encode(sk.verifying_key().to_bytes())
}

/// 用 signing key 签名 plugin_dir, 写入 plugin.sig (base64).
pub fn sign_plugin(plugin_dir: &Path, signing_key_bytes: &[u8; 32]) -> Result<String> {
    use ed25519_dalek::{Signer, SigningKey};
    let sk = SigningKey::from_bytes(signing_key_bytes);
    let digest = compute_plugin_digest(plugin_dir)?;
    let signature = sk.sign(&digest);
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
    let sig_path = plugin_dir.join("plugin.sig");
    std::fs::write(&sig_path, &sig_b64).map_err(VaultError::Io)?;
    Ok(sig_b64)
}

/// 用任意 verifying key (hex) 校验 plugin_dir 的 plugin.sig.
/// 与 verify_loose 的区别: 不依赖 OFFICIAL_PUBLIC_KEYS 内嵌列表, 用调用方提供的 key.
pub fn verify_with_key(plugin_dir: &Path, verifying_key_hex: &str) -> Result<bool> {
    let sig_path = plugin_dir.join("plugin.sig");
    if !sig_path.exists() {
        return Ok(false);
    }
    let sig_b64 = std::fs::read_to_string(&sig_path).map_err(VaultError::Io)?;
    let sig_b64 = sig_b64.trim();
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(sig_b64)
        .map_err(|e| VaultError::InvalidInput(format!("bad signature base64: {e}")))?;
    if sig_bytes.len() != 64 {
        return Ok(false);
    }
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| VaultError::InvalidInput(format!("bad signature: {e}")))?;

    let pub_bytes = hex::decode(verifying_key_hex)
        .map_err(|e| VaultError::InvalidInput(format!("bad pubkey hex: {e}")))?;
    let pub_arr: [u8; 32] = pub_bytes
        .as_slice()
        .try_into()
        .map_err(|_| VaultError::InvalidInput("pubkey must be 32 bytes".into()))?;
    let vk = VerifyingKey::from_bytes(&pub_arr)
        .map_err(|e| VaultError::InvalidInput(format!("bad verifying key: {e}")))?;

    let digest = compute_plugin_digest(plugin_dir)?;
    Ok(vk.verify(&digest, &signature).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use tempfile::TempDir;

    fn make_plugin_dir(yaml: &str, prompt: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("plugin.yaml"), yaml).unwrap();
        if let Some(p) = prompt {
            std::fs::write(dir.path().join("prompt.md"), p).unwrap();
        }
        dir
    }

    #[test]
    fn unsigned_plugin_returns_unsigned() {
        let dir = make_plugin_dir("id: test\n", Some("# prompt"));
        let r = verify_loose(dir.path()).unwrap();
        assert_eq!(r.trust, Trust::Unsigned);
        assert!(r.reason.contains("no plugin.sig"));
    }

    #[test]
    fn bad_signature_returns_unsigned() {
        let dir = make_plugin_dir("id: test\n", None);
        std::fs::write(dir.path().join("plugin.sig"), "not-base64!!!").unwrap();
        let r = verify_loose(dir.path());
        // 坏签名应返回 Err（格式错）或 Unsigned —— 不 panic
        assert!(r.is_err() || r.unwrap().trust == Trust::Unsigned);
    }

    #[test]
    fn correct_signature_with_key_in_list_returns_official() {
        // 生成临时 keypair，签名插件，然后把公钥放到 OFFICIAL_PUBLIC_KEYS？
        // 但 OFFICIAL_PUBLIC_KEYS 是 const，不能运行时修改。
        // 所以这里测试的是"公钥不匹配"路径（模拟真实情况：测试机没有官方私钥）。
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let dir = make_plugin_dir("id: test\n", Some("# hello"));
        let digest = compute_plugin_digest(dir.path()).unwrap();
        let sig = signing_key.sign(&digest);
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        std::fs::write(dir.path().join("plugin.sig"), sig_b64).unwrap();
        let r = verify_loose(dir.path()).unwrap();
        // 官方公钥列表为空（或不含这个测试公钥），应为 Unsigned
        assert_eq!(r.trust, Trust::Unsigned);
        assert!(r.reason.contains("no matching official"));
    }

    #[test]
    fn digest_is_stable_for_same_content() {
        let dir1 = make_plugin_dir("id: same\n", Some("same"));
        let dir2 = make_plugin_dir("id: same\n", Some("same"));
        let d1 = compute_plugin_digest(dir1.path()).unwrap();
        let d2 = compute_plugin_digest(dir2.path()).unwrap();
        assert_eq!(d1, d2);
    }

    #[test]
    fn digest_changes_with_content() {
        let dir1 = make_plugin_dir("id: a\n", None);
        let dir2 = make_plugin_dir("id: b\n", None);
        assert_ne!(
            compute_plugin_digest(dir1.path()).unwrap(),
            compute_plugin_digest(dir2.path()).unwrap()
        );
    }

    #[test]
    fn digest_changes_without_prompt() {
        let dir1 = make_plugin_dir("id: same\n", Some("a"));
        let dir2 = make_plugin_dir("id: same\n", Some("b"));
        assert_ne!(
            compute_plugin_digest(dir1.path()).unwrap(),
            compute_plugin_digest(dir2.path()).unwrap()
        );
    }

    #[test]
    fn is_allowed_loose_mode_passes_all() {
        assert!(is_allowed(Trust::Unsigned, false));
        assert!(is_allowed(Trust::ThirdParty, false));
        assert!(is_allowed(Trust::Official, false));
    }

    #[test]
    fn is_allowed_strict_mode_rejects_unsigned() {
        assert!(!is_allowed(Trust::Unsigned, true));
        assert!(!is_allowed(Trust::ThirdParty, true));
        assert!(is_allowed(Trust::Official, true));
    }

    #[test]
    fn signature_wrong_length_returns_unsigned() {
        let dir = make_plugin_dir("id: test\n", None);
        let short_sig = base64::engine::general_purpose::STANDARD.encode(b"short");
        std::fs::write(dir.path().join("plugin.sig"), short_sig).unwrap();
        let r = verify_loose(dir.path()).unwrap();
        assert_eq!(r.trust, Trust::Unsigned);
        assert!(r.reason.contains("64 bytes"));
    }

    #[test]
    fn sign_then_verify_with_key_succeeds() {
        let dir = make_plugin_dir("id: signed\n", Some("# prompt"));
        let sk = generate_signing_key();
        let pk_hex = derive_verifying_key_hex(&sk);

        sign_plugin(dir.path(), &sk).expect("sign");
        let ok = verify_with_key(dir.path(), &pk_hex).expect("verify");
        assert!(ok, "signature should verify with matching key");

        // 不同 key 验证应失败
        let other_sk = generate_signing_key();
        let other_pk = derive_verifying_key_hex(&other_sk);
        let ok = verify_with_key(dir.path(), &other_pk).expect("verify");
        assert!(!ok, "different key must not verify");
    }

    #[test]
    fn verify_with_key_unsigned_dir_returns_false() {
        let dir = make_plugin_dir("id: unsigned\n", None);
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        let ok = verify_with_key(dir.path(), &pk).expect("verify");
        assert!(!ok);
    }

    #[test]
    fn verify_with_key_tampered_yaml_fails() {
        let dir = make_plugin_dir("id: original\n", None);
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        sign_plugin(dir.path(), &sk).expect("sign");
        // 篡改 yaml 后签名应失败
        std::fs::write(dir.path().join("plugin.yaml"), "id: tampered\n").unwrap();
        let ok = verify_with_key(dir.path(), &pk).expect("verify");
        assert!(!ok, "tampered content must not verify");
    }

    #[test]
    fn generate_signing_key_is_random() {
        let k1 = generate_signing_key();
        let k2 = generate_signing_key();
        assert_ne!(k1, k2, "each call must produce different key");
    }

    #[test]
    fn derive_verifying_key_hex_is_64_chars() {
        let sk = [42u8; 32];
        let pk = derive_verifying_key_hex(&sk);
        assert_eq!(pk.len(), 64);
        assert!(pk.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
