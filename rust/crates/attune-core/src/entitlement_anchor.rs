//! Entitlement 签名信任域 anchor + 快照验签(SEC-1,spec §0.2 + T-auth-1)。
//!
//! ## 独立信任域(决策 0.2)
//!
//! cloud entitlement 签名用**独立的 entitlement 签名 keypair**(与插件签名锚
//! [`crate::plugin_anchor`] 物理隔离,私钥不同 KMS 条目)。其公钥作
//! **独立信任域 anchor**——[`ENTITLEMENT_SIGNING_PUBKEYS`]。
//!
//! 为何不并入 plugin anchor:决策 1 消除的是"同一用途(插件签名)的两份信任根"
//! (split-brain)。entitlement 签名是**不同信任域**(license 真实性 vs 插件来源),
//! 合法独立 anchor——两个列表用途互斥、各自 SSOT,无 split-brain。
//!
//! ## SEC-1:吊销逃逸闭合
//!
//! 客户端在 entitlement **转 Active 前**强制验 cloud `/member/verify` 响应的 Ed25519
//! 签名(作用于 canonical `signed_payload`)。验签失败 → strict 不转 Active(走
//! verify-fail→宽限);warn 记警告但容忍(grandfather 老 cloud)。若没有这道签名门,
//! hosts 重定向(零代码)即可伪造 `valid=true active` 复活已吊销 license。
//!
//! ## 机会性 SPKI cert-pin
//!
//! verify 连接复用 slice8 `cert_pin::pinned_client_config()`(已 enforced 于
//! `cloud_client.rs`),对 verify 连接做机会性 SPKI pin —— non-fatal 加固层(pin
//! 集空=disabled,不阻断;真签名验签是 fatal 主防线)。**本模块无需新增 pin 代码**。

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::cloud_client::EntitlementSnapshot;
use crate::plugin_sig::TrustMode;

/// Entitlement 签名公钥 allowlist(独立信任域,**不引用** plugin anchor)。
///
/// 每条是 64-char 小写 hex 的 Ed25519 verifying key。生产值由 cloud v4 entitlement
/// keypair 的公钥填入(随 desktop release ship,编译期 const,运行期不可覆盖,§3.2
/// 防降级)。本 sprint 先留**空占位** + 测试注入(同 `plugin_sig::verify_strict_against_keys`
/// 可注入 keys 内核模式)——cloud v4 上线后填入真值,等价 plugin_sig G1 闭环。
///
/// 轮转:dual-pin 窗口(≤ 3),prepend 新锚 → 发版 → 等升级 → 删旧。
pub const ENTITLEMENT_SIGNING_PUBKEYS: &[&str] = &[
    // cloud v4 entitlement 签名公钥待交付后填入(占位;warn grandfather 桥接跨仓发布顺序)。
];

/// 编译期上限(与 plugin anchor 同构,§4.1 dual-pin ≤ 3)。
pub const MAX_ENTITLEMENT_ANCHORS: usize = 3;

/// 验签内核:用调用方提供的 entitlement 公钥列表校验 canonical signed_payload。
///
/// `verify_loose` 类比:生产用 [`ENTITLEMENT_SIGNING_PUBKEYS`];测试注入测试公钥走通
/// 验签通过路径(内嵌列表是 const 不能运行时改)。返回 `true` 仅当签名由列表中某个
/// 公钥校验通过。
pub fn verify_entitlement_signature(
    signed_payload_canonical: &[u8],
    sig_b64: &str,
    keys: &[&str],
) -> bool {
    let Ok(sig_bytes) = base64::engine::general_purpose::STANDARD.decode(sig_b64.trim()) else {
        return false;
    };
    if sig_bytes.len() != 64 {
        return false;
    }
    let Ok(signature) = Signature::from_slice(&sig_bytes) else {
        return false;
    };
    for pub_hex in keys {
        let Ok(pub_bytes) = hex::decode(pub_hex.trim()) else {
            continue;
        };
        let Ok(pub_arr): std::result::Result<[u8; 32], _> = pub_bytes.as_slice().try_into() else {
            continue;
        };
        let Ok(vk) = VerifyingKey::from_bytes(&pub_arr) else {
            continue;
        };
        if vk.verify(signed_payload_canonical, &signature).is_ok() {
            return true;
        }
    }
    false
}

/// 验签 → 状态决策的接缝结果(纯函数,无锁/无网络/无 DB)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotAuthorization {
    /// 验签通过 —— 携带验签覆盖体的 `status`。**只有此路径**能驱动 T5 转 Active。
    Authorized(String),
    /// warn 下未签名/验签失败 —— 记警告但按业务状态继续(grandfather 老 cloud)。
    AuthorizedWithWarning(String),
    /// strict 下缺签名/验签失败/nonce 不符 —— 不转 Active(走 verify-fail→宽限)。
    Unauthorized(&'static str),
}

/// SEC-1 验签门(纯函数)。strict 缺签名/验签失败 → `Unauthorized`;warn 仅记警告
/// 返 `AuthorizedWithWarning`;验签 ok → `Authorized(signed_payload.status)`。
///
/// **注**:T-auth-2 在 [`crate::entitlement`] 层叠加 nonce 回显 + verified_at 单调校验
/// (此函数只管签名真伪 + 缺签名策略)。本函数不直接信顶层 `valid`/`entitlements`。
pub fn authorize_snapshot(
    resp: &EntitlementSnapshot,
    mode: TrustMode,
    keys: &[&str],
) -> SnapshotAuthorization {
    // 未签名响应(老 cloud)→ warn 容忍 / strict 拒。
    if resp.is_unsigned_response() {
        return match mode {
            TrustMode::Strict => SnapshotAuthorization::Unauthorized("entitlement-unsigned-strict"),
            // off / warn: grandfather. 业务状态从 signed_payload(若有)否则用顶层 plan。
            _ => {
                let status = resp
                    .signed_payload
                    .as_ref()
                    .map(|p| p.status.clone())
                    .unwrap_or_else(|| if resp.valid { "active".into() } else { "suspended".into() });
                SnapshotAuthorization::AuthorizedWithWarning(status)
            }
        };
    }

    // 有签名:验签作用于 canonical signed_payload。
    let payload = resp.signed_payload.as_ref().expect("checked by is_unsigned_response");
    let sig = resp.signature.as_deref().expect("checked by is_unsigned_response");
    let ok = verify_entitlement_signature(&payload.canonical_bytes(), sig, keys);
    if ok {
        SnapshotAuthorization::Authorized(payload.status.clone())
    } else {
        // 验签失败(篡改 / 伪造 / 非 anchor 签名)。
        match mode {
            TrustMode::Strict => SnapshotAuthorization::Unauthorized("entitlement-sig-invalid"),
            _ => SnapshotAuthorization::AuthorizedWithWarning(payload.status.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_client::{EntitlementSnapshot, SignedPayload};
    use ed25519_dalek::{Signer, SigningKey};

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn pubkey_hex(sk: &SigningKey) -> String {
        hex::encode(sk.verifying_key().to_bytes())
    }

    /// Build a snapshot whose signature is produced by `signer` over the canonical
    /// signed_payload. `tamper` mutates the payload AFTER signing if set.
    fn signed_snapshot(
        signer: &SigningKey,
        status: &str,
        nonce: &str,
        tamper: bool,
    ) -> EntitlementSnapshot {
        let payload = SignedPayload {
            status: status.into(),
            allowed_plugins: vec!["law-pro".into()],
            expires_at: Some("2026-12-31T00:00:00+00:00".into()),
            nonce: nonce.into(),
            verified_at: "2026-06-12T00:00:00+00:00".into(),
        };
        let sig = signer.sign(&payload.canonical_bytes());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        let final_payload = if tamper {
            SignedPayload { status: "active".into(), ..payload }
        } else {
            payload
        };
        // Re-serialize through JSON to mimic a real wire response.
        let json = serde_json::json!({
            "valid": true,
            "plan": "pro",
            "entitlement_schema": 1,
            "nonce": nonce,
            "signed_payload": serde_json::to_value(&final_payload).unwrap(),
            "signature": sig_b64,
            "entitlements": [],
            "next_verify_after_hours": 24
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn valid_signature_authorizes() {
        let signer = signing_key(11);
        let keys = [pubkey_hex(&signer)];
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let snap = signed_snapshot(&signer, "active", "n1", false);
        assert_eq!(
            authorize_snapshot(&snap, TrustMode::Strict, &key_refs),
            SnapshotAuthorization::Authorized("active".into())
        );
    }

    #[test]
    fn forged_signature_rejected_strict() {
        // Attacker (non-anchor key) signs an "active" payload → strict Unauthorized.
        let attacker = signing_key(99);
        let official = signing_key(11);
        let keys = [pubkey_hex(&official)]; // attacker NOT in list
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let snap = signed_snapshot(&attacker, "active", "n1", false);
        assert_eq!(
            authorize_snapshot(&snap, TrustMode::Strict, &key_refs),
            SnapshotAuthorization::Unauthorized("entitlement-sig-invalid")
        );
    }

    #[test]
    fn tampered_payload_rejected() {
        // Signature valid for the pre-tamper payload, but payload changed after.
        let signer = signing_key(11);
        let keys = [pubkey_hex(&signer)];
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        // sign "suspended" then flip to "active" → canonical bytes differ → verify fails.
        let snap = signed_snapshot(&signer, "suspended", "n1", true);
        assert_eq!(
            authorize_snapshot(&snap, TrustMode::Strict, &key_refs),
            SnapshotAuthorization::Unauthorized("entitlement-sig-invalid")
        );
    }

    #[test]
    fn missing_signature_warn_tolerated() {
        let json = r#"{"valid": true, "plan": "pro"}"#; // old cloud, no signature
        let snap: EntitlementSnapshot = serde_json::from_str(json).unwrap();
        let auth = authorize_snapshot(&snap, TrustMode::Warn, &[]);
        assert!(matches!(auth, SnapshotAuthorization::AuthorizedWithWarning(_)));
    }

    #[test]
    fn missing_signature_strict_rejected() {
        let json = r#"{"valid": true, "plan": "pro"}"#;
        let snap: EntitlementSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(
            authorize_snapshot(&snap, TrustMode::Strict, &[]),
            SnapshotAuthorization::Unauthorized("entitlement-unsigned-strict")
        );
    }

    #[test]
    fn entitlement_anchor_independent_from_plugin_anchor() {
        // 私钥隔离 / 无 split-brain: entitlement anchor 中没有任何元素是 plugin 信任根
        // (决策 0.2). 用 plugin_anchor::is_official_anchor 检测 membership —— 不直接命名
        // plugin anchor 常量 (本模块的独立信任域不引用插件锚, acceptance grep == 0).
        for ent in ENTITLEMENT_SIGNING_PUBKEYS {
            assert!(
                !crate::plugin_anchor::is_official_anchor(ent),
                "entitlement anchor {ent} must NOT be a plugin trust root (private-key isolation)"
            );
        }
        assert!(ENTITLEMENT_SIGNING_PUBKEYS.len() <= MAX_ENTITLEMENT_ANCHORS);
    }

    /// SEC-1 主断言:revoked 后攻击者重定向到伪造 active 200(非 anchor 签名)→
    /// strict Unauthorized → 不转 Active(吊销逃逸闭合)。
    #[test]
    fn revoked_then_forged_active_rejected_strict() {
        let official = signing_key(11);
        let attacker = signing_key(42);
        let keys = [pubkey_hex(&official)];
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        // Attacker forges an "active" 200 signed with their own (non-anchor) key.
        let forged = signed_snapshot(&attacker, "active", "n-replay", false);
        let auth = authorize_snapshot(&forged, TrustMode::Strict, &key_refs);
        assert_eq!(
            auth,
            SnapshotAuthorization::Unauthorized("entitlement-sig-invalid"),
            "forged active (non-anchor sig) must NOT authorize → cannot revive revoked license"
        );
        assert!(
            !matches!(auth, SnapshotAuthorization::Authorized(_)),
            "no path to Authorized for a forged signature"
        );
    }

    #[test]
    fn verify_entitlement_signature_rejects_bad_base64() {
        assert!(!verify_entitlement_signature(b"payload", "not!!base64", &["abcd"]));
        assert!(!verify_entitlement_signature(b"payload", "c2hvcnQ=", &["abcd"])); // wrong len
    }
}
