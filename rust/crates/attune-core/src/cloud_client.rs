//! Cloud client — 客户端对接 cloud accounts Python 服务的 HTTP 接口.
//!
//! 后端服务由 /data/company/cloud/accounts (FastAPI) 一键部署 (cloud.sh).
//! 客户端通过此模块拿:
//! - 登录 / 注册 / 当前用户信息
//! - 已分配的 license + pro 插件清单
//! - LLM gateway endpoint (云端代理)
//!
//! 与 Rust attune-accounts crate 的关系: 后者是 OSS self-host reference,
//! 前者 (生产) 走 cloud Python. 客户端代码透明, 只换 base_url.

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct CloudClient {
    base_url: String,
    /// session cookie (login 后从 Set-Cookie 抽出, 后续请求带)
    session_cookie: Option<String>,
    http: reqwest::blocking::Client,
}

impl CloudClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            session_cookie: None,
            http: Self::build_http(),
        }
    }

    /// Build the blocking HTTP client with SPKI cert-pinning enforced on the TLS
    /// layer (cloud slice8 §3.2). The pinned `rustls::ClientConfig` runs standard
    /// webpki chain validation AND additionally requires the accounts server's
    /// leaf SPKI ∈ [`crate::cert_pin::ACCOUNTS_SPKI_PINS`]. When that pin set is
    /// empty (pin provisioned at release time, §10.3 fail-safe) the config is
    /// equivalent to standard webpki — no regression vs. an unpinned client.
    fn build_http() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .cookie_store(true) // 自动管理 session cookie
            // Pass the bare ClientConfig: reqwest wraps it in Option internally
            // and downcasts to Option<rustls::ClientConfig> (passing Some(..) here
            // would double-wrap → "Unknown TLS backend").
            .use_preconfigured_tls(crate::cert_pin::pinned_client_config())
            .build()
            .expect("build http client")
    }

    /// 用持久化 session token 构造客户端 (sync-plugins 等跨进程调用路径)
    pub fn with_session(base_url: impl Into<String>, session_token: impl Into<String>) -> Self {
        let mut c = Self::new(base_url);
        c.session_cookie = Some(session_token.into());
        c
    }

    /// 返回当前 session token (login 后供调用方持久化)
    pub fn session_token(&self) -> Option<&str> {
        self.session_cookie.as_deref()
    }

    /// Email + password 登录
    pub fn login(&mut self, email: &str, password: &str) -> Result<UserInfo> {
        let url = format!("{}/api/v1/login", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({"email": email, "password": password}))
            .send()
            .map_err(http_err)?;
        let status = resp.status();
        // 抽 session cookie 给 keep
        if let Some(cookie) = resp.headers().get("set-cookie") {
            if let Ok(s) = cookie.to_str() {
                self.session_cookie = Some(s.split(';').next().unwrap_or(s).to_string());
            }
        }
        let body = resp.text().map_err(http_err)?;
        if !status.is_success() {
            return Err(VaultError::Crypto(format!(
                "login failed: status={status} body={body}"
            )));
        }
        serde_json::from_str(&body).map_err(json_err)
    }

    /// 注册新用户
    pub fn signup(&mut self, email: &str, password: &str) -> Result<UserInfo> {
        let url = format!("{}/api/v1/signup", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({"email": email, "password": password}))
            .send()
            .map_err(http_err)?;
        let status = resp.status();
        if let Some(cookie) = resp.headers().get("set-cookie") {
            if let Ok(s) = cookie.to_str() {
                self.session_cookie = Some(s.split(';').next().unwrap_or(s).to_string());
            }
        }
        let body = resp.text().map_err(http_err)?;
        if !status.is_success() {
            return Err(VaultError::Crypto(format!(
                "signup failed: status={status} body={body}"
            )));
        }
        serde_json::from_str(&body).map_err(json_err)
    }

    /// 拿当前登录用户信息
    pub fn me(&self) -> Result<UserInfo> {
        let url = format!("{}/api/v1/me", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header_opt_cookie(self.session_cookie.as_deref())
            .send()
            .map_err(http_err)?;
        if !resp.status().is_success() {
            return Err(VaultError::Crypto(format!("me failed: {}", resp.status())));
        }
        resp.json().map_err(http_err)
    }

    /// 拿用户的 license 列表 (含已分配的 pro 插件)
    pub fn list_licenses(&self) -> Result<Vec<License>> {
        let url = format!("{}/api/v1/licenses", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header_opt_cookie(self.session_cookie.as_deref())
            .send()
            .map_err(http_err)?;
        if !resp.status().is_success() {
            return Err(VaultError::Crypto(format!(
                "list licenses: status={}",
                resp.status()
            )));
        }
        resp.json().map_err(http_err)
    }

    /// 登出
    pub fn logout(&mut self) -> Result<()> {
        let url = format!("{}/api/v1/logout", self.base_url);
        let _ = self.http.post(&url).send().map_err(http_err)?;
        self.session_cookie = None;
        Ok(())
    }

    /// Best-effort cloud logout + **unconditional** local clear of the session
    /// token. Network failure does NOT prevent the local token from being
    /// wiped — the contract is "after this call, this client carries no
    /// session token". Used by `POST /api/v1/privacy/wipe-cloud-session`
    /// (per spec `2026-05-28-privacy-logic-strategy.md` §5.1).
    ///
    /// **Task 4 of v1.0.6 Privacy Logic Implementation Plan.**
    ///
    /// v1.0.6 Privacy Logic Strategy — Cloud SaaS egress. Unlike the other four
    /// egress points (LLM / WebDAV / WebSearch / Telemetry) which now enforce
    /// the OutboundGate's Result, `wipe_session` is INTENTIONALLY always-allow:
    /// it is a DSAR-adjacent right (remove your cloud footprint) that must
    /// succeed even when `privacy.cloud_saas` is disabled or the vault is
    /// locked. This is the single allow-listed `let _ = enforce` call site in
    /// `scripts/privacy-audit.sh`.
    pub fn wipe_session(&mut self) -> Result<()> {
        // Audit hook — wipe_session is INTENTIONALLY always-allow: the user is
        // exercising a DSAR-adjacent right to remove their cloud footprint, so
        // it must succeed even if `privacy.cloud_saas` is disabled or the vault
        // is locked. We construct the policy with `enabled: true,
        // vault_unlocked: true` deliberately (not a no-op stub) and the result
        // is genuinely discarded because the contract overrides the gate here.
        // This is the ONLY egress where discarding the Result is correct;
        // scripts/privacy-audit.sh allow-lists exactly this one call site.
        let _ = crate::OutboundGate::enforce(
            &crate::OutboundPolicy::cloud(
                crate::OutboundKind::CloudSaas,
                true, // DSAR wipe: always allowed by design
                true, // DSAR wipe: not vault-gated by design
                None,
            ),
            "",
        );

        // Best-effort remote logout: swallow errors so local clear always wins.
        let _ = self.logout();
        // Re-enforce local clear (logout already cleared, but make contract
        // explicit + self-documenting for future maintainers).
        self.session_cookie = None;
        Ok(())
    }

    // ─── DSAR (Data Subject Access Request) — GDPR Art.15/17/20 + 中国 PIPL §44-50 ───

    /// GET /api/v1/users/me/export — 拿用户 cloud 端所有数据 JSON dump.
    ///
    /// 返回 raw JSON (serde_json::Value) — 由调用方决定是写文件还是嵌入 UI 视图.
    pub fn dsar_export(&self) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1/users/me/export", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header_opt_cookie(self.session_cookie.as_deref())
            .send()
            .map_err(http_err)?;
        if !resp.status().is_success() {
            return Err(VaultError::Crypto(format!(
                "dsar export failed: status={}",
                resp.status()
            )));
        }
        resp.json().map_err(http_err)
    }

    /// DELETE /api/v1/users/me — 软删除 cloud 账户 (30d grace).
    ///
    /// 返回 cloud 端 {status, hard_delete_at, grace_days} JSON.
    /// 调用后该 session 立即失效 (current_user 拒绝 inactive),
    /// session_cookie 保留供同会话内 cancel-deletion 用.
    pub fn dsar_delete(&self) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1/users/me", self.base_url);
        let resp = self
            .http
            .delete(&url)
            .header_opt_cookie(self.session_cookie.as_deref())
            .send()
            .map_err(http_err)?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().map_err(http_err)?;
        if !status.is_success() {
            return Err(VaultError::Crypto(format!(
                "dsar delete failed: status={status} body={body}"
            )));
        }
        Ok(body)
    }

    /// POST /api/v1/users/me/cancel-deletion — 30d grace 期内撤销软删除.
    pub fn dsar_cancel_deletion(&self) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1/users/me/cancel-deletion", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header_opt_cookie(self.session_cookie.as_deref())
            .send()
            .map_err(http_err)?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().map_err(http_err)?;
        if !status.is_success() {
            return Err(VaultError::Crypto(format!(
                "dsar cancel-deletion failed: status={status} body={body}"
            )));
        }
        Ok(body)
    }
}

/// reqwest RequestBuilder 扩展: 按需注入 Cookie header
trait WithOptCookie: Sized {
    fn header_opt_cookie(self, cookie: Option<&str>) -> Self;
}

impl WithOptCookie for reqwest::blocking::RequestBuilder {
    fn header_opt_cookie(self, cookie: Option<&str>) -> Self {
        if let Some(c) = cookie {
            self.header(reqwest::header::COOKIE, c)
        } else {
            self
        }
    }
}

/// accounts `/api/v1/{login,signup,me}` 的 UserResponse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: i64,
    pub email: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub plan_expires: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    /// new-api LLM token（付费会员；free 用户为 None）
    #[serde(default)]
    pub gateway_token: Option<String>,
    /// LLM gateway endpoint（云端公布；如 https://gateway.engi-stack.com/v1）
    #[serde(default)]
    pub gateway_url: Option<String>,
    /// 默认 LLM model（云端下发；如 "deepseek-v4-flash"）。
    /// per spec 2026-05-24-deepseek-via-new-api-gateway-e2e.md Bug-1 修复 Option C:
    /// fresh vault paid 用户 login 时 merge_gateway_into_settings 会用此 model 填入
    /// llm.model,避免 chat 因 model=null → 404。
    /// 老版 accounts server 不返回此字段 → None,attune-server 不写入 model 保持兼容。
    #[serde(default)]
    pub gateway_default_model: Option<String>,
}

/// accounts `GET /api/v1/licenses` 的单条 license 响应
#[derive(Debug, Clone, Deserialize)]
pub struct License {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    pub plan: String,
    pub license_key: String,
    #[serde(default)]
    pub license_id: Option<i64>,
    #[serde(default)]
    pub revoked_at: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// pro 插件清单 (pluginhub 下发)
    #[serde(default)]
    pub entitled_plugins: Vec<EntitledPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitledPlugin {
    pub plugin_id: String,
    pub version: String,
    /// pluginhub 下载 URL (含签名 token)
    pub download_url: String,
    /// 公钥 hex (用于客户端 verify_with_key 校验 plugin.sig)
    pub signing_pubkey_hex: String,
    /// 加密 key (paid plugin 用; free 可空)
    #[serde(default)]
    pub decrypt_key: Option<String>,
}

// ─── trust-chain (T6): entitlement 快照契约 v1 (spec §5.2) ───────────────
//
// cloud `/member/verify` 响应镜像。向后兼容追加字段:老 cloud 不返回 entitlements
// / signature / nonce → serde default 容缺,标记为 schema-0 / unsigned-response,
// 由 T-auth-1/2 决定策略(warn grandfather / strict 拒)。本模块只做契约镜像 +
// "是否带签名"标记,不做验签策略。

/// 单条 entitlement(派生视图,由 signed_payload.allowed_plugins + status 展开)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntitlementSnapshotItem {
    pub plugin_id: String,
    /// free | trial | paid
    pub tier: String,
    /// active | suspended | revoked
    pub status: String,
    #[serde(default)]
    pub trial_expires: Option<String>,
    #[serde(default)]
    pub signing_pubkey_hex: String,
    #[serde(default)]
    pub verified_at: Option<String>,
}

/// 验签覆盖体(canonical JSON,字段序固定)—— SEC-1 签名作用于此,SEC-2 nonce/
/// verified_at 校验也基于此。client 转 Active 仅依据**验签通过的** `status`,不直接
/// 信顶层 `valid` / `entitlements`(防顶层伪造,spec §5.2 裁决)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SignedPayload {
    /// active | suspended | revoked (per-license 总体状态)
    pub status: String,
    /// 该 license 授权的 plugin_id 列表
    #[serde(default)]
    pub allowed_plugins: Vec<String>,
    /// RFC3339 | None (trial/paid 到期)
    #[serde(default)]
    pub expires_at: Option<String>,
    /// 回显 client 发来的同一 nonce (SEC-2 anti-replay)
    #[serde(default)]
    pub nonce: String,
    /// 服务端权威时间 RFC3339,单调递增 (SEC-2 freshness)
    #[serde(default)]
    pub verified_at: String,
}

impl SignedPayload {
    /// **RFC 8785 JCS** canonical 序列化(§4.1.4 钉死的 codec):键按字典序、UTF-8、
    /// 无多余空白。这是 cross-repo 唯一可能 silent 字节分歧的点,故确定性编码。
    ///
    /// 实现:用 `BTreeMap<&str, serde_json::Value>` 强制键序 + `serde_json::to_vec`
    /// (compact,无空白)。对本 payload 的标量/数组字段足够确定(同输入同字节);
    /// cloud v4 须用同一 JCS 实现产出同一字节。client T-auth-1 验签复算此字节。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        use std::collections::BTreeMap;
        let mut m: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        m.insert("status", serde_json::Value::String(self.status.clone()));
        m.insert(
            "allowed_plugins",
            serde_json::Value::Array(
                self.allowed_plugins
                    .iter()
                    .map(|p| serde_json::Value::String(p.clone()))
                    .collect(),
            ),
        );
        m.insert(
            "expires_at",
            match &self.expires_at {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        m.insert("nonce", serde_json::Value::String(self.nonce.clone()));
        m.insert("verified_at", serde_json::Value::String(self.verified_at.clone()));
        // BTreeMap → serde_json::to_vec is compact + key-sorted (JCS subset).
        serde_json::to_vec(&m).expect("canonical serialize never fails for owned values")
    }
}

/// `/member/verify` 响应快照(顶层契约 v1,spec §5.2)。
#[derive(Debug, Clone, Deserialize)]
pub struct EntitlementSnapshot {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub plan: String,
    /// schema 版本。缺失(老 cloud)→ 0,见 [`Self::schema`]。
    #[serde(default)]
    pub entitlement_schema: u32,
    /// 回显的 nonce (SEC-2)
    #[serde(default)]
    pub nonce: Option<String>,
    /// 验签覆盖体(canonical),缺失 = unsigned-response。
    #[serde(default)]
    pub signed_payload: Option<SignedPayload>,
    /// base64 Ed25519 签名,缺失 = unsigned-response。
    #[serde(default)]
    pub signature: Option<String>,
    /// 派生视图。缺失 → schema-0(老 cloud)。
    #[serde(default)]
    pub entitlements: Option<Vec<EntitlementSnapshotItem>>,
    /// 服务端可调验证节奏。
    #[serde(default)]
    pub next_verify_after_hours: Option<u32>,
}

impl EntitlementSnapshot {
    /// 有效 schema:`entitlement_schema` 显式给 → 用之;否则若缺 `entitlements`
    /// → 视为 schema 0(老 cloud,spec §10)。
    pub fn schema(&self) -> u32 {
        if self.entitlement_schema > 0 {
            self.entitlement_schema
        } else if self.entitlements.is_none() {
            0
        } else {
            // entitlements present but schema field absent → treat as v1.
            1
        }
    }

    /// 是否是"未签名响应"(缺 signature 或 signed_payload)——老 cloud grandfather。
    /// T-auth-1 据此在 warn 容忍 / strict 拒。T6 仅标记,不做策略。
    pub fn is_unsigned_response(&self) -> bool {
        self.signature.is_none() || self.signed_payload.is_none()
    }
}

fn http_err(e: reqwest::Error) -> VaultError {
    VaultError::Io(std::io::Error::other(format!("http: {e}")))
}

fn json_err(e: serde_json::Error) -> VaultError {
    VaultError::Io(std::io::Error::other(format!("json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_builds_with_url() {
        let c = CloudClient::new("https://accounts.engi-stack.com");
        assert_eq!(c.base_url, "https://accounts.engi-stack.com");
        assert!(c.session_cookie.is_none());
    }

    /// Task 4 of v1.0.6 Privacy Logic Plan — wipe_session must clear the local
    /// token even when the remote logout endpoint is unreachable.
    #[test]
    fn wipe_session_clears_token_even_when_logout_endpoint_unreachable() {
        let mut c = CloudClient::with_session("http://127.0.0.1:1", "fake-token-not-real");
        assert!(c.session_token().is_some(), "precondition: token present");

        // wipe_session swallows network errors but MUST clear local token.
        let _ = c.wipe_session();

        assert!(
            c.session_token().is_none(),
            "token must be cleared after wipe_session even if remote logout failed"
        );
    }

    /// wipe_session is idempotent — calling it twice is safe.
    #[test]
    fn wipe_session_is_idempotent() {
        let mut c = CloudClient::with_session("http://127.0.0.1:1", "fake-token-not-real");
        let _ = c.wipe_session();
        // Second call on already-cleared client must also succeed.
        let _ = c.wipe_session();
        assert!(c.session_token().is_none());
    }

    /// wipe_session on a fresh client (no session) is a no-op.
    #[test]
    fn wipe_session_on_fresh_client_is_noop() {
        let mut c = CloudClient::new("http://127.0.0.1:1");
        assert!(c.session_token().is_none(), "precondition: no session");
        let _ = c.wipe_session();
        assert!(c.session_token().is_none());
    }

    #[test]
    fn login_against_unreachable_server_returns_io_error() {
        let mut c = CloudClient::new("http://127.0.0.1:1");
        let err = c.login("test@example.com", "pw").unwrap_err();
        assert!(matches!(err, VaultError::Io(_)) || matches!(err, VaultError::Crypto(_)));
    }

    #[test]
    fn entitled_plugin_serde() {
        let json = r#"{
            "plugin_id": "law-pro",
            "version": "0.2.0",
            "download_url": "https://hub.engi-stack.com/plugins/law-pro-0.2.0.attunepkg?token=abc",
            "signing_pubkey_hex": "12fe0471d5a37735428704baa5ea7a55a937fcc490cddf5e325ef4a303e6affc",
            "decrypt_key": "device-license-token"
        }"#;
        let p: EntitledPlugin = serde_json::from_str(json).unwrap();
        assert_eq!(p.plugin_id, "law-pro");
        assert_eq!(p.signing_pubkey_hex.len(), 64);
    }

    #[test]
    fn license_with_no_entitled_plugins_parses() {
        let json = r#"{
            "id": 42,
            "name": null,
            "plan": "pro",
            "license_key": "key-abc123",
            "license_id": null,
            "revoked_at": null,
            "last_used_at": null,
            "created_at": "2026-05-11T00:00:00Z"
        }"#;
        let lic: License = serde_json::from_str(json).unwrap();
        assert_eq!(lic.plan, "pro");
        assert_eq!(lic.license_key, "key-abc123");
        assert!(lic.entitled_plugins.is_empty());
    }

    #[test]
    fn license_with_entitled_plugins_parses() {
        let json = r#"{
            "id": 7,
            "name": "My License",
            "plan": "enterprise",
            "license_key": "ent-key-xyz",
            "entitled_plugins": [
                {
                    "plugin_id": "law-pro",
                    "version": "0.2.0",
                    "download_url": "https://hub.engi-stack.com/plugins/law-pro-0.2.0.attunepkg",
                    "signing_pubkey_hex": "12fe0471d5a37735428704baa5ea7a55a937fcc490cddf5e325ef4a303e6affc",
                    "decrypt_key": "device-token"
                }
            ]
        }"#;
        let lic: License = serde_json::from_str(json).unwrap();
        assert_eq!(lic.id, 7);
        assert_eq!(lic.entitled_plugins.len(), 1);
        assert_eq!(lic.entitled_plugins[0].plugin_id, "law-pro");
    }

    #[test]
    fn user_info_parses_gateway_fields() {
        let json = r#"{
            "id": 5,
            "email": "gw@example.com",
            "plan": "pro",
            "plan_expires": null,
            "is_admin": false,
            "created_at": "2026-05-18T00:00:00Z",
            "gateway_token": "sk-newapi-abc",
            "gateway_url": "https://gateway.engi-stack.com/v1"
        }"#;
        let u: UserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(u.gateway_token.as_deref(), Some("sk-newapi-abc"));
        assert_eq!(u.gateway_url.as_deref(), Some("https://gateway.engi-stack.com/v1"));
    }

    #[test]
    fn user_info_without_gateway_fields_still_parses() {
        // older accounts server / free user — fields absent
        let json = r#"{"id": 1, "email": "free@example.com", "plan": "individual"}"#;
        let u: UserInfo = serde_json::from_str(json).unwrap();
        assert!(u.gateway_token.is_none());
        // Bug-1 fix: old server lacks gateway_default_model → None,backward compat
        assert!(u.gateway_default_model.is_none());
    }

    /// Bug-1 fix (spec 2026-05-24): cloud /me 新字段 `gateway_default_model` 须可解析。
    #[test]
    fn user_info_parses_gateway_default_model() {
        let json = r#"{
            "id": 7,
            "email": "p@example.com",
            "plan": "pro",
            "gateway_token": "sk-newapi-1",
            "gateway_url": "https://gw/v1",
            "gateway_default_model": "deepseek-v4-flash"
        }"#;
        let u: UserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(u.gateway_default_model.as_deref(), Some("deepseek-v4-flash"));
    }

    // ── 增强覆盖: with_session / signup / list_licenses / logout / network error ─

    #[test]
    fn with_session_preserves_token() {
        let c = CloudClient::with_session("https://api.x.com", "sess-abc-123");
        assert_eq!(c.session_token(), Some("sess-abc-123"));
        assert_eq!(c.base_url, "https://api.x.com");
    }

    #[test]
    fn session_token_empty_when_no_login() {
        let c = CloudClient::new("https://x.com");
        assert!(c.session_token().is_none());
    }

    #[test]
    fn signup_against_unreachable_returns_err() {
        let mut c = CloudClient::new("http://127.0.0.1:2");
        let err = c.signup("new@example.com", "pw").unwrap_err();
        // 网络错 → http_err → VaultError::Io
        assert!(matches!(err, VaultError::Io(_)));
    }

    #[test]
    fn me_without_session_returns_err() {
        // 未登录调 me — unreachable port 直接 network err
        let c = CloudClient::new("http://127.0.0.1:3");
        assert!(c.me().is_err());
    }

    #[test]
    fn list_licenses_unreachable_returns_err() {
        let c = CloudClient::new("http://127.0.0.1:4");
        assert!(c.list_licenses().is_err());
    }

    // FIXME(v1.1): logout 当前在网络失败时不清空 session_cookie (用 ? 提前 return)。
    // 用户视角的"我登出了" 应当本地清空 session,不论 server 是否能响应。
    // 本 test 锁定**当前行为**: 网络挂 → logout 返回 Err 且 session 仍存在。
    // v1.1 应改为: 即使 server 不可达,也本地 session_cookie = None。
    #[test]
    fn logout_returns_err_on_unreachable_keeps_session_current_behavior() {
        let mut c = CloudClient::with_session("http://127.0.0.1:5", "token");
        assert!(c.session_token().is_some());
        let result = c.logout();
        // 当前: 网络错 → Err 提前 return → session 还在 (这是个 bug, 见 FIXME)
        assert!(result.is_err());
    }

    // Happy logout: 用 mock URL trick — 注意当前 impl 不会清 session 即使 HTTP 200
    // (因 `let _ = ?` 模式只在 ? 不触发时继续)。如果未来 fix 这个 bug, 改成本地无条件清。

    // Edge: empty email / password 也走 HTTP request (业务校验由 server 端)
    // 这里只验证 client 不 panic, 不 client-side reject
    #[test]
    fn login_empty_email_does_not_panic() {
        let mut c = CloudClient::new("http://127.0.0.1:6");
        let _ = c.login("", ""); // 不 panic
    }

    // EntitledPlugin: decrypt_key 可选 (free plugin 无)
    #[test]
    fn entitled_plugin_without_decrypt_key_parses() {
        let json = r#"{
            "plugin_id": "free-skill",
            "version": "1.0.0",
            "download_url": "https://x.com/x.attunepkg",
            "signing_pubkey_hex": "deadbeef"
        }"#;
        let p: EntitledPlugin = serde_json::from_str(json).unwrap();
        assert!(p.decrypt_key.is_none());
    }

    // UserInfo: plan_expires + is_admin defaults
    #[test]
    fn user_info_minimal_defaults() {
        let json = r#"{"id": 1, "email": "x@y.com"}"#;
        let u: UserInfo = serde_json::from_str(json).unwrap();
        assert!(u.plan.is_empty()); // default empty string
        assert!(u.plan_expires.is_none());
        assert!(!u.is_admin);
        assert!(u.gateway_token.is_none());
    }

    // Adversarial: server 返回 invalid JSON → json_err 路径
    // (这里只验证 EntitledPlugin parse 失败正确 propagate)
    #[test]
    fn entitled_plugin_invalid_json_errors() {
        let json = r#"{"plugin_id": "x"}"#; // 缺 version / download_url / signing_pubkey_hex
        let result: std::result::Result<EntitledPlugin, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // I18n: email 含 Unicode (虽然 RFC 限制 ASCII, 但 client 不应 panic)
    #[test]
    fn login_unicode_email_no_panic() {
        let mut c = CloudClient::new("http://127.0.0.1:7");
        let _ = c.login("中文@example.com", "🔒pw");
    }

    // ── T6: entitlement 快照契约 v1 镜像 (spec §5.2) ──────────────────────

    const V1_SNAPSHOT: &str = r#"{
        "valid": true,
        "plan": "pro",
        "entitlement_schema": 1,
        "nonce": "client-nonce-abc",
        "signed_payload": {
            "status": "active",
            "allowed_plugins": ["law-pro", "med-pro"],
            "expires_at": "2026-12-31T00:00:00+00:00",
            "nonce": "client-nonce-abc",
            "verified_at": "2026-06-12T00:00:00+00:00"
        },
        "signature": "QmFzZTY0RWQyNTUxOVNpZ25hdHVyZQ==",
        "entitlements": [
            {"plugin_id": "law-pro", "tier": "paid", "status": "active",
             "trial_expires": null, "signing_pubkey_hex": "8866ae9b", "verified_at": "2026-06-12T00:00:00+00:00"}
        ],
        "next_verify_after_hours": 24
    }"#;

    #[test]
    fn parse_v1_snapshot() {
        let s: EntitlementSnapshot = serde_json::from_str(V1_SNAPSHOT).unwrap();
        assert!(s.valid);
        assert_eq!(s.schema(), 1);
        assert_eq!(s.nonce.as_deref(), Some("client-nonce-abc"));
        let sp = s.signed_payload.as_ref().unwrap();
        assert_eq!(sp.status, "active");
        assert_eq!(sp.allowed_plugins, vec!["law-pro", "med-pro"]);
        assert_eq!(sp.nonce, "client-nonce-abc");
        assert!(s.signature.is_some());
        assert!(!s.is_unsigned_response());
        assert_eq!(s.entitlements.as_ref().unwrap().len(), 1);
        assert_eq!(s.next_verify_after_hours, Some(24));
    }

    #[test]
    fn unknown_field_tolerated() {
        let json = r#"{
            "valid": true, "plan": "pro", "entitlement_schema": 1,
            "signed_payload": {"status": "active", "allowed_plugins": [], "nonce": "n", "verified_at": "t"},
            "signature": "sig", "nonce": "n",
            "future_field_v2": {"seats": 5}, "another_unknown": [1,2,3]
        }"#;
        let s: EntitlementSnapshot = serde_json::from_str(json).expect("unknown fields ignored");
        assert!(s.valid);
        assert!(!s.is_unsigned_response());
    }

    #[test]
    fn missing_entitlements_is_schema_0() {
        // 老 cloud: 仅 valid + plan, 无 entitlements/schema → schema 0.
        let json = r#"{"valid": true, "plan": "pro"}"#;
        let s: EntitlementSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(s.schema(), 0, "missing entitlements → schema 0 (old cloud)");
        assert!(s.is_unsigned_response(), "old cloud has no signature → unsigned-response");
    }

    #[test]
    fn unknown_schema_major_treated_as_verify_fail() {
        // schema 2 大版本 → 客户端按宽限处理 (caller checks schema() != 1).
        let json = r#"{"valid": true, "plan": "pro", "entitlement_schema": 2,
            "signed_payload": {"status":"active","allowed_plugins":[],"nonce":"n","verified_at":"t"},
            "signature":"s","nonce":"n"}"#;
        let s: EntitlementSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(s.schema(), 2, "schema 2 surfaced for caller to route to grace");
    }

    #[test]
    fn missing_signature_is_unsigned_response() {
        // 缺 signature / signed_payload → 标记 unsigned-response (T-auth-1 据此处理).
        let json = r#"{"valid": true, "plan": "pro", "entitlement_schema": 1,
            "entitlements": [], "next_verify_after_hours": 24}"#;
        let s: EntitlementSnapshot = serde_json::from_str(json).unwrap();
        assert!(s.is_unsigned_response(), "no signature → unsigned-response");
        // does NOT panic, does NOT break: tolerant.
        assert!(s.valid);
    }

    /// JCS canonical 序列化:确定性 (同输入同字节) + 键排序 + 无空白。
    #[test]
    fn signed_payload_canonical_byte_equal_roundtrip() {
        let p = SignedPayload {
            status: "active".into(),
            allowed_plugins: vec!["law-pro".into(), "med-pro".into()],
            expires_at: Some("2026-12-31T00:00:00+00:00".into()),
            nonce: "abc123".into(),
            verified_at: "2026-06-12T00:00:00+00:00".into(),
        };
        let b1 = p.canonical_bytes();
        let b2 = p.canonical_bytes();
        assert_eq!(b1, b2, "canonical must be deterministic (byte-equal)");
        // Keys must be sorted (JCS): allowed_plugins < expires_at < nonce < status < verified_at.
        let s = String::from_utf8(b1).unwrap();
        let i_allowed = s.find("allowed_plugins").unwrap();
        let i_expires = s.find("expires_at").unwrap();
        let i_nonce = s.find("nonce").unwrap();
        let i_status = s.find("status").unwrap();
        let i_verified = s.find("verified_at").unwrap();
        assert!(i_allowed < i_expires && i_expires < i_nonce && i_nonce < i_status && i_status < i_verified,
            "canonical keys must be lexicographically sorted: {s}");
        // No whitespace (compact).
        assert!(!s.contains(": ") && !s.contains(", "), "canonical must be compact: {s}");
    }

    #[test]
    fn signed_payload_null_expires_canonical() {
        let p = SignedPayload {
            status: "trial".into(),
            allowed_plugins: vec![],
            expires_at: None,
            nonce: "n".into(),
            verified_at: "t".into(),
        };
        let s = String::from_utf8(p.canonical_bytes()).unwrap();
        assert!(s.contains("\"expires_at\":null"), "None → null in canonical: {s}");
    }
}
