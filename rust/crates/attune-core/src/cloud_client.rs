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
    /// LLM gateway endpoint（云端公布；如 https://gateway.attune.ai/v1）
    #[serde(default)]
    pub gateway_url: Option<String>,
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
        let c = CloudClient::new("https://accounts.attune.ai");
        assert_eq!(c.base_url, "https://accounts.attune.ai");
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
            "download_url": "https://hub.attune.ai/plugins/law-pro-0.2.0.attunepkg?token=abc",
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
                    "download_url": "https://hub.attune.ai/plugins/law-pro-0.2.0.attunepkg",
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
            "gateway_url": "https://gateway.attune.ai/v1"
        }"#;
        let u: UserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(u.gateway_token.as_deref(), Some("sk-newapi-abc"));
        assert_eq!(u.gateway_url.as_deref(), Some("https://gateway.attune.ai/v1"));
    }

    #[test]
    fn user_info_without_gateway_fields_still_parses() {
        // older accounts server / free user — fields absent
        let json = r#"{"id": 1, "email": "free@example.com", "plan": "individual"}"#;
        let u: UserInfo = serde_json::from_str(json).unwrap();
        assert!(u.gateway_token.is_none());
    }
}
