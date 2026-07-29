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
use std::path::Path;

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
    /// Device-bound manifest decrypt key for encrypted official plugins. This is
    /// sensitive: callers persist it into the vault entitlement table; it must
    /// never be serialized back to the browser UI.
    #[serde(default, skip_serializing)]
    pub decrypt_key: Option<String>,
    /// trial 启动时间（仅 Free 用户首次启动 trial 时非空）
    pub trial_started: Option<String>,
    /// trial 结束时间
    pub trial_expires: Option<String>,
    /// 相对 hub URL，需配合 base_url 拼成绝对 URL
    pub download_url: String,
}

/// Best-effort local filesystem marketplace listing.
///
/// When the server is running with the offline mock hub, the Marketplace route
/// uses this to surface already-installed plugins instead of showing an empty
/// catalog. Invalid plugins are ignored by `PluginRegistry::scan`; callers still
/// get every successfully loaded manifest.
pub fn scan_local_plugins(plugins_dir: &Path) -> Vec<PluginListing> {
    let Ok((registry, _warnings)) = crate::plugin_registry::PluginRegistry::scan(plugins_dir)
    else {
        return Vec::new();
    };
    local_plugin_listings_from_registry(&registry)
}

/// Convert an already-loaded plugin registry into Marketplace listing rows.
///
/// This is used by the server's key-aware local fallback: encrypted commercial
/// plugins can only be loaded after the vault entitlement keys are available, so
/// callers should pass a live registry instead of forcing a plaintext filesystem
/// scan.
pub fn local_plugin_listings_from_registry(
    registry: &crate::plugin_registry::PluginRegistry,
) -> Vec<PluginListing> {
    let mut plugins: Vec<PluginListing> = registry
        .plugins()
        .filter_map(|plugin| {
            let manifest = &plugin.manifest;
            let id = manifest.id.trim();
            if id.is_empty() {
                return None;
            }

            let name = manifest.name.trim();
            let plugin_type = manifest.plugin_type.trim();
            let category = manifest.category.trim();
            let description = manifest.description.trim();
            let version = manifest.version.trim();
            let pricing_tier = manifest
                .pricing
                .as_ref()
                .map(|p| p.tier.trim())
                .unwrap_or("free");
            let min_plan = match pricing_tier {
                "paid" | "trial" => "pro",
                _ => "individual",
            };
            let mut tags = Vec::new();
            if !category.is_empty() {
                tags.push(category.to_string());
            }
            if !plugin_type.is_empty() && plugin_type != category {
                tags.push(plugin_type.to_string());
            }

            Some(PluginListing {
                id: id.to_string(),
                name: if name.is_empty() {
                    id.to_string()
                } else {
                    name.to_string()
                },
                plugin_type: if plugin_type.is_empty() {
                    "plugin".to_string()
                } else {
                    plugin_type.to_string()
                },
                category: if category.is_empty() {
                    "local".to_string()
                } else {
                    category.to_string()
                },
                description: description.to_string(),
                latest_version: if version.is_empty() {
                    "0.0.0".to_string()
                } else {
                    version.to_string()
                },
                tags,
                min_plan: min_plan.to_string(),
                available: true,
                trial_available: false,
                trial_days: 0,
            })
        })
        .collect();

    plugins.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    plugins
}

/// PluginHub 客户端 trait — 由 attune-pro/crates/hub-client 真实 HTTP 实现，
/// 或 OSS 内 Mock 实现（用于测试）
pub trait PluginHubProvider: Send + Sync {
    /// 列出当前 license 可见的全部插件（按 plan 过滤）
    fn list_plugins(&self) -> Result<PluginListingResponse>;

    /// 启动 trial 或确认安装
    /// - device_fp: 与 license-key-design 同步的设备指纹
    fn install_plugin(&self, plugin_id: &str, device_fp: Option<&str>) -> Result<InstallResponse>;

    /// 下载 .tar.gz 字节流
    fn download_plugin(&self, plugin_id: &str, version: &str) -> Result<Vec<u8>>;

    /// 下载 hub install 响应返回的包地址。
    ///
    /// 默认实现保留旧 provider 的兼容性；HTTP provider 会支持相对路径、同源绝对
    /// URL 和签名 CDN URL。
    fn download_plugin_url(&self, download_url: &str) -> Result<Vec<u8>> {
        Err(VaultError::ModelLoad(format!(
            "provider does not support install download_url: {download_url}"
        )))
    }

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
}

impl PluginHubProvider for MockPluginHubProvider {
    fn list_plugins(&self) -> Result<PluginListingResponse> {
        Ok(PluginListingResponse {
            hub_version: "1.1-mock".into(),
            user_plan: self.user_plan.clone(),
            upgrade_url: "https://accounts.engi-stack.com/upgrade".into(),
            plugins: vec![],
        })
    }

    fn install_plugin(&self, plugin_id: &str, _device_fp: Option<&str>) -> Result<InstallResponse> {
        // Mock install is unsupported — all plugins come from the real HttpPluginHubProvider.
        // The marketplace route handles local-fs fallback for listing; install always requires a hub.
        Err(VaultError::ModelLoad(format!(
            "mock: install not supported for {plugin_id} — configure pluginhub.url + license_key in Settings → PluginHub"
        )))
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

    fn resolve_download_url(&self, download_url: &str) -> String {
        let trimmed = download_url.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else if trimmed.starts_with('/') {
            self.url(trimmed)
        } else {
            self.url(&format!("/{trimmed}"))
        }
    }

    fn should_send_auth_to(&self, url: &str) -> bool {
        let Ok(base) = reqwest::Url::parse(self.base_url.trim_end_matches('/')) else {
            return false;
        };
        let Ok(target) = reqwest::Url::parse(url) else {
            return false;
        };
        base.scheme() == target.scheme()
            && base.host_str() == target.host_str()
            && base.port_or_known_default() == target.port_or_known_default()
    }

    fn validate_download_url(&self, url: &str) -> Result<()> {
        if std::env::var_os("ATTUNE_ALLOW_LOCAL_PLUGINHUB").is_some() {
            return Ok(());
        }
        crate::net::url_guard::validate_open_outbound_url(
            url,
            &crate::net::url_guard::system_resolve,
        )
        .map(|_| ())
        .map_err(|e| VaultError::InvalidInput(format!("plugin-download-url-blocked: {e}")))
    }

    fn validate_base_url(&self) -> Result<()> {
        if std::env::var_os("ATTUNE_ALLOW_LOCAL_PLUGINHUB").is_some() {
            return Ok(());
        }
        crate::net::url_guard::validate_open_outbound_url(
            &self.base_url,
            &crate::net::url_guard::system_resolve,
        )
        .map(|_| ())
        .map_err(|e| VaultError::InvalidInput(format!("pluginhub-url-blocked: {e}")))
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
    decrypt_key: Option<String>,
    #[serde(default)]
    trial_started: Option<String>,
    #[serde(default)]
    trial_expires: Option<String>,
    download_url: String,
}

impl PluginHubProvider for HttpPluginHubProvider {
    fn list_plugins(&self) -> Result<PluginListingResponse> {
        self.validate_base_url()?;
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
        self.validate_base_url()?;
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
            decrypt_key: r.decrypt_key,
            trial_started: r.trial_started,
            trial_expires: r.trial_expires,
            download_url: r.download_url,
        })
    }

    fn download_plugin(&self, plugin_id: &str, version: &str) -> Result<Vec<u8>> {
        self.download_plugin_url(&format!("/api/v1/packages/{plugin_id}-{version}.tar.gz"))
    }

    fn download_plugin_url(&self, download_url: &str) -> Result<Vec<u8>> {
        let url = self.resolve_download_url(download_url);
        self.validate_download_url(&url)?;
        let mut req = self.client.get(&url);
        if self.should_send_auth_to(&url) {
            req = req.header("Authorization", self.auth_header());
        }
        let bytes = req
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
        assert_eq!(
            h.url("/api/v1/index.json"),
            "https://hub.engi-stack.com/api/v1/index.json"
        );
        let h2 = HttpPluginHubProvider::new("https://hub.engi-stack.com", "key");
        assert_eq!(
            h2.url("/api/v1/index.json"),
            "https://hub.engi-stack.com/api/v1/index.json"
        );
    }

    #[test]
    fn http_provider_resolves_install_download_url() {
        let h = HttpPluginHubProvider::new("https://hub.engi-stack.com/base", "key");
        assert_eq!(
            h.resolve_download_url("/api/v1/packages/law-pro.tar.gz"),
            "https://hub.engi-stack.com/base/api/v1/packages/law-pro.tar.gz"
        );
        assert_eq!(
            h.resolve_download_url("https://cdn.example.com/signed/law-pro.tar.gz?sig=1"),
            "https://cdn.example.com/signed/law-pro.tar.gz?sig=1"
        );
    }

    #[test]
    fn http_provider_auth_header() {
        let h = HttpPluginHubProvider::new("https://x", "abc");
        assert_eq!(h.auth_header(), "Bearer abc");
    }

    #[test]
    fn install_request_carries_device_fingerprint_when_available() {
        let body = InstallReq {
            device_fp: Some("fp-device-bound-123"),
        };
        let encoded = serde_json::to_value(&body).unwrap();
        assert_eq!(encoded["device_fp"], "fp-device-bound-123");

        let no_device = InstallReq { device_fp: None };
        let encoded = serde_json::to_value(&no_device).unwrap();
        assert!(
            encoded.get("device_fp").is_none(),
            "None must omit device_fp rather than sending an empty value"
        );
    }

    #[test]
    fn http_provider_only_sends_auth_to_same_origin_downloads() {
        let h = HttpPluginHubProvider::new("https://hub.engi-stack.com", "key");
        assert!(h.should_send_auth_to("https://hub.engi-stack.com/api/v1/packages/a.tar.gz"));
        assert!(!h.should_send_auth_to("https://cdn.example.com/signed/a.tar.gz"));
    }

    #[test]
    fn http_provider_name_distinguishes_from_mock() {
        let h = HttpPluginHubProvider::new("https://x", "k");
        assert_eq!(h.name(), "http-pluginhub");
    }

    #[test]
    fn http_provider_rejects_local_pluginhub_url_by_default() {
        let h = HttpPluginHubProvider::new("http://localhost:8090", "k");
        let err = h.validate_base_url().unwrap_err();
        assert!(
            err.to_string().contains("pluginhub-url-blocked"),
            "local pluginhub URL must be blocked before sending license key, got: {err}"
        );
    }
}
