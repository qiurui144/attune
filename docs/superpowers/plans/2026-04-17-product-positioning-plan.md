# 产品定位重设 + 浏览器网络搜索重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 spec `docs/superpowers/specs/2026-04-17-product-positioning-design.md` 重写网络搜索为唯一的浏览器自动化方案，并同步更新 README / CLAUDE / DEVELOP 把产品定位从"加密仓库"改为"主动进化的私有 AI 知识伙伴"。

**Architecture:** 新增 `BrowserSearchProvider`（chromiumoxide 驱动系统已装 Chrome/Edge），删除 Brave/Tavily/SearXNG 三个 API provider。`WebSearchProvider` trait 保留作为扩展点。前端不动（§6 设计原则约束未来 UI 改动，不在本 plan 范围内）。

**Tech Stack:** Rust (vault-core + vault-server), chromiumoxide 0.7, tokio, scraper。文档使用 Markdown。

**Branch:** 继续在 `feature/search-rerank-infer` 分支（已有相关提交基础）。

---

## File Structure

### 新增文件

| 文件 | 职责 |
|------|------|
| `npu-vault/crates/vault-core/src/web_search_engines.rs` | `SearchEngineStrategy` trait + `DuckDuckGoEngine` HTML parser |
| `npu-vault/crates/vault-core/src/web_search_browser.rs` | `BrowserSearchProvider` 实现（chromiumoxide 驱动） + 系统浏览器检测 |
| `npu-vault/crates/vault-core/tests/fixtures/duckduckgo_sample.html` | DuckDuckGo HTML 响应样例，用于解析单元测试 |

### 重写文件

| 文件 | 改动范围 |
|------|---------|
| `npu-vault/crates/vault-core/src/web_search.rs` | 删除 Brave/Tavily/SearXNG 三个 provider + 测试；重写 `from_settings`；re-export BrowserSearchProvider |
| `npu-vault/crates/vault-core/Cargo.toml` | 新增 chromiumoxide、scraper 依赖 |
| `npu-vault/crates/vault-core/src/lib.rs` | 注册两个新模块 |
| `npu-vault/crates/vault-server/src/routes/settings.rs` | 更新 `default_settings()` 和 `ALLOWED_KEYS` 中 web_search 字段结构 |
| `npu-vault/crates/vault-server/src/routes/chat.rs` | 浏览器不可用时追加用户提示 |
| `npu-vault/README.md` | 重写头部 + 按三大支柱重排功能列表 + 新增"主权与透明"小节 |
| `README.md`（仓库顶层） | 更新 tagline + 删除"1Password 式加密" |
| `CLAUDE.md`（仓库顶层） | 更新双产品线架构段落 |
| `npu-vault/DEVELOP.md` | 仅更新开头定位一行 |

### 决策说明

**Flat 模块结构 vs 嵌套目录**：Spec §7 原建议 `web_search/{mod.rs, browser.rs, engines/*}` 嵌套结构。实施时改为 flat `web_search.rs` + `web_search_browser.rs` + `web_search_engines.rs`，与 codebase 现有风格（`scanner.rs` + `scanner_patent.rs` + `scanner_webdav.rs`）一致。Trait 扩展性不受影响。

---

## Task 1: 添加 chromiumoxide 和 scraper 依赖

**Files:**
- Modify: `npu-vault/crates/vault-core/Cargo.toml:25-39`

- [ ] **Step 1: 修改 Cargo.toml 添加依赖**

在 `npu-vault/crates/vault-core/Cargo.toml` 的 `[dependencies]` 段末尾追加：

```toml
chromiumoxide = { version = "0.7", features = ["tokio-runtime"] }
scraper = "0.21"
```

- [ ] **Step 2: 验证编译通过**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo build -p vault-core 2>&1 | tail -3
```

Expected: `Finished` without error (会下载 chromiumoxide 及其依赖，初次约 2 分钟)

- [ ] **Step 3: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/crates/vault-core/Cargo.toml npu-vault/Cargo.lock
git commit -m "build(vault-core): add chromiumoxide + scraper deps for browser-based web search"
```

---

## Task 2: 创建 DuckDuckGo HTML 解析引擎

### 背景
我们驱动 Chrome 加载 `https://html.duckduckgo.com/html/?q={query}`，然后从渲染后的 DOM 提取结果。解析逻辑用 `scraper` 库做，可以 unit-test 不依赖真实浏览器。

**Files:**
- Create: `npu-vault/crates/vault-core/src/web_search_engines.rs`
- Create: `npu-vault/crates/vault-core/tests/fixtures/duckduckgo_sample.html`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 创建 HTML fixture**

创建 `npu-vault/crates/vault-core/tests/fixtures/duckduckgo_sample.html`，内容为 DuckDuckGo HTML 结果页的精简样本（3 条结果，足以覆盖解析逻辑）：

```html
<!DOCTYPE html>
<html><body>
<div class="results">
  <div class="result results_links results_links_deep web-result">
    <h2 class="result__title">
      <a class="result__a" href="https://example.com/first" rel="nofollow">第一个结果标题</a>
    </h2>
    <a class="result__snippet" href="https://example.com/first">第一个结果的摘要内容</a>
  </div>
  <div class="result results_links results_links_deep web-result">
    <h2 class="result__title">
      <a class="result__a" href="https://example.com/second" rel="nofollow">Second Result Title</a>
    </h2>
    <a class="result__snippet" href="https://example.com/second">Second result snippet with some text</a>
  </div>
  <div class="result results_links results_links_deep web-result">
    <h2 class="result__title">
      <a class="result__a" href="https://example.com/third" rel="nofollow">第三个</a>
    </h2>
    <a class="result__snippet" href="https://example.com/third">第三个摘要</a>
  </div>
</div>
</body></html>
```

- [ ] **Step 2: 创建 web_search_engines.rs 骨架（仅 trait 和失败测试）**

创建 `npu-vault/crates/vault-core/src/web_search_engines.rs`：

```rust
// npu-vault/crates/vault-core/src/web_search_engines.rs
//
// 搜索引擎策略接口：把 DuckDuckGo / Google / Bing 等不同引擎的 DOM 解析逻辑隔离，
// 每加一个引擎只增加一个 impl block，不改动 BrowserSearchProvider。

use crate::web_search::WebSearchResult;

/// 搜索引擎策略：负责 URL 构造和 HTML 解析
pub trait SearchEngineStrategy: Send + Sync {
    /// 给定查询词，返回请求 URL
    fn build_url(&self, query: &str) -> String;
    /// 给定 HTML 响应，解析成结果列表
    fn parse(&self, html: &str, limit: usize) -> Vec<WebSearchResult>;
    /// 引擎名，用于日志和调试
    fn name(&self) -> &str;
}

/// DuckDuckGo HTML 端点引擎（对爬虫友好）
pub struct DuckDuckGoEngine;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckduckgo_parses_sample_html() {
        let html = include_str!("../tests/fixtures/duckduckgo_sample.html");
        let engine = DuckDuckGoEngine;
        let results = engine.parse(html, 5);

        assert_eq!(results.len(), 3, "sample has 3 results");
        assert_eq!(results[0].title, "第一个结果标题");
        assert_eq!(results[0].url, "https://example.com/first");
        assert!(results[0].snippet.contains("第一个结果的摘要"));
        assert_eq!(results[1].title, "Second Result Title");
    }

    #[test]
    fn duckduckgo_respects_limit() {
        let html = include_str!("../tests/fixtures/duckduckgo_sample.html");
        let engine = DuckDuckGoEngine;
        let results = engine.parse(html, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn duckduckgo_builds_url() {
        let engine = DuckDuckGoEngine;
        let url = engine.build_url("rust async");
        assert!(url.starts_with("https://html.duckduckgo.com/html/"));
        assert!(url.contains("q=rust"));
    }
}
```

- [ ] **Step 3: 注册模块**

修改 `npu-vault/crates/vault-core/src/lib.rs`，在 `pub mod web_search;` 前一行添加：

```rust
pub mod web_search_engines;
```

- [ ] **Step 4: 跑测试应当失败（尚未实现 trait for DuckDuckGoEngine）**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo test -p vault-core web_search_engines 2>&1 | tail -10
```

Expected: 编译错误，提示 `DuckDuckGoEngine` 未实现 `SearchEngineStrategy`。

- [ ] **Step 5: 实现 DuckDuckGoEngine**

在 `web_search_engines.rs` 的 `pub struct DuckDuckGoEngine;` 之后追加：

```rust
impl SearchEngineStrategy for DuckDuckGoEngine {
    fn build_url(&self, query: &str) -> String {
        let encoded = urlencoding::encode(query);
        format!("https://html.duckduckgo.com/html/?q={encoded}")
    }

    fn parse(&self, html: &str, limit: usize) -> Vec<WebSearchResult> {
        use scraper::{Html, Selector};

        let document = Html::parse_document(html);
        let result_sel = Selector::parse("div.result").expect("result selector");
        let title_sel = Selector::parse("a.result__a").expect("title selector");
        let snippet_sel = Selector::parse("a.result__snippet, .result__snippet").expect("snippet selector");

        let mut results = Vec::new();
        for node in document.select(&result_sel).take(limit) {
            let title_el = match node.select(&title_sel).next() {
                Some(t) => t,
                None => continue,
            };
            let title = title_el.text().collect::<String>().trim().to_string();
            let url = title_el.value().attr("href").unwrap_or("").to_string();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = node
                .select(&snippet_sel)
                .next()
                .map(|s| s.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            results.push(WebSearchResult {
                title,
                url,
                snippet,
                published_date: None,
            });
        }
        results
    }

    fn name(&self) -> &str { "duckduckgo" }
}
```

- [ ] **Step 6: 给 vault-core 添加 urlencoding 依赖**

修改 `npu-vault/crates/vault-core/Cargo.toml` 追加：

```toml
urlencoding = "2"
```

- [ ] **Step 7: 跑测试通过**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo test -p vault-core web_search_engines 2>&1 | tail -10
```

Expected:
```
running 3 tests
test web_search_engines::tests::duckduckgo_builds_url ... ok
test web_search_engines::tests::duckduckgo_parses_sample_html ... ok
test web_search_engines::tests::duckduckgo_respects_limit ... ok

test result: ok. 3 passed; 0 failed
```

- [ ] **Step 8: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/crates/vault-core/src/lib.rs \
        npu-vault/crates/vault-core/src/web_search_engines.rs \
        npu-vault/crates/vault-core/tests/fixtures/duckduckgo_sample.html \
        npu-vault/crates/vault-core/Cargo.toml \
        npu-vault/Cargo.lock
git commit -m "feat(web-search): DuckDuckGoEngine HTML parser + SearchEngineStrategy trait"
```

---

## Task 3: 创建系统浏览器检测工具

### 背景
chromiumoxide 需要一个 Chrome/Edge 可执行文件路径。不同 OS 有不同的默认路径。本任务实现跨平台检测。

**Files:**
- Create: `npu-vault/crates/vault-core/src/web_search_browser.rs`（仅 browser_detect 部分）
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 创建文件，含测试**

创建 `npu-vault/crates/vault-core/src/web_search_browser.rs`：

```rust
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_with_returns_first_existing_path() {
        // 模拟 /usr/bin/chromium 存在
        let target = PathBuf::from("/usr/bin/chromium");
        let result = detect_with(|p: &Path| p == target);
        assert_eq!(result, Some(target));
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
```

- [ ] **Step 2: 注册模块**

修改 `npu-vault/crates/vault-core/src/lib.rs`，在 `pub mod web_search_engines;` 后添加：

```rust
pub mod web_search_browser;
```

- [ ] **Step 3: 跑测试通过**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo test -p vault-core web_search_browser 2>&1 | tail -10
```

Expected: 3 tests passed。

- [ ] **Step 4: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/crates/vault-core/src/lib.rs \
        npu-vault/crates/vault-core/src/web_search_browser.rs
git commit -m "feat(web-search): cross-platform system browser detection"
```

---

## Task 4: 实现 BrowserSearchProvider

### 背景
chromiumoxide 是异步 API，但 `WebSearchProvider::search()` 是同步的（被 chat.rs 里 spawn_blocking 包裹）。在 search() 内创建一个 current-thread tokio runtime 跑异步代码。

**Files:**
- Modify: `npu-vault/crates/vault-core/src/web_search_browser.rs`（追加 Provider 实现）

- [ ] **Step 1: 追加 BrowserSearchProvider 结构和测试**

在 `web_search_browser.rs` 文件末尾（在 `#[cfg(test)] mod tests` 之前）追加：

```rust
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

    /// 异步核心：启动浏览器、加载页面、抓取 HTML、关闭
    async fn fetch_html(&self, url: String) -> Result<String> {
        use chromiumoxide::browser::{Browser, BrowserConfig};
        use futures::StreamExt;

        let config = BrowserConfig::builder()
            .chrome_executable(&self.browser_path)
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("browser config: {e}")))?;

        let (mut browser, mut handler) = tokio::time::timeout(
            BROWSER_LAUNCH_TIMEOUT,
            Browser::launch(config),
        )
        .await
        .map_err(|_| VaultError::LlmUnavailable("browser launch timed out".into()))?
        .map_err(|e| VaultError::LlmUnavailable(format!("browser launch: {e}")))?;

        // handler 任务必须持续 poll，否则 CDP 通道会阻塞
        let handler_task = tokio::spawn(async move {
            while let Some(res) = handler.next().await {
                if res.is_err() {
                    break;
                }
            }
        });

        let result = async {
            let page = browser.new_page(&url).await
                .map_err(|e| VaultError::LlmUnavailable(format!("new_page: {e}")))?;
            tokio::time::timeout(PAGE_LOAD_TIMEOUT, page.wait_for_navigation())
                .await
                .map_err(|_| VaultError::LlmUnavailable("page load timed out".into()))?
                .map_err(|e| VaultError::LlmUnavailable(format!("wait_for_navigation: {e}")))?;
            let html = page.content().await
                .map_err(|e| VaultError::LlmUnavailable(format!("get content: {e}")))?;
            Ok::<String, VaultError>(html)
        }
        .await;

        let _ = browser.close().await;
        handler_task.abort();
        result
    }
}

impl WebSearchProvider for BrowserSearchProvider {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        self.rate_limit();

        let url = self.engine.build_url(query);
        let engine = self.engine.clone();
        let path = self.browser_path.clone();

        // 在 spawn_blocking 上下文内没有 tokio runtime，需要自建一个
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("runtime build: {e}")))?;

        let html = rt.block_on(async {
            // 重新构造一个短命 provider 调用 fetch_html；
            // 不共用 self 防止 Mutex/Arc 跨 runtime 语义问题
            let tmp = BrowserSearchProvider::new(path, engine.clone());
            tmp.fetch_html(url).await
        })?;

        Ok(engine.parse(&html, limit.min(10).max(1)))
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
```

- [ ] **Step 2: 给 vault-core 添加 futures 依赖**

chromiumoxide 的 handler stream 需要 `futures::StreamExt`。修改 `npu-vault/crates/vault-core/Cargo.toml` 追加：

```toml
futures = "0.3"
```

- [ ] **Step 3: 编译验证**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo build -p vault-core 2>&1 | tail -5
```

Expected: `Finished` 无 error。若有 chromiumoxide API 不匹配（版本差异），根据错误信息微调 `BrowserConfig::builder()` 或 `Browser::launch()` 调用。

- [ ] **Step 4: 跑单元测试（不含 ignored）**

```bash
cargo test -p vault-core web_search_browser 2>&1 | tail -10
```

Expected: 3 单元测试通过（浏览器检测相关），integration test 被 skipped。

- [ ] **Step 5: 可选：手动跑集成测试（需系统装 Chrome 和联网）**

```bash
cargo test -p vault-core -- --ignored browser_integration 2>&1 | tail -15
```

Expected: `real_duckduckgo_search ... ok`。如果你在 CI 或无 GUI 环境，跳过此步。

- [ ] **Step 6: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/crates/vault-core/src/web_search_browser.rs \
        npu-vault/crates/vault-core/Cargo.toml \
        npu-vault/Cargo.lock
git commit -m "feat(web-search): BrowserSearchProvider drives system Chrome via chromiumoxide"
```

---

## Task 5: 删除 Brave / Tavily / SearXNG providers 及其测试

### 背景
现在 `web_search.rs` 里还有三个付费 API provider。按 spec §4 全部删除，只保留 `WebSearchProvider` trait、`WebSearchResult` struct、以及更新后的 `from_settings`。

**Files:**
- Modify: `npu-vault/crates/vault-core/src/web_search.rs`（大改）

- [ ] **Step 1: 重写 web_search.rs 保留 trait 和类型，删除三个 provider**

用以下内容**完全替换** `npu-vault/crates/vault-core/src/web_search.rs`：

```rust
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
```

- [ ] **Step 2: 编译并跑 web_search 相关测试**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo test -p vault-core web_search:: 2>&1 | tail -15
```

Expected: 5 tests passed（truncate × 2、from_settings × 3）。

- [ ] **Step 3: 跑完整 vault-core 测试套件**

```bash
cargo test -p vault-core 2>&1 | tail -20
```

Expected: 全部通过。若有引用已删除类型（`BraveSearchProvider` 等）的代码报错，修复这些 import。

- [ ] **Step 4: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/crates/vault-core/src/web_search.rs
git commit -m "refactor(web-search): drop Brave/Tavily/SearXNG API providers, browser is only impl

- 彻底删除三个付费 API provider 及其测试
- WebSearchProvider trait + WebSearchResult 保留作为扩展点
- from_settings 只返回 BrowserSearchProvider 或 None
- 浏览器路径无效时返回 None（不静默降级）"
```

---

## Task 6: 更新 settings 路由

**Files:**
- Modify: `npu-vault/crates/vault-server/src/routes/settings.rs:66-87`

- [ ] **Step 1: 修改 default_settings()**

打开 `npu-vault/crates/vault-server/src/routes/settings.rs`，找到 `fn default_settings()`，把其中的 `"web_search"` 块替换为：

```rust
        "web_search": {
            "enabled": true,
            "engine": "duckduckgo",
            "browser_path": null,
            "min_interval_ms": 2000
        }
```

整段 `default_settings()` 变为：

```rust
fn default_settings() -> serde_json::Value {
    serde_json::json!({
        "injection_mode": "auto",
        "injection_budget": 2000,
        "excluded_domains": ["mail.google.com", "web.whatsapp.com"],
        "search": {
            "default_top_k": 10,
            "vector_weight": 0.6,
            "fulltext_weight": 0.4
        },
        "embedding": {
            "model": "bge-m3",
            "ollama_url": "http://localhost:11434"
        },
        "web_search": {
            "enabled": true,
            "engine": "duckduckgo",
            "browser_path": null,
            "min_interval_ms": 2000
        }
    })
}
```

（`ALLOWED_KEYS` 不改动 —— 只要 `"web_search"` 在白名单里即可，字段结构改变不影响白名单。）

- [ ] **Step 2: 编译验证**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo build -p vault-server 2>&1 | tail -5
```

Expected: `Finished` 无 error。

- [ ] **Step 3: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/crates/vault-server/src/routes/settings.rs
git commit -m "chore(settings): default web_search to browser-only, remove api_key/base_url fields"
```

---

## Task 7: 更新 chat.rs 浏览器不可用时的提示

### 背景
当 `state.web_search` 为 None（系统无 Chromium 浏览器）且本地无结果时，Chat 当前返回空 knowledge 但不告诉用户原因。按 spec §4 "失败时的用户体验"，要追加明确提示。

**Files:**
- Modify: `npu-vault/crates/vault-server/src/routes/chat.rs`（在 fallback 逻辑末尾）

- [ ] **Step 1: 定位当前 fallback 逻辑**

```bash
grep -n "web_search_used\|ws_provider\|knowledge.is_empty" /data/company/project/npu-webhook/npu-vault/crates/vault-server/src/routes/chat.rs | head -20
```

- [ ] **Step 2: 修改 chat.rs，在响应 JSON 中加入 hint 字段**

找到返回 JSON 的部分（搜索 `"web_search_used": web_search_used,`），改为：

```rust
    // 6. Build response with optional hint when web search unavailable
    let mut response_json = serde_json::json!({
        "content": response,
        "citations": citations,
        "knowledge_count": knowledge.len(),
        "session_id": session_id,
        "web_search_used": web_search_used,
    });

    // 本地无结果 + 浏览器不可用：明确告知用户而非静默失败
    if knowledge.is_empty() {
        let ws_available = state.web_search.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if !ws_available {
            response_json["hint"] = serde_json::Value::String(
                "本地知识库无相关内容；网络搜索不可用（未检测到 Chrome 或 Edge 浏览器）。\
                 请安装 Chromium 内核浏览器后重试，或手动录入相关知识。".into(),
            );
        }
    }

    Ok(Json(response_json))
```

将原来的 `Ok(Json(serde_json::json!({...})))` 整块替换。

- [ ] **Step 3: 跑 server 测试**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo test -p vault-server 2>&1 | tail -10
```

Expected: 27 tests passed（与基线一致）。

- [ ] **Step 4: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/crates/vault-server/src/routes/chat.rs
git commit -m "feat(chat): explicit hint when local empty + browser unavailable

明确告知用户原因（未检测到 Chrome/Edge），不静默失败。"
```

---

## Task 8: 更新 npu-vault/README.md

**Files:**
- Modify: `npu-vault/README.md`（全文从头开始的定位段和功能列表）

- [ ] **Step 1: 重写 README 头部**

打开 `npu-vault/README.md`，把开头（第 1-5 行）的：

```markdown
# npu-vault

**本地优先、端到端加密的个人知识库引擎。** 跨 Linux / Windows / NAS（HTTPS 远程），通过 Chrome 扩展、本地文件扫描、文件上传自动积累知识，让云端 AI 更懂你。

单一静态 Rust 二进制，零运行时依赖，28 MB 含完整 Web UI、TLS 和加密搜索引擎。
```

替换为：

```markdown
# npu-vault

**私有 AI 知识伙伴** — 本地决定，全网增强，越用越懂你的专业。

npu-vault 是为知识密集型专业人士打造的本地 AI 知识伙伴。你的专业领域它会越用越懂；本地知识够用时在本地决定，不够用时主动上网补全；所有数据加密存在你自己的设备上，换设备、换工作都能带走。

单一静态 Rust 二进制约 28 MB，含完整 Web UI、TLS 和加密搜索引擎。

## 三大支柱

### 主动进化
它从每次查询中学习，不需要你配置。本地无命中的查询自动沉淀为信号，后台定期让 LLM 分析并生成同义词扩展，静默生效 —— 三个月后搜同一个词结果明显更准。

### 对话伙伴
RAG Chat 为主界面，每条回答带可追溯的引用源；会话持久化并可搜索，跨时间、跨项目的知识能顺着对话接上。

### 混合智能
本地知识库优先；本地无结果时自动通过**后台浏览器自动化**补充（驱动系统已装 Chrome / Edge，零 API 费用）；回答明确标注来源。专业积累留在本地、加密；公开信息现查现用。

## 主权与透明

- Argon2id(64MB/3轮) + AES-256-GCM 字段级加密 + Device Secret 多因子，所有数据本地持有
- 单二进制分发，零运行时依赖
- 换设备通过加密导出/导入无损迁移
- **你只付两样钱**：软件本身 + 你自己的 LLM token（如果你用云端 LLM）。无中间商、无搜索 API 订阅、无隐藏费用
```

- [ ] **Step 2: 重写"功能"列表**

找到 `## 功能` 这一节（当前第 7 行后的列表），整段替换为按三大支柱 + 数据主权重组的新列表：

```markdown
## 核心能力

### 主动进化
- 失败信号自动沉淀 + 后台 SkillEvolver 进化（4h 或累积 10 条信号触发）
- 查询词自动扩展（learned_expansions 静默生效）

### 对话伙伴
- RAG Chat + 引用源追溯（本地文档 / 网络）
- 三阶段检索：vector（usearch HNSW）+ BM25（tantivy + jieba 中文分词）→ rerank → top-k
- 会话持久化 + 跨会话知识联动
- HDBSCAN 聚类"回忆"，自动发现知识主题群组

### 混合智能
- 本地全文 + 向量混合检索
- 浏览器自动化网络搜索（后台驱动系统已装 Chrome / Edge，零 API 成本）
- 可插拔 Embedding（Ollama / ONNX）和 LLM（Ollama / OpenAI 兼容端点）
- 领域插件（patent / law / tech / presales + 运行时加载用户自定义 YAML）
- USPTO 专利实时检索（`POST /api/v1/patent/search`）

### 数据主权与透明
- 加密本地存储（Argon2id + AES-256-GCM + Device Secret）
- 单二进制分发，零运行时依赖
- NAS 模式（`--host 0.0.0.0` + rustls TLS + Bearer token 认证）
- 加密导出 / 导入跨设备迁移
- Chrome 扩展兼容 18 个 API 端点
- 嵌入式 Web UI（单页 HTML，`include_str!` 编译进二进制，移动响应式）
```

- [ ] **Step 3: 新增"目标用户"小节（在"核心能力"之后、"快速开始"之前插入）**

在 README 中找到 `## 快速开始` 行，在它之前插入以下内容（保持与 spec §5 一致，但为 README 场景精简到 4 行表格）：

```markdown
## 谁适合用

| 用户 | 主要价值 |
|------|---------|
| **律师 / 专利代理** | 案件、判例、技术交底长期加密积累；专利 / 法律领域插件；换律所可携带 |
| **研究员 / 学者** | 对话式检索跨课题文献，引用可追溯到原文段落 |
| **独立顾问 / 分析师** | 行业插件 + 本地 + 网络融合检索，跨项目复用方法论 |
| **AI 重度用户 / 技术 Prosumer** | 私有版 AI 记忆：本地加密 + 可插拔 LLM + 自托管 |

详细场景见 [产品定位设计文档](../docs/superpowers/specs/2026-04-17-product-positioning-design.md)。
```

- [ ] **Step 4: 审视改动，确保下面"快速开始"等章节无需配套改动**

```bash
head -80 /data/company/project/npu-webhook/npu-vault/README.md
```

目视确认"快速开始"之后的章节（构建、CLI、HTTP Server、NAS 模式）内容仍有效。若其中还提到 "1Password 式加密"，手动改掉（`grep -n "1Password" /data/company/project/npu-webhook/npu-vault/README.md`）。

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/npu-webhook
git add npu-vault/README.md
git commit -m "docs(README): 重写定位为私有 AI 知识伙伴，按三大支柱重排功能列表"
```

---

## Task 9: 更新仓库顶层 README.md

**Files:**
- Modify: `README.md`

- [ ] **Step 1: 重写开头定位段**

打开 `/data/company/project/npu-webhook/README.md`，把第 1-11 行替换为：

```markdown
# npu-webhook

个人 AI 知识库 + 记忆增强系统。

本仓库包含两条并行的产品线：

- **Python 原型线**（本目录 `src/npu_webhook/`）— 快速验证算法与实验特性。基于 FastAPI + ChromaDB + SQLite FTS5
- **Rust 商用线**（`npu-vault/`）— 面向知识密集型专业人士的**私有 AI 知识伙伴**：主动进化、对话式、混合智能、本地加密。详见 [`npu-vault/README.md`](npu-vault/README.md)

Chrome 扩展协议相同，两个后端可任意切换。
```

- [ ] **Step 2: 检查并移除任何剩余的"1Password"提及**

```bash
grep -n "1Password" /data/company/project/npu-webhook/README.md
```

如有匹配，按上下文改写为描述底层密码学（Argon2id + AES-256-GCM + Device Secret）。

- [ ] **Step 3: Commit**

```bash
cd /data/company/project/npu-webhook
git add README.md
git commit -m "docs(root-readme): 同步新定位，移除 1Password 类比"
```

---

## Task 10: 更新仓库顶层 CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: 定位需要改的段落**

```bash
grep -n "1Password\|Rust 商用线" /data/company/project/npu-webhook/CLAUDE.md
```

- [ ] **Step 2: 修改 "双产品线架构" 中 Rust 商用线的描述**

找到描述 Rust 商用线特性的段落（类似"1Password 式加密（Argon2id + AES-256-GCM + Device Secret）"的一行），改为：

```markdown
   - Axum + rusqlite + tantivy + usearch + hdbscan
   - 加密模型：Argon2id + AES-256-GCM + Device Secret
   - 定位：**私有 AI 知识伙伴**（主动进化 + 对话式 + 混合智能，详见 docs/superpowers/specs/2026-04-17-product-positioning-design.md）
```

（仅删除 "1Password 式" 三个字 + 追加定位一行，其他特性点保留）

- [ ] **Step 3: 验证无剩余 1Password 提及**

```bash
grep -n "1Password" /data/company/project/npu-webhook/CLAUDE.md
```

Expected: 无输出。

- [ ] **Step 4: Commit**

```bash
cd /data/company/project/npu-webhook
git add CLAUDE.md
git commit -m "docs(claude-md): 同步 Rust 商用线新定位到项目指令"
```

---

## Task 11: 更新 npu-vault/DEVELOP.md

**Files:**
- Modify: `npu-vault/DEVELOP.md`（仅开头）

- [ ] **Step 1: 检查开头**

```bash
head -10 /data/company/project/npu-webhook/npu-vault/DEVELOP.md
```

- [ ] **Step 2: 若开头有产品定位描述，更新为新 tagline**

如果开头有类似 "npu-vault 是..." 的介绍段，替换为：

```markdown
# npu-vault 开发文档

**私有 AI 知识伙伴** — 本地决定，全网增强，越用越懂你的专业。

本文档面向开发者，说明代码结构、构建流程和扩展点。面向用户的介绍见 [README.md](README.md)。
```

（如果 DEVELOP.md 开头没有定位描述，跳过此步，Task 11 改动为 0 行，不 commit）

- [ ] **Step 3: Commit（如果有改动）**

```bash
cd /data/company/project/npu-webhook
if ! git diff --quiet npu-vault/DEVELOP.md; then
  git add npu-vault/DEVELOP.md
  git commit -m "docs(develop): 同步开头定位"
fi
```

---

## Task 12: 全量回归测试

**Files:**
- 无改动，仅验证

- [ ] **Step 1: 跑 vault-core 所有测试**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo test -p vault-core 2>&1 | tail -10
```

Expected: 全部通过（包含 web_search / web_search_engines / web_search_browser 的新测试，以及 skill_evolution 等已有测试）。

- [ ] **Step 2: 跑 vault-server 所有测试**

```bash
cargo test -p vault-server 2>&1 | tail -10
```

Expected: 27 tests passed（与基线一致）。

- [ ] **Step 3: 全工作区测试**

```bash
cd /data/company/project/npu-webhook/npu-vault
cargo test 2>&1 | tail -15
```

Expected: 全部 suite 通过，无 failure。

- [ ] **Step 4: 可选 — 手动 smoke test 浏览器搜索**

若本地有 Chrome 且联网，跑集成测试验证端到端：

```bash
cargo test -p vault-core -- --ignored browser_integration 2>&1 | tail -10
```

Expected: `real_duckduckgo_search ... ok`，能看到从 DuckDuckGo 抓取到真实结果。

- [ ] **Step 5: Git log 确认**

```bash
cd /data/company/project/npu-webhook
git log --oneline feature/search-rerank-infer ^main | head -20
```

Expected: 能看到本 plan 的 11 个 feat/refactor/chore/docs commits 整齐排列在分支最前端。

---

## 实施完成后

按 superpowers:finishing-a-development-branch 技能流程：

1. 运行全量测试确认绿色
2. 呈现四选一：
   - Merge to main 本地合并
   - Push + Create PR
   - 保持分支
   - 丢弃

由用户选择如何收尾当前分支。本 plan 覆盖从 spec 到可 merge 的全部工作。
