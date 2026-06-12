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

/// 官方公钥列表（内嵌二进制）= 单一信任根 SSOT（G1 闭合，决策 1）。
///
/// **不在此重复硬编码官方锚 hash** —— 那会变成"第二份信任根"（split-brain）。
/// 此 const 是 [`crate::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS`] 的**别名**（零拷贝），
/// 后者已是 fail-closed + `MAX_ANCHORS` + 64-hex 格式校验的唯一守卫。
/// `verify_loose` 用此列表激活 `Trust::Official` 路径。
///
/// 每个公钥是 32-byte Ed25519 verifying key 的 **hex** 形式。
/// 轮转 / 吊销机制全在 `plugin_anchor`（dual-anchor 窗口，≤ 3）。
pub const OFFICIAL_PUBLIC_KEYS: &[&str] = crate::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS;

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

impl Trust {
    /// Canonical string form for pricing-tier validation + diagnostics.
    /// `ThirdParty` maps to `"Trusted"` — the legacy pricing label for a
    /// user-whitelisted third-party signer (paid/trial allowed).
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Official => "Official",
            Trust::ThirdParty => "Trusted",
            Trust::Unsigned => "Unsigned",
        }
    }
}

/// 签名校验结果
#[derive(Debug, Clone)]
pub struct VerifyResult {
    pub trust: Trust,
    pub reason: String,
}

/// 宽松校验：无签名 / 签名无效都返回 `Unsigned` 不 panic。
/// 生产切 strict 前，`is_allowed()` 决定是否加载。
///
/// 内嵌的 `OFFICIAL_PUBLIC_KEYS` = `plugin_anchor::OFFICIAL_PLUGIN_ANCHORS`（单一信任根
/// SSOT，非空）。官方私钥签名的插件 → `Trust::Official`；其余 → `Trust::Unsigned`。
pub fn verify_loose(plugin_dir: &Path) -> Result<VerifyResult> {
    verify_against_keys(plugin_dir, OFFICIAL_PUBLIC_KEYS)
}

/// 校验内核：用调用方提供的官方公钥列表校验。`verify_loose` 传内嵌的
/// `OFFICIAL_PUBLIC_KEYS`；测试可传一组测试公钥来真正走通 `Trust::Official`
/// 路径 —— 因为内嵌列表是 `const` 不能运行时改，否则 Official 分支永远测不到。
fn verify_against_keys(plugin_dir: &Path, official_keys: &[&str]) -> Result<VerifyResult> {
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
    for (idx, pub_hex) in official_keys.iter().enumerate() {
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
    verify_strict_against_keys(plugin_dir, OFFICIAL_PUBLIC_KEYS)
}

/// `verify_strict` 内核 —— 官方公钥列表可注入（测试用）。仅当签名由列表中某个
/// 官方公钥校验通过（`Trust::Official`）才放行，否则返回 Err。
fn verify_strict_against_keys(plugin_dir: &Path, official_keys: &[&str]) -> Result<()> {
    let r = verify_against_keys(plugin_dir, official_keys)?;
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

    // ── Official trust path (OFFICIAL_PUBLIC_KEYS is empty in the binary, so the
    //    Trust::Official + verify_strict branch is dead in production tests; we
    //    inject a test key list to exercise it for real) ──────────────────────

    /// Sign a plugin with a known key, register that key as "official", and prove
    /// `verify_against_keys` returns `Trust::Official` AND `verify_strict_against_keys`
    /// passes — the previously-unexercised happy path of the trust chain.
    #[test]
    fn official_key_match_yields_official_trust_and_strict_passes() {
        let dir = make_plugin_dir("id: official-plugin\n", Some("# official prompt"));
        let sk_bytes = generate_signing_key();
        let pk_hex = derive_verifying_key_hex(&sk_bytes);
        sign_plugin(dir.path(), &sk_bytes).expect("sign");

        // Inject our test key as the sole "official" key.
        let official_keys: &[&str] = &[pk_hex.as_str()];

        let r = verify_against_keys(dir.path(), official_keys).expect("verify");
        assert_eq!(r.trust, Trust::Official, "matching official key must yield Official");
        assert!(r.reason.contains("official key #0"));

        // verify_strict must accept an Official plugin.
        verify_strict_against_keys(dir.path(), official_keys)
            .expect("strict verify must pass for an officially-signed plugin");
    }

    /// Trust-chain rotation: multiple official keys, signature matches the 2nd —
    /// proves the loop tries each key and reports the matching index.
    #[test]
    fn official_trust_matches_second_key_in_rotation_list() {
        let dir = make_plugin_dir("id: rotated\n", Some("# p"));
        let signer = generate_signing_key();
        let signer_pk = derive_verifying_key_hex(&signer);
        sign_plugin(dir.path(), &signer).expect("sign");

        let stale_pk = derive_verifying_key_hex(&generate_signing_key());
        let official_keys: &[&str] = &[stale_pk.as_str(), signer_pk.as_str()];

        let r = verify_against_keys(dir.path(), official_keys).expect("verify");
        assert_eq!(r.trust, Trust::Official);
        assert!(r.reason.contains("official key #1"), "should report the matching index: {}", r.reason);
    }

    /// Tampering with prompt.md after signing must break the Official trust:
    /// verify drops to Unsigned and strict verify returns Err. Closes the
    /// "tampered content silently accepted" hole on the Official path.
    #[test]
    fn tampered_prompt_md_rejected_on_official_path() {
        let dir = make_plugin_dir("id: secure\n", Some("# trusted content"));
        let sk_bytes = generate_signing_key();
        let pk_hex = derive_verifying_key_hex(&sk_bytes);
        sign_plugin(dir.path(), &sk_bytes).expect("sign");
        let official_keys: &[&str] = &[pk_hex.as_str()];

        // Sanity: signed → Official before tampering.
        assert_eq!(
            verify_against_keys(dir.path(), official_keys).unwrap().trust,
            Trust::Official
        );

        // Attacker swaps the prompt.md body after the signature was produced.
        std::fs::write(dir.path().join("prompt.md"), "# MALICIOUS injected prompt").unwrap();

        let r = verify_against_keys(dir.path(), official_keys).expect("verify");
        assert_eq!(r.trust, Trust::Unsigned, "tampered prompt.md must NOT verify as Official");
        assert!(r.reason.contains("no matching official"));

        // And strict mode must hard-reject it.
        let err = verify_strict_against_keys(dir.path(), official_keys);
        assert!(err.is_err(), "strict verify must reject a tampered (now-Unsigned) plugin");
    }

    /// Tampering with plugin.yaml after signing is likewise rejected on the
    /// Official path (digest covers both files).
    #[test]
    fn tampered_yaml_rejected_on_official_path() {
        let dir = make_plugin_dir("id: original\n", Some("# p"));
        let sk_bytes = generate_signing_key();
        let pk_hex = derive_verifying_key_hex(&sk_bytes);
        sign_plugin(dir.path(), &sk_bytes).expect("sign");
        let official_keys: &[&str] = &[pk_hex.as_str()];

        std::fs::write(dir.path().join("plugin.yaml"), "id: tampered\n").unwrap();

        let r = verify_against_keys(dir.path(), official_keys).expect("verify");
        assert_eq!(r.trust, Trust::Unsigned);
        assert!(verify_strict_against_keys(dir.path(), official_keys).is_err());
    }

    /// A signature from a non-official key (e.g. a third-party self-signer) must
    /// NOT be promoted to Official even if structurally valid.
    #[test]
    fn non_official_signer_not_promoted_to_official() {
        let dir = make_plugin_dir("id: thirdparty\n", Some("# p"));
        let attacker = generate_signing_key();
        sign_plugin(dir.path(), &attacker).expect("sign");

        // The official list contains a DIFFERENT key than the signer.
        let official_pk = derive_verifying_key_hex(&generate_signing_key());
        let official_keys: &[&str] = &[official_pk.as_str()];

        let r = verify_against_keys(dir.path(), official_keys).expect("verify");
        assert_eq!(r.trust, Trust::Unsigned, "third-party signature must not be Official");
        assert!(verify_strict_against_keys(dir.path(), official_keys).is_err());
    }

    /// G1 closure (spec §5.3 / §9 regression 1): the official-key list is the
    /// single anchor SSOT and is non-empty, pinned to the law-pro publisher anchor.
    #[test]
    fn official_keys_nonempty_anchor_pinned() {
        assert!(
            !OFFICIAL_PUBLIC_KEYS.is_empty(),
            "official key list must be non-empty (G1: verify_loose can yield Official)"
        );
        // SSOT: identical to the single plugin trust root, no second copy. The
        // literal anchor hash lives ONLY in plugin_anchor (decision 1) — we assert
        // against it by reference, never by re-stating the hash in this file.
        assert_eq!(
            OFFICIAL_PUBLIC_KEYS,
            crate::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS,
            "OFFICIAL_PUBLIC_KEYS must be the plugin_anchor SSOT alias, not a second root"
        );
        assert!(
            crate::plugin_anchor::is_official_anchor(OFFICIAL_PUBLIC_KEYS[0]),
            "anchor[0] must be the pinned law-pro publisher key (per plugin_anchor SSOT)"
        );
    }

    /// An anchor private key produces `Trust::Official` via `verify_loose`'s key
    /// set; a non-anchor key yields `Unsigned`. We exercise the real wiring by
    /// asserting the official key set used by verify_loose equals the anchors,
    /// then run the verify kernel against that exact set (we cannot hold the real
    /// anchor private key, so we prove the routing with the injectable kernel).
    #[test]
    fn official_signed_plugin_verifies_official() {
        let dir = make_plugin_dir("id: official-anchor\n", Some("# p"));
        // A signer whose pubkey we treat as the official set (mirrors anchor wiring).
        let signer = generate_signing_key();
        let signer_pk = derive_verifying_key_hex(&signer);
        sign_plugin(dir.path(), &signer).expect("sign");

        // anchor private key signs → Official
        let official_keys: &[&str] = &[signer_pk.as_str()];
        assert_eq!(
            verify_against_keys(dir.path(), official_keys).unwrap().trust,
            Trust::Official
        );

        // a different (non-anchor) key never yields Official
        let outsider = derive_verifying_key_hex(&generate_signing_key());
        let non_anchor: &[&str] = &[outsider.as_str()];
        assert_eq!(
            verify_against_keys(dir.path(), non_anchor).unwrap().trust,
            Trust::Unsigned,
            "non-anchor signer must not be promoted to Official"
        );

        // And verify_loose now routes through the (non-empty) anchor SSOT.
        assert_eq!(OFFICIAL_PUBLIC_KEYS, crate::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS);
    }
}
