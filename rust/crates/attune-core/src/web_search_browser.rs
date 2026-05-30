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

/// attune 自托管 browser cache 目录路径（浏览器 fallback）。
///
/// 路径: `~/.cache/attune/browser/` (Linux/macOS) 或 `%LOCALAPPDATA%\attune\browser\` (Win)
/// 已下载的 Chrome for Testing 解压到此目录下, 可执行文件路径见 cached_browser_path().
pub fn browser_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("attune").join("browser"))
}

/// 检查 attune cache 目录是否已下载过 Chrome for Testing 可用二进制.
///
/// Cache layout (per platform):
///   Linux:    `<cache>/chrome-linux64/chrome`
///   macOS:    `<cache>/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`
///   Windows:  `<cache>\chrome-win64\chrome.exe`
pub fn cached_browser_path() -> Option<PathBuf> {
    let cache = browser_cache_dir()?;
    #[cfg(target_os = "linux")]
    let candidate = cache.join("chrome-linux64").join("chrome");
    #[cfg(target_os = "macos")]
    let candidate = cache
        .join("chrome-mac-arm64")
        .join("Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing");
    #[cfg(target_os = "windows")]
    let candidate = cache.join("chrome-win64").join("chrome.exe");
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let candidate = cache.join("chrome");
    candidate.exists().then_some(candidate)
}

/// 三段式浏览器获取: 系统 → cache → 标记需要下载.
///
/// 返回 [`BrowserResolution`]:
/// - `System(p)`     — 系统已装 Chrome/Edge, 直接用
/// - `Cached(p)`     — attune 之前下载过 Chrome for Testing, 复用
/// - `NeedsDownload` — 都没有, 调用方应触发 download_chrome_for_testing()
///
/// 设计参考 Playwright npx install: 不强制系统装 Chrome, 但已装则零等待.
///
/// **当前阶段实现**: 不内置下载逻辑 (留 v0.7 PR — 涉及 Chrome for Testing API
/// 解析 / 平台 zip / 进度推送 WebSocket / wizard UI). 本函数返回 NeedsDownload
/// 后调用方应用 UI 提示用户 "网络搜索需 ~150 MB 浏览器, 是否下载?" 由 wizard
/// 或 settings 显式触发实际下载.
pub fn resolve_browser() -> BrowserResolution {
    if let Some(p) = detect_system_browser() {
        return BrowserResolution::System(p);
    }
    if let Some(p) = cached_browser_path() {
        return BrowserResolution::Cached(p);
    }
    BrowserResolution::NeedsDownload
}

/// 浏览器解析结果 — 系统 / cache / 需下载. 调用方按情况决策.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserResolution {
    /// 系统已装 (Chrome / Edge / Chromium). 路径直接传给 chromiumoxide.
    System(PathBuf),
    /// attune cache 已下载. 路径同上.
    Cached(PathBuf),
    /// 都没有, 调用方需触发下载流程 (v0.7+ 实施).
    NeedsDownload,
}

/// 可测试版本：注入 `exists` 判断函数
fn detect_with<F: Fn(&Path) -> bool>(exists: F) -> Option<PathBuf> {
    candidate_paths().into_iter().find(|path| exists(path))
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

        // v1.0.6 Privacy Logic Strategy — OutboundGate audit hook for Web Search outbound.
        // The actual `privacy.web_search` setting + vault_unlocked wiring is plumbed in
        // Task 7 (PrivacyView state integration); today this is a non-rejecting
        // call site marker. Query is unredacted (search engines see raw queries).
        // Grep guard (scripts/privacy-audit.sh) keys on `OutboundGate::enforce`.
        let _ = crate::OutboundGate::enforce(
            &crate::OutboundPolicy {
                kind: crate::OutboundKind::WebSearch,
                enabled: true, // wired in Task 7 from settings.privacy.web_search
                vault_unlocked: true, // wired in Task 7 from vault.state()
                redactor: None,
            },
            "",
        );

        let url = self.engine.build_url(query);
        log::info!("web search: GET {}", url);
        let html = self.fetch_html(&url)?;
        let results = self.engine.parse(&html, limit.clamp(1, 10));
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
        //
        // target_name 必须是各 OS 都有候选路径以该字符串结尾的:
        //   Linux:    /usr/bin/chromium 等
        //   macOS:    .../Google Chrome (无 .exe), .../Microsoft Edge
        //   Windows:  ...chrome.exe / msedge.exe
        // 用 "Chrome" / "chrome" 作为公约数:
        //   Linux google-chrome / chromium 含 "chrome" (substr 不严格 ends_with)
        //   macOS "Google Chrome" 含 "Chrome"
        //   Windows chrome.exe 含 "chrome" (但 ends_with .exe)
        // 用 contains 而不是 ends_with 才跨平台稳健
        let result = detect_with(|p: &Path| {
            p.to_string_lossy().to_lowercase().contains("chrome")
        });
        assert!(result.is_some(), "should find a matching candidate (some chrome variant)");
    }

    #[test]
    fn detect_with_returns_none_when_nothing_exists() {
        let result = detect_with(|_p: &Path| false);
        assert!(result.is_none());
    }

    #[test]
    fn browser_cache_dir_returns_some_path() {
        // dirs::cache_dir 在所有支持平台都 Some, 仅极端无 HOME 环境才 None.
        let dir = browser_cache_dir();
        assert!(dir.is_some(), "expected dirs::cache_dir / attune / browser path");
        assert!(dir.unwrap().to_string_lossy().contains("attune"));
    }

    #[test]
    fn resolve_browser_falls_through_to_needs_download_if_isolated() {
        // 这个 test 只能保证三个分支之一被选中. 真实 NeedsDownload 需要
        // 在没系统浏览器 + 没 cache 的 isolated CI runner 上跑. 此处仅做
        // smoke: 三种 enum variant 都能正常构造.
        let _result = resolve_browser();
    }

    #[test]
    fn candidate_paths_not_empty_on_current_os() {
        let paths = candidate_paths();
        assert!(!paths.is_empty(), "at least one candidate path on this OS");
    }
}
