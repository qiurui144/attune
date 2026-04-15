// npu-vault/crates/vault-core/src/web_search.rs
//
// 网络搜索提供者抽象层。
// 当本地知识库无相关内容时，Chat 引擎可以调用网络搜索作为补充。
//
// 内置提供者：
//   - BraveSearchProvider  — Brave Search API（免费 2000 次/月，需 API Key）
//   - TavilySearchProvider — Tavily API（RAG 优化，免费 1000 次/月，需 API Key）
//   - SearxngSearchProvider — SearXNG 自托管实例（无需 API Key）

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};

const MAX_RESULTS: usize = 5;
/// 单条摘要截取字符数上限（避免注入过多网络内容撑满 context window）
const MAX_SNIPPET_CHARS: usize = 800;

// ── 公共接口 ──────────────────────────────────────────────────────────────────

/// 单条网络搜索结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published_date: Option<String>,
}

impl WebSearchResult {
    fn truncate_snippet(s: &str) -> String {
        s.chars().take(MAX_SNIPPET_CHARS).collect()
    }
}

/// 网络搜索提供者 trait
pub trait WebSearchProvider: Send + Sync {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>>;
    fn provider_name(&self) -> &str;
    fn is_configured(&self) -> bool;
}

// ── Brave Search API ──────────────────────────────────────────────────────────

const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";

pub struct BraveSearchProvider {
    client: reqwest::blocking::Client,
    api_key: String,
}

impl BraveSearchProvider {
    pub fn new(api_key: &str) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .user_agent("npu-vault/0.5 web-search")
                .build()
                .expect("BraveSearch HTTP client"),
            api_key: api_key.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}
#[derive(Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}
#[derive(Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: Option<String>,
    age: Option<String>,
}

impl WebSearchProvider for BraveSearchProvider {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>> {
        let limit = limit.min(MAX_RESULTS).max(1);
        let resp = self.client
            .get(BRAVE_SEARCH_URL)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &limit.to_string())])
            .send()
            .map_err(|e| VaultError::LlmUnavailable(format!("Brave search request: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(VaultError::LlmUnavailable(format!("Brave HTTP {status}: {body}")));
        }

        let raw: BraveResponse = resp.json()
            .map_err(|e| VaultError::LlmUnavailable(format!("Brave response parse: {e}")))?;

        let results = raw.web.map(|w| w.results).unwrap_or_default()
            .into_iter()
            .map(|r| WebSearchResult {
                title: r.title,
                url: r.url,
                snippet: WebSearchResult::truncate_snippet(&r.description.unwrap_or_default()),
                published_date: r.age,
            })
            .collect();

        Ok(results)
    }

    fn provider_name(&self) -> &str { "brave" }
    fn is_configured(&self) -> bool { !self.api_key.is_empty() }
}

// ── Tavily API ────────────────────────────────────────────────────────────────

const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";

pub struct TavilySearchProvider {
    client: reqwest::blocking::Client,
    api_key: String,
}

impl TavilySearchProvider {
    pub fn new(api_key: &str) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .user_agent("npu-vault/0.5 web-search")
                .build()
                .expect("Tavily HTTP client"),
            api_key: api_key.to_string(),
        }
    }
}

#[derive(Serialize)]
struct TavilyRequest<'a> {
    api_key: &'a str,
    query: &'a str,
    max_results: usize,
    include_answer: bool,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    published_date: Option<String>,
}

impl WebSearchProvider for TavilySearchProvider {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>> {
        let limit = limit.min(MAX_RESULTS).max(1);
        let req = TavilyRequest {
            api_key: &self.api_key,
            query,
            max_results: limit,
            include_answer: false,
        };
        let resp = self.client
            .post(TAVILY_SEARCH_URL)
            .json(&req)
            .send()
            .map_err(|e| VaultError::LlmUnavailable(format!("Tavily search request: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(VaultError::LlmUnavailable(format!("Tavily HTTP {status}: {body}")));
        }

        let raw: TavilyResponse = resp.json()
            .map_err(|e| VaultError::LlmUnavailable(format!("Tavily response parse: {e}")))?;

        let results = raw.results.into_iter().map(|r| WebSearchResult {
            title: r.title,
            url: r.url,
            snippet: WebSearchResult::truncate_snippet(&r.content),
            published_date: r.published_date,
        }).collect();

        Ok(results)
    }

    fn provider_name(&self) -> &str { "tavily" }
    fn is_configured(&self) -> bool { !self.api_key.is_empty() }
}

// ── SearXNG 自托管 ────────────────────────────────────────────────────────────

pub struct SearxngSearchProvider {
    client: reqwest::blocking::Client,
    base_url: String,
}

impl SearxngSearchProvider {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .user_agent("npu-vault/0.5 web-search")
                .build()
                .expect("SearXNG HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

#[derive(Deserialize)]
struct SearxngResponse {
    results: Vec<SearxngResult>,
}

#[derive(Deserialize)]
struct SearxngResult {
    title: String,
    url: String,
    content: Option<String>,
    #[serde(rename = "publishedDate")]
    published_date: Option<String>,
}

impl WebSearchProvider for SearxngSearchProvider {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>> {
        let limit = limit.min(MAX_RESULTS).max(1);
        let url = format!("{}/search", self.base_url);
        let resp = self.client
            .get(&url)
            .query(&[("q", query), ("format", "json"), ("pageno", "1")])
            .send()
            .map_err(|e| VaultError::LlmUnavailable(format!("SearXNG request: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(VaultError::LlmUnavailable(format!("SearXNG HTTP {status}: {body}")));
        }

        let raw: SearxngResponse = resp.json()
            .map_err(|e| VaultError::LlmUnavailable(format!("SearXNG response parse: {e}")))?;

        let results = raw.results.into_iter().take(limit).map(|r| WebSearchResult {
            title: r.title,
            url: r.url,
            snippet: WebSearchResult::truncate_snippet(&r.content.unwrap_or_default()),
            published_date: r.published_date,
        }).collect();

        Ok(results)
    }

    fn provider_name(&self) -> &str { "searxng" }
    fn is_configured(&self) -> bool { !self.base_url.is_empty() }
}

// ── 工厂函数：从 settings JSON 构造 provider ──────────────────────────────────

/// 从 app_settings 中的 `web_search` 块构造 WebSearchProvider。
/// ```json
/// "web_search": {
///   "enabled": true,
///   "provider": "brave",      // "brave" | "tavily" | "searxng"
///   "api_key": "BSA...",      // brave 或 tavily 需要
///   "base_url": "http://..."  // searxng 需要
/// }
/// ```
pub fn from_settings(settings: &serde_json::Value) -> Option<std::sync::Arc<dyn WebSearchProvider>> {
    let ws = settings.get("web_search")?;
    if !ws.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        return None;
    }
    let provider = ws.get("provider").and_then(|v| v.as_str()).unwrap_or("brave");
    let api_key = ws.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
    let base_url = ws.get("base_url").and_then(|v| v.as_str()).unwrap_or("");

    match provider {
        "brave" if !api_key.is_empty() => {
            Some(std::sync::Arc::new(BraveSearchProvider::new(api_key)))
        }
        "tavily" if !api_key.is_empty() => {
            Some(std::sync::Arc::new(TavilySearchProvider::new(api_key)))
        }
        "searxng" if !base_url.is_empty() => {
            Some(std::sync::Arc::new(SearxngSearchProvider::new(base_url)))
        }
        _ => None,
    }
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_snippet_within_limit() {
        let s = "a".repeat(MAX_SNIPPET_CHARS + 100);
        let t = WebSearchResult::truncate_snippet(&s);
        assert_eq!(t.len(), MAX_SNIPPET_CHARS);
    }

    #[test]
    fn truncate_snippet_short_unchanged() {
        let s = "hello world";
        assert_eq!(WebSearchResult::truncate_snippet(s), s);
    }

    #[test]
    fn from_settings_disabled_returns_none() {
        let settings = serde_json::json!({"web_search": {"enabled": false, "provider": "brave", "api_key": "key"}});
        assert!(from_settings(&settings).is_none());
    }

    #[test]
    fn from_settings_no_block_returns_none() {
        let settings = serde_json::json!({"injection_mode": "auto"});
        assert!(from_settings(&settings).is_none());
    }

    #[test]
    fn from_settings_brave_creates_provider() {
        let settings = serde_json::json!({"web_search": {"enabled": true, "provider": "brave", "api_key": "test-key"}});
        let p = from_settings(&settings).expect("should create brave provider");
        assert_eq!(p.provider_name(), "brave");
        assert!(p.is_configured());
    }

    #[test]
    fn from_settings_tavily_creates_provider() {
        let settings = serde_json::json!({"web_search": {"enabled": true, "provider": "tavily", "api_key": "tvly-test"}});
        let p = from_settings(&settings).expect("should create tavily provider");
        assert_eq!(p.provider_name(), "tavily");
    }

    #[test]
    fn from_settings_searxng_creates_provider() {
        let settings = serde_json::json!({
            "web_search": {"enabled": true, "provider": "searxng", "base_url": "http://localhost:8080"}
        });
        let p = from_settings(&settings).expect("should create searxng provider");
        assert_eq!(p.provider_name(), "searxng");
    }

    #[test]
    fn from_settings_brave_no_key_returns_none() {
        let settings = serde_json::json!({"web_search": {"enabled": true, "provider": "brave", "api_key": ""}});
        assert!(from_settings(&settings).is_none(), "empty api_key should return None");
    }
}
