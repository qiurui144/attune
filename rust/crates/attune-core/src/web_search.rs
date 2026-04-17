// npu-vault/crates/vault-core/src/web_search.rs
//
// 网络搜索提供者抽象层。
// 唯一内置实现：BrowserSearchProvider（见 web_search_browser.rs）
//
// 设计原则（来自 2026-04-17 定位设计 spec）：
//   - 零 API 依赖：本地无结果时通过后台浏览器自动化搜索公开网络
//   - 零降级到付费服务：浏览器不可用时明确失败而非静默调用 API
//   - 未来扩展新 provider 只需实现 WebSearchProvider trait

use crate::error::Result;
use serde::{Deserialize, Serialize};

/// 单条摘要截取字符数上限（防止注入过多网络内容撑满 LLM context window）
pub const MAX_SNIPPET_CHARS: usize = 800;

// ── 公共接口 ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published_date: Option<String>,
}

impl WebSearchResult {
    pub fn truncate_snippet(s: &str) -> String {
        s.chars().take(MAX_SNIPPET_CHARS).collect()
    }
}

pub trait WebSearchProvider: Send + Sync {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>>;
    fn provider_name(&self) -> &str;
    fn is_configured(&self) -> bool;
}

// ── 工厂函数：从 settings 构造 provider ──────────────────────────────────────

/// 从 app_settings 中的 `web_search` 块构造 WebSearchProvider。
///
/// 新 settings 形状（默认即用，零配置）：
/// ```json
/// "web_search": {
///   "enabled": true,
///   "engine": "duckduckgo",
///   "browser_path": null,
///   "min_interval_ms": 2000
/// }
/// ```
///
/// - `enabled: false` 或系统无 Chromium 内核浏览器时返回 None
/// - `browser_path: null` 表示自动检测；显式字符串则使用该路径
pub fn from_settings(
    settings: &serde_json::Value,
) -> Option<std::sync::Arc<dyn WebSearchProvider>> {
    let ws = settings.get("web_search")?;
    if !ws.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true) {
        return None;
    }

    let min_interval_ms = ws
        .get("min_interval_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(2000);

    let provider_opt = match ws.get("browser_path").and_then(|v| v.as_str()) {
        Some(p) if !p.is_empty() => {
            let path = std::path::PathBuf::from(p);
            if !path.exists() {
                return None;
            }
            Some(crate::web_search_browser::BrowserSearchProvider::new(
                path,
                std::sync::Arc::new(crate::web_search_engines::DuckDuckGoEngine),
            ))
        }
        _ => crate::web_search_browser::BrowserSearchProvider::auto(),
    };

    provider_opt.map(|p| {
        std::sync::Arc::new(p.with_min_interval_ms(min_interval_ms))
            as std::sync::Arc<dyn WebSearchProvider>
    })
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

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
        let settings = serde_json::json!({"web_search": {"enabled": false}});
        assert!(from_settings(&settings).is_none());
    }

    #[test]
    fn from_settings_no_block_returns_none() {
        let settings = serde_json::json!({"injection_mode": "auto"});
        assert!(from_settings(&settings).is_none());
    }

    #[test]
    fn from_settings_invalid_browser_path_returns_none() {
        let settings = serde_json::json!({
            "web_search": {
                "enabled": true,
                "browser_path": "/nonexistent/path/to/chrome"
            }
        });
        assert!(from_settings(&settings).is_none(),
            "bad browser_path must not fall back to auto-detect silently");
    }
}
