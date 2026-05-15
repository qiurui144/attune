//! Tool calling foundation (Sprint v0.7 / F6).
//!
//! 提供通用 Tool trait + ToolRegistry，让 LLM 通过 OpenAI-style function calling
//! 调用各 plugin skill / web search / fs read 等能力。本 commit 只搭基础设施 + 2
//! 个内置 tool；plugin tool 注入 + LLM 实际 tool-call loop 留给后续 commit。
//!
//! ## 设计原则
//!
//! 1. **schema 用 JSON Schema 子集**：与 OpenAI function calling / Claude tool use
//!    协议直接兼容（type / properties / required / description），不另造 DSL
//! 2. **trait 用 async-trait**：`invoke()` 必须 async（FS / network / subprocess）
//! 3. **dyn 安全**：tools 在 ToolRegistry 里以 `Arc<dyn Tool>` 存，可跨线程共享
//! 4. **FS tool 严格沙箱**：所有 fs 操作限定在 `~/.local/share/attune/` 下，
//!    防止 path traversal（`..` / 绝对路径 / symlink 逃逸）

use crate::error::{Result, VaultError};
use crate::web_search::WebSearchProvider;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

/// 通用 boxed future 别名 —— 不依赖 async-trait crate 实现 dyn-compatible
/// async trait。所有 Tool::invoke 返回此类型。
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>>;

// ── Trait & Registry ─────────────────────────────────────────────────────────

/// 通用 Tool 抽象。LLM 通过 function calling 调用本接口。
///
/// 实现注意：
///   - `name()` 必须全局唯一，建议使用 `<namespace>_<verb>` 格式（如 `fs_read`）
///   - `schema()` 返回 JSON Schema (object/properties/required)，用于发给 LLM 提示
///   - `invoke()` 内部应做严格输入校验 —— LLM 生成的 args 不可信
///
/// `invoke` 用手写 `BoxFuture` 而非 `#[async_trait]`：attune-core 当前不直接依赖
/// `async-trait` crate（虽然 transitive 存在）。手写 boxed future 维持 dyn 兼容。
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    fn invoke<'a>(&'a self, args: Value) -> ToolFuture<'a>;
}

/// 全局 Tool 注册表。线程安全，可 clone 共享。
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    pub fn lookup(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// 列出所有 tool 的 OpenAI function-calling tool spec —— 整批发给 LLM
    /// 作为 `tools` 字段，供 LLM 决定调谁。
    pub fn list(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.schema(),
                    }
                })
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

// ── Builtin: WebSearchTool ───────────────────────────────────────────────────

/// Builtin tool: 让 LLM 触发后台浏览器自动化网络搜索。
///
/// 输入 schema:
///   { query: string (required), limit: int (optional, default 5, max 10) }
pub struct WebSearchTool {
    provider: Arc<dyn WebSearchProvider>,
}

impl WebSearchTool {
    pub fn new(provider: Arc<dyn WebSearchProvider>) -> Self {
        Self { provider }
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the public web for up-to-date information when local knowledge base \
         does not contain the answer. Returns titles, URLs and snippets."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (natural language)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max number of results to return (1-10). Default 5.",
                    "minimum": 1,
                    "maximum": 10,
                }
            },
            "required": ["query"]
        })
    }

    fn invoke<'a>(&'a self, args: Value) -> ToolFuture<'a> {
        let provider = self.provider.clone();
        Box::pin(async move {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| VaultError::InvalidInput("missing 'query' field".into()))?;
            if query.trim().is_empty() {
                return Err(VaultError::InvalidInput("query cannot be empty".into()));
            }

            let limit_raw = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5);
            let limit = limit_raw.clamp(1, 10) as usize;

            let q = query.to_string();
            // WebSearchProvider::search 是同步阻塞调用（chromiumoxide 内部 spawn 浏览器）
            // 必须放进 spawn_blocking，避免阻塞 async runtime worker
            let results = tokio::task::spawn_blocking(move || provider.search(&q, limit))
                .await
                .map_err(|e| VaultError::InvalidInput(format!("tool task join: {e}")))??;

            let items: Vec<Value> = results
                .iter()
                .map(|r| {
                    json!({
                        "title": r.title,
                        "url": r.url,
                        "snippet": r.snippet,
                        "published_date": r.published_date,
                    })
                })
                .collect();
            Ok(json!({ "results": items, "count": items.len() }))
        })
    }
}

// ── Builtin: FsReadTool ──────────────────────────────────────────────────────

/// Builtin tool: 让 LLM 读 attune data dir 下的文件。
///
/// 严格沙箱：所有路径必须在 `~/.local/share/attune/` (或 dirs::data_dir/attune)
/// 之下。拒绝绝对路径、`..` 父目录跳转、symlink 逃逸。
pub struct FsReadTool {
    /// 沙箱根目录（已 canonicalize，无 symlink）。
    root: PathBuf,
}

impl FsReadTool {
    /// 用 attune 默认 data dir 创建（生产路径）。
    pub fn with_default_root() -> Result<Self> {
        let base = dirs::data_dir()
            .ok_or_else(|| VaultError::InvalidInput("system data_dir unavailable".into()))?
            .join("attune");
        std::fs::create_dir_all(&base)?;
        // canonicalize 一次，后续比较都是 canonical 路径
        let root = dunce_canonicalize(&base)?;
        Ok(Self { root })
    }

    /// 用自定义根目录创建（测试 / 行业 plugin 复用）。
    pub fn with_root(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        let root = dunce_canonicalize(&root)?;
        Ok(Self { root })
    }

    /// 校验输入路径合法且在 root 沙箱内。
    ///
    /// 拒绝：
    ///   - 绝对路径（必须 root 相对）
    ///   - 包含 `..` 的相对路径（即使最终路径在 root 内也拒，避免 LLM 学到歧义模板）
    ///   - canonicalize 后不在 root prefix 之下（symlink 逃逸防御）
    fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let p = Path::new(rel);
        if p.is_absolute() {
            return Err(VaultError::InvalidInput(format!(
                "absolute path not allowed: {rel}"
            )));
        }
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    return Err(VaultError::InvalidInput(format!(
                        "parent-dir component '..' not allowed: {rel}"
                    )));
                }
                Component::Prefix(_) | Component::RootDir => {
                    return Err(VaultError::InvalidInput(format!(
                        "root/prefix component not allowed: {rel}"
                    )));
                }
                _ => {}
            }
        }
        let candidate = self.root.join(p);
        // 文件可能尚未存在 —— 用 parent canonicalize 防 symlink 逃逸
        let parent = candidate
            .parent()
            .ok_or_else(|| VaultError::InvalidInput("path has no parent".into()))?;
        let parent_real = dunce_canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        if !parent_real.starts_with(&self.root) {
            return Err(VaultError::InvalidInput(format!(
                "path escapes sandbox: {}",
                candidate.display()
            )));
        }
        Ok(candidate)
    }
}

impl Tool for FsReadTool {
    fn name(&self) -> &str {
        "fs_read"
    }

    fn description(&self) -> &str {
        "Read a text file inside the attune data directory. Paths must be \
         relative to the attune data root; '..' and absolute paths are rejected."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path inside attune data dir, e.g. 'vault/foo.md'."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Max bytes to read. Default 65536, max 1MB.",
                    "minimum": 1,
                    "maximum": 1_048_576,
                }
            },
            "required": ["path"]
        })
    }

    fn invoke<'a>(&'a self, args: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let rel = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| VaultError::InvalidInput("missing 'path' field".into()))?;
            let max_bytes = args
                .get("max_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(65_536)
                .min(1_048_576) as usize;

            let path = self.resolve(rel)?;
            let rel_owned = rel.to_string();

            let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
                let data = std::fs::read(&path)?;
                Ok(data.into_iter().take(max_bytes).collect())
            })
            .await
            .map_err(|e| VaultError::InvalidInput(format!("fs_read task join: {e}")))??;

            let content = String::from_utf8_lossy(&bytes).into_owned();
            Ok(json!({
                "path": rel_owned,
                "size": bytes.len(),
                "truncated": bytes.len() == max_bytes,
                "content": content,
            }))
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Canonicalize 但不引入 dunce 依赖（attune-server 用 dunce，core 不直接依赖）。
/// std::fs::canonicalize 在 Windows 上会带 `\\?\` UNC 前缀，但本模块 root 比对
/// 用同一函数 canonicalize 双侧，前缀对称就不出 bug。Linux 上 std 本身即正确。
fn dunce_canonicalize(p: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(p).map_err(|e| {
        VaultError::Io(std::io::Error::new(
            e.kind(),
            format!("canonicalize {} failed: {e}", p.display()),
        ))
    })
}

// ── 单元测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_search::{WebSearchProvider, WebSearchResult};

    struct MockWebSearch {
        results: Vec<WebSearchResult>,
    }

    impl WebSearchProvider for MockWebSearch {
        fn search(&self, _q: &str, limit: usize) -> Result<Vec<WebSearchResult>> {
            Ok(self.results.iter().take(limit).cloned().collect())
        }
        fn provider_name(&self) -> &str {
            "mock"
        }
        fn is_configured(&self) -> bool {
            true
        }
    }

    fn mock_provider() -> Arc<dyn WebSearchProvider> {
        Arc::new(MockWebSearch {
            results: vec![
                WebSearchResult {
                    title: "Rust async book".into(),
                    url: "https://rust-lang.github.io/async-book".into(),
                    snippet: "Async/await in Rust".into(),
                    published_date: None,
                },
                WebSearchResult {
                    title: "Tokio docs".into(),
                    url: "https://tokio.rs".into(),
                    snippet: "Tokio runtime".into(),
                    published_date: None,
                },
            ],
        })
    }

    #[test]
    fn registry_register_and_lookup() {
        let mut reg = ToolRegistry::new();
        assert!(reg.is_empty());
        let tool: Arc<dyn Tool> = Arc::new(WebSearchTool::new(mock_provider()));
        reg.register(tool);
        assert_eq!(reg.len(), 1);
        assert!(reg.lookup("web_search").is_some());
        assert!(reg.lookup("nonexistent").is_none());
    }

    #[test]
    fn registry_list_returns_openai_function_spec() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(WebSearchTool::new(mock_provider())));
        let specs = reg.list();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.get("type").and_then(|v| v.as_str()), Some("function"));
        let func = spec.get("function").unwrap();
        assert_eq!(func.get("name").and_then(|v| v.as_str()), Some("web_search"));
        assert!(func.get("description").is_some());
        assert!(func.get("parameters").is_some());
    }

    #[test]
    fn web_search_tool_schema_has_required_query() {
        let t = WebSearchTool::new(mock_provider());
        let schema = t.schema();
        let req = schema.get("required").unwrap().as_array().unwrap();
        assert!(req.iter().any(|v| v.as_str() == Some("query")));
    }

    #[tokio::test]
    async fn web_search_tool_invokes_mock_provider() {
        let t = WebSearchTool::new(mock_provider());
        let out = t.invoke(json!({ "query": "rust async", "limit": 5 })).await.unwrap();
        let results = out.get("results").unwrap().as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(out.get("count").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(
            results[0].get("title").and_then(|v| v.as_str()),
            Some("Rust async book")
        );
    }

    #[tokio::test]
    async fn web_search_tool_rejects_empty_query() {
        let t = WebSearchTool::new(mock_provider());
        let err = t.invoke(json!({ "query": "  " })).await.unwrap_err();
        match err {
            VaultError::InvalidInput(_) => {}
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fs_read_tool_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let t = FsReadTool::with_root(tmp.path().to_path_buf()).unwrap();

        // 绝对路径拒绝
        let abs = if cfg!(windows) { "C:/etc/passwd" } else { "/etc/passwd" };
        assert!(t.invoke(json!({ "path": abs })).await.is_err());

        // .. 拒绝
        assert!(t.invoke(json!({ "path": "../escape.txt" })).await.is_err());
        assert!(t.invoke(json!({ "path": "sub/../../escape.txt" })).await.is_err());
    }

    #[tokio::test]
    async fn fs_read_tool_reads_within_sandbox() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("hello.txt"), "hello tool").unwrap();
        let t = FsReadTool::with_root(tmp.path().to_path_buf()).unwrap();
        let out = t.invoke(json!({ "path": "hello.txt" })).await.unwrap();
        assert_eq!(
            out.get("content").and_then(|v| v.as_str()),
            Some("hello tool")
        );
        assert_eq!(out.get("size").and_then(|v| v.as_u64()), Some(10));
    }
}
