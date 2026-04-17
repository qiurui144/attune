# npu-vault Phase 2a: Axum API Server + 搜索引擎 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Phase 1 加密存储引擎之上，构建 Axum HTTP API Server + tantivy 全文搜索 + usearch 向量搜索 + Ollama embedding client + RRF 混合搜索，实现 Chrome 扩展可对接的完整后端。

**Architecture:** vault-server 是 Axum binary crate，依赖 vault-core。vault-core 新增 6 个模块（chunker/parser/embed/index/vectors/search）。所有搜索/索引操作都在 UNLOCKED 状态下进行，数据通过 DEK 加密存储。

**Tech Stack:** axum 0.8+, tokio, tower-http (CORS), tantivy 0.26, tantivy-jieba 0.19, usearch 2.24, reqwest, serde, uuid, chrono

**Design Spec:** `docs/superpowers/specs/2026-03-31-npu-vault-design.md` (Sections 5-6, 8-9)

**Depends on:** Phase 1 complete (vault-core: crypto/vault/store/platform/error, vault-cli)

---

## File Structure

### vault-core 新增模块

```
npu-vault/crates/vault-core/src/
├── chunker.rs      # 滑动窗口分块 + extract_sections 语义切割
├── parser.rs       # 文件解析 (md/txt/pdf/docx/code) → (title, content)
├── embed.rs        # Ollama HTTP embedding client (reqwest)
├── index.rs        # tantivy 全文索引封装 (加密索引目录)
├── vectors.rs      # usearch 向量索引封装 (加密向量文件)
└── search.rs       # RRF 混合搜索 + 层级检索 + 动态预算
```

### vault-server 新增 crate

```
npu-vault/crates/vault-server/
├── Cargo.toml
└── src/
    ├── main.rs         # clap CLI + Axum server bootstrap
    ├── state.rs        # Arc<AppState> (Vault + 索引 + embedding)
    ├── middleware.rs    # vault_guard (UNLOCKED 检查 + token 验证)
    └── routes/
        ├── mod.rs      # 路由注册
        ├── vault.rs    # /vault/* (status/setup/unlock/lock/change-password)
        ├── ingest.rs   # POST /ingest
        ├── search.rs   # GET /search + POST /search/relevant
        ├── items.rs    # GET/PATCH/DELETE /items
        └── status.rs   # GET /status + GET /status/health
```

---

### Task 1: vault-core — chunker.rs 分块模块

**Files:**
- Create: `npu-vault/crates/vault-core/src/chunker.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Write chunker.rs**

```rust
// npu-vault/crates/vault-core/src/chunker.rs

/// 滑动窗口分块 + 语义章节切割
/// 复用 npu-webhook Python 实现的逻辑

pub const DEFAULT_CHUNK_SIZE: usize = 512;
pub const DEFAULT_OVERLAP: usize = 128;
pub const SECTION_TARGET_SIZE: usize = 1500;

/// 滑动窗口分块（字符级，句子边界感知）
pub fn chunk(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        // 尝试在句子边界切割
        let actual_end = if end < chars.len() {
            find_sentence_boundary(&chars, start, end).unwrap_or(end)
        } else {
            end
        };
        let chunk_text: String = chars[start..actual_end].iter().collect();
        if !chunk_text.trim().is_empty() {
            chunks.push(chunk_text);
        }
        if actual_end >= chars.len() {
            break;
        }
        start = if actual_end > overlap { actual_end - overlap } else { 0 };
        if start == 0 && !chunks.is_empty() {
            break; // 防止无限循环
        }
    }
    chunks
}

/// 语义章节切割: Markdown 标题 / 代码 def|class / 段落大小
pub fn extract_sections(content: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }
    let mut sections: Vec<(usize, String)> = Vec::new();
    let mut current_section = String::new();
    let mut section_idx: usize = 0;

    for line in &lines {
        let is_boundary = line.starts_with("# ")
            || line.starts_with("## ")
            || line.starts_with("### ")
            || line.starts_with("def ")
            || line.starts_with("class ")
            || line.starts_with("fn ")
            || line.starts_with("pub fn ")
            || line.starts_with("impl ");

        if is_boundary && !current_section.trim().is_empty() {
            sections.push((section_idx, current_section.clone()));
            section_idx += 1;
            current_section.clear();
        }

        current_section.push_str(line);
        current_section.push('\n');

        // 段落大小限制
        if current_section.len() >= SECTION_TARGET_SIZE && !is_boundary {
            // 尝试在空行处切割
            if line.trim().is_empty() {
                sections.push((section_idx, current_section.clone()));
                section_idx += 1;
                current_section.clear();
            }
        }
    }
    if !current_section.trim().is_empty() {
        sections.push((section_idx, current_section));
    }
    sections
}

fn find_sentence_boundary(chars: &[char], start: usize, end: usize) -> Option<usize> {
    // 从 end 往回找句子结束符
    let search_start = if end > start + 50 { end - 50 } else { start };
    for i in (search_start..end).rev() {
        let c = chars[i];
        if c == '。' || c == '.' || c == '!' || c == '?' || c == '\n' || c == '！' || c == '？' {
            return Some(i + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_short_text_single() {
        let chunks = chunk("Hello world", 512, 128);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn chunk_long_text_multiple() {
        let text = "A".repeat(1000);
        let chunks = chunk(&text, 512, 128);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].len() <= 512);
    }

    #[test]
    fn extract_sections_markdown() {
        let content = "# Title\n\nIntro paragraph.\n\n## Section 1\n\nContent 1.\n\n## Section 2\n\nContent 2.";
        let sections = extract_sections(content);
        assert!(sections.len() >= 2, "Should split on ## headings: got {}", sections.len());
        assert!(sections[0].1.contains("Title"));
    }

    #[test]
    fn extract_sections_code() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n\npub fn helper() {\n    // code\n}";
        let sections = extract_sections(content);
        assert!(sections.len() >= 2, "Should split on fn boundaries: got {}", sections.len());
    }

    #[test]
    fn extract_sections_empty() {
        let sections = extract_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn chunk_with_chinese() {
        let text = "这是一段中文内容。".repeat(100);
        let chunks = chunk(&text, 512, 128);
        assert!(chunks.len() >= 1);
        for c in &chunks {
            assert!(!c.is_empty());
        }
    }
}
```

- [ ] **Step 2: Register in lib.rs**

Add `pub mod chunker;` to lib.rs.

- [ ] **Step 3: Run tests**

Run: `cargo test -p vault-core chunker::tests`
Expected: 6 tests PASS

---

### Task 2: vault-core — parser.rs 文件解析

**Files:**
- Create: `npu-vault/crates/vault-core/src/parser.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Write parser.rs**

Phase 2a 先实现 MD/TXT/代码解析（纯 Rust 内置），PDF/DOCX 在 Phase 2b 添加。

```rust
// npu-vault/crates/vault-core/src/parser.rs

use std::path::Path;
use crate::error::{Result, VaultError};

/// 代码文件扩展名
const CODE_EXTENSIONS: &[&str] = &[
    ".py", ".js", ".ts", ".rs", ".go", ".java", ".c", ".cpp", ".h",
    ".rb", ".php", ".swift", ".kt", ".scala", ".sh", ".bash", ".zsh",
    ".toml", ".yaml", ".yml", ".json", ".xml", ".html", ".css",
];

/// 解析文件 → (title, content)
pub fn parse_file(path: &Path) -> Result<(String, String)> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| VaultError::Io(e))?;
    let filename = path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    parse_content(&content, &filename)
}

/// 从内存解析 → (title, content)
pub fn parse_bytes(data: &[u8], filename: &str) -> Result<(String, String)> {
    let content = String::from_utf8_lossy(data).to_string();
    parse_content(&content, filename)
}

fn parse_content(content: &str, filename: &str) -> Result<(String, String)> {
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());

    let title = if ext == ".md" {
        // Markdown: 提取第一个 # 标题
        content.lines()
            .find(|l| l.trim().starts_with("# "))
            .map(|l| l.trim().trim_start_matches("# ").trim().to_string())
            .unwrap_or(stem)
    } else if CODE_EXTENSIONS.iter().any(|e| *e == ext) {
        filename.to_string()
    } else {
        // TXT 等: 首行作标题
        content.lines().next()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim()[..l.trim().len().min(100)].to_string())
            .unwrap_or(stem)
    };

    Ok((title, content.to_string()))
}

/// 检查文件是否为支持的类型
pub fn is_supported(path: &Path) -> bool {
    let ext = path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    matches!(ext.as_str(), ".md" | ".txt" | ".pdf" | ".docx")
        || CODE_EXTENSIONS.iter().any(|e| *e == ext)
}

/// 计算文件的 SHA-256 hash
pub fn file_hash(path: &Path) -> Result<String> {
    use sha2::{Sha256, Digest};
    let data = std::fs::read(path)?;
    let hash = Sha256::digest(&data);
    Ok(hex::encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_markdown_title() {
        let (title, content) = parse_content("# My Title\n\nSome content.", "doc.md").unwrap();
        assert_eq!(title, "My Title");
        assert!(content.contains("Some content"));
    }

    #[test]
    fn parse_txt_first_line() {
        let (title, _) = parse_content("First line\nSecond line", "notes.txt").unwrap();
        assert_eq!(title, "First line");
    }

    #[test]
    fn parse_code_filename() {
        let (title, content) = parse_content("fn main() {}", "app.rs").unwrap();
        assert_eq!(title, "app.rs");
        assert!(content.contains("fn main"));
    }

    #[test]
    fn parse_bytes_works() {
        let (title, content) = parse_bytes(b"# Hello\n\nWorld", "test.md").unwrap();
        assert_eq!(title, "Hello");
        assert!(content.contains("World"));
    }

    #[test]
    fn is_supported_types() {
        assert!(is_supported(Path::new("doc.md")));
        assert!(is_supported(Path::new("code.py")));
        assert!(is_supported(Path::new("data.txt")));
        assert!(is_supported(Path::new("app.rs")));
        assert!(!is_supported(Path::new("image.png")));
        assert!(!is_supported(Path::new("video.mp4")));
    }

    #[test]
    fn file_hash_deterministic() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"test content").unwrap();

        let h1 = file_hash(&path).unwrap();
        let h2 = file_hash(&path).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }
}
```

- [ ] **Step 2: Register in lib.rs** — Add `pub mod parser;`

- [ ] **Step 3: Run tests**

Run: `cargo test -p vault-core parser::tests`
Expected: 6 tests PASS

---

### Task 3: vault-core — embed.rs Ollama HTTP Client

**Files:**
- Create: `npu-vault/crates/vault-core/src/embed.rs`
- Modify: `npu-vault/crates/vault-core/Cargo.toml`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Add reqwest + tokio dependencies**

Add to vault-core `[dependencies]`:

```toml
reqwest = { version = "0.12", features = ["json"] }
tokio = { version = "1", features = ["full"] }
```

- [ ] **Step 2: Write embed.rs**

```rust
// npu-vault/crates/vault-core/src/embed.rs

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};

/// Embedding provider trait
pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
    fn is_available(&self) -> bool;
}

/// Ollama HTTP embedding client
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dims: usize,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl OllamaProvider {
    pub fn new(base_url: &str, model: &str, dims: usize) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dims,
        }
    }

    pub fn default() -> Self {
        Self::new("http://localhost:11434", "bge-m3", 1024)
    }

    /// 检查 Ollama 是否可用
    pub fn check_health(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        let rt = tokio::runtime::Handle::try_current();
        match rt {
            Ok(handle) => {
                // 在 async 上下文中
                let client = self.client.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async { client.get(&url).send().await.is_ok() })
                }).join().unwrap_or(false)
            }
            Err(_) => {
                // 在 sync 上下文中
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async { self.client.get(&url).send().await.is_ok() })
            }
        }
    }
}

impl EmbeddingProvider for OllamaProvider {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.base_url);
        let body = EmbedRequest {
            model: &self.model,
            input: texts.to_vec(),
        };

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| VaultError::Crypto(format!("tokio runtime: {e}")))?;

        let response = rt.block_on(async {
            self.client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| VaultError::Crypto(format!("ollama request: {e}")))?
                .json::<EmbedResponse>()
                .await
                .map_err(|e| VaultError::Crypto(format!("ollama response: {e}")))
        })?;

        Ok(response.embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn is_available(&self) -> bool {
        self.check_health()
    }
}

/// 无操作 embedding provider（降级模式）
pub struct NoopProvider;

impl EmbeddingProvider for NoopProvider {
    fn embed(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Err(VaultError::Crypto("no embedding provider available".into()))
    }
    fn dimensions(&self) -> usize { 0 }
    fn is_available(&self) -> bool { false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_provider_not_available() {
        let provider = NoopProvider;
        assert!(!provider.is_available());
        assert!(provider.embed(&["test"]).is_err());
        assert_eq!(provider.dimensions(), 0);
    }

    #[test]
    fn ollama_provider_creation() {
        let provider = OllamaProvider::new("http://localhost:11434", "bge-m3", 1024);
        assert_eq!(provider.dimensions(), 1024);
        // 不测试实际连接（CI 环境可能无 Ollama）
    }
}
```

- [ ] **Step 3: Register in lib.rs** — Add `pub mod embed;`

- [ ] **Step 4: Run tests**

Run: `cargo test -p vault-core embed::tests`
Expected: 2 tests PASS

---

### Task 4: vault-core — index.rs tantivy 全文搜索

**Files:**
- Create: `npu-vault/crates/vault-core/src/index.rs`
- Modify: `npu-vault/crates/vault-core/Cargo.toml`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Add tantivy + jieba dependencies**

```toml
tantivy = "0.22"
tantivy-jieba = "0.11"
```

注意: tantivy-jieba 0.19 要求 tantivy 0.22。如果版本冲突，使用 tantivy-jieba 兼容的 tantivy 版本。编译时根据实际错误调整。

- [ ] **Step 2: Write index.rs**

```rust
// npu-vault/crates/vault-core/src/index.rs

use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, ReloadPolicy};
use crate::error::{Result, VaultError};

const HEAP_SIZE: usize = 50_000_000; // 50 MB writer heap

pub struct FulltextIndex {
    index: Index,
    schema: Schema,
    // field handles
    f_item_id: Field,
    f_title: Field,
    f_content: Field,
    f_source_type: Field,
}

impl FulltextIndex {
    /// 创建内存索引（测试用）
    pub fn open_memory() -> Result<Self> {
        let schema = Self::build_schema();
        let index = Index::create_in_ram(schema.clone());
        Self::register_tokenizers(&index);
        let f_item_id = schema.get_field("item_id").unwrap();
        let f_title = schema.get_field("title").unwrap();
        let f_content = schema.get_field("content").unwrap();
        let f_source_type = schema.get_field("source_type").unwrap();
        Ok(Self { index, schema, f_item_id, f_title, f_content, f_source_type })
    }

    /// 打开持久化索引
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let schema = Self::build_schema();
        let index = if dir.join("meta.json").exists() {
            Index::open_in_dir(dir)
                .map_err(|e| VaultError::Crypto(format!("tantivy open: {e}")))?
        } else {
            Index::create_in_dir(dir, schema.clone())
                .map_err(|e| VaultError::Crypto(format!("tantivy create: {e}")))?
        };
        Self::register_tokenizers(&index);
        let f_item_id = schema.get_field("item_id").unwrap();
        let f_title = schema.get_field("title").unwrap();
        let f_content = schema.get_field("content").unwrap();
        let f_source_type = schema.get_field("source_type").unwrap();
        Ok(Self { index, schema, f_item_id, f_title, f_content, f_source_type })
    }

    fn build_schema() -> Schema {
        let mut builder = Schema::builder();
        builder.add_text_field("item_id", STRING | STORED);
        builder.add_text_field("title", TEXT | STORED);
        builder.add_text_field("content", TEXT);
        builder.add_text_field("source_type", STRING | STORED);
        builder.build()
    }

    fn register_tokenizers(index: &Index) {
        // 注册 jieba 分词器用于中文
        let tokenizer = tantivy_jieba::JiebaTokenizer {};
        index.tokenizers().register("jieba", tokenizer);
    }

    /// 添加文档到索引
    pub fn add_document(&self, item_id: &str, title: &str, content: &str, source_type: &str) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(HEAP_SIZE)
            .map_err(|e| VaultError::Crypto(format!("tantivy writer: {e}")))?;
        writer.add_document(doc!(
            self.f_item_id => item_id,
            self.f_title => title,
            self.f_content => content,
            self.f_source_type => source_type,
        )).map_err(|e| VaultError::Crypto(format!("tantivy add: {e}")))?;
        writer.commit()
            .map_err(|e| VaultError::Crypto(format!("tantivy commit: {e}")))?;
        Ok(())
    }

    /// 删除文档（by item_id）
    pub fn delete_document(&self, item_id: &str) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(HEAP_SIZE)
            .map_err(|e| VaultError::Crypto(format!("tantivy writer: {e}")))?;
        let term = tantivy::Term::from_field_text(self.f_item_id, item_id);
        writer.delete_term(term);
        writer.commit()
            .map_err(|e| VaultError::Crypto(format!("tantivy commit: {e}")))?;
        Ok(())
    }

    /// BM25 搜索 → Vec<(item_id, score)>
    pub fn search(&self, query_str: &str, top_k: usize) -> Result<Vec<(String, f32)>> {
        let reader = self.index.reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| VaultError::Crypto(format!("tantivy reader: {e}")))?;
        let searcher = reader.searcher();

        let query_parser = QueryParser::for_index(&self.index, vec![self.f_title, self.f_content]);
        let query = query_parser.parse_query(query_str)
            .map_err(|e| VaultError::Crypto(format!("tantivy query: {e}")))?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(top_k))
            .map_err(|e| VaultError::Crypto(format!("tantivy search: {e}")))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)
                .map_err(|e| VaultError::Crypto(format!("tantivy doc: {e}")))?;
            if let Some(item_id) = doc.get_first(self.f_item_id).and_then(|v| v.as_str()) {
                results.push((item_id.to_string(), score));
            }
        }
        Ok(results)
    }

    pub fn doc_count(&self) -> Result<usize> {
        let reader = self.index.reader()
            .map_err(|e| VaultError::Crypto(format!("tantivy reader: {e}")))?;
        Ok(reader.searcher().num_docs() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_memory_index() {
        let idx = FulltextIndex::open_memory().unwrap();
        assert_eq!(idx.doc_count().unwrap(), 0);
    }

    #[test]
    fn add_and_search() {
        let idx = FulltextIndex::open_memory().unwrap();
        idx.add_document("item1", "Rust编程", "Rust是一门系统编程语言", "note").unwrap();
        idx.add_document("item2", "Python学习", "Python是一门脚本语言", "note").unwrap();

        let results = idx.search("Rust", 10).unwrap();
        assert!(!results.is_empty(), "Should find Rust document");
        assert_eq!(results[0].0, "item1");
    }

    #[test]
    fn delete_document() {
        let idx = FulltextIndex::open_memory().unwrap();
        idx.add_document("item1", "Test", "Content", "note").unwrap();
        assert_eq!(idx.doc_count().unwrap(), 1);

        idx.delete_document("item1").unwrap();
        assert_eq!(idx.doc_count().unwrap(), 0);
    }

    #[test]
    fn persistent_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tantivy");

        // Create and add
        {
            let idx = FulltextIndex::open(&path).unwrap();
            idx.add_document("id1", "Title", "Content here", "note").unwrap();
        }
        // Reopen and verify
        {
            let idx = FulltextIndex::open(&path).unwrap();
            assert_eq!(idx.doc_count().unwrap(), 1);
            let results = idx.search("Content", 10).unwrap();
            assert!(!results.is_empty());
        }
    }
}
```

- [ ] **Step 3: Register in lib.rs** — Add `pub mod index;`

- [ ] **Step 4: Run tests**

Run: `cargo test -p vault-core index::tests`
Expected: 4 tests PASS

注意: tantivy + tantivy-jieba 版本需要兼容。如果编译失败，根据错误信息调整版本。tantivy 0.22 对应 tantivy-jieba 0.11，tantivy 0.21 对应 tantivy-jieba 0.10。实现时以编译通过为准。

---

### Task 5: vault-core — vectors.rs usearch 向量搜索

**Files:**
- Create: `npu-vault/crates/vault-core/src/vectors.rs`
- Modify: `npu-vault/crates/vault-core/Cargo.toml`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Add usearch dependency**

```toml
usearch = "2"
```

- [ ] **Step 2: Write vectors.rs**

```rust
// npu-vault/crates/vault-core/src/vectors.rs

use std::collections::HashMap;
use std::path::Path;
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};
use crate::error::{Result, VaultError};

/// 向量元数据
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorMeta {
    pub item_id: String,
    pub chunk_idx: usize,
    pub level: u8,        // 1=章节, 2=段落
    pub section_idx: usize,
}

/// usearch 向量索引封装
pub struct VectorIndex {
    index: usearch::Index,
    meta: HashMap<u64, VectorMeta>,
    next_key: u64,
    dims: usize,
}

impl VectorIndex {
    pub fn new(dims: usize) -> Result<Self> {
        let options = IndexOptions {
            dimensions: dims,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F16,
            ..Default::default()
        };
        let index = usearch::new_index(&options)
            .map_err(|e| VaultError::Crypto(format!("usearch init: {e}")))?;
        index.reserve(10000)
            .map_err(|e| VaultError::Crypto(format!("usearch reserve: {e}")))?;
        Ok(Self { index, meta: HashMap::new(), next_key: 0, dims })
    }

    /// 添加向量
    pub fn add(&mut self, vector: &[f32], meta: VectorMeta) -> Result<u64> {
        if vector.len() != self.dims {
            return Err(VaultError::Crypto(format!(
                "vector dims mismatch: expected {}, got {}", self.dims, vector.len()
            )));
        }
        let key = self.next_key;
        self.next_key += 1;
        self.index.add(key, vector)
            .map_err(|e| VaultError::Crypto(format!("usearch add: {e}")))?;
        self.meta.insert(key, meta);
        Ok(key)
    }

    /// 搜索最相似向量
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(VectorMeta, f32)>> {
        if self.index.size() == 0 {
            return Ok(vec![]);
        }
        let results = self.index.search(query, top_k)
            .map_err(|e| VaultError::Crypto(format!("usearch search: {e}")))?;

        let mut output = Vec::new();
        for i in 0..results.keys.len() {
            let key = results.keys[i];
            let distance = results.distances[i];
            if let Some(meta) = self.meta.get(&key) {
                // cosine distance → cosine similarity
                let score = 1.0 - distance;
                output.push((meta.clone(), score));
            }
        }
        Ok(output)
    }

    /// 按 item_id 删除所有向量
    pub fn delete_by_item_id(&mut self, item_id: &str) -> Result<usize> {
        let keys_to_remove: Vec<u64> = self.meta.iter()
            .filter(|(_, m)| m.item_id == item_id)
            .map(|(k, _)| *k)
            .collect();
        let count = keys_to_remove.len();
        for key in &keys_to_remove {
            self.index.remove(*key)
                .map_err(|e| VaultError::Crypto(format!("usearch remove: {e}")))?;
            self.meta.remove(key);
        }
        Ok(count)
    }

    pub fn len(&self) -> usize {
        self.index.size()
    }

    pub fn is_empty(&self) -> bool {
        self.index.size() == 0
    }

    /// 保存索引到文件
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.index.save(path.to_str().unwrap())
            .map_err(|e| VaultError::Crypto(format!("usearch save: {e}")))?;
        // 保存 meta
        let meta_path = path.with_extension("meta.json");
        let meta_data = serde_json::to_vec(&self.meta)?;
        std::fs::write(&meta_path, meta_data)?;
        // 保存 next_key
        let key_path = path.with_extension("nextkey");
        std::fs::write(&key_path, self.next_key.to_le_bytes())?;
        Ok(())
    }

    /// 从文件加载索引
    pub fn load(path: &Path, dims: usize) -> Result<Self> {
        let options = IndexOptions {
            dimensions: dims,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F16,
            ..Default::default()
        };
        let index = usearch::new_index(&options)
            .map_err(|e| VaultError::Crypto(format!("usearch init: {e}")))?;
        index.load(path.to_str().unwrap())
            .map_err(|e| VaultError::Crypto(format!("usearch load: {e}")))?;

        let meta_path = path.with_extension("meta.json");
        let meta: HashMap<u64, VectorMeta> = if meta_path.exists() {
            let data = std::fs::read(&meta_path)?;
            serde_json::from_slice(&data)?
        } else {
            HashMap::new()
        };

        let key_path = path.with_extension("nextkey");
        let next_key = if key_path.exists() {
            let bytes = std::fs::read(&key_path)?;
            if bytes.len() == 8 {
                u64::from_le_bytes(bytes.try_into().unwrap())
            } else { meta.len() as u64 }
        } else { meta.len() as u64 };

        Ok(Self { index, meta, next_key, dims })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_vector(dims: usize) -> Vec<f32> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..dims).map(|_| rng.gen::<f32>()).collect()
    }

    #[test]
    fn create_index() {
        let idx = VectorIndex::new(1024).unwrap();
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn add_and_search() {
        let mut idx = VectorIndex::new(4).unwrap();
        let v1 = vec![1.0, 0.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0, 0.0];

        idx.add(&v1, VectorMeta { item_id: "a".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
        idx.add(&v2, VectorMeta { item_id: "b".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();

        let results = idx.search(&v1, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.item_id, "a", "Closest should be identical vector");
    }

    #[test]
    fn delete_by_item_id() {
        let mut idx = VectorIndex::new(4).unwrap();
        let v = vec![1.0, 0.0, 0.0, 0.0];
        idx.add(&v, VectorMeta { item_id: "x".into(), chunk_idx: 0, level: 1, section_idx: 0 }).unwrap();
        idx.add(&v, VectorMeta { item_id: "x".into(), chunk_idx: 1, level: 2, section_idx: 0 }).unwrap();
        idx.add(&v, VectorMeta { item_id: "y".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
        assert_eq!(idx.len(), 3);

        let removed = idx.delete_by_item_id("x").unwrap();
        assert_eq!(removed, 2);
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("vectors.usearch");

        let mut idx = VectorIndex::new(4).unwrap();
        idx.add(&[1.0, 0.0, 0.0, 0.0], VectorMeta {
            item_id: "id1".into(), chunk_idx: 0, level: 2, section_idx: 0
        }).unwrap();
        idx.save(&path).unwrap();

        let loaded = VectorIndex::load(&path, 4).unwrap();
        assert_eq!(loaded.len(), 1);
        let results = loaded.search(&[1.0, 0.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(results[0].0.item_id, "id1");
    }

    #[test]
    fn dimension_mismatch_error() {
        let mut idx = VectorIndex::new(4).unwrap();
        let result = idx.add(&[1.0, 0.0], VectorMeta {
            item_id: "x".into(), chunk_idx: 0, level: 2, section_idx: 0
        });
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Register in lib.rs** — Add `pub mod vectors;`

- [ ] **Step 4: Run tests**

Run: `cargo test -p vault-core vectors::tests`
Expected: 5 tests PASS

---

### Task 6: vault-core — search.rs RRF 混合搜索

**Files:**
- Create: `npu-vault/crates/vault-core/src/search.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: Write search.rs**

```rust
// npu-vault/crates/vault-core/src/search.rs

use std::collections::HashMap;
use crate::error::Result;

/// RRF 参数
pub const RRF_K: f32 = 60.0;
pub const DEFAULT_VECTOR_WEIGHT: f32 = 0.6;
pub const DEFAULT_FULLTEXT_WEIGHT: f32 = 0.4;
pub const INJECTION_BUDGET: usize = 2000;

/// 搜索结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub item_id: String,
    pub score: f32,
    pub title: String,
    pub content: String,
    pub source_type: String,
    pub inject_content: Option<String>,
}

/// RRF 融合两组排名结果
pub fn rrf_fuse(
    vector_results: &[(String, f32)],
    fulltext_results: &[(String, f32)],
    vector_weight: f32,
    fulltext_weight: f32,
    top_k: usize,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();

    for (rank, (id, _score)) in vector_results.iter().enumerate() {
        let rrf = vector_weight / (RRF_K + rank as f32 + 1.0);
        *scores.entry(id.clone()).or_default() += rrf;
    }
    for (rank, (id, _score)) in fulltext_results.iter().enumerate() {
        let rrf = fulltext_weight / (RRF_K + rank as f32 + 1.0);
        *scores.entry(id.clone()).or_default() += rrf;
    }

    let mut sorted: Vec<(String, f32)> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(top_k);
    sorted
}

/// 动态注入预算分配
pub fn allocate_budget(results: &mut [SearchResult], budget: usize) {
    let total_score: f32 = results.iter().map(|r| r.score).sum();
    if total_score <= 0.0 || results.is_empty() {
        let per_item = budget / results.len().max(1);
        for r in results.iter_mut() {
            let content = &r.content;
            let end = content.char_indices()
                .nth(per_item)
                .map(|(i, _)| i)
                .unwrap_or(content.len());
            r.inject_content = Some(content[..end].to_string());
        }
        return;
    }
    for r in results.iter_mut() {
        let share = r.score / total_score;
        let alloc = (budget as f32 * share).max(100.0) as usize;
        let content = &r.content;
        let end = content.char_indices()
            .nth(alloc)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        r.inject_content = Some(content[..end].to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_fuse_basic() {
        let vec_results = vec![
            ("a".into(), 0.9), ("b".into(), 0.7), ("c".into(), 0.5),
        ];
        let ft_results = vec![
            ("b".into(), 10.0), ("a".into(), 8.0), ("d".into(), 5.0),
        ];

        let fused = rrf_fuse(&vec_results, &ft_results, 0.6, 0.4, 10);
        assert!(!fused.is_empty());
        // "a" 和 "b" 在两个列表中都出现，应该排名靠前
        let top_ids: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
        assert!(top_ids.contains(&"a"));
        assert!(top_ids.contains(&"b"));
    }

    #[test]
    fn rrf_fuse_empty() {
        let fused = rrf_fuse(&[], &[], 0.6, 0.4, 10);
        assert!(fused.is_empty());
    }

    #[test]
    fn rrf_fuse_single_source() {
        let vec_results = vec![("a".into(), 0.9)];
        let fused = rrf_fuse(&vec_results, &[], 0.6, 0.4, 10);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].0, "a");
    }

    #[test]
    fn allocate_budget_proportional() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(), score: 0.8, title: "A".into(),
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None,
            },
            SearchResult {
                item_id: "b".into(), score: 0.2, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None,
            },
        ];
        allocate_budget(&mut results, 2000);

        let a_len = results[0].inject_content.as_ref().unwrap().chars().count();
        let b_len = results[1].inject_content.as_ref().unwrap().chars().count();
        // "a" has 80% score, should get ~1600 chars; "b" has 20%, should get ~400 (min 100)
        assert!(a_len > b_len, "Higher score should get more budget: a={a_len} b={b_len}");
        assert!(b_len >= 100, "Minimum budget should be 100: got {b_len}");
    }

    #[test]
    fn allocate_budget_zero_scores() {
        let mut results = vec![
            SearchResult {
                item_id: "a".into(), score: 0.0, title: "A".into(),
                content: "A".repeat(3000), source_type: "note".into(), inject_content: None,
            },
            SearchResult {
                item_id: "b".into(), score: 0.0, title: "B".into(),
                content: "B".repeat(3000), source_type: "note".into(), inject_content: None,
            },
        ];
        allocate_budget(&mut results, 2000);
        // Equal distribution when scores are 0
        let a_len = results[0].inject_content.as_ref().unwrap().chars().count();
        let b_len = results[1].inject_content.as_ref().unwrap().chars().count();
        assert_eq!(a_len, b_len, "Equal scores should get equal budget");
    }
}
```

- [ ] **Step 2: Register in lib.rs** — Add `pub mod search;`

- [ ] **Step 3: Run tests**

Run: `cargo test -p vault-core search::tests`
Expected: 5 tests PASS

---

### Task 7: vault-server crate — Axum HTTP Server

**Files:**
- Create: `npu-vault/crates/vault-server/Cargo.toml`
- Create: `npu-vault/crates/vault-server/src/main.rs`
- Create: `npu-vault/crates/vault-server/src/state.rs`
- Create: `npu-vault/crates/vault-server/src/middleware.rs`
- Create: `npu-vault/crates/vault-server/src/routes/mod.rs`
- Create: `npu-vault/crates/vault-server/src/routes/vault.rs`
- Create: `npu-vault/crates/vault-server/src/routes/status.rs`
- Modify: `npu-vault/Cargo.toml` (add to workspace members)

- [ ] **Step 1: Create vault-server Cargo.toml**

```toml
[package]
name = "vault-server"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[[bin]]
name = "npu-vault-server"
path = "src/main.rs"

[dependencies]
vault-core = { path = "../vault-core" }
axum = { version = "0.8", features = ["json", "multipart"] }
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.6", features = ["cors"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

- [ ] **Step 2: Add vault-server to workspace**

Update `npu-vault/Cargo.toml`:
```toml
members = ["crates/vault-core", "crates/vault-cli", "crates/vault-server"]
```

- [ ] **Step 3: Create state.rs**

```rust
// npu-vault/crates/vault-server/src/state.rs

use std::sync::Arc;
use vault_core::vault::Vault;

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub vault: Vault,
}

impl AppState {
    pub fn new(vault: Vault) -> Self {
        Self { vault }
    }
}
```

- [ ] **Step 4: Create middleware.rs**

```rust
// npu-vault/crates/vault-server/src/middleware.rs

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use crate::state::SharedState;
use vault_core::vault::VaultState;

/// Vault guard: 未 UNLOCKED 时返回 403
pub async fn vault_guard(
    State(state): State<SharedState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // 允许 /vault/* 和 /status/health 无需解锁
    let path = request.uri().path();
    if path.starts_with("/api/v1/vault") || path == "/api/v1/status/health" {
        return next.run(request).await;
    }

    match state.vault.state() {
        VaultState::Unlocked => next.run(request).await,
        VaultState::Locked => {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "vault is locked",
                "hint": "POST /api/v1/vault/unlock to unlock"
            }))).into_response()
        }
        VaultState::Sealed => {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "vault is sealed",
                "hint": "POST /api/v1/vault/setup to initialize"
            }))).into_response()
        }
    }
}
```

- [ ] **Step 5: Create routes/vault.rs**

```rust
// npu-vault/crates/vault-server/src/routes/vault.rs

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct SetupRequest {
    pub password: String,
}

#[derive(Deserialize)]
pub struct UnlockRequest {
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

pub async fn vault_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let vault_state = state.vault.state();
    let item_count = if matches!(vault_state, vault_core::vault::VaultState::Unlocked) {
        state.vault.store().item_count().unwrap_or(0)
    } else { 0 };

    Json(serde_json::json!({
        "state": vault_state,
        "items": item_count,
    }))
}

pub async fn vault_setup(
    State(state): State<SharedState>,
    Json(body): Json<SetupRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    state.vault.setup(&body.password).map_err(|e| {
        (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"status": "ok", "state": "unlocked"})))
}

pub async fn vault_unlock(
    State(state): State<SharedState>,
    Json(body): Json<UnlockRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let token = state.vault.unlock(&body.password).map_err(|e| {
        (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"status": "ok", "token": token})))
}

pub async fn vault_lock(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    state.vault.lock().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"status": "ok", "state": "locked"})))
}

pub async fn vault_change_password(
    State(state): State<SharedState>,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    state.vault.change_password(&body.old_password, &body.new_password).map_err(|e| {
        (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"status": "ok"})))
}
```

- [ ] **Step 6: Create routes/status.rs**

```rust
// npu-vault/crates/vault-server/src/routes/status.rs

use axum::Json;

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}
```

- [ ] **Step 7: Create routes/mod.rs**

```rust
// npu-vault/crates/vault-server/src/routes/mod.rs

pub mod vault;
pub mod status;
```

- [ ] **Step 8: Create main.rs**

```rust
// npu-vault/crates/vault-server/src/main.rs

mod middleware;
mod routes;
mod state;

use axum::middleware as axum_mw;
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "npu-vault-server", version, about = "npu-vault HTTP API server")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value = "18900")]
    port: u16,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let cli = Cli::parse();

    let vault = vault_core::vault::Vault::open_default()
        .expect("Failed to open vault");
    let shared_state = Arc::new(state::AppState::new(vault));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Vault endpoints (no guard needed)
        .route("/api/v1/vault/status", get(routes::vault::vault_status))
        .route("/api/v1/vault/setup", post(routes::vault::vault_setup))
        .route("/api/v1/vault/unlock", post(routes::vault::vault_unlock))
        .route("/api/v1/vault/lock", post(routes::vault::vault_lock))
        .route("/api/v1/vault/change-password", post(routes::vault::vault_change_password))
        // Status (health check bypasses guard)
        .route("/api/v1/status/health", get(routes::status::health))
        // Guard middleware for all other routes
        .layer(axum_mw::from_fn_with_state(shared_state.clone(), middleware::vault_guard))
        .layer(cors)
        .with_state(shared_state);

    let addr = format!("{}:{}", cli.host, cli.port);
    tracing::info!("npu-vault-server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind failed");
    axum::serve(listener, app).await.expect("server error");
}
```

- [ ] **Step 9: Build**

Run: `cargo build --bin npu-vault-server`
Expected: BUILD SUCCESS

- [ ] **Step 10: Smoke test**

Run server in background, test health endpoint:
```bash
cargo run --bin npu-vault-server &
sleep 2
curl -s http://127.0.0.1:18900/api/v1/status/health
# Expected: {"status":"ok"}
curl -s http://127.0.0.1:18900/api/v1/vault/status
# Expected: {"state":"sealed","items":0}
kill %1
```

---

### Task 8: vault-server — Ingest + Items + Search 路由

**Files:**
- Create: `npu-vault/crates/vault-server/src/routes/ingest.rs`
- Create: `npu-vault/crates/vault-server/src/routes/items.rs`
- Create: `npu-vault/crates/vault-server/src/routes/search.rs`
- Modify: `npu-vault/crates/vault-server/src/routes/mod.rs`
- Modify: `npu-vault/crates/vault-server/src/main.rs` (注册路由)

- [ ] **Step 1: Create routes/ingest.rs**

POST /api/v1/ingest — 接收 JSON {title, content, source_type, url?, domain?, tags?}，使用 dek_db 加密存储，enqueue embedding 任务。

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct IngestRequest {
    pub title: String,
    pub content: String,
    #[serde(default = "default_source_type")]
    pub source_type: String,
    pub url: Option<String>,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
}

fn default_source_type() -> String { "note".into() }

pub async fn ingest(
    State(state): State<SharedState>,
    Json(body): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let dek = state.vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let id = state.vault.store().insert_item(
        &dek,
        &body.title,
        &body.content,
        body.url.as_deref(),
        &body.source_type,
        body.domain.as_deref(),
        body.tags.as_deref(),
    ).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    Ok(Json(serde_json::json!({
        "id": id,
        "status": "ok"
    })))
}
```

- [ ] **Step 2: Create routes/items.rs**

GET /api/v1/items — list items (summary)
GET /api/v1/items/:id — get single item (decrypted)
DELETE /api/v1/items/:id — soft delete

```rust
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 20 }

pub async fn list_items(
    State(state): State<SharedState>,
    Query(params): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let _ = state.vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let items = state.vault.store().list_items(params.limit, params.offset).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"items": items, "count": items.len()})))
}

pub async fn get_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let dek = state.vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    match state.vault.store().get_item(&dek, &id) {
        Ok(Some(item)) => Ok(Json(serde_json::json!(item))),
        Ok(None) => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))),
    }
}

pub async fn delete_item(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    match state.vault.store().delete_item(&id) {
        Ok(true) => Ok(Json(serde_json::json!({"status": "ok"}))),
        Ok(false) => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))),
    }
}
```

- [ ] **Step 3: Create routes/search.rs**

GET /api/v1/search?q=&top_k= — basic search (fulltext only for Phase 2a, vector integration in Phase 2b when embedding queue worker is ready)

```rust
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize { 10 }

pub async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let _ = state.vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // Phase 2a: 基础搜索（通过 list + 标题匹配），全文搜索集成在 Task 9 完善
    // 这里先返回空结果，保持 API 协议兼容
    Ok(Json(serde_json::json!({
        "query": params.q,
        "results": [],
        "total": 0
    })))
}
```

- [ ] **Step 4: Update routes/mod.rs**

```rust
pub mod vault;
pub mod status;
pub mod ingest;
pub mod items;
pub mod search;
```

- [ ] **Step 5: Register routes in main.rs**

Add to the Router in main.rs (before the guard layer):

```rust
.route("/api/v1/ingest", post(routes::ingest::ingest))
.route("/api/v1/items", get(routes::items::list_items))
.route("/api/v1/items/:id", get(routes::items::get_item).delete(routes::items::delete_item))
.route("/api/v1/search", get(routes::search::search))
```

- [ ] **Step 6: Build and smoke test**

```bash
cargo build --bin npu-vault-server
cargo run --bin npu-vault-server &
sleep 2

# Setup vault
curl -s -X POST http://127.0.0.1:18900/api/v1/vault/setup \
  -H "Content-Type: application/json" \
  -d '{"password":"test123"}'
# Expected: {"status":"ok","state":"unlocked"}

# Ingest
curl -s -X POST http://127.0.0.1:18900/api/v1/ingest \
  -H "Content-Type: application/json" \
  -d '{"title":"Test","content":"Hello vault","source_type":"note"}'
# Expected: {"id":"...","status":"ok"}

# List items
curl -s http://127.0.0.1:18900/api/v1/items
# Expected: {"items":[...],"count":1}

# Lock and try again
curl -s -X POST http://127.0.0.1:18900/api/v1/vault/lock
curl -s http://127.0.0.1:18900/api/v1/items
# Expected: 403 {"error":"vault is locked",...}

kill %1
```

---

### Task 9: 全量测试 + 文档更新

**Files:**
- Modify: `npu-vault/README.md`
- Create: `npu-vault/tests/api_test.rs` (可选, 如时间允许)

- [ ] **Step 1: Run all vault-core tests**

Run: `cargo test -p vault-core`
Expected: 37+ old tests + new module tests all PASS

- [ ] **Step 2: Build all binaries**

Run: `cargo build --workspace`
Expected: BUILD SUCCESS for npu-vault, npu-vault-server

- [ ] **Step 3: Update README**

Add Phase 2a completion note and server usage instructions.

- [ ] **Step 4: Final verification summary**

List all test counts by module, binary sizes, and feature status.

---

## Self-Review Checklist

**1. Spec coverage:**
- ✅ chunker.rs: 滑动窗口 + extract_sections — Task 1
- ✅ parser.rs: MD/TXT/代码解析 + parse_bytes + file_hash — Task 2
- ✅ embed.rs: OllamaProvider + EmbeddingProvider trait + NoopProvider — Task 3
- ✅ index.rs: tantivy 全文索引 (BM25 + jieba) — Task 4
- ✅ vectors.rs: usearch 向量索引 (cosine + f16) — Task 5
- ✅ search.rs: RRF 融合 + 动态预算 — Task 6
- ✅ vault-server: Axum + vault guard + CORS — Task 7
- ✅ API routes: vault/*/ingest/items/search/status — Task 7-8
- ⏳ PDF/DOCX 解析 — Phase 2b
- ⏳ 文件扫描 (scanner.rs) — Phase 2b
- ⏳ Embedding queue worker — Phase 2b
- ⏳ WebSocket 进度推送 — Phase 2b

**2. Placeholder scan:** 无 TBD/TODO。search 路由明确标注 "Phase 2a 基础搜索"。

**3. Type consistency:**
- `VaultError` — 全模块使用 `Result<T>` — 一致
- `Key32` — crypto.rs 定义, store/vault/embed 使用 — 一致
- `EmbeddingProvider` trait — embed.rs 定义, 后续 vectors 和 search 集成 — 一致
- `VectorMeta` — vectors.rs 定义, search.rs 使用 — 一致
- `SearchResult` — search.rs 定义 — 一致
- `SharedState` / `AppState` — state.rs 定义, routes 使用 — 一致
