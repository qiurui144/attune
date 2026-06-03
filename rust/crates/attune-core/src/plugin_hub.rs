//! PluginHub Provider trait — attune-server 通过此接口与插件市场交互
//!
//! 实现：
//! - `MockPluginHubProvider` — 内嵌固定 listing，无网络依赖（默认 + 测试）
//! - `HttpPluginHubProvider` — HTTP 调 cloud/pluginhub /api/v1/* (v0.7+)
//!
//! 选用：attune-server 启动时按 settings.pluginhub_url 决定：
//! - URL 未配 → Mock
//! - URL + license_key 已配 → HttpPluginHubProvider
//!
//! 使用：
//! ```rust,no_run
//! use attune_core::plugin_hub::{PluginHubProvider, MockPluginHubProvider};
//! let hub: Box<dyn PluginHubProvider> = Box::new(MockPluginHubProvider::default());
//! let listings = hub.list_plugins().unwrap();
//! ```

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};

/// 单个插件在 hub 上的 listing（与 cloud/pluginhub /api/v1/index.json v1.1 schema 对齐）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginListing {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub plugin_type: String, // crawler / search / skill / workflow / channel / industry
    pub category: String,
    pub description: String,
    pub latest_version: String,
    pub tags: Vec<String>,
    /// 该插件最低需要哪个 plan: "individual" / "pro" / "enterprise"
    pub min_plan: String,
    /// 当前 license 是否可永久访问（plan 满足）
    pub available: bool,
    /// 当前 license 是否可启动 trial（plan 不够但插件允许试用）
    pub trial_available: bool,
    /// trial 天数 (0 = 不可试用)
    pub trial_days: i32,
}

/// 顶层 listing 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginListingResponse {
    pub hub_version: String,
    pub user_plan: String,
    pub upgrade_url: String,
    pub plugins: Vec<PluginListing>,
}

/// 单次 install / trial 启动的响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResponse {
    pub install_id: i64,
    pub plugin_id: String,
    pub version: String,
    pub sha256: String,
    /// trial 启动时间（仅 Free 用户首次启动 trial 时非空）
    pub trial_started: Option<String>,
    /// trial 结束时间
    pub trial_expires: Option<String>,
    /// 相对 hub URL，需配合 base_url 拼成绝对 URL
    pub download_url: String,
}

/// PluginHub 客户端 trait — 由 attune-pro/crates/hub-client 真实 HTTP 实现，
/// 或 OSS 内 Mock 实现（用于测试）
pub trait PluginHubProvider: Send + Sync {
    /// 列出当前 license 可见的全部插件（按 plan 过滤）
    fn list_plugins(&self) -> Result<PluginListingResponse>;

    /// 启动 trial 或确认安装
    /// - device_fp: 与 license-key-design 同步的设备指纹
    fn install_plugin(&self, plugin_id: &str, device_fp: Option<&str>) -> Result<InstallResponse>;

    /// 下载 .attunepkg 字节流
    fn download_plugin(&self, plugin_id: &str, version: &str) -> Result<Vec<u8>>;

    /// hub 名（用于诊断）："real-hub" / "mock"
    fn name(&self) -> &str;
}

// ── Mock 实现（OSS 测试用）──────────────────────────────────────────

/// Mock provider — 内嵌固定的 4 个 vertical plugin listing，用于测试 + offline demo
#[derive(Debug, Clone)]
pub struct MockPluginHubProvider {
    pub user_plan: String,
}

impl Default for MockPluginHubProvider {
    fn default() -> Self {
        Self {
            user_plan: "individual".into(),
        }
    }
}

impl MockPluginHubProvider {
    pub fn with_plan(plan: &str) -> Self {
        Self {
            user_plan: plan.into(),
        }
    }

    fn _builtin_plugins(&self) -> Vec<PluginListing> {
        // S4b OSS decoupling (2026-06-03): industry catalog (law-pro / patent-pro /
        // presales-pro / tech-pro) removed from OSS MockPluginHubProvider.
        // Real catalog is served by HttpPluginHubProvider from cloud/pluginhub.
        // Marketplace UI receives [] → shows "no plugins available" (existing fallback UI).
        // v1.1.x: dynamic loading from pluginhub_url.
        // See: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-4
        vec![]
    }
}

impl PluginHubProvider for MockPluginHubProvider {
    fn list_plugins(&self) -> Result<PluginListingResponse> {
        Ok(PluginListingResponse {
            hub_version: "1.1-mock".into(),
            user_plan: self.user_plan.clone(),
            upgrade_url: "https://accounts.engi-stack.com/upgrade".into(),
            plugins: self._builtin_plugins(),
        })
    }

    fn install_plugin(&self, plugin_id: &str, _device_fp: Option<&str>) -> Result<InstallResponse> {
        let plugins = self._builtin_plugins();
        let plugin = plugins
            .iter()
            .find(|p| p.id == plugin_id)
            .ok_or_else(|| VaultError::ModelLoad(format!("mock: plugin {plugin_id} not found")))?;

        if !plugin.available && !plugin.trial_available {
            return Err(VaultError::ModelLoad(format!(
                "mock: plan_required — {plugin_id} 需要 {} plan",
                plugin.min_plan
            )));
        }

        Ok(InstallResponse {
            install_id: 1,
            plugin_id: plugin_id.into(),
            version: plugin.latest_version.clone(),
            sha256: "mock-sha256".into(),
            trial_started: None,
            trial_expires: None,
            download_url: format!("/api/v1/packages/{plugin_id}-{}.tar.gz", plugin.latest_version),
        })
    }

    fn download_plugin(&self, _plugin_id: &str, _version: &str) -> Result<Vec<u8>> {
        Err(VaultError::ModelLoad(
            "mock: download not supported (use attune-pro hub-client for real downloads)".into(),
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }
}

// ── HTTP 实装（v0.7+，调 cloud/pluginhub /api/v1/* 真服务）───────────

/// HTTP PluginHub provider — 阻塞 HTTP（与 hf_hub 风格一致）。
/// attune-server 在 spawn_blocking 里调，避免引入 async runtime 复杂度。
pub struct HttpPluginHubProvider {
    base_url: String,
    license_key: String,
    client: reqwest::blocking::Client,
}

impl HttpPluginHubProvider {
    pub fn new(base_url: impl Into<String>, license_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            license_key: license_key.into(),
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("reqwest blocking build never fails"),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.license_key)
    }
}

#[derive(Debug, Deserialize)]
struct ServerIndexEntry {
    id: String,
    name: String,
    #[serde(rename = "type")]
    plugin_type: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    latest_version: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "_default_min_plan")]
    min_plan: String,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    trial_available: bool,
    #[serde(default)]
    trial_days: i32,
}

fn _default_min_plan() -> String {
    "individual".into()
}

#[derive(Debug, Deserialize)]
struct ServerIndexResponse {
    #[serde(default = "_default_hub_version")]
    hub_version: String,
    #[serde(default = "_default_min_plan")]
    user_plan: String,
    #[serde(default = "_default_upgrade_url")]
    upgrade_url: String,
    plugins: Vec<ServerIndexEntry>,
}

fn _default_hub_version() -> String {
    "1.0".into()
}
fn _default_upgrade_url() -> String {
    "https://accounts.engi-stack.com/upgrade".into()
}

#[derive(Debug, Serialize)]
struct InstallReq<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    device_fp: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct ServerInstallResp {
    install_id: i64,
    plugin_id: String,
    version: String,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    trial_started: Option<String>,
    #[serde(default)]
    trial_expires: Option<String>,
    download_url: String,
}

impl PluginHubProvider for HttpPluginHubProvider {
    fn list_plugins(&self) -> Result<PluginListingResponse> {
        let resp: ServerIndexResponse = self
            .client
            .get(self.url("/api/v1/index.json"))
            .header("Authorization", self.auth_header())
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|e| VaultError::ModelLoad(format!("hub list_plugins: {e}")))?;

        let plugins = resp
            .plugins
            .into_iter()
            .map(|e| PluginListing {
                id: e.id,
                name: e.name,
                plugin_type: e.plugin_type,
                category: e.category,
                description: e.description,
                latest_version: e.latest_version,
                tags: e.tags,
                min_plan: e.min_plan,
                available: e.available,
                trial_available: e.trial_available,
                trial_days: e.trial_days,
            })
            .collect();

        Ok(PluginListingResponse {
            hub_version: resp.hub_version,
            user_plan: resp.user_plan,
            upgrade_url: resp.upgrade_url,
            plugins,
        })
    }

    fn install_plugin(&self, plugin_id: &str, device_fp: Option<&str>) -> Result<InstallResponse> {
        let url = self.url(&format!("/api/v1/plugins/{plugin_id}/install"));
        let body = InstallReq { device_fp };

        let response = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .map_err(|e| VaultError::ModelLoad(format!("hub install send: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            let prefix = if status == reqwest::StatusCode::FORBIDDEN
                || status == reqwest::StatusCode::PAYMENT_REQUIRED
            {
                "plan_required"
            } else if status == reqwest::StatusCode::NOT_FOUND {
                "not found"
            } else {
                "hub install"
            };
            return Err(VaultError::ModelLoad(format!(
                "{prefix}: HTTP {status} — {text}"
            )));
        }

        let r: ServerInstallResp = response
            .json()
            .map_err(|e| VaultError::ModelLoad(format!("hub install parse: {e}")))?;

        Ok(InstallResponse {
            install_id: r.install_id,
            plugin_id: r.plugin_id,
            version: r.version,
            sha256: r.sha256,
            trial_started: r.trial_started,
            trial_expires: r.trial_expires,
            download_url: r.download_url,
        })
    }

    fn download_plugin(&self, plugin_id: &str, version: &str) -> Result<Vec<u8>> {
        let url = self.url(&format!("/api/v1/packages/{plugin_id}-{version}.tar.gz"));
        let bytes = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| VaultError::ModelLoad(format!("hub download: {e}")))?;
        Ok(bytes.to_vec())
    }

    fn name(&self) -> &str {
        "http-pluginhub"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S4b regression: OSS MockPluginHubProvider returns empty catalog (no industry plugins).
    /// Marketplace UI receives [] → shows "no plugins available" (expected fallback).
    #[test]
    fn mock_hub_default_returns_empty_plugin_list() {
        let hub = MockPluginHubProvider::default();
        let resp = hub.list_plugins().unwrap();
        assert!(
            resp.plugins.is_empty(),
            "S4b: MockPluginHubProvider must return empty catalog; \
             industry catalog served by HttpPluginHubProvider from cloud/pluginhub; \
             got {} plugins",
            resp.plugins.len()
        );
    }

    /// S4b regression: no industry plugin IDs in OSS mock listing.
    #[test]
    fn mock_hub_no_industry_ids_in_listing() {
        let hub = MockPluginHubProvider::with_plan("pro");
        let resp = hub.list_plugins().unwrap();
        for p in &resp.plugins {
            for industry_id in ["law-pro", "patent-pro", "presales-pro", "tech-pro"] {
                assert_ne!(
                    p.id.as_str(),
                    industry_id,
                    "S4b: industry plugin '{}' must not appear in OSS MockPluginHubProvider catalog",
                    industry_id,
                );
            }
        }
    }

    // S4b: mock_individual_user_sees_all_plugins_with_trial — superseded by
    // mock_hub_default_returns_empty_plugin_list (industry catalog removed from OSS mock).
    // HttpPluginHubProvider serves the real catalog from cloud/pluginhub.

    // S4b: mock_pro_user_sees_all_plugins_available — superseded (see above).

    // S4b: mock_install_individual_starts_trial — law-pro not in OSS mock catalog.
    // install_plugin("law-pro") now returns Err (plugin not found in empty catalog).
    #[test]
    fn mock_install_unknown_plugin_fails() {
        let hub = MockPluginHubProvider::default();
        let r = hub.install_plugin("nonexistent", None);
        assert!(r.is_err(), "unknown plugin must return Err");
    }

    /// S4b: industry plugin install returns Err (not in OSS mock catalog).
    #[test]
    fn mock_install_industry_plugin_fails_in_oss_mock() {
        let hub = MockPluginHubProvider::with_plan("pro");
        // law-pro removed from OSS mock catalog — even pro plan gets Err
        let r = hub.install_plugin("law-pro", None);
        assert!(
            r.is_err(),
            "S4b: law-pro not in OSS MockPluginHubProvider — install must return Err"
        );
    }

    #[test]
    fn mock_provider_name() {
        let hub = MockPluginHubProvider::default();
        assert_eq!(hub.name(), "mock");
    }

    #[test]
    fn http_provider_url_join() {
        let h = HttpPluginHubProvider::new("https://hub.engi-stack.com/", "key");
        assert_eq!(h.url("/api/v1/index.json"), "https://hub.engi-stack.com/api/v1/index.json");
        let h2 = HttpPluginHubProvider::new("https://hub.engi-stack.com", "key");
        assert_eq!(h2.url("/api/v1/index.json"), "https://hub.engi-stack.com/api/v1/index.json");
    }

    #[test]
    fn http_provider_auth_header() {
        let h = HttpPluginHubProvider::new("https://x", "abc");
        assert_eq!(h.auth_header(), "Bearer abc");
    }

    #[test]
    fn http_provider_name_distinguishes_from_mock() {
        let h = HttpPluginHubProvider::new("https://x", "k");
        assert_eq!(h.name(), "http-pluginhub");
    }
}
