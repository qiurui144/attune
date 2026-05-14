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

    /// Email + password 登录
    pub fn login(&mut self, email: &str, password: &str) -> Result<UserInfo> {
        let url = format!("{}/api/v1/users/login", self.base_url);
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
        let url = format!("{}/api/v1/users/signup", self.base_url);
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
        let url = format!("{}/api/v1/users/me", self.base_url);
        let resp = self
            .http
            .get(&url)
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
        let url = format!("{}/api/v1/users/logout", self.base_url);
        let _ = self.http.post(&url).send().map_err(http_err)?;
        self.session_cookie = None;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct License {
    pub id: String,
    pub license_code: String,
    pub tier: String,
    pub max_devices: usize,
    pub llm_monthly_quota: u64,
    pub issued_at: String,
    pub expires_at: Option<String>,
    /// 该 license 关联的 pro 插件清单 (云端 pluginhub 提供)
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
            "id": "lic-1",
            "license_code": "eyJj...",
            "tier": "free",
            "max_devices": 1,
            "llm_monthly_quota": 0,
            "issued_at": "2026-05-11T00:00:00Z",
            "expires_at": null
        }"#;
        let lic: License = serde_json::from_str(json).unwrap();
        assert_eq!(lic.tier, "free");
        assert!(lic.entitled_plugins.is_empty());
    }
}
