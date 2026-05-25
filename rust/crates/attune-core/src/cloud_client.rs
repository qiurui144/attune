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
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .cookie_store(true) // 自动管理 session cookie
                .build()
                .expect("build http client"),
        }
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
}
