// npu-vault/crates/vault-core/src/web_search_browser.rs
//
// BrowserSearchProvider：chromiumoxide 驱动系统已装的 Chrome/Edge 完成网络搜索。
// 本文件前半部分是跨平台浏览器检测，后半部分是 Provider 实现（Task 4 追加）。

use std::path::{Path, PathBuf};

/// 在常见安装路径中查找一个 Chromium 内核浏览器。
///
/// 查找顺序（首个存在的即返回）：
///   Linux:   google-chrome → chromium → microsoft-edge
///   macOS:   Google Chrome.app → Microsoft Edge.app
///   Windows: Chrome → Edge（ProgramFiles + ProgramFiles(x86) + LocalAppData）
///
/// 返回 None 表示系统无 Chromium 内核浏览器，网络搜索将禁用。
pub fn detect_system_browser() -> Option<PathBuf> {
    detect_with(|p: &Path| p.exists())
}

/// 可测试版本：注入 `exists` 判断函数
fn detect_with<F: Fn(&Path) -> bool>(exists: F) -> Option<PathBuf> {
    for path in candidate_paths() {
        if exists(&path) {
            return Some(path);
        }
    }
    None
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "linux")]
    {
        paths.push(PathBuf::from("/usr/bin/google-chrome"));
        paths.push(PathBuf::from("/usr/bin/google-chrome-stable"));
        paths.push(PathBuf::from("/usr/bin/chromium"));
        paths.push(PathBuf::from("/usr/bin/chromium-browser"));
        paths.push(PathBuf::from("/snap/bin/chromium"));
        paths.push(PathBuf::from("/usr/bin/microsoft-edge"));
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        paths.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        paths.push(PathBuf::from(
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ));
    }

    #[cfg(target_os = "windows")]
    {
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
        let pf86 = std::env::var("ProgramFiles(x86)")
            .unwrap_or_else(|_| "C:\\Program Files (x86)".into());
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();

        paths.push(PathBuf::from(format!(
            "{pf}\\Google\\Chrome\\Application\\chrome.exe"
        )));
        paths.push(PathBuf::from(format!(
            "{pf86}\\Google\\Chrome\\Application\\chrome.exe"
        )));
        if !local.is_empty() {
            paths.push(PathBuf::from(format!(
                "{local}\\Google\\Chrome\\Application\\chrome.exe"
            )));
        }
        paths.push(PathBuf::from(format!(
            "{pf}\\Microsoft\\Edge\\Application\\msedge.exe"
        )));
        paths.push(PathBuf::from(format!(
            "{pf86}\\Microsoft\\Edge\\Application\\msedge.exe"
        )));
    }

    paths
}

// ── BrowserSearchProvider ────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::Duration;

use crate::error::{Result, VaultError};
use crate::web_search::{WebSearchProvider, WebSearchResult};
use crate::web_search_engines::{DuckDuckGoEngine, SearchEngineStrategy};

/// 默认速率限制：连续两次搜索最小间隔
const DEFAULT_MIN_INTERVAL_MS: u64 = 2000;

/// 浏览器启动超时
const BROWSER_LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 页面加载超时
const PAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(20);

pub struct BrowserSearchProvider {
    browser_path: PathBuf,
    engine: Arc<dyn SearchEngineStrategy>,
    min_interval: Duration,
    last_query_at: std::sync::Mutex<Option<std::time::Instant>>,
}

impl BrowserSearchProvider {
    /// 使用系统检测到的浏览器 + DuckDuckGo 引擎创建 provider。
    /// 返回 None 表示系统无 Chromium 内核浏览器。
    pub fn auto() -> Option<Self> {
        let path = detect_system_browser()?;
        Some(Self::new(path, Arc::new(DuckDuckGoEngine)))
    }

    pub fn new(browser_path: PathBuf, engine: Arc<dyn SearchEngineStrategy>) -> Self {
        Self {
            browser_path,
            engine,
            min_interval: Duration::from_millis(DEFAULT_MIN_INTERVAL_MS),
            last_query_at: std::sync::Mutex::new(None),
        }
    }

    pub fn with_min_interval_ms(mut self, ms: u64) -> Self {
        self.min_interval = Duration::from_millis(ms);
        self
    }

    /// 速率限制：若距离上次查询太近则 sleep
    fn rate_limit(&self) {
        let mut guard = self.last_query_at.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = *guard {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                std::thread::sleep(self.min_interval - elapsed);
            }
        }
        *guard = Some(std::time::Instant::now());
    }

    /// 抓取 HTML。
    ///
    /// 采用直接 HTTP 请求（带浏览器 User-Agent）而非完整 Chromium 实例的理由：
    /// - DuckDuckGo HTML 端点设计目标就是兼容极简客户端（无 JS 渲染需求）
    /// - chromiumoxide 对当前 Chrome 的 CDP 协议常有反序列化不兼容问题（WS Invalid message）
    /// - HTTP 方式启动瞬时、零依赖、结果同样稳定；browser_path 仍作为"系统有 Chrome"的信号位
    ///
    /// 未来若需要 JS-heavy 站点抓取，可按 SearchEngineStrategy 扩展出新 engine，
    /// 或在此文件内 re-introduce chromiumoxide 分支。
    fn fetch_html(&self, url: &str) -> Result<String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(PAGE_LOAD_TIMEOUT)
            .connect_timeout(BROWSER_LAUNCH_TIMEOUT)
            .user_agent(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/125.0.0.0 Safari/537.36",
            )
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("http client: {e}")))?;

        let resp = client
            .get(url)
            .header("Accept-Language", "en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7")
            .header("Accept", "text/html,application/xhtml+xml")
            .send()
            .map_err(|e| VaultError::LlmUnavailable(format!("web search fetch: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(VaultError::LlmUnavailable(format!(
                "web search HTTP {status} at {url}"
            )));
        }

        resp.text()
            .map_err(|e| VaultError::LlmUnavailable(format!("web search body: {e}")))
    }
}

impl WebSearchProvider for BrowserSearchProvider {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        self.rate_limit();

        let url = self.engine.build_url(query);
        log::info!("web search: GET {}", url);
        let html = self.fetch_html(&url)?;
        let results = self.engine.parse(&html, limit.min(10).max(1));
        log::info!("web search: parsed {} results from {}", results.len(), self.engine.name());
        Ok(results)
    }

    fn provider_name(&self) -> &str { "browser" }
    fn is_configured(&self) -> bool { self.browser_path.exists() }
}

// ── 集成测试（需要系统装 Chrome，默认 ignored） ──────────────────────────────

#[cfg(test)]
mod browser_integration {
    use super::*;

    #[test]
    #[ignore] // 运行：cargo test -p vault-core -- --ignored browser_integration
    fn real_duckduckgo_search() {
        let provider = match BrowserSearchProvider::auto() {
            Some(p) => p,
            None => {
                eprintln!("skip: no chromium browser on this system");
                return;
            }
        };
        let results = provider.search("rust programming language", 3)
            .expect("search should succeed on a live system");
        assert!(!results.is_empty(), "DuckDuckGo should return at least 1 result");
        for r in &results {
            assert!(!r.title.is_empty());
            assert!(r.url.starts_with("http"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_with_returns_first_existing_path() {
        // 注入的 closure 只对特定路径返回 true，其他全部 false
        // 断言：返回的路径确实是我们"允许"的那条
        let target_name = "chromium"; // linux 里有 /usr/bin/chromium 候选
        let result = detect_with(|p: &Path| {
            p.to_string_lossy().ends_with(target_name)
        });
        assert!(result.is_some(), "should find a matching candidate");
        assert!(result.unwrap().to_string_lossy().ends_with(target_name));
    }

    #[test]
    fn detect_with_returns_none_when_nothing_exists() {
        let result = detect_with(|_p: &Path| false);
        assert!(result.is_none());
    }

    #[test]
    fn candidate_paths_not_empty_on_current_os() {
        let paths = candidate_paths();
        assert!(!paths.is_empty(), "at least one candidate path on this OS");
    }
}
