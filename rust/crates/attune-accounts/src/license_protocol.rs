//! License protocol — Ed25519-signed license claims + LLM gateway endpoint shape.
//!
//! **Quarantined here (2026-05-20)**: previously lived in `attune-core::license` /
//! `attune-core::license_cache` but were unused by the live cloud-token path
//! (`cloud_client.rs` treats `license_key` as opaque Bearer token; server verifies HMAC).
//! Only this OSS reference SaaS reads/writes this schema, so it lives next to its
//! single consumer instead of polluting `attune-core`.
//!
//! Production accounts services should replace this with their real signing /
//! storage backend; this module exists for the OSS reference deployment and
//! end-to-end tests only.

use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Reference SaaS error — narrow enough for the offline reference flow.
/// (Production SaaS should use its own error taxonomy.)
#[derive(Debug)]
pub struct LicenseError(pub String);

impl std::fmt::Display for LicenseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for LicenseError {}

pub type Result<T> = std::result::Result<T, LicenseError>;

fn err(s: impl Into<String>) -> LicenseError {
    LicenseError(s.into())
}

/// Server-signed license claims (unsigned form).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LicenseClaims {
    /// 唯一 license id (UUID, 防重放)
    pub license_id: String,
    /// 账号 id (用户或团队)
    pub account_id: String,
    /// 计费等级 (free / paid / enterprise)
    pub tier: String,
    /// 最大设备数 (per 协议 1:2)
    pub max_devices: usize,
    /// 月度 LLM token quota (云端 gateway 限额; 0 = 无 LLM 权益)
    pub llm_monthly_quota: u64,
    /// 签发时间 (Unix epoch seconds)
    pub issued_at: i64,
    /// 过期时间 (Unix epoch seconds; 0 = 永不过期, 给企业永久授权用)
    pub expires_at: i64,
    /// 可选元信息 (公司名 / 备注)
    #[serde(default)]
    pub note: String,
}

impl LicenseClaims {
    pub fn is_expired(&self, now_unix: i64) -> bool {
        self.expires_at > 0 && now_unix >= self.expires_at
    }

    /// 序列化为规范字节用于签名 — 字段顺序固定不依赖 serde_json 序列化顺序.
    fn canonical_bytes(&self) -> Vec<u8> {
        let s = format!(
            "v1|{}|{}|{}|{}|{}|{}|{}|{}",
            self.license_id,
            self.account_id,
            self.tier,
            self.max_devices,
            self.llm_monthly_quota,
            self.issued_at,
            self.expires_at,
            self.note
        );
        s.into_bytes()
    }
}

/// 完整签名后的 license — 可序列化为 base64 字符串作为 "license code".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedLicense {
    pub claims: LicenseClaims,
    /// base64 ed25519 signature
    pub signature_b64: String,
}

impl SignedLicense {
    /// 编码为单行 base64 字符串 (CLI / UI 复制粘贴友好)
    pub fn to_code(&self) -> Result<String> {
        let json =
            serde_json::to_vec(self).map_err(|e| err(format!("license serialize: {e}")))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
    }

    /// 从 license code 字符串解码 (不校验签名, 仅 parse)
    pub fn from_code(code: &str) -> Result<Self> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(code.trim())
            .map_err(|e| err(format!("license b64 decode: {e}")))?;
        serde_json::from_slice(&bytes).map_err(|e| err(format!("license json parse: {e}")))
    }

    /// 用 verifying key 校验签名 + 检查过期.
    pub fn verify(&self, verifying_key_hex: &str, now_unix: i64) -> Result<()> {
        let pk_bytes =
            hex::decode(verifying_key_hex).map_err(|e| err(format!("pubkey hex: {e}")))?;
        let pk_arr: [u8; 32] = pk_bytes
            .as_slice()
            .try_into()
            .map_err(|_| err("pubkey must be 32 bytes"))?;
        let vk = VerifyingKey::from_bytes(&pk_arr)
            .map_err(|e| err(format!("bad verifying key: {e}")))?;

        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.signature_b64)
            .map_err(|e| err(format!("signature b64: {e}")))?;
        if sig_bytes.len() != 64 {
            return Err(err("signature must be 64 bytes"));
        }
        let sig = Signature::from_slice(&sig_bytes).map_err(|e| err(format!("signature: {e}")))?;

        let payload = self.claims.canonical_bytes();
        vk.verify(&payload, &sig)
            .map_err(|_| err("signature INVALID"))?;

        if self.claims.is_expired(now_unix) {
            return Err(err(format!(
                "license expired at {} (now {})",
                self.claims.expires_at, now_unix
            )));
        }
        Ok(())
    }
}

/// 服务器侧 API: 用 signing key 签 claims.
pub fn sign_license(claims: LicenseClaims, signing_key_bytes: &[u8; 32]) -> SignedLicense {
    let sk = SigningKey::from_bytes(signing_key_bytes);
    let payload = claims.canonical_bytes();
    let sig = sk.sign(&payload);
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    SignedLicense {
        claims,
        signature_b64: sig_b64,
    }
}

// ── LLM gateway endpoint ──────────────────────────────────

/// 云端 LLM gateway 给客户端返的 endpoint 信息 (用户不直接持 raw OpenAI key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEndpointInfo {
    /// OpenAI 兼容 endpoint URL (云端代理, 内部转发到真实 OpenAI API)
    pub endpoint: String,
    /// 调用时使用的 bearer token (云端发的, 不是真实 OpenAI key)
    pub gateway_token: String,
    /// 推荐模型 (云端按 tier 配置)
    pub default_model: String,
    /// 月度剩余 token (本月已用 = quota - remaining)
    pub remaining_quota: u64,
    /// quota 重置时间 (Unix epoch seconds)
    pub quota_reset_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::plugin_sig::{derive_verifying_key_hex, generate_signing_key};

    fn sample_claims() -> LicenseClaims {
        LicenseClaims {
            license_id: "lic-12345".into(),
            account_id: "acc-1".into(),
            tier: "paid".into(),
            max_devices: 2,
            llm_monthly_quota: 1_000_000,
            issued_at: 1_700_000_000,
            expires_at: 1_700_000_000 + 365 * 86_400,
            note: "Pro 个人版年度".into(),
        }
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        let signed = sign_license(sample_claims(), &sk);
        signed.verify(&pk, 1_700_001_000).expect("verify");
    }

    #[test]
    fn verify_wrong_key_fails() {
        let sk1 = generate_signing_key();
        let signed = sign_license(sample_claims(), &sk1);
        let sk2 = generate_signing_key();
        let pk2 = derive_verifying_key_hex(&sk2);
        assert!(signed.verify(&pk2, 1_700_001_000).is_err());
    }

    #[test]
    fn expired_license_rejected() {
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        let mut claims = sample_claims();
        claims.expires_at = 1_700_000_100;
        let signed = sign_license(claims, &sk);
        // now > expires_at → reject
        let err = signed.verify(&pk, 1_700_000_200).unwrap_err();
        assert!(format!("{err:?}").contains("expired"));
    }

    #[test]
    fn permanent_license_never_expires() {
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        let mut claims = sample_claims();
        claims.expires_at = 0; // 永久授权
        let signed = sign_license(claims, &sk);
        // 远未来仍校验通过
        signed.verify(&pk, 9_999_999_999).expect("permanent");
    }

    #[test]
    fn tampered_claims_fail_verify() {
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        let signed = sign_license(sample_claims(), &sk);
        // 篡改 license code 中的 claims (改 max_devices 后绕过签名 → 应失败)
        let mut tampered = signed.clone();
        tampered.claims.max_devices = 999;
        assert!(tampered.verify(&pk, 1_700_001_000).is_err());
    }

    #[test]
    fn code_roundtrip_base64() {
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        let signed = sign_license(sample_claims(), &sk);
        let code = signed.to_code().expect("encode");
        // license code 必须可粘贴 (无换行 / 不含 /)
        assert!(!code.contains('\n'));
        assert!(!code.contains('/')); // URL_SAFE_NO_PAD 用 - _
        let back = SignedLicense::from_code(&code).expect("decode");
        back.verify(&pk, 1_700_001_000).expect("verify after roundtrip");
    }

    #[test]
    fn empty_signature_rejected() {
        let mut signed = sign_license(sample_claims(), &generate_signing_key());
        signed.signature_b64 = "".into();
        let pk = derive_verifying_key_hex(&[0u8; 32]);
        assert!(signed.verify(&pk, 1_700_001_000).is_err());
    }

    #[test]
    fn license_with_zero_quota_serializes_ok() {
        let sk = generate_signing_key();
        let pk = derive_verifying_key_hex(&sk);
        let mut claims = sample_claims();
        claims.llm_monthly_quota = 0; // 没 LLM 权益
        let signed = sign_license(claims.clone(), &sk);
        signed.verify(&pk, 1_700_001_000).expect("verify");
        // 解析后字段保持
        let code = signed.to_code().unwrap();
        let back = SignedLicense::from_code(&code).unwrap();
        assert_eq!(back.claims.llm_monthly_quota, 0);
    }
}
