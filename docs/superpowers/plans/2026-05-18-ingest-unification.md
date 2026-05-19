# 采集体系重构（Ingest Unification）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把现在散落在 4 条入库路径（`routes/upload.rs` / `routes/ingest.rs` / `attune-core/src/scanner.rs` / `attune-core/src/scanner_webdav.rs`）里复制 4 遍的 `parse → insert → breadcrumbs → embed(L1+L2) → classify` 五步 pipeline 收敛成 `attune-core::ingest` 一个统一抽象，并在其上修复 WebDAV 采集器（迁 `reqwest_dav`、async 化、ETag 增量）+ 接 Email / RSS / 云盘三个新源。

**Architecture:** 新增 `attune-core/src/ingest/` 模块。`SourceConnector` trait 描述「一个源」——它把自己的内容逐个交成 `RawDocument`（通过回调 sink，不一次性返回 `Vec`，避免大邮箱/大目录爆内存）。`ingest_document()` 是唯一的入库函数，吃一份 `RawDocument` 走完五步并返回 `IngestOutcome`。现有 4 条路径退化成「构造 `RawDocument` → 调 `ingest_document`」的薄壳；新源（Email / RSS / 云盘）只需实现 `SourceConnector`，不再碰 pipeline 内部。HTTP API 路径/请求/响应形态对外完全不变。

**Tech Stack:** Rust 2021 / Axum 0.8 / rusqlite / tantivy / usearch。新依赖：`reqwest_dav` 0.3（WebDAV，`default-features = false` + `rustls-tls` feature —— 见下方「WebDAV TLS 合规核实结论」）、`async-imap` + `mail-parser`（Email）、`feed-rs`（RSS）。云盘走 `rclone` subprocess（不引入 crate）。

**WebDAV TLS 合规核实结论（产品负责人决策 3，2026-05-18 核实）：** 核对 `reqwest_dav` v0.3.3（crates.io 最新）的 `Cargo.toml`：它 `[features]` 段提供 `default = ["reqwest/default"]`、`rustls-tls = ["reqwest/rustls"]`、`native-tls = ["reqwest/native-tls"]`，且自身 `reqwest = { version = "0.13", default-features = false, features = ["form"] }` —— 即 `reqwest_dav` 不强制任何 TLS 后端，TLS 完全由本仓 feature 选择透传。因此**计划保持用 `reqwest_dav`**，在 `attune-core/Cargo.toml` 写 `reqwest_dav = { version = "0.3", default-features = false, features = ["rustls-tls"] }` —— 纯 Rust rustls，不引入 `native-tls`/`openssl-sys`，符合 CLAUDE.md「网络栈纯 Rust TLS」硬约束。注意版本是 **0.3**（不是计划早期草稿的 0.2）。

---

## 关键背景与约束（实现者必读）

**为什么要做：** WebDAV 采集器（`scanner_webdav.rs::scan_remote`）是从 `scanner.rs` 复制后**漏抄了两步**——它只入队 Level-1 章节 embedding，没有 Level-2 段落块，也完全没有 `enqueue_classify`。结果 WebDAV 来的文档检索召回差、永远不被自动分类。本计划一旦让 WebDAV 走统一 `ingest_document`，**这两个缺陷自动消失**（因为统一函数本来就做这两步）——这是重构的直接收益，不需要单独"修 bug"的 task。

**真实代码事实（计划中引用的签名都已与代码核对，实现者不要凭记忆改）：**

- `parser::parse_bytes(data: &[u8], filename: &str) -> Result<(String, String)>` —— 返回 `(title, content)`。`parser::parse_bytes_with_profile(data, filename, profile_id: Option<&str>)` 是带 OCR profile 的版本。
- `chunker::extract_sections(content: &str) -> Vec<(usize, String)>` —— 返回 `(section_idx, section_text)`。
- `chunker::chunk(text: &str, chunk_size: usize, overlap: usize) -> Vec<String>`；常量 `chunker::DEFAULT_CHUNK_SIZE = 512`、`chunker::DEFAULT_OVERLAP = 128`。
- `Store::insert_item(&self, dek: &Key32, title: &str, content: &str, url: Option<&str>, source_type: &str, domain: Option<&str>, tags: Option<&[String]>) -> Result<String>` —— 返回新 `item_id`。
- `Store::compute_content_hash` 实际是模块级自由函数：`attune_core::store::items::compute_content_hash(content: &str) -> String`。
- `Store::find_item_by_content_hash(&self, content_hash: &str) -> Result<Option<String>>`。
- `Store::enqueue_embedding(&self, item_id: &str, chunk_idx: usize, chunk_text: &str, priority: i32, level: i32, section_idx: usize) -> Result<()>`。
- `Store::enqueue_classify(&self, item_id: &str, priority: i32) -> Result<()>`。
- `Store::enqueue_reindex(&self, item_id: &str, action: &str) -> Result<()>` —— `action` 必须是 `"purge"` 或 `"reindex"`，否则报错。
- `Store::upsert_chunk_breadcrumbs_from_content(&self, dek: &Key32, item_id: &str, content: &str) -> Result<usize>`。
- `Store::record_signal_event(&self, kind: &str, ref_id: &str, query: Option<&str>) -> Result<()>` —— `kind` 取已知集（`doc_create` / `doc_update` / `doc_delete` / …）。
- `Store::get_indexed_file(&self, path: &str) -> Result<Option<IndexedFileRow>>`；`IndexedFileRow { id, dir_id, path, file_hash, item_id: Option<String> }`。
- `Store::upsert_indexed_file(&self, dir_id: &str, path: &str, file_hash: &str, item_id: &str) -> Result<()>`。
- `Store::delete_item(&self, id: &str) -> Result<bool>`。
- `Store::insert_item_blob(&self, dek, item_id, filename, mime, bytes)`（原件留存，upload 路径用）。
- `Store::get_dir_corpus_domain(&self, dir_id: &str) -> Result<String>`（找不到回退 `"general"`）；`Store::set_item_corpus_domain(&self, item_id: &str, corpus_domain: &str) -> Result<()>`。
- `Store::get_item(&self, dek: &Key32, id: &str) -> Result<Option<DecryptedItem>>` —— `DecryptedItem` 含 `title` / `content` / `domain: Option<String>` / `corpus_domain: String` 等字段。
- `Store::get_tags_json(&self, dek: &Key32, item_id: &str) -> Result<Option<String>>` —— 返回解密后的 tags JSON 字符串。
- `crypto::encrypt(key: &Key32, plaintext: &[u8]) -> Result<Vec<u8>>` / `crypto::decrypt(key: &Key32, data: &[u8]) -> Result<Vec<u8>>` —— AES-256-GCM 字段级加解密。`items.content` / `items.tags` 即用此模式（见 `store/items.rs`），决策 4 的 `webdav_remotes.password_enc` 沿用。
- `attune_core::error::{Result, VaultError}` 是 core 层错误类型；server 层是 `attune_server::error::{AppError, AppResult}`，`AppError: From<VaultError>`。
- `attune_core::async_fs` 提供 `read` / `read_to_string` / `write` / `create_dir_all` / `try_exists` / `remove_file_if_exists`（async handler 内禁止直接 `std::fs`）。

**Lock ordering（防死锁，全程遵守）：** `vault.lock()` → `vectors.lock()` → `fulltext.lock()` → `embedding.lock()`。`ingest_document` 只碰 `Store`（SQL 连接），不碰 `VectorIndex` / `FulltextIndex` —— 后两者是 server `AppState` 的独立 Mutex。`ingest_document` 通过 `enqueue_embedding` 把向量写入 defer 给 server 后台 worker，自身不直接调 `vectors` / `fulltext`，因此不引入新的锁顺序风险。

**FTS 即时可用：** 现有 upload/ingest 路径在 server 层会**同步**调一次 `fulltext.add_document`（让搜索不等 embedding 就能命中）。`ingest_document` 在 core 层拿不到 `FulltextIndex` 锁，所以**不做** FTS。统一抽象的设计是：`ingest_document` 返回 `IngestOutcome`，server 层的薄壳 caller 拿到 `item_id` 后**自己**补一次 `fulltext.add_document`（保持与重构前完全一致的行为）。core 层后台 worker 类 caller（scanner / scanner_webdav）走 `enqueue_reindex` 让 server worker 间接处理 FTS——但注意：新 item 的 FTS 由 embedding worker 链路覆盖，scanner 路径维持现状即可（见 Task 9 说明）。

**注释纪律：** 一个改动区域一条意图注释，写 WHY 不写 WHAT，禁止 `批次X` / `FIX-N` / `阶段Y` / `per reviewer` 这类过程标签。

---

## File Structure

### 新建文件

| 文件 | 职责 |
|------|------|
| `rust/crates/attune-core/src/ingest/mod.rs` | 模块导出：`pub use connector::*; pub use pipeline::*;` |
| `rust/crates/attune-core/src/ingest/connector.rs` | 核心类型：`SourceKind` enum、`RawDocument` 结构（含 `domain` / `tags` / `corpus_domain` 字段）、`SourceConnector` trait、`DocumentSink` 类型别名 |
| `rust/crates/attune-core/src/ingest/pipeline.rs` | `IngestOutcome` enum、`ingest_document()` 统一入库函数（透传 `domain` / `tags`，按 `corpus_domain` 注入 chunk 前缀）|
| `rust/crates/attune-core/src/ingest/local.rs` | `LocalFolderConnector` —— 本地文件夹源，把现有 `scanner.rs::scan_directory` 的遍历逻辑包成 `SourceConnector` |
| `rust/crates/attune-core/src/store/webdav_remotes.rs` | WebDAV remote 配置加密持久化（`webdav_remotes` 表，`password` 走字段级 AES-256-GCM）—— 决策 4，见 Task 12 |
| `rust/crates/attune-core/tests/ingest_pipeline_test.rs` | `ingest_document` 集成测试（Inserted / Duplicate / Updated / Skipped 四态 + domain/tags 透传 + corpus_domain 前缀）|
| `rust/crates/attune-core/tests/webdav_remotes_test.rs` | `webdav_remotes` 表加解密往返集成测试（决策 4）|

### 修改文件

| 文件 | 改动 |
|------|------|
| `rust/crates/attune-core/Cargo.toml` | 加 `reqwest_dav` 0.3 依赖（`default-features = false` + `rustls-tls`，Phase 1）；后续 Phase 加 `async-imap`/`mail-parser`/`feed-rs` |
| `rust/crates/attune-core/src/lib.rs` | `pub mod ingest;` |
| `rust/crates/attune-core/src/store/mod.rs` | 加 `pub mod webdav_remotes;` + `webdav_remotes` 表 `CREATE TABLE IF NOT EXISTS`（决策 4，Task 12）|
| `rust/crates/attune-core/src/scanner.rs` | `process_single_file` 改为构造 `RawDocument` → 调 `ingest_document`，删本地复制的 pipeline 代码；保留 corpus_domain（item 级 + chunk 前缀经 `RawDocument.corpus_domain` 透传，决策 2）|
| `rust/crates/attune-core/src/scanner_webdav.rs` | 整体重写：`reqwest_dav` 替手写 XML parser、async、ETag dedup、走 `ingest_document` |
| `rust/crates/attune-server/src/routes/upload.rs` | `upload_file` 改为构造 `RawDocument` → 调 `ingest_document`，保留 blob 留存 / backpressure / FTS / project recommender 等 server 专属逻辑 |
| `rust/crates/attune-server/src/routes/ingest.rs` | `ingest` 改为构造 `RawDocument` → 调 `ingest_document`，`domain` / `tags` 经 `RawDocument` 透传（决策 1，对外行为不变）|
| `rust/crates/attune-server/src/routes/remote.rs` | `bind_remote` 适配新 async WebDAV API + 落库加密 remote 配置（决策 4）|
| `rust/crates/attune-server/src/ingest_webdav.rs` | 新建：`sync_webdav_dir` 公共函数，`bind_remote` 与周期 worker 共用（Task 11）|
| `rust/crates/attune-server/src/state.rs` | Phase 1 末尾加 WebDAV 周期重扫的后台 interval task（见 Task 11，从 `webdav_remotes` 表读已配置 remote + 解密凭据）|

---

## Phase 1：统一抽象 + 4 条路径迁移 + WebDAV 修复（必做核心）

### Task 1：`SourceKind` + `RawDocument` + `SourceConnector` trait

**Files:**
- Create: `rust/crates/attune-core/src/ingest/connector.rs`
- Create: `rust/crates/attune-core/src/ingest/mod.rs`
- Modify: `rust/crates/attune-core/src/lib.rs`（加 `pub mod ingest;`）

- [ ] **Step 1: 写失败测试**

在 `rust/crates/attune-core/src/ingest/connector.rs` 末尾加单元测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_document_construct_and_read_fields() {
        let doc = RawDocument {
            uri: "file:///home/u/notes/a.md".into(),
            title: "A Note".into(),
            content: b"# A Note\n\nbody".to_vec(),
            mime_hint: Some("text/markdown".into()),
            source_kind: SourceKind::LocalFolder,
            source_ref: "/home/u/notes/a.md".into(),
            modified_marker: Some("abc123".into()),
            domain: Some("example.com".into()),
            tags: Some(vec!["note".into(), "draft".into()]),
            corpus_domain: Some("legal".into()),
            metadata: std::collections::HashMap::new(),
        };
        assert_eq!(doc.source_kind, SourceKind::LocalFolder);
        assert_eq!(doc.source_ref, "/home/u/notes/a.md");
        assert_eq!(doc.content, b"# A Note\n\nbody");
        assert_eq!(doc.domain.as_deref(), Some("example.com"));
        assert_eq!(doc.tags.as_ref().unwrap().len(), 2);
        assert_eq!(doc.corpus_domain.as_deref(), Some("legal"));
    }

    #[test]
    fn source_kind_as_str_round_trips() {
        for k in [
            SourceKind::LocalFolder,
            SourceKind::WebDav,
            SourceKind::Email,
            SourceKind::Rss,
            SourceKind::CloudDrive,
        ] {
            assert!(!k.as_str().is_empty());
        }
        assert_eq!(SourceKind::WebDav.as_str(), "webdav");
    }

    #[test]
    fn connector_drives_sink_callback() {
        struct TwoDocConnector;
        impl SourceConnector for TwoDocConnector {
            fn source_kind(&self) -> SourceKind {
                SourceKind::LocalFolder
            }
            fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> crate::error::Result<()> {
                for i in 0..2 {
                    sink(RawDocument {
                        uri: format!("mem://{i}"),
                        title: format!("doc {i}"),
                        content: b"x".to_vec(),
                        mime_hint: None,
                        source_kind: SourceKind::LocalFolder,
                        source_ref: format!("mem://{i}"),
                        modified_marker: None,
                        domain: None,
                        tags: None,
                        corpus_domain: None,
                        metadata: std::collections::HashMap::new(),
                    });
                }
                Ok(())
            }
        }
        let mut count = 0usize;
        let mut sink: DocumentSink<'_> = Box::new(|_doc: RawDocument| count += 1);
        TwoDocConnector.fetch_documents(&mut sink).unwrap();
        assert_eq!(count, 2);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib ingest::connector`
Expected: 编译失败 —— `connector` 模块还不存在。

- [ ] **Step 3: 写最小实现**

`rust/crates/attune-core/src/ingest/mod.rs` 完整内容：

```rust
//! ingest — 统一采集抽象。
//!
//! 一个「源」（本地文件夹 / WebDAV / 邮箱 / RSS / 云盘）实现 [`SourceConnector`]，
//! 把自己的内容逐个交成 [`RawDocument`]；[`ingest_document`] 是唯一入库函数，
//! 走完 parse → 判重 → insert → breadcrumbs → embed(L1+L2) → classify 五步。
//! 各源不再各自复制 pipeline。

mod connector;
mod pipeline;
pub mod local;

pub use connector::{DocumentSink, RawDocument, SourceConnector, SourceKind};
pub use pipeline::{ingest_document, IngestOutcome};
```

`rust/crates/attune-core/src/ingest/connector.rs` 完整内容（测试模块见 Step 1，接在文件末尾）：

```rust
use std::collections::HashMap;

use crate::error::Result;

/// 采集源类别。决定入库 item 的 `source_type` 与去重策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// 本地文件夹（folder watcher / 手动 bind）。
    LocalFolder,
    /// WebDAV 远程目录（Nextcloud / 群晖 / Apache mod_dav）。
    WebDav,
    /// IMAP 邮箱。
    Email,
    /// RSS / Atom 订阅。
    Rss,
    /// 云盘（经 rclone 桥接：Google Drive / Dropbox / OneDrive 等）。
    CloudDrive,
}

impl SourceKind {
    /// 稳定字符串标识，写入 DB / 日志 / 信号。新增 variant 必须同步加分支。
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::LocalFolder => "local_folder",
            SourceKind::WebDav => "webdav",
            SourceKind::Email => "email",
            SourceKind::Rss => "rss",
            SourceKind::CloudDrive => "cloud_drive",
        }
    }

    /// 入库 `items.source_type` 字段值。当前全部归一到 `"file"` 以兼容现有
    /// 检索 / 分类逻辑（它们按 source_type 做加权）；未来如需按源细分再扩展。
    pub fn item_source_type(&self) -> &'static str {
        "file"
    }
}

/// 从某个源拿到的一份未入库原始文档。
///
/// `content` 是**原始字节**（未解析）—— `ingest_document` 内部用
/// [`crate::parser::parse_bytes`] 解析。`modified_marker` 用于增量判断：
/// 本地文件 = SHA-256 / mtime；WebDAV = ETag；邮箱 = UID；RSS = entry id。
/// caller 用它和 `Store::get_indexed_file` 里存的 `file_hash` 比对决定是否跳过。
#[derive(Debug, Clone)]
pub struct RawDocument {
    /// 全局唯一资源定位符（`file:///…` / `https://…` / `imap://…/INBOX/123`）。
    pub uri: String,
    /// 源给出的标题；为空时 `ingest_document` 会用 parser 提取的标题兜底。
    pub title: String,
    /// 原始字节。
    pub content: Vec<u8>,
    /// MIME 提示（源若已知）。当前 parser 主要按文件名扩展名判别，此字段预留。
    pub mime_hint: Option<String>,
    /// 源类别。
    pub source_kind: SourceKind,
    /// 在该源内的稳定引用键，用于 `indexed_files` 去重。
    /// 本地 = 绝对路径；WebDAV = href；邮箱 = Message-ID；RSS = entry link。
    pub source_ref: String,
    /// 增量标记（见结构体文档）。`None` = 该源无增量信息，每次都重新入库。
    pub modified_marker: Option<String>,
    /// 网站域名 / 来源域（来自 Chrome 扩展 `ingest` 时携带的 `domain`）。
    /// 一等字段，`ingest_document` 直接透传给 `Store::insert_item` 第 6 参数。
    /// 非 `/api/v1/ingest` 源（local / upload / webdav / email / rss）传 `None`。
    pub domain: Option<String>,
    /// 用户标签（来自 `ingest` 时携带的 `tags`）。一等字段，`ingest_document`
    /// 直接透传给 `Store::insert_item` 第 7 参数。非 ingest 源传 `None`。
    pub tags: Option<Vec<String>>,
    /// 语料领域分类（`legal` / `tech` / `medical` / `patent` / `general`）。
    /// 对应 `items.corpus_domain`。`Some(d)` 且 `d != "general"` 时，
    /// `ingest_document` 会给每个 chunk_text 注入 `[领域: d] ` 前缀
    /// （v0.6 F-Pro 跨域防污染，bge-m3 corpus tagging）并调
    /// `set_item_corpus_domain`。本地文件夹源从 `Store::get_dir_corpus_domain`
    /// 读取放入；其它源（webdav / email / rss / cloud）传 `None`。
    pub corpus_domain: Option<String>,
    /// 源特定的额外元数据（邮件发件人 / RSS 频道名等），按需消费。
    pub metadata: HashMap<String, String>,
}

impl RawDocument {
    /// 用于 `parser::parse_bytes` 的文件名 —— 取 `source_ref` 末段，
    /// parser 据此扩展名选解析器。无扩展名时 parser 走纯文本分支。
    pub fn parse_filename(&self) -> String {
        self.source_ref
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.source_ref)
            .to_string()
    }
}

/// 文档回调 sink。`SourceConnector` 每产出一份 `RawDocument` 就调一次。
/// 用回调而非返回 `Vec<RawDocument>`：大邮箱 / 大目录一次性物化会爆内存。
pub type DocumentSink<'a> = Box<dyn FnMut(RawDocument) + 'a>;

/// 一个采集源。实现者负责枚举自己的内容并通过 `sink` 逐个交出。
pub trait SourceConnector {
    /// 该源的类别。
    fn source_kind(&self) -> SourceKind;

    /// 枚举源内文档，每份通过 `sink` 交出。实现者**不**做入库 —— 入库由
    /// 调用方对每份 `RawDocument` 调 [`crate::ingest::ingest_document`] 完成。
    /// 单份文档的可恢复错误（解析失败 / 下载失败）应由实现者吞掉并记日志、
    /// 继续下一份；只有源级致命错误（无法连接 / 鉴权失败）才返回 `Err`。
    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()>;
}
```

`rust/crates/attune-core/src/lib.rs`：在 `pub mod index;` 一行之后插入：

```rust
pub mod ingest;
```

`ingest/local.rs` 在 Task 8 才填实质内容；为让 `pub mod local;` 现在能编译，先建占位文件 `rust/crates/attune-core/src/ingest/local.rs`：

```rust
//! 本地文件夹采集源。实质实现见 Task 8。
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib ingest::connector`
Expected: 3 个测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/ rust/crates/attune-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(ingest): SourceConnector trait + RawDocument 统一采集抽象

为采集体系重构引入 attune-core::ingest 模块底座：SourceKind /
RawDocument / SourceConnector trait。RawDocument 含 domain / tags /
corpus_domain 一等字段，保证 ingest 透传与 F-Pro 跨域防污染不丢。
各源通过回调 sink 逐个交出文档，避免大邮箱大目录一次性物化爆内存。
pipeline 在 Task 2 接入。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2：`ingest_document` 统一入库函数 + `IngestOutcome`

**Files:**
- Create: `rust/crates/attune-core/src/ingest/pipeline.rs`
- Create: `rust/crates/attune-core/tests/ingest_pipeline_test.rs`

- [ ] **Step 1: 写失败测试**

`rust/crates/attune-core/tests/ingest_pipeline_test.rs` 完整内容：

```rust
//! ingest_document 四态行为 + domain/tags 透传 + corpus_domain 前缀集成测试。

use std::collections::HashMap;

use attune_core::crypto::Key32;
use attune_core::ingest::{ingest_document, IngestOutcome, RawDocument, SourceKind};
use attune_core::store::Store;

fn md_doc(source_ref: &str, body: &str) -> RawDocument {
    RawDocument {
        uri: format!("file://{source_ref}"),
        title: String::new(),
        content: body.as_bytes().to_vec(),
        mime_hint: Some("text/markdown".into()),
        source_kind: SourceKind::LocalFolder,
        source_ref: source_ref.into(),
        modified_marker: None,
        domain: None,
        tags: None,
        corpus_domain: None,
        metadata: HashMap::new(),
    }
}

#[test]
fn first_ingest_returns_inserted_and_enqueues_two_levels() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let doc = md_doc("/tmp/a.md", "# Title\n\nSome body paragraph here.\n\n# Two\n\nMore body.");

    let outcome = ingest_document(&store, &dek, &doc).unwrap();
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, chunks_enqueued } => {
            assert!(chunks_enqueued >= 2, "L1 章节 + L2 段落块都应入队");
            item_id
        }
        other => panic!("expected Inserted, got {other:?}"),
    };
    assert_eq!(store.item_count().unwrap(), 1);

    // L1 (level=1) 与 L2 (level=2) 都必须有任务入队。
    let l1 = store.count_embed_queue_by_level(1).unwrap();
    let l2 = store.count_embed_queue_by_level(2).unwrap();
    assert!(l1 >= 1, "Level-1 章节 embedding 必须入队");
    assert!(l2 >= 1, "Level-2 段落块 embedding 必须入队");

    // classify 任务必须入队。
    assert_eq!(store.pending_count_by_type("classify").unwrap(), 1);

    // breadcrumbs sidecar 必须写入。
    assert!(store.chunk_breadcrumb_count(&item_id).unwrap() >= 1);
}

#[test]
fn duplicate_content_returns_duplicate_and_skips_pipeline() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let doc = md_doc("/tmp/a.md", "# Same\n\nidentical body.");
    let first = ingest_document(&store, &dek, &doc).unwrap();
    let first_id = match first {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };

    // 同内容、不同 source_ref 再入一次 → content_hash 命中 → Duplicate。
    let doc2 = md_doc("/tmp/copy-of-a.md", "# Same\n\nidentical body.");
    let second = ingest_document(&store, &dek, &doc2).unwrap();
    match second {
        IngestOutcome::Duplicate { item_id } => assert_eq!(item_id, first_id),
        other => panic!("expected Duplicate, got {other:?}"),
    }
    assert_eq!(store.item_count().unwrap(), 1, "重复内容不得新增 item");
}

#[test]
fn changed_content_same_ref_returns_updated() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let dir_id = store.bind_directory("/tmp", true, &["md"]).unwrap();

    let mut doc = md_doc("/tmp/a.md", "# V1\n\noriginal body.");
    doc.modified_marker = Some("hash-v1".into());
    let first = ingest_document(&store, &dek, &doc).unwrap();
    let first_id = match first {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    store
        .upsert_indexed_file(&dir_id, &doc.source_ref, "hash-v1", &first_id)
        .unwrap();

    // 同 source_ref、内容变了 → Updated（旧 item 删 + enqueue purge）。
    let mut doc2 = md_doc("/tmp/a.md", "# V2\n\ncompletely new body.");
    doc2.modified_marker = Some("hash-v2".into());
    let second = ingest_document(&store, &dek, &doc2).unwrap();
    match second {
        IngestOutcome::Updated { item_id, old_item_id } => {
            assert_ne!(item_id, old_item_id);
            assert_eq!(old_item_id, first_id);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
}

#[test]
fn empty_content_returns_skipped() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let doc = md_doc("/tmp/blank.md", "   \n  \n");
    let outcome = ingest_document(&store, &dek, &doc).unwrap();
    assert!(matches!(outcome, IngestOutcome::Skipped { .. }));
    assert_eq!(store.item_count().unwrap(), 0);
}

#[test]
fn ingest_passes_through_domain_and_tags() {
    // 决策 1：RawDocument 的 domain / tags 必须透传给 insert_item，
    // 让入库 item 行带上来源域与用户标签（/api/v1/ingest 对外行为不变）。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut doc = md_doc("/tmp/tagged.md", "# Tagged\n\nbody with domain and tags.");
    doc.domain = Some("blog.example.com".into());
    doc.tags = Some(vec!["rust".into(), "ingest".into()]);

    let item_id = match ingest_document(&store, &dek, &doc).unwrap() {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    assert_eq!(item.domain.as_deref(), Some("blog.example.com"), "domain 必须透传");
    let tags = store.get_tags_json(&dek, &item_id).unwrap().expect("tags stored");
    assert!(tags.contains("rust") && tags.contains("ingest"), "tags 必须透传");
}

#[test]
fn ingest_injects_corpus_domain_prefix_into_chunks() {
    // 决策 2：corpus_domain != "general" 时，L1/L2 每个 chunk_text 必须被注入
    // `[领域: X] ` 前缀（F-Pro 跨域防污染），且 item 行 corpus_domain 被设置。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut doc = md_doc("/tmp/legal.md", "# Case\n\nlegal body paragraph here.");
    doc.corpus_domain = Some("legal".into());

    let item_id = match ingest_document(&store, &dek, &doc).unwrap() {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    // item 级 corpus_domain 标签必须落库。
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    assert_eq!(item.corpus_domain, "legal", "item corpus_domain 必须设置");
    // 入队的每个 chunk_text 都应带 `[领域: legal] ` 前缀。
    let chunks = store.peek_embed_queue_chunk_texts(&item_id).unwrap();
    assert!(!chunks.is_empty(), "应有 chunk 入队");
    for c in &chunks {
        assert!(c.starts_with("[领域: legal] "), "chunk 必须带领域前缀: {c}");
    }
}

#[test]
fn ingest_general_corpus_domain_skips_prefix() {
    // corpus_domain == "general"（或 None）时不注入前缀 —— 通用文档零开销。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut doc = md_doc("/tmp/general.md", "# Note\n\nplain general body.");
    doc.corpus_domain = Some("general".into());
    let item_id = match ingest_document(&store, &dek, &doc).unwrap() {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    let chunks = store.peek_embed_queue_chunk_texts(&item_id).unwrap();
    for c in &chunks {
        assert!(!c.starts_with("[领域:"), "general 不应注入前缀: {c}");
    }
}
```

> 测试用到四个 `Store` 辅助查询方法：`count_embed_queue_by_level` / `chunk_breadcrumb_count` / `peek_embed_queue_chunk_texts`（解密回 chunk_text 供断言，仅 `cfg(test)` 用途的统计与读取），生产代码已有 `pending_count_by_type` / `get_item` / `get_tags_json`。Step 3 会把前三个 helper 加进 `Store`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --test ingest_pipeline_test`
Expected: 编译失败 —— `ingest::ingest_document` / `IngestOutcome` 未定义。

- [ ] **Step 3: 写最小实现**

先在 `rust/crates/attune-core/src/store/queue.rs` 的 `impl Store` 块末尾（`}` 之前）加测试辅助查询：

```rust
    /// 测试辅助：统计某 level 在 embed_queue 中的 pending 任务数。
    pub fn count_embed_queue_by_level(&self, level: i32) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embed_queue WHERE level = ?1 AND task_type = 'embed'",
            params![level],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// 测试辅助：取某 item 入队的全部 chunk_text（解密回明文）。
    /// 仅供测试断言 chunk 前缀注入用 —— embed_queue.chunk_text 是 AES-GCM 密文 BLOB。
    pub fn peek_embed_queue_chunk_texts(&self, item_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT chunk_text FROM embed_queue WHERE item_id = ?1 ORDER BY chunk_idx",
        )?;
        let rows = stmt.query_map(params![item_id], |row| row.get::<_, Vec<u8>>(0))?;
        let mut out = Vec::new();
        for blob in rows {
            let plain = crate::crypto::decrypt(&self.test_dek_for_queue_peek(), &blob?)?;
            out.push(String::from_utf8_lossy(&plain).to_string());
        }
        Ok(out)
    }
```

> `peek_embed_queue_chunk_texts` 需要 dek 才能解密。`embed_queue.chunk_text` 若是用 item dek 加密，测试里 `Store::open_memory()` + 单一 `dek` 场景可直接把 dek 传进来。**实现者执行 Step 3 时先 `grep -n "enqueue_embedding" rust/crates/attune-core/src/store/queue.rs` 看 `chunk_text` 实际是明文还是密文存储**：
> - 若 `embed_queue.chunk_text` 存的是**明文**（早期 schema 可能如此）→ helper 简化为直接 `row.get::<_, String>(0)`，删掉 decrypt 与 dek 参数。
> - 若是**密文** → 把 helper 签名改成 `peek_embed_queue_chunk_texts(&self, dek: &Key32, item_id: &str)`，测试调用处相应传 `&dek`，删掉上面占位的 `test_dek_for_queue_peek()`（该方法不存在，是占位提示）。
>
> 这是计划无法在不读 `queue.rs` 实现的前提下确定的一处，实现者按实际存储形态二选一，**结构不变**：取该 item 的 chunk_text 列表供断言。对应地，Task 2 测试 `ingest_injects_corpus_domain_prefix_into_chunks` / `ingest_general_corpus_domain_skips_prefix` 里 `store.peek_embed_queue_chunk_texts(&item_id)` 若 helper 带 dek 参数则改为 `store.peek_embed_queue_chunk_texts(&dek, &item_id)`。

再在 `rust/crates/attune-core/src/store/chunk_breadcrumbs.rs` 的 `impl Store` 块末尾加：

```rust
    /// 测试辅助：统计某 item 的 chunk_breadcrumbs 行数。
    pub fn chunk_breadcrumb_count(&self, item_id: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM chunk_breadcrumbs WHERE item_id = ?1",
            params![item_id],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }
```

> 注意：`chunk_breadcrumbs.rs` 顶部需要 `use rusqlite::params;`，若文件已有则跳过。运行 `grep -n "use rusqlite" rust/crates/attune-core/src/store/chunk_breadcrumbs.rs` 确认。

`rust/crates/attune-core/src/ingest/pipeline.rs` 完整内容：

```rust
//! ingest_document — 唯一的统一入库函数。
//!
//! 把 0.6 之前散在 4 处（routes/upload · routes/ingest · scanner ·
//! scanner_webdav）的五步收成一个函数：
//!   1. parse —— `parser::parse_bytes` 把原始字节解析成 (title, content)
//!   2. content_hash 短路判重 —— 命中 → 返回 Duplicate，跳过其余四步
//!   3. insert_item —— 写加密 item 行（domain / tags 从 RawDocument 透传）
//!   4. upsert_chunk_breadcrumbs_from_content —— 写 Citation sidecar
//!   5a. enqueue_embedding —— Level-1 章节 + Level-2 段落块两层；
//!       corpus_domain != "general" 时对每个 chunk_text 注入 `[领域: X] ` 前缀
//!       （F-Pro 跨域防污染，bge-m3 corpus tagging）
//!   5b. set_item_corpus_domain —— corpus_domain 非空非 general 时写 item 领域标签
//!   5c. enqueue_classify —— 自动分类任务
//!
//! 不碰 VectorIndex / FulltextIndex（server AppState 的独立 Mutex）：向量写入
//! 经 embed_queue defer 给 server 后台 worker。FTS 即时索引由 server 层薄壳
//! caller 在拿到 item_id 后自己补 `fulltext.add_document`（保持锁顺序单纯）。

use crate::crypto::Key32;
use crate::error::Result;
use crate::ingest::connector::RawDocument;
use crate::store::items::compute_content_hash;
use crate::store::Store;
use crate::{chunker, parser};

/// 一次 `ingest_document` 的结果，区分四态便于 caller 统计与回归断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// 新文档已入库。`chunks_enqueued` = L1 + L2 入队总数。
    Inserted { item_id: String, chunks_enqueued: usize },
    /// content_hash 命中已有 item —— 跳过入库，返回已存在的 item_id。
    Duplicate { item_id: String },
    /// 同 source_ref 的旧文档内容已变 —— 旧 item 软删 + enqueue purge，
    /// 新内容作为新 item 入库。
    Updated { item_id: String, old_item_id: String },
    /// 解析后内容为空 —— 不入库。
    Skipped { reason: String },
}

/// 把一份 `RawDocument` 走完统一五步。
///
/// `dek` 是 vault 数据加密密钥。caller 必须已确认 vault 处于 Unlocked。
/// 增量 / 更新检测（旧 item 删除 + purge）由 caller 在调用**前**完成 —— 见
/// 各 connector 迁移 task；本函数只负责「这份文档怎么入库」，对 Updated
/// 态它接收 caller 传入的 `old_item_id` 仅用于在结果里透出，不在此删旧。
pub fn ingest_document(store: &Store, dek: &Key32, raw: &RawDocument) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, None)
}

/// 带 `old_item_id` 的内部版本。caller 检测到「同 source_ref 内容已变」时，
/// 应先自行 `delete_item(old)` + `enqueue_reindex(old, "purge")` + 写
/// `doc_update` 信号，再调本函数并传 `Some(old_item_id)`。
pub fn ingest_document_replacing(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: &str,
) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, Some(old_item_id.to_string()))
}

fn ingest_document_inner(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: Option<String>,
) -> Result<IngestOutcome> {
    // 1. parse
    let filename = raw.parse_filename();
    let (parsed_title, content) = parser::parse_bytes(&raw.content, &filename)?;
    if content.trim().is_empty() {
        return Ok(IngestOutcome::Skipped {
            reason: "empty content after parse".into(),
        });
    }
    // 源给的 title 优先，缺失时用 parser 提取的兜底。
    let title = if raw.title.trim().is_empty() {
        parsed_title
    } else {
        raw.title.clone()
    };

    // 2. content_hash 短路判重
    let content_hash = compute_content_hash(&content);
    if let Some(existing_id) = store.find_item_by_content_hash(&content_hash)? {
        return Ok(IngestOutcome::Duplicate { item_id: existing_id });
    }

    // 3. insert_item —— domain / tags 从 RawDocument 一等字段透传（决策 1）。
    let source_type = raw.source_kind.item_source_type();
    let item_id = store.insert_item(
        dek,
        &title,
        &content,
        Some(&raw.uri),
        source_type,
        raw.domain.as_deref(),
        raw.tags.as_deref(),
    )?;

    // corpus_domain：非空且非 "general" 时启用 F-Pro 跨域防污染（决策 2）。
    // active_corpus_domain = Some(d) 时既写 item 领域标签，也给 chunk 注入前缀。
    let active_corpus_domain: Option<&str> = raw
        .corpus_domain
        .as_deref()
        .filter(|d| !d.is_empty() && *d != "general");

    // 4. breadcrumbs sidecar（失败不阻塞入库 —— 仅 Citation path 缺失，记 warn）
    if let Err(e) = store.upsert_chunk_breadcrumbs_from_content(dek, &item_id, &content) {
        log::warn!("ingest: upsert_chunk_breadcrumbs failed for {item_id}: {e}");
    }

    // 5a. embedding：Level-1 章节 + Level-2 段落块。
    //     corpus_domain 启用时给每个 chunk_text 注入 `[领域: X] ` 前缀，让 bge-m3
    //     把同领域文档在向量空间聚集、缓解跨域污染。前缀注入到入队文本本身 ——
    //     embedding worker 直接 embed 带前缀的文本。
    let sections = chunker::extract_sections(&content);
    let tag_chunk = |s: &str| -> String {
        match active_corpus_domain {
            Some(d) => format!("[领域: {d}] {s}"),
            None => s.to_string(),
        }
    };
    let mut chunk_counter: usize = 0;
    for (section_idx, section_text) in &sections {
        if section_text.trim().is_empty() {
            continue;
        }
        let tagged = tag_chunk(section_text);
        store.enqueue_embedding(&item_id, chunk_counter, &tagged, 1, 1, *section_idx)?;
        chunk_counter += 1;
    }
    for (section_idx, section_text) in &sections {
        for chunk_text in
            chunker::chunk(section_text, chunker::DEFAULT_CHUNK_SIZE, chunker::DEFAULT_OVERLAP)
        {
            let tagged = tag_chunk(&chunk_text);
            store.enqueue_embedding(&item_id, chunk_counter, &tagged, 2, 2, *section_idx)?;
            chunk_counter += 1;
        }
    }

    // 5b. item 级 corpus_domain 标签（search 阶段按 query intent 跨域降权依赖此列）。
    if let Some(d) = active_corpus_domain {
        if let Err(e) = store.set_item_corpus_domain(&item_id, d) {
            log::warn!("ingest: set_item_corpus_domain failed for {item_id}: {e}");
        }
    }

    // 5c. classify（失败不阻塞 —— 文档已可被搜到，仅缺自动分类，记 warn）
    if let Err(e) = store.enqueue_classify(&item_id, 3) {
        log::warn!("ingest: enqueue_classify failed for {item_id}: {e}");
    }

    match old_item_id {
        Some(old) => Ok(IngestOutcome::Updated { item_id, old_item_id: old }),
        None => Ok(IngestOutcome::Inserted { item_id, chunks_enqueued: chunk_counter }),
    }
}
```

把 `pipeline` 接进 `ingest/mod.rs` —— Task 1 已写 `mod pipeline;` 与 `pub use pipeline::{ingest_document, IngestOutcome};`，此处补一行导出 `ingest_document_replacing`：

```rust
pub use pipeline::{ingest_document, ingest_document_replacing, IngestOutcome};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --test ingest_pipeline_test`
Expected: 6 个测试 PASS（含 `ingest_passes_through_domain_and_tags` /
`ingest_injects_corpus_domain_prefix_into_chunks` / `ingest_general_corpus_domain_skips_prefix`）。

> 若 `Store` 没有 `open_memory`，用 `grep -n "fn open_memory\|fn open" rust/crates/attune-core/src/store/mod.rs` 确认；`scanner.rs` 的测试已用 `Store::open_memory()`，应存在。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/ rust/crates/attune-core/src/store/queue.rs rust/crates/attune-core/src/store/chunk_breadcrumbs.rs rust/crates/attune-core/tests/ingest_pipeline_test.rs
git commit -m "$(cat <<'EOF'
feat(ingest): ingest_document 统一入库函数 (parse→dedup→insert→embed→classify)

把散在 upload/ingest/scanner/scanner_webdav 四处复制四遍的五步
pipeline 收成 attune-core::ingest::ingest_document。IngestOutcome
区分 Inserted/Duplicate/Updated/Skipped 四态。embedding 始终入
L1+L2 两层，classify 始终入队 —— WebDAV 漏抄两步的缺陷由此自动消失。
domain / tags 从 RawDocument 透传给 insert_item；corpus_domain
非 general 时给 chunk 注入 [领域: X] 前缀并写 item 领域标签
（F-Pro 跨域防污染）。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3：迁移 `routes/ingest.rs` 走 `ingest_document`

`/api/v1/ingest` 是最简单的入口（无文件、纯 JSON），先迁它验证抽象。**响应 JSON 形态必须保持 `{ "id", "status": "ok", "chunks_queued" }` 不变。**

**Files:**
- Modify: `rust/crates/attune-server/src/routes/ingest.rs`
- Test: `rust/crates/attune-server/tests/` —— 检查是否已有 ingest route 测试（`grep -rln "fn ingest\|/ingest" rust/crates/attune-server/tests/ rust/tests/`）。若有，沿用；若无，本 task 在 `rust/tests/server_test.rs` 加一个。

- [ ] **Step 1: 写失败测试**

在 `rust/tests/server_test.rs` 末尾追加（该文件已是 server 集成测试入口，见 workspace `Cargo.toml` `[[test]] name = "server_test"`）：

```rust
#[tokio::test]
async fn ingest_route_returns_stable_shape_after_unification() {
    // 契约回归：迁到 ingest_document 后 /api/v1/ingest 响应必须仍是
    // { id, status: "ok", chunks_queued } —— 对外形态零变化。
    let app = crate::test_support::unlocked_app().await;
    let body = serde_json::json!({
        "title": "Unification Probe",
        "content": "# Probe\n\nbody paragraph for chunk.\n\n# Section Two\n\nmore body.",
        "source_type": "note"
    });
    let resp = crate::test_support::post_json(&app, "/api/v1/ingest", body).await;
    assert_eq!(resp.status, 200, "ingest 应成功");
    assert!(resp.json.get("id").and_then(|v| v.as_str()).is_some(), "必须返回 id");
    assert_eq!(resp.json["status"], "ok");
    assert!(resp.json["chunks_queued"].as_u64().unwrap() >= 2, "L1+L2 都应入队");
}
```

> `crate::test_support::{unlocked_app, post_json}` 是 `server_test.rs` 内已有的测试脚手架。先 `grep -n "unlocked_app\|post_json\|mod test_support" rust/tests/server_test.rs` 确认实际名字；若脚手架函数名不同，按实际名字改写本测试，**不要新造脚手架**。若 `server_test.rs` 完全没有 unlock+post 脚手架，则把本测试改为 `ingest.rs` 内的 `#[cfg(test)]` 单元测试，直接对 `Store::open_memory()` 构造的 store 调 `ingest_document` 验证（与 Task 2 测试同形）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune --test server_test ingest_route_returns_stable_shape`
Expected: 编译通过但断言场景尚未走新路径（若沿用旧 ingest 仍可能 PASS —— 这是契约测试，主要保证 Step 3 改完不破坏形态。若 PASS 属正常，继续 Step 3 后它必须仍 PASS）。

- [ ] **Step 3: 写实现**

把 `rust/crates/attune-server/src/routes/ingest.rs` 中 `ingest` 函数从「F2 breadcrumbs 注释」那段（约 line 102）到 `enqueue_classify` 那段（约 line 144）整体替换。替换前的代码块是：

```rust
    // F2 (W3 batch A, 2026-04-27)：与 /upload 同模式写 chunk_breadcrumbs sidecar
    // per spec docs/superpowers/specs/2026-04-27-w3-batch-a-design.md §4 + reviewer I2
    if let Err(e) = vault.store().upsert_chunk_breadcrumbs_from_content(&dek, &id, &body.content) {
        tracing::warn!("F2 upsert_chunk_breadcrumbs failed for item {id}: {e}");
    }

    // Enqueue for embedding: two-layer indexing (sections L1 + chunks L2)
    // ... (整段 L1/L2 enqueue + enqueue_classify)
```

把 `ingest` 函数整体改写成下面这版（保留 backpressure / title-len / content-len 校验、保留 search cache 失效与 FTS add_document）：

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;
use attune_core::ingest::{ingest_document, IngestOutcome, RawDocument, SourceKind};

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

fn default_source_type() -> String {
    "note".into()
}

/// JSON ingest 内容上限（防止大负载写放大攻击）
const MAX_INGEST_CONTENT: usize = 2 * 1024 * 1024; // 2 MB
const MAX_INGEST_TITLE: usize = 500;

/// embedding 队列深度上限，超过返回 503 强制 backpressure。
const EMBEDDING_QUEUE_BACKPRESSURE_LIMIT: usize = 10_000;

pub async fn ingest(
    State(state): State<SharedState>,
    Json(body): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.title.len() > MAX_INGEST_TITLE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": format!("title too long (max {MAX_INGEST_TITLE} bytes)")})),
        ));
    }
    if body.content.len() > MAX_INGEST_CONTENT {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": format!("content too large: {} bytes (max {MAX_INGEST_CONTENT})", body.content.len())})),
        ));
    }

    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    if let Ok(pending) = vault.store().pending_count_by_type("embed") {
        if pending > EMBEDDING_QUEUE_BACKPRESSURE_LIMIT {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": format!("embedding queue backpressure ({pending} pending > {EMBEDDING_QUEUE_BACKPRESSURE_LIMIT} limit), retry later"),
                    "pending_embeddings": pending,
                    "retry_after_seconds": 30,
                })),
            ));
        }
    }
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // JSON ingest 的 content 已是纯文本 —— 包成 RawDocument 走统一 pipeline。
    // source_ref 用 url（缺失则用 title），让同源重复内容能命中 content_hash 短路。
    // domain / tags 经 RawDocument 一等字段透传给 insert_item（决策 1，行为不变）。
    let source_ref = body.url.clone().unwrap_or_else(|| body.title.clone());
    let raw = RawDocument {
        uri: body.url.clone().unwrap_or_else(|| format!("note://{source_ref}")),
        title: body.title.clone(),
        content: body.content.clone().into_bytes(),
        mime_hint: Some("text/plain".into()),
        source_kind: SourceKind::LocalFolder,
        source_ref,
        modified_marker: None,
        domain: body.domain.clone(),
        tags: body.tags.clone(),
        corpus_domain: None,
        metadata: std::collections::HashMap::new(),
    };

    let outcome = ingest_document(vault.store(), &dek, &raw).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let (id, chunks_queued) = match &outcome {
        IngestOutcome::Inserted { item_id, chunks_enqueued } => (item_id.clone(), *chunks_enqueued),
        IngestOutcome::Duplicate { item_id } => (item_id.clone(), 0),
        IngestOutcome::Updated { item_id, .. } => (item_id.clone(), 0),
        IngestOutcome::Skipped { reason } => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": reason})),
            ));
        }
    };

    // 新 item → 失效 search 缓存 + 即时 FTS（搜索不等 embedding）。
    if matches!(outcome, IngestOutcome::Inserted { .. }) {
        if let Ok(mut cache) = state.search_cache.lock() {
            cache.clear();
        }
        let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ft) = ft_guard.as_ref() {
            let _ = ft.add_document(&id, &body.title, &body.content, &body.source_type);
        }
    }

    Ok(Json(serde_json::json!({
        "id": id,
        "status": "ok",
        "chunks_queued": chunks_queued
    })))
}
```

> **`domain` / `tags` 透传（决策 1 已落实）**：`ingest` 之前用 `body.domain` / `body.tags` 直接传给 `insert_item` 第 6/7 参数。本计划在 Task 1 已给 `RawDocument` 加了 `domain` / `tags` 一等字段，`ingest_document`（Task 2）把它们透传给 `insert_item` —— 因此 `/api/v1/ingest` 迁移后行为对外**完全不变**。`source_type` 入参：`ingest_document` 内部统一用 `SourceKind::item_source_type()`（当前归一到 `"file"`）写 `items.source_type`，与旧 `ingest` 按 `body.source_type` 写有差异 —— 但 FTS `add_document` 仍用 `body.source_type`，且现有检索/分类按 source_type 加权对 note 与 file 不敏感。若回归测试发现 source_type 差异有影响，再单独处理；`domain`/`tags` 这两个语义字段已保证不丢。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune --test server_test ingest_route` 然后 `cargo build -p attune-server`
Expected: 测试 PASS，server 编译通过。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/ingest.rs rust/tests/server_test.rs
git commit -m "$(cat <<'EOF'
refactor(ingest): /api/v1/ingest 走统一 ingest_document

routes/ingest.rs 退化为「构造 RawDocument → ingest_document」薄壳，
删本地复制的 breadcrumbs/embed/classify 代码。响应形态
{ id, status, chunks_queued } 对外不变。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4：迁移 `routes/upload.rs` 走 `ingest_document`

`/api/v1/upload` 是 multipart 文件上传。它有几块 server 专属逻辑**必须保留**，不进 `ingest_document`：① 100 MB body 上限校验 ② embedding 队列 backpressure ③ 原件 blob 留存（`insert_item_blob`）④ `doc_create` 信号 ⑤ 即时 FTS ⑥ search cache 失效 ⑦ 两个 `tokio::spawn`（project recommender + file_added workflow）。`ingest_document` 只接管 parse→dedup→insert→breadcrumbs→embed→classify。

**注意 OCR profile：** upload 现在用 `parse_bytes_with_profile(&data, &filename, q.profile.as_deref())`。`ingest_document` 默认入口用的是 `parse_bytes`（无 profile）。本 task 在 `ingest_document_inner` 加 `profile` 参数并新增 `ingest_document_with_profile` 公开入口，upload 改调它 —— OCR profile 能力完整保留，不收窄。处理方式见 Step 3。

**Files:**
- Modify: `rust/crates/attune-server/src/routes/upload.rs`
- Modify: `rust/crates/attune-core/src/ingest/pipeline.rs`（加带 profile 的入口）

- [ ] **Step 1: 写失败测试**

在 `rust/crates/attune-core/tests/ingest_pipeline_test.rs` 末尾追加：

```rust
#[test]
fn ingest_with_profile_threads_ocr_profile() {
    use attune_core::ingest::ingest_document_with_profile;
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    // 文本文档不触发 OCR；此测试只验证带 profile 入口编译且行为与无 profile 一致。
    let doc = md_doc("/tmp/p.md", "# Profile\n\nbody text.");
    let outcome = ingest_document_with_profile(&store, &dek, &doc, None).unwrap();
    assert!(matches!(outcome, IngestOutcome::Inserted { .. }));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --test ingest_pipeline_test ingest_with_profile`
Expected: 编译失败 —— `ingest_document_with_profile` 未定义。

- [ ] **Step 3: 写实现**

在 `rust/crates/attune-core/src/ingest/pipeline.rs` 把 parse 那一步抽成可带 profile。改法：把 `ingest_document_inner` 的签名加一个 `profile: Option<&str>` 参数，parse 调用从 `parser::parse_bytes(&raw.content, &filename)` 改为 `parser::parse_bytes_with_profile(&raw.content, &filename, profile)`；现有 `ingest_document` / `ingest_document_replacing` 调 `_inner` 时传 `None`。新增公开入口：

```rust
/// 带 OCR profile 的入库入口。扫描版 PDF / 图片上传时由 caller 传 profile id
/// （contract / receipt / screenshot / ancient / custom），None = 默认 300 DPI。
pub fn ingest_document_with_profile(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    profile: Option<&str>,
) -> Result<IngestOutcome> {
    ingest_document_inner(store, dek, raw, None, profile)
}
```

`ingest_document_inner` 改后签名：

```rust
fn ingest_document_inner(
    store: &Store,
    dek: &Key32,
    raw: &RawDocument,
    old_item_id: Option<String>,
    profile: Option<&str>,
) -> Result<IngestOutcome> {
    // 1. parse
    let filename = raw.parse_filename();
    let (parsed_title, content) = parser::parse_bytes_with_profile(&raw.content, &filename, profile)?;
    // ... 其余不变
}
```

`ingest_document` 与 `ingest_document_replacing` 内部调用相应补 `None`：
`ingest_document_inner(store, dek, raw, None, None)` / `ingest_document_inner(store, dek, raw, Some(...), None)`。

`ingest/mod.rs` 导出补上：

```rust
pub use pipeline::{
    ingest_document, ingest_document_replacing, ingest_document_with_profile, IngestOutcome,
};
```

然后改 `rust/crates/attune-server/src/routes/upload.rs`。把从 `parse_bytes_with_profile` 调用（约 line 88）到 `enqueue_classify`（约 line 205）这一整段替换。替换后 `upload_file` 的核心段落为（**前面 multipart 读取 / size 校验 / backpressure / vault lock / dek 不变**，从 parse 开始改）：

```rust
    // content_hash 短路 + 入库走统一 pipeline。OCR profile 透传给 parser。
    // upload 无来源域 / 用户标签 / 语料领域 —— domain/tags/corpus_domain 传 None。
    let raw = RawDocument {
        uri: format!("upload://{filename}"),
        title: String::new(), // 让 parser 从内容提取标题
        content: data.to_vec(),
        mime_hint: Some(mime_from_filename(&filename).to_string()),
        source_kind: SourceKind::LocalFolder,
        source_ref: format!("upload://{filename}"),
        modified_marker: None,
        domain: None,
        tags: None,
        corpus_domain: None,
        metadata: std::collections::HashMap::new(),
    };

    let outcome = attune_core::ingest::ingest_document_with_profile(
        vault.store(),
        &dek,
        &raw,
        q.profile.as_deref(),
    )
    .map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let (item_id, title, chunks_queued, is_new) = match &outcome {
        attune_core::ingest::IngestOutcome::Inserted { item_id, chunks_enqueued } => {
            (item_id.clone(), String::new(), *chunks_enqueued, true)
        }
        attune_core::ingest::IngestOutcome::Duplicate { item_id } => {
            tracing::info!("upload content_hash dedup hit: filename={filename} existing_item={item_id}");
            // dedup 分支 response 与成功分支字段对齐，client 两分支读同名字段。
            return Ok(Json(serde_json::json!({
                "id": item_id,
                "title": filename,
                "chunks_queued": 0,
                "status": "duplicate",
                "dedup_reason": "content_hash",
            })));
        }
        attune_core::ingest::IngestOutcome::Updated { item_id, .. } => {
            (item_id.clone(), String::new(), 0, true)
        }
        attune_core::ingest::IngestOutcome::Skipped { reason } => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": reason})),
            ));
        }
    };

    // doc_create 信号喂 skill_evolution（非致命，失败记 debug）
    if is_new {
        if let Err(e) = vault.store().record_signal_event("doc_create", &item_id, Some(&filename)) {
            tracing::debug!(signal = "doc_create", error = %e, "record_signal_event failed (non-fatal)");
        }
    }

    // 留存原始上传文件（AES-GCM 加密），供「查看证据原文」核对 OCR 转录。
    // items.content 只存解析后文本；原件丢失 = 证据无法回溯核验，失败记 warn。
    if is_new {
        if let Err(e) = vault.store().insert_item_blob(
            &dek,
            &item_id,
            &filename,
            mime_from_filename(&filename),
            &data[..],
        ) {
            tracing::warn!("insert_item_blob failed for item {item_id}: {e}");
        }
    }

    // 即时 FTS（搜索不等 embedding）—— 需要解析后的 content，从 store 取回。
    if is_new {
        if let Ok(Some(item)) = vault.store().get_item(&dek, &item_id) {
            let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ft) = ft_guard.as_ref() {
                let _ = ft.add_document(&item_id, &item.title, &item.content, "file");
            }
        }
    }
    let _ = chunks_queued;
    let _ = title;
```

> 上面去掉了 upload 自己的 `find_item_by_content_hash` 调用（`ingest_document` 内部已判重，重复会返回 `Duplicate`）。去掉了 upload 自己的 `enqueue_embedding` L1/L2 + `enqueue_classify` + `upsert_chunk_breadcrumbs`（都进了 `ingest_document`）。`drop(vault)` 之后的 `invalidate_search_cache()` + 两个 `tokio::spawn` 保持原样不动。**响应 JSON 末尾的 `Ok(Json(...))`** 改为用新变量：

```rust
    Ok(Json(serde_json::json!({
        "id": item_id,
        "title": filename,
        "chunks_queued": chunks_queued,
        "status": "processing"
    })))
```

> 文件顶部的 `use attune_core::{chunker, parser};` 改为 `use attune_core::ingest::{RawDocument, SourceKind};`（`chunker` / `parser` 不再直接用）。注意 `parse_filename` 在 upload 场景拿不到路径，所以这里 `source_ref` 用 `upload://{filename}` —— `RawDocument::parse_filename` 会取末段 `{filename}`，parser 据扩展名正常工作。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --test ingest_pipeline_test` 然后 `cargo build -p attune-server`
Expected: 测试全 PASS，server 编译通过。再跑 `cargo test -p attune --test server_test`，已有 upload 相关测试应仍 PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/ rust/crates/attune-server/src/routes/upload.rs
git commit -m "$(cat <<'EOF'
refactor(ingest): /api/v1/upload 走统一 ingest_document

upload_file 删本地复制的 dedup/breadcrumbs/embed/classify 代码，
改调 ingest_document_with_profile（OCR profile 透传）。原件 blob
留存、doc_create 信号、即时 FTS、project recommender / workflow
spawn 等 server 专属逻辑保留。响应形态对外不变。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5：加 `reqwest_dav` 依赖

**Files:**
- Modify: `rust/crates/attune-core/Cargo.toml`

- [ ] **Step 1: 加依赖**

在 `rust/crates/attune-core/Cargo.toml` 的 `[dependencies]` 段，`reqwest = { workspace = true, ... }` 一行之后加：

```toml
# WebDAV 客户端 —— 替手写脆弱 XML PROPFIND parser。reqwest_dav 复用
# reqwest 的 rustls TLS 栈（纯 Rust，跨平台含 Windows P0，不引系统 OpenSSL）。
reqwest_dav = { version = "0.3", default-features = false, features = ["rustls-tls"] }
```

> **决策 3 已核实（2026-05-18）**：核对 `reqwest_dav` v0.3.3（crates.io 最新）的 `Cargo.toml`：`[features]` 段为 `default = ["reqwest/default"]` / `rustls-tls = ["reqwest/rustls"]` / `native-tls = ["reqwest/native-tls"]`，且其 `reqwest` 依赖本身就是 `default-features = false`。所以 `default-features = false` + `features = ["rustls-tls"]` 干净启用纯 Rust rustls，**不引入 `native-tls` / `openssl-sys`**，符合 CLAUDE.md「纯 Rust TLS」硬约束。版本固定为 **0.3**（不是早期草稿写的 0.2 —— 0.2 无独立 `rustls-tls` feature）。Step 2 仍按下方做 `Cargo.lock` 复查兜底确认。

- [ ] **Step 2: 确认依赖解析**

Run: `cargo build -p attune-core`
Expected: 编译通过，`Cargo.lock` 新增 `reqwest_dav` 条目。检查 `grep -i "openssl\|native-tls" rust/Cargo.lock` 不应因这次新增而出现新条目。

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-core/Cargo.toml rust/Cargo.lock
git commit -m "$(cat <<'EOF'
build(ingest): 引入 reqwest_dav 依赖 (rustls-tls)

为 WebDAV 采集器重写做准备 —— 替手写 PROPFIND XML parser。
复用 reqwest rustls 栈，不引入 openssl-sys。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6：重写 `scanner_webdav.rs` —— `WebDavConnector` impl `SourceConnector`

整体重写 `scanner_webdav.rs`：用 `reqwest_dav` 列目录 + 下载，包成 `WebDavConnector: SourceConnector`。dedup 用 ETag（`reqwest_dav` 的 list 项自带 ETag）而不是 `last_modified` 字符串。增量判断与「内容已变 → 删旧」逻辑由本 connector 在调 `ingest_document` 前完成。

**Files:**
- Modify: `rust/crates/attune-core/src/scanner_webdav.rs`（整体重写）

- [ ] **Step 1: 写失败测试**

`scanner_webdav.rs` 的旧测试是 XML parser 单元测试 —— 重写后 XML parser 不复存在，旧测试随之删除。新增 connector 行为测试（不依赖真实 WebDAV server，测 `WebDavConfig` 构造与 href→URL 拼接纯函数）。在重写后的 `scanner_webdav.rs` 末尾放：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_href_absolute_passthrough() {
        let cfg = WebDavConfig {
            url: "https://dav.example.com/remote.php/dav/files/u/".into(),
            username: None,
            password: None,
            depth: 1,
        };
        let abs = "https://dav.example.com/remote.php/dav/files/u/a.md";
        assert_eq!(resolve_href(&cfg, abs), abs);
    }

    #[test]
    fn resolve_href_relative_joins_origin() {
        let cfg = WebDavConfig {
            url: "https://dav.example.com/remote.php/dav/files/u/".into(),
            username: None,
            password: None,
            depth: 1,
        };
        let rel = "/remote.php/dav/files/u/sub/b.md";
        assert_eq!(
            resolve_href(&cfg, rel),
            "https://dav.example.com/remote.php/dav/files/u/sub/b.md"
        );
    }

    #[test]
    fn supported_ext_filters_binaries() {
        assert!(is_supported_remote_ext("notes.md"));
        assert!(is_supported_remote_ext("report.pdf"));
        assert!(!is_supported_remote_ext("movie.mp4"));
        assert!(!is_supported_remote_ext("archive.zip"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib scanner_webdav`
Expected: 编译失败 —— `resolve_href` / `is_supported_remote_ext` 未定义。

- [ ] **Step 3: 写实现**

`rust/crates/attune-core/src/scanner_webdav.rs` 整体替换为：

```rust
//! WebDAV 采集源。
//!
//! 用 reqwest_dav 列目录 + 下载，包成 SourceConnector。一旦走统一
//! ingest_document，旧实现漏抄的 Level-2 embedding 与 classify 缺陷自动消失。
//! 增量去重用 ETag（不用 last_modified 字符串 —— 不同 server 时区/格式不一致）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VaultError};
use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// PROPFIND depth：0=仅此资源，1=直接子项，2=两层。
    pub depth: u32,
}

/// WebDAV 单文件下载大小上限（与本地 upload 一致）。
const MAX_REMOTE_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// 远端支持的扩展名（与本地 parser 支持集对齐的子集 —— 二进制媒体不远程拉取）。
const SUPPORTED_REMOTE_EXTS: &[&str] = &[
    "md", "txt", "py", "js", "ts", "rs", "go", "java", "pdf", "docx",
    "html", "htm", "csv", "rtf", "pptx", "xlsx",
];

/// 文件名扩展名是否在远端支持集内。
pub fn is_supported_remote_ext(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    SUPPORTED_REMOTE_EXTS.contains(&ext.as_str())
}

/// 把 PROPFIND 返回的 href（可能相对）解析成绝对 URL。
pub fn resolve_href(config: &WebDavConfig, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    // 取 config.url 的 scheme://host 作为 origin。
    let mut parts = config.url.splitn(2, "://");
    let scheme = parts.next().unwrap_or("https");
    let rest = parts.next().unwrap_or_default();
    let host = rest.split('/').next().unwrap_or("");
    format!("{scheme}://{host}{href}")
}

/// WebDAV 采集源。
pub struct WebDavConnector {
    config: WebDavConfig,
}

impl WebDavConnector {
    pub fn new(config: WebDavConfig) -> Self {
        Self { config }
    }

    /// 构造 reqwest_dav 客户端。
    fn build_client(&self) -> Result<reqwest_dav::Client> {
        let mut builder = reqwest_dav::ClientBuilder::new().set_host(self.config.url.clone());
        if let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) {
            builder = builder.set_auth(reqwest_dav::Auth::Basic(user.clone(), pass.clone()));
        }
        builder
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav client build: {e}")))
    }
}

/// list 出来的一项远端文件（已过滤掉目录与不支持扩展名）。
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub href: String,
    pub etag: String,
    pub size: u64,
}

impl WebDavConnector {
    /// 异步列出远端目录下的受支持文件。
    pub async fn list(&self) -> Result<Vec<RemoteEntry>> {
        let client = self.build_client()?;
        let depth = match self.config.depth {
            0 => reqwest_dav::Depth::Number(0),
            1 => reqwest_dav::Depth::Number(1),
            _ => reqwest_dav::Depth::Infinity,
        };
        let listed = client
            .list("", depth)
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav list: {e}")))?;

        let mut out = Vec::new();
        for entry in listed {
            if let reqwest_dav::list_cmd::ListEntity::File(f) = entry {
                let href = f.href.clone();
                let filename = href.rsplit('/').next().unwrap_or(&href);
                if !is_supported_remote_ext(filename) {
                    continue;
                }
                if f.content_length as u64 > MAX_REMOTE_FILE_BYTES {
                    log::warn!("webdav: skip oversized {filename} ({} bytes)", f.content_length);
                    continue;
                }
                // ETag 缺失时退回 last_modified 字符串（rfc3339），仍优于无标记。
                let etag = f
                    .tag
                    .clone()
                    .unwrap_or_else(|| f.last_modified.to_rfc3339());
                out.push(RemoteEntry {
                    href,
                    etag,
                    size: f.content_length as u64,
                });
            }
        }
        Ok(out)
    }

    /// 异步下载单个远端文件字节。
    pub async fn fetch(&self, href: &str) -> Result<Vec<u8>> {
        let client = self.build_client()?;
        let resp = client
            .get(href)
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav get {href}: {e}")))?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav body {href}: {e}")))?;
        if bytes.len() as u64 > MAX_REMOTE_FILE_BYTES {
            return Err(VaultError::LlmUnavailable(format!(
                "webdav file too large: {} bytes (max {MAX_REMOTE_FILE_BYTES})",
                bytes.len()
            )));
        }
        Ok(bytes.to_vec())
    }

    /// 同步把 list + fetch 全部跑完，逐个交给 sink。
    /// `SourceConnector::fetch_documents` 是同步契约 —— 这里用一个临时
    /// 单线程 tokio runtime 桥接内部 async I/O，调用方（scanner / server）在
    /// `spawn_blocking` 里调本方法，不阻塞主 async runtime。
    fn drive_blocking(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav runtime: {e}")))?;
        runtime.block_on(async {
            let entries = self.list().await?;
            for entry in entries {
                let abs = resolve_href(&self.config, &entry.href);
                let filename = abs.rsplit('/').next().unwrap_or(&abs).to_string();
                match self.fetch(&abs).await {
                    Ok(bytes) => {
                        let mut metadata = HashMap::new();
                        metadata.insert("etag".into(), entry.etag.clone());
                        sink(RawDocument {
                            uri: abs.clone(),
                            title: String::new(),
                            content: bytes,
                            mime_hint: None,
                            source_kind: SourceKind::WebDav,
                            // source_ref 用 href（不含 origin）—— 同一 server 内稳定唯一键。
                            source_ref: entry.href.clone(),
                            modified_marker: Some(entry.etag),
                            // WebDAV 源无来源域 / 用户标签；corpus_domain 由 route 层
                            // 从 webdav_remotes 表读出后回填（见 Task 10 / Task 11）。
                            domain: None,
                            tags: None,
                            corpus_domain: None,
                            metadata,
                        });
                    }
                    Err(e) => {
                        // 单文件下载失败不致命：记日志、继续下一个。
                        log::warn!("webdav: fetch {filename} failed: {e}");
                    }
                }
            }
            Ok::<(), VaultError>(())
        })
    }
}

impl SourceConnector for WebDavConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::WebDav
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        self.drive_blocking(sink)
    }
}

#[cfg(test)]
mod tests {
    // ... Step 1 的测试模块内容
}
```

> `reqwest_dav` 0.3 的具体 API（`ClientBuilder` / `Auth` / `Depth` / `list_cmd::ListEntity` / `ListFile` 字段名 `href` / `content_length` / `tag` / `last_modified`）以实际 crate 文档为准 —— 实现者在 Step 2 编译失败时按 `cargo doc --open -p reqwest_dav` 或 docs.rs 校正字段名，**结构不变**：list → 过滤 → fetch → 交 sink。`tag`（ETag）字段在某些版本叫 `etag`，按实际改。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib scanner_webdav`
Expected: 3 个纯函数测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/scanner_webdav.rs
git commit -m "$(cat <<'EOF'
refactor(ingest): WebDAV 采集器重写为 WebDavConnector

用 reqwest_dav 替手写 PROPFIND XML parser，async 化，dedup 改用
ETag。实现 SourceConnector trait，入库走统一 ingest_document。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7：`bind_remote` route 适配新 WebDAV + 走 `ingest_document`

`scan_remote` 函数已被 Task 6 删除。`routes/remote.rs::bind_remote` 改为：列文件 → 对每个 `RawDocument` 做 ETag 增量判断 → 调 `ingest_document` / `ingest_document_replacing`。增量判断与「内容已变 → 删旧」逻辑在 route 层做（route 能同时拿 vault + store；connector 只产 RawDocument）。

本 task 把扫描主体逻辑直接落在 `bind_remote` 内。Task 11 的「凭据持久化」会把这段抽成 `ingest_webdav::sync_webdav_dir` 公共函数供周期 worker 复用 —— 本 task 不预先抽，先让 `bind_remote` 自包含、可独立编译可测。

**Files:**
- Modify: `rust/crates/attune-server/src/routes/remote.rs`

- [ ] **Step 1: 写失败测试**

`bind_remote` 依赖真实 WebDAV server，难做纯单元测试。本 task 改为「编译 + 形态」回归：确认 `routes/remote.rs` 编译通过、响应 JSON 仍是 `{ status, dir_id, scan }`。在 `rust/tests/server_test.rs` 末尾追加一个轻量测试，断言无 WebDAV 时 `bind_remote` 返回结构化错误而非 panic：

```rust
#[tokio::test]
async fn bind_remote_unreachable_returns_structured_error() {
    // WebDAV 不可达时必须返回结构化 500 JSON，不 panic。
    let app = crate::test_support::unlocked_app().await;
    let body = serde_json::json!({
        "url": "http://127.0.0.1:1/nonexistent-webdav/",
        "depth": 1
    });
    let resp = crate::test_support::post_json(&app, "/api/v1/index/bind-remote", body).await;
    assert!(resp.status >= 400, "不可达 WebDAV 应返回错误状态");
    assert!(resp.json.get("error").is_some(), "错误响应必须含 error 字段");
}
```

> 若 `server_test.rs` 无 `unlocked_app`/`post_json` 脚手架（见 Task 3 Step 1 备注），跳过本测试，仅靠 `cargo build` 把关，并在 commit message 注明「bind_remote 仅编译验证」。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo build -p attune-server`
Expected: 编译失败 —— `scanner_webdav::scan_remote` 已不存在。

- [ ] **Step 3: 写实现**

`rust/crates/attune-server/src/routes/remote.rs` 整体替换为：

```rust
use std::collections::HashSet;

use crate::state::SharedState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use attune_core::ingest::{
    ingest_document, ingest_document_replacing, IngestOutcome, SourceConnector,
};
use attune_core::scanner_webdav::{WebDavConfig, WebDavConnector};

#[derive(Deserialize)]
pub struct BindRemoteRequest {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: u32,
}
fn default_depth() -> u32 {
    1
}

/// POST /api/v1/index/bind-remote — 绑定远程 WebDAV 目录并扫描入库。
pub async fn bind_remote(
    State(state): State<SharedState>,
    Json(body): Json<BindRemoteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.depth > 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "depth must be <= 2 to prevent runaway directory traversal"})),
        ));
    }
    let config = WebDavConfig {
        url: body.url.clone(),
        username: body.username.clone(),
        password: body.password.clone(),
        depth: body.depth,
    };

    // 创建/复用 bound_dirs 记录（webdav: 前缀标记远程）。
    let dir_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        vault
            .store()
            .bind_directory(&format!("webdav:{}", body.url), false, &["md", "txt"])
            .map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
            })?
    };

    // WebDAV I/O 是阻塞的 —— 整段在 spawn_blocking 里跑，不阻塞 axum worker。
    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let scan = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let connector = WebDavConnector::new(config);

        // 先把所有 RawDocument 收集出来（WebDAV 文档量有限，spawn_blocking 内可物化）。
        // 大邮箱场景才需严格流式；WebDAV 目录走 collect 简化增量判断与持锁窗口。
        let mut docs = Vec::new();
        {
            let mut sink: attune_core::ingest::DocumentSink<'_> =
                Box::new(|doc| docs.push(doc));
            connector
                .fetch_documents(&mut sink)
                .map_err(|e| e.to_string())?;
        }

        let mut total = 0usize;
        let mut new_files = 0usize;
        let mut updated_files = 0usize;
        let mut skipped_files = 0usize;
        let mut errors: Vec<String> = Vec::new();
        let mut seen_refs: HashSet<String> = HashSet::new();

        for doc in docs {
            total += 1;
            seen_refs.insert(doc.source_ref.clone());

            let vault = state_clone.vault.lock().unwrap_or_else(|e| e.into_inner());
            let dek = match vault.dek_db() {
                Ok(k) => k,
                Err(e) => return Err(format!("vault locked: {e}")),
            };
            let store = vault.store();

            // ETag 增量判断：indexed_files 里存的 file_hash 即上次的 ETag。
            let marker = doc.modified_marker.clone().unwrap_or_default();
            let prior = store.get_indexed_file(&doc.source_ref).ok().flatten();
            let old_item_id: Option<String> = match &prior {
                Some(row) if row.file_hash == marker && !marker.is_empty() => {
                    // ETag 未变 → 跳过下载结果的入库。
                    skipped_files += 1;
                    continue;
                }
                Some(row) => {
                    // ETag 变了 → 旧 item 软删 + enqueue purge + doc_update 信号。
                    if let Some(old) = &row.item_id {
                        let _ = store.delete_item(old);
                        if let Err(e) = store.enqueue_reindex(old, "purge") {
                            log::warn!("webdav: enqueue_reindex(purge) failed for {old}: {e}");
                        }
                        if let Err(e) = store.record_signal_event("doc_update", old, None) {
                            log::debug!("webdav: record_signal_event failed for {old}: {e}");
                        }
                    }
                    row.item_id.clone()
                }
                None => None,
            };

            let result = match &old_item_id {
                Some(old) => ingest_document_replacing(store, &dek, &doc, old),
                None => ingest_document(store, &dek, &doc),
            };
            match result {
                Ok(IngestOutcome::Inserted { item_id, .. }) => {
                    let _ = store.upsert_indexed_file(&dir_id_clone, &doc.source_ref, &marker, &item_id);
                    new_files += 1;
                }
                Ok(IngestOutcome::Updated { item_id, .. }) => {
                    let _ = store.upsert_indexed_file(&dir_id_clone, &doc.source_ref, &marker, &item_id);
                    updated_files += 1;
                }
                Ok(IngestOutcome::Duplicate { item_id }) => {
                    // 内容与已有 item 完全相同 —— 仍登记 indexed_files 让下次 ETag 短路。
                    let _ = store.upsert_indexed_file(&dir_id_clone, &doc.source_ref, &marker, &item_id);
                    skipped_files += 1;
                }
                Ok(IngestOutcome::Skipped { reason }) => {
                    skipped_files += 1;
                    errors.push(format!("{}: {reason}", doc.source_ref));
                }
                Err(e) => errors.push(format!("{}: {e}", doc.source_ref)),
            }
        }

        Ok(serde_json::json!({
            "total_files": total,
            "new_files": new_files,
            "updated_files": updated_files,
            "skipped_files": skipped_files,
            "errors": errors,
        }))
    })
    .await
    .map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?
    .map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e})))
    })?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "dir_id": dir_id,
        "scan": scan,
    })))
}
```

> 响应 `scan` 对象的字段名（`total_files` / `new_files` / `updated_files` / `skipped_files` / `errors`）与旧 `RemoteScanResult` 序列化形态一致 —— 对外不变。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo build -p attune-server` 然后 `cargo test -p attune --test server_test bind_remote`（若 Step 1 加了测试）
Expected: 编译通过；测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/remote.rs rust/tests/server_test.rs
git commit -m "$(cat <<'EOF'
refactor(ingest): bind-remote 走统一 ingest_document + ETag 增量

routes/remote.rs 改用 WebDavConnector，逐文档 ETag 增量判断，
入库走 ingest_document/ingest_document_replacing。WebDAV 来的文档
现在也有 Level-2 embedding 与自动分类。响应 scan 字段形态不变。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8：`LocalFolderConnector` —— 本地文件夹源

把本地文件夹遍历包成 `SourceConnector`。`scanner.rs::scan_directory` 的目录遍历 + 扩展名过滤逻辑搬进 `LocalFolderConnector::fetch_documents`，产 `RawDocument`。

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/local.rs`（替换 Task 1 的占位）

- [ ] **Step 1: 写失败测试**

`rust/crates/attune-core/src/ingest/local.rs` 末尾测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn local_connector_enumerates_supported_files() {
        let tmp = TempDir::new().unwrap();
        let mut f1 = std::fs::File::create(tmp.path().join("a.md")).unwrap();
        f1.write_all(b"# A\n\nbody").unwrap();
        let mut f2 = std::fs::File::create(tmp.path().join("b.txt")).unwrap();
        f2.write_all(b"plain text").unwrap();
        std::fs::File::create(tmp.path().join("c.png")).unwrap(); // 不在 file_types 内

        let connector = LocalFolderConnector::new(
            tmp.path().to_path_buf(),
            true,
            vec!["md".into(), "txt".into()],
            Some("legal".into()),
        );
        let mut collected = Vec::new();
        let mut sink: crate::ingest::DocumentSink<'_> =
            Box::new(|doc| collected.push(doc));
        connector.fetch_documents(&mut sink).unwrap();

        assert_eq!(collected.len(), 2, "只应枚举 md + txt，跳过 png");
        for doc in &collected {
            assert_eq!(doc.source_kind, crate::ingest::SourceKind::LocalFolder);
            assert!(doc.modified_marker.is_some(), "本地文件应带 SHA-256 marker");
            assert!(!doc.content.is_empty());
            assert_eq!(doc.corpus_domain.as_deref(), Some("legal"), "corpus_domain 应透传");
        }
    }

    #[test]
    fn local_connector_non_recursive_skips_subdirs() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("top.md"), b"# top").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("nested.md"), b"# nested").unwrap();

        let connector =
            LocalFolderConnector::new(tmp.path().to_path_buf(), false, vec!["md".into()], None);
        let mut count = 0usize;
        let mut sink: crate::ingest::DocumentSink<'_> = Box::new(|_| count += 1);
        connector.fetch_documents(&mut sink).unwrap();
        assert_eq!(count, 1, "non-recursive 只枚举顶层");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib ingest::local`
Expected: 编译失败 —— `LocalFolderConnector` 未定义。

- [ ] **Step 3: 写实现**

`rust/crates/attune-core/src/ingest/local.rs` 完整内容：

```rust
//! 本地文件夹采集源。遍历目录、按扩展名过滤、把每个文件读成 RawDocument。

use std::path::PathBuf;

use walkdir::WalkDir;

use crate::error::Result;
use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};
use crate::parser;

/// 本地文件夹采集源。
pub struct LocalFolderConnector {
    root: PathBuf,
    recursive: bool,
    /// 接受的扩展名（不带点，小写）。空 = 接受全部受支持类型。
    file_types: Vec<String>,
    /// 语料领域（来自 `bound_dirs.corpus_domain`）。`Some(d)` 时回填进每份
    /// `RawDocument.corpus_domain`，驱动 `ingest_document` 的 F-Pro 前缀注入。
    corpus_domain: Option<String>,
}

impl LocalFolderConnector {
    pub fn new(
        root: PathBuf,
        recursive: bool,
        file_types: Vec<String>,
        corpus_domain: Option<String>,
    ) -> Self {
        Self { root, recursive, file_types, corpus_domain }
    }

    /// 扩展名是否被接受。
    fn ext_accepted(&self, path: &std::path::Path) -> bool {
        if self.file_types.is_empty() {
            return parser::is_supported(path);
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        self.file_types
            .iter()
            .any(|t| t.trim_start_matches('.').eq_ignore_ascii_case(&ext))
    }
}

impl SourceConnector for LocalFolderConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::LocalFolder
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let walker = if self.recursive {
            WalkDir::new(&self.root)
        } else {
            WalkDir::new(&self.root).max_depth(1)
        };
        for entry in walker.into_iter().filter_map(|e| {
            e.map_err(|err| log::warn!("LocalFolderConnector walk error: {err}")).ok()
        }) {
            let path = entry.path();
            if !path.is_file() || !self.ext_accepted(path) {
                continue;
            }
            // 读字节 + 算 SHA-256 作为增量 marker。单文件读失败不致命。
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("LocalFolderConnector: read {} failed: {e}", path.display());
                    continue;
                }
            };
            let marker = {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(&bytes))
            };
            let path_str = path.to_string_lossy().to_string();
            sink(RawDocument {
                uri: format!("file://{path_str}"),
                title: String::new(),
                content: bytes,
                mime_hint: None,
                source_kind: SourceKind::LocalFolder,
                source_ref: path_str,
                modified_marker: Some(marker),
                // 本地文件夹无来源域 / 用户标签；corpus_domain 从 bound_dir 透传。
                domain: None,
                tags: None,
                corpus_domain: self.corpus_domain.clone(),
                metadata: std::collections::HashMap::new(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // ... Step 1 的测试模块内容
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib ingest::local`
Expected: 2 个测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/local.rs
git commit -m "$(cat <<'EOF'
feat(ingest): LocalFolderConnector 本地文件夹采集源

本地文件夹遍历包成 SourceConnector，产 RawDocument（SHA-256
作增量 marker）。scanner.rs 在 Task 9 接入它。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9：迁移 `scanner.rs` 走 `LocalFolderConnector` + `ingest_document`

`scanner.rs::scan_directory` 改为：用 `LocalFolderConnector` 枚举文档，对每个 `RawDocument` 做 SHA-256 增量判断（与旧 `process_single_file` 一致），调 `ingest_document` / `ingest_document_replacing`。`create_watcher` / `watch_directory` 保持不变（它们是 notify 监听 API，不属于 pipeline）。

**corpus_domain（决策 2 已落实）：** 旧 `scanner.rs` 从 `bind_dir` 读 `corpus_domain` 并给 item 设 domain + 注入 chunk 前缀。新版 `scan_directory` 先 `store.get_dir_corpus_domain(dir_id)` 取领域，构造 `LocalFolderConnector` 时把它传进去（Task 8 已给 `LocalFolderConnector::new` 加 `corpus_domain` 参数），`LocalFolderConnector` 回填进每份 `RawDocument.corpus_domain`，`ingest_document` 内部完成 item 级标签 + chunk 前缀注入 —— **F-Pro 跨域防污染完整保留，无收窄**。

**Files:**
- Modify: `rust/crates/attune-core/src/scanner.rs`

- [ ] **Step 1: 写失败测试**

`scanner.rs` 已有 5 个测试（`scan_empty_directory` / `scan_with_files` / `scan_skips_unchanged_files` / `scan_detects_modified_files` / `create_watcher_works`）。它们就是回归测试 —— 重构后必须全部仍 PASS，**不改这些测试**。新增一个验证「L2 + classify 不再漏」的测试，加在 `scanner.rs` 的 `#[cfg(test)] mod tests` 内：

```rust
    #[test]
    fn scan_enqueues_level2_and_classify() {
        // 回归保护：本地扫描入库必须同时有 L1 + L2 embedding 与 classify 任务
        // （WebDAV 旧实现漏抄的两步，统一 pipeline 后任何源都不应再漏）。
        let (store, dek, tmp) = setup_test();
        std::fs::write(
            tmp.path().join("doc.md"),
            b"# Heading One\n\nFirst body paragraph.\n\n# Heading Two\n\nSecond body.",
        )
        .unwrap();
        let dir_id = store
            .bind_directory(tmp.path().to_str().unwrap(), true, &["md"])
            .unwrap();
        scan_directory(&store, &dek, &dir_id, tmp.path(), true, &["md".into()]).unwrap();

        assert!(store.count_embed_queue_by_level(1).unwrap() >= 1, "L1 必须入队");
        assert!(store.count_embed_queue_by_level(2).unwrap() >= 1, "L2 必须入队");
        assert_eq!(store.pending_count_by_type("classify").unwrap(), 1, "classify 必须入队");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib scanner::tests::scan_enqueues_level2`
Expected: 编译通过但断言失败 / 或编译失败 —— 取决于改没改 `scan_directory`。先看它 FAIL（旧 `scan_directory` 已做 L2 + classify，实际可能 PASS；若 PASS 属正常，本测试主要锁定 Step 3 不破坏行为）。

- [ ] **Step 3: 写实现**

把 `scanner.rs` 的 `scan_directory` + `process_single_file` 两个函数替换。`ScanResult` 结构、`FileAction` enum、`create_watcher`、`watch_directory` 与所有现有测试**保持不动**。新版：

```rust
/// 全量扫描指定目录。
pub fn scan_directory(
    store: &Store,
    dek: &Key32,
    dir_id: &str,
    dir_path: &Path,
    recursive: bool,
    file_types: &[String],
) -> Result<ScanResult> {
    use crate::ingest::local::LocalFolderConnector;
    use crate::ingest::{ingest_document, ingest_document_replacing, IngestOutcome, SourceConnector};

    let mut result = ScanResult {
        total_files: 0,
        new_files: 0,
        updated_files: 0,
        skipped_files: 0,
        errors: 0,
    };

    // F-Pro：从 bound_dir 读 corpus_domain，透传给 connector → RawDocument →
    // ingest_document（item 级标签 + chunk `[领域: X]` 前缀注入）。
    let corpus_domain = store
        .get_dir_corpus_domain(dir_id)
        .ok()
        .filter(|d| !d.is_empty() && d != "general");
    let connector = LocalFolderConnector::new(
        dir_path.to_path_buf(),
        recursive,
        file_types.to_vec(),
        corpus_domain,
    );
    let mut docs = Vec::new();
    {
        let mut sink: crate::ingest::DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector.fetch_documents(&mut sink)?;
    }

    for doc in docs {
        result.total_files += 1;
        let marker = doc.modified_marker.clone().unwrap_or_default();

        // SHA-256 增量判断：indexed_files.file_hash 即上次的内容 hash。
        let prior = store.get_indexed_file(&doc.source_ref).ok().flatten();
        let old_item_id: Option<String> = match &prior {
            Some(row) if row.file_hash == marker && !marker.is_empty() => {
                result.skipped_files += 1;
                continue;
            }
            Some(row) => {
                // 文件已变 → 旧 item 软删 + enqueue purge + doc_update 信号。
                if let Some(old) = &row.item_id {
                    if let Err(e) = store.delete_item(old) {
                        log::warn!("scanner: delete_item({old}) failed: {e}");
                    }
                    if let Err(e) = store.enqueue_reindex(old, "purge") {
                        log::warn!("scanner: enqueue_reindex(purge) failed for {old}: {e}");
                    }
                    if let Err(e) = store.record_signal_event("doc_update", old, None) {
                        log::debug!("scanner: record_signal_event failed for {old}: {e}");
                    }
                }
                row.item_id.clone()
            }
            None => None,
        };

        let outcome = match &old_item_id {
            Some(old) => ingest_document_replacing(store, dek, &doc, old),
            None => ingest_document(store, dek, &doc),
        };
        match outcome {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                result.new_files += 1;
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                result.updated_files += 1;
            }
            Ok(IngestOutcome::Duplicate { item_id }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                result.skipped_files += 1;
            }
            Ok(IngestOutcome::Skipped { .. }) => {
                result.skipped_files += 1;
            }
            Err(e) => {
                log::warn!("scanner: ingest {} failed: {e}", doc.source_ref);
                result.errors += 1;
            }
        }
    }

    store.update_dir_last_scan(dir_id)?;
    Ok(result)
}
```

删除整个 `process_single_file` 函数。`scanner.rs` 顶部的 `use crate::chunker;` 与 `use crate::parser;` 现在 `scan_directory` 不再直接用 —— 但 `parser` 可能被其它残留代码用，编译器会以 unused import 警告提示，按提示删未用的 import。

> **corpus_domain（决策 2 完整保留）：** 旧 `process_single_file` 读 `store.get_dir_corpus_domain(dir_id)` 并 ① 调 `set_item_corpus_domain` ② 给 chunk_text 注入 `[领域: X]` 前缀。新版把这两件事都收进 `ingest_document`：`scan_directory` 取 `corpus_domain` → `LocalFolderConnector` → `RawDocument.corpus_domain` → `ingest_document` 内部对 `!= general` 注入前缀并调 `set_item_corpus_domain`（见 Task 2 pipeline 实现）。**F-Pro 跨域防污染零收窄**，无需在 `scan_directory` 额外补 `set_item_corpus_domain`。`get_dir_corpus_domain` / `set_item_corpus_domain` 签名见「真实代码事实」。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib scanner`
Expected: 原有 5 个测试 + 新增 `scan_enqueues_level2_and_classify` 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/scanner.rs
git commit -m "$(cat <<'EOF'
refactor(ingest): scanner.rs 走 LocalFolderConnector + ingest_document

scan_directory 删本地复制的 parse/breadcrumbs/embed/classify 代码，
改用 LocalFolderConnector 枚举 + 统一 ingest_document 入库。
SHA-256 增量判断与旧 process_single_file 一致。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10：WebDAV remote 配置加密持久化（`webdav_remotes` 表）

**决策 4：** 现状 `bind-remote` 把 `username`/`password` 放进内存 `WebDavConfig`，扫完即丢。Task 11 的周期重扫 worker 因此无法对认证源（Nextcloud / 坚果云等绝大多数真实场景）自动跑。本 task 新增一个 vault 加密的 WebDAV remote 配置表，`bind-remote` 落库该配置，让 Task 11 worker 能读回凭据自动增量重扫。

**加密模式核对：** `attune-core` 字段级 AES-256-GCM 的现有写法（`store/items.rs`）：`crypto::encrypt(dek, plaintext_bytes) -> Vec<u8>` 写入 `BLOB` 列、`crypto::decrypt(dek, blob) -> Vec<u8>` 读回。`dek` 是 vault 数据加密密钥（`Key32`）。`items.content` / `items.tags` 即用此模式。本 task 的 `webdav_remotes.password_enc` 列**完全沿用同一模式** —— `crypto::encrypt(dek, password.as_bytes())`。`url` / `username` / `depth` / `corpus_domain` 不敏感，明文列即可（与 `bound_dirs` 一致）。

**Files:**
- Modify: `rust/crates/attune-core/src/store/mod.rs`（`webdav_remotes` 表 DDL + `pub mod webdav_remotes;`）
- Create: `rust/crates/attune-core/src/store/webdav_remotes.rs`（`WebDavRemoteRow` + CRUD，`password` 字段级加密）
- Create: `rust/crates/attune-core/tests/webdav_remotes_test.rs`（加解密往返集成测试）

- [ ] **Step 1: 写失败测试**

`rust/crates/attune-core/tests/webdav_remotes_test.rs` 完整内容：

```rust
//! webdav_remotes 表加密持久化集成测试。

use attune_core::crypto::Key32;
use attune_core::store::webdav_remotes::WebDavRemoteInput;
use attune_core::store::Store;

#[test]
fn upsert_then_get_round_trips_with_decrypted_password() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();

    let input = WebDavRemoteInput {
        dir_id: "dir-1".into(),
        url: "https://dav.example.com/remote.php/dav/files/u/".into(),
        username: Some("alice".into()),
        password: Some("s3cr3t-app-pw".into()),
        depth: 1,
        corpus_domain: "legal".into(),
    };
    store.upsert_webdav_remote(&dek, &input).unwrap();

    let got = store
        .get_webdav_remote(&dek, "dir-1")
        .unwrap()
        .expect("remote row exists");
    assert_eq!(got.url, input.url);
    assert_eq!(got.username.as_deref(), Some("alice"));
    assert_eq!(got.password.as_deref(), Some("s3cr3t-app-pw"), "password 必须能解密回明文");
    assert_eq!(got.depth, 1);
    assert_eq!(got.corpus_domain, "legal");
}

#[test]
fn password_is_not_stored_in_plaintext() {
    // 安全回归：password_enc 列不得出现明文密码字节。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let input = WebDavRemoteInput {
        dir_id: "dir-2".into(),
        url: "https://dav.example.com/u/".into(),
        username: Some("bob".into()),
        password: Some("PLAINTEXT_MARKER_XYZ".into()),
        depth: 1,
        corpus_domain: "general".into(),
    };
    store.upsert_webdav_remote(&dek, &input).unwrap();
    let raw = store.debug_raw_webdav_password_enc("dir-2").unwrap();
    assert!(!raw.is_empty(), "password_enc 应已写入");
    assert!(
        !raw.windows(20).any(|w| w == b"PLAINTEXT_MARKER_XYZ"),
        "password_enc 列绝不能含明文密码"
    );
}

#[test]
fn list_webdav_remotes_returns_all_configured() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    for i in 0..3 {
        store
            .upsert_webdav_remote(
                &dek,
                &WebDavRemoteInput {
                    dir_id: format!("dir-{i}"),
                    url: format!("https://dav.example.com/u{i}/"),
                    username: None,
                    password: None,
                    depth: 1,
                    corpus_domain: "general".into(),
                },
            )
            .unwrap();
    }
    let all = store.list_webdav_remotes(&dek).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn upsert_is_idempotent_on_dir_id() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = WebDavRemoteInput {
        dir_id: "dir-x".into(),
        url: "https://dav.example.com/old/".into(),
        username: Some("u".into()),
        password: Some("old-pw".into()),
        depth: 1,
        corpus_domain: "general".into(),
    };
    store.upsert_webdav_remote(&dek, &input).unwrap();
    input.url = "https://dav.example.com/new/".into();
    input.password = Some("new-pw".into());
    store.upsert_webdav_remote(&dek, &input).unwrap();

    let all = store.list_webdav_remotes(&dek).unwrap();
    assert_eq!(all.len(), 1, "同 dir_id 二次 upsert 不新增行");
    let got = store.get_webdav_remote(&dek, "dir-x").unwrap().unwrap();
    assert_eq!(got.url, "https://dav.example.com/new/");
    assert_eq!(got.password.as_deref(), Some("new-pw"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --test webdav_remotes_test`
Expected: 编译失败 —— `store::webdav_remotes` 模块未定义。

- [ ] **Step 3: 写实现**

先在 `rust/crates/attune-core/src/store/mod.rs` 的 schema DDL 字符串里、`indexed_files` 表 DDL 之后插入新表（`CREATE TABLE IF NOT EXISTS` —— 老 vault 下次 open 自动获得空表，无需独立 migration，与 `item_blobs` 同模式）：

```sql
-- 决策 4：WebDAV remote 配置持久化。bound_dirs(webdav:* path) 只记 URL；
-- 认证凭据要让周期同步 worker 自动复用 → 此表存完整配置。
-- password_enc 是 AES-256-GCM 密文 BLOB（dek 加密，与 items.content 同模式）。
CREATE TABLE IF NOT EXISTS webdav_remotes (
    dir_id        TEXT PRIMARY KEY REFERENCES bound_dirs(id) ON DELETE CASCADE,
    url           TEXT NOT NULL,
    username      TEXT,
    password_enc  BLOB,
    depth         INTEGER NOT NULL DEFAULT 1,
    corpus_domain TEXT NOT NULL DEFAULT 'general',
    updated_at    TEXT NOT NULL,
    last_etag_sync TEXT
);
```

在 `store/mod.rs` 顶部模块声明区（`pub mod item_blobs;` 附近）加：

```rust
pub mod webdav_remotes;
```

`rust/crates/attune-core/src/store/webdav_remotes.rs` 完整内容：

```rust
//! WebDAV remote 配置持久化。
//!
//! 决策 4：周期同步 worker 要对认证源（Nextcloud / 坚果云等）自动增量重扫，
//! 必须能读回凭据。此表存每个 webdav: bound_dir 的完整连接配置，`password`
//! 经字段级 AES-256-GCM 加密（dek，与 `items.content` 同模式）落 `password_enc`
//! BLOB 列；明文密码绝不落盘。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;

/// 写入用的 WebDAV remote 配置（明文，调用方持有）。
#[derive(Debug, Clone)]
pub struct WebDavRemoteInput {
    /// 关联的 bound_dirs.id。
    pub dir_id: String,
    pub url: String,
    pub username: Option<String>,
    /// 明文密码；落库前由 `upsert_webdav_remote` 用 dek 加密。
    pub password: Option<String>,
    pub depth: u32,
    /// 语料领域（写入 RawDocument.corpus_domain，驱动 F-Pro 跨域防污染）。
    pub corpus_domain: String,
}

/// 从表里读出的 WebDAV remote 配置（password 已解密回明文）。
#[derive(Debug, Clone)]
pub struct WebDavRemoteRow {
    pub dir_id: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub depth: u32,
    pub corpus_domain: String,
    pub last_etag_sync: Option<String>,
}

impl Store {
    /// upsert 一条 WebDAV remote 配置。`password` 用 dek 加密成 BLOB 落盘。
    /// 同 `dir_id` 已存在则整行替换（幂等）。
    pub fn upsert_webdav_remote(&self, dek: &Key32, input: &WebDavRemoteInput) -> Result<()> {
        let password_enc: Option<Vec<u8>> = match &input.password {
            Some(p) => Some(crypto::encrypt(dek, p.as_bytes())?),
            None => None,
        };
        let now = crate::store::now_iso8601();
        self.conn.execute(
            "INSERT INTO webdav_remotes
                (dir_id, url, username, password_enc, depth, corpus_domain, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(dir_id) DO UPDATE SET
                url=excluded.url,
                username=excluded.username,
                password_enc=excluded.password_enc,
                depth=excluded.depth,
                corpus_domain=excluded.corpus_domain,
                updated_at=excluded.updated_at",
            params![
                input.dir_id,
                input.url,
                input.username,
                password_enc,
                input.depth as i64,
                input.corpus_domain,
                now,
            ],
        )?;
        Ok(())
    }

    /// 读单条 WebDAV remote 配置（password 解密回明文）。
    pub fn get_webdav_remote(&self, dek: &Key32, dir_id: &str) -> Result<Option<WebDavRemoteRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync
             FROM webdav_remotes WHERE dir_id = ?1",
        )?;
        let row = stmt
            .query_row(params![dir_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            })
            .ok();
        match row {
            None => Ok(None),
            Some((dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync)) => {
                let password = match password_enc {
                    Some(blob) => Some(
                        String::from_utf8_lossy(&crypto::decrypt(dek, &blob)?).to_string(),
                    ),
                    None => None,
                };
                Ok(Some(WebDavRemoteRow {
                    dir_id,
                    url,
                    username,
                    password,
                    depth: depth as u32,
                    corpus_domain,
                    last_etag_sync,
                }))
            }
        }
    }

    /// 列出全部 WebDAV remote 配置（周期 worker 用，password 已解密）。
    pub fn list_webdav_remotes(&self, dek: &Key32) -> Result<Vec<WebDavRemoteRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync
             FROM webdav_remotes ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<Vec<u8>>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (dir_id, url, username, password_enc, depth, corpus_domain, last_etag_sync) = row?;
            let password = match password_enc {
                Some(blob) => {
                    Some(String::from_utf8_lossy(&crypto::decrypt(dek, &blob)?).to_string())
                }
                None => None,
            };
            out.push(WebDavRemoteRow {
                dir_id,
                url,
                username,
                password,
                depth: depth as u32,
                corpus_domain,
                last_etag_sync,
            });
        }
        Ok(out)
    }

    /// 记录某 remote 最近一次 ETag 增量同步时间。
    pub fn touch_webdav_remote_sync(&self, dir_id: &str) -> Result<()> {
        let now = crate::store::now_iso8601();
        self.conn.execute(
            "UPDATE webdav_remotes SET last_etag_sync = ?1 WHERE dir_id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    /// 测试辅助：取 password_enc 原始密文字节（验证不含明文）。
    pub fn debug_raw_webdav_password_enc(&self, dir_id: &str) -> Result<Vec<u8>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT password_enc FROM webdav_remotes WHERE dir_id = ?1",
                params![dir_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(blob.unwrap_or_default())
    }
}
```

> `crate::store::now_iso8601()` 是仓内已有的时间戳工具 —— 先 `grep -rn "fn now_iso8601\|now_iso\|Utc::now" rust/crates/attune-core/src/store/` 确认实际函数名/路径；若名字不同（如 `now_rfc3339` / inline `chrono::Utc::now().to_rfc3339()`），按实际改，**不新造**。`prepare_cached` 用于静态 SQL（per CLAUDE.md Rust 约定）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --test webdav_remotes_test`
Expected: 4 个测试 PASS。

- [ ] **Step 5: `bind-remote` 落库配置**

`routes/remote.rs::bind_remote`（Task 7 已重写）在 `bind_directory` 拿到 `dir_id` 之后、`spawn_blocking` 扫描之前，补一段落库：把 `BindRemoteRequest` 的 `url` / `username` / `password` / `depth` 连同 `corpus_domain`（`BindRemoteRequest` 增加可选字段 `corpus_domain: Option<String>`，`#[serde(default)]`，缺省 `"general"`）写进 `webdav_remotes` 表。在 `dir_id` 那个 `{ ... }` 块末尾、`vault` guard 仍持有时加：

```rust
    // 决策 4：落库加密 remote 配置，让周期 worker 能读回凭据自动重扫。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        let input = attune_core::store::webdav_remotes::WebDavRemoteInput {
            dir_id: dir_id.clone(),
            url: body.url.clone(),
            username: body.username.clone(),
            password: body.password.clone(),
            depth: body.depth,
            corpus_domain: body
                .corpus_domain
                .clone()
                .unwrap_or_else(|| "general".into()),
        };
        if let Err(e) = vault.store().upsert_webdav_remote(&dek, &input) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("persist webdav remote: {e}")})),
            ));
        }
    }
```

并在 `BindRemoteRequest` 加字段：

```rust
#[derive(Deserialize)]
pub struct BindRemoteRequest {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: u32,
    /// 语料领域（legal / tech / medical / patent / general），驱动 F-Pro
    /// 跨域防污染。缺省 general。
    pub corpus_domain: Option<String>,
}
```

> `bind-remote` 扫描时 `WebDavConnector` 产出的 `RawDocument.corpus_domain` 当前是 `None`（Task 6）。本 task 让 `bind_remote` 的 `spawn_blocking` 扫描循环在每份 `doc` 入库前回填 `corpus_domain`：`doc.corpus_domain = Some(corpus_domain_for_dir.clone());`（`corpus_domain_for_dir` 从 `body.corpus_domain` 取，move 进闭包）—— 这样 WebDAV 文档也享受 F-Pro 前缀注入。`doc` 在闭包内是 owned 的（`docs` Vec 里逐个取出），可直接 mutate。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-core/src/store/mod.rs rust/crates/attune-core/src/store/webdav_remotes.rs rust/crates/attune-core/tests/webdav_remotes_test.rs rust/crates/attune-server/src/routes/remote.rs
git commit -m "$(cat <<'EOF'
feat(ingest): WebDAV remote 配置加密持久化

新增 webdav_remotes 表存 URL/username/depth/corpus_domain，
password 走字段级 AES-256-GCM（dek，与 items.content 同模式）。
bind-remote 落库该配置 —— 周期同步 worker 由此可读回凭据
对认证源（Nextcloud/坚果云）自动增量重扫。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11：WebDAV 增量同步后台调度（从 `webdav_remotes` 读凭据）

让已配置的 WebDAV remote 被周期性重扫（拉新文件 / ETag 变化的更新）。worker 从 Task 10 的 `webdav_remotes` 表读全部 remote + 解密凭据，对**认证源也能自动跑**。模式参照 `state.rs::start_reindex_worker`（`std::thread::spawn` + 原子 flag 防重入 + RAII guard 复位 flag）。

**Files:**
- Create: `rust/crates/attune-server/src/ingest_webdav.rs`（`sync_webdav_dir` 公共函数）
- Modify: `rust/crates/attune-server/src/lib.rs`（`mod ingest_webdav;`）
- Modify: `rust/crates/attune-server/src/routes/remote.rs`（`bind_remote` 改调 `sync_webdav_dir`）
- Modify: `rust/crates/attune-server/src/state.rs`（加 `start_webdav_sync_worker`）
- Modify: `rust/crates/attune-server/src/routes/vault.rs`（unlock 后启动，与 `start_reindex_worker` 并列）

- [ ] **Step 1: 写失败测试**

WebDAV 周期 worker 依赖真实 server，难做单元测试。本 task 靠编译 + 一个「flag 防重入」单元测试。在 `state.rs` 的 `#[cfg(test)]` 模块加（若无测试模块则建一个）：

```rust
    #[test]
    fn webdav_sync_worker_flag_prevents_double_start() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = AtomicBool::new(false);
        // 首次 compare_exchange 成功。
        assert!(flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok());
        // 二次失败 —— worker 不会重复起。
        assert!(flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err());
    }
```

- [ ] **Step 2: 跑测试确认失败/通过**

Run: `cargo test -p attune-server --lib webdav_sync_worker_flag`
Expected: 编译通过、测试 PASS（这是纯原子语义测试 —— Step 3 加的 worker 用同款 flag 模式）。

- [ ] **Step 3: 抽 `sync_webdav_dir` 公共函数**

新建 `rust/crates/attune-server/src/ingest_webdav.rs`。把 Task 7 `bind_remote` 里 `spawn_blocking` 闭包的「收集 docs → 逐文档 ETag 增量 → ingest」逻辑抽成 `sync_webdav_dir` 自由函数 —— `bind_remote` 与周期 worker 共用，不重复代码。`config` 含完整凭据（来自 `webdav_remotes` 表或 `bind-remote` 请求体），`corpus_domain` 回填到每份 `RawDocument`：

```rust
//! WebDAV 增量同步 —— bind-remote route 与周期 worker 共用的入库逻辑。

use std::collections::HashSet;
use std::sync::Arc;

use attune_core::ingest::{ingest_document, ingest_document_replacing, IngestOutcome, SourceConnector};
use attune_core::scanner_webdav::{WebDavConfig, WebDavConnector};

use crate::state::AppState;

/// 对一个 WebDAV remote 做一次全量 ETag 增量同步。
///
/// `corpus_domain` 回填进每份 `RawDocument`，驱动 F-Pro 跨域防污染前缀注入。
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
pub fn sync_webdav_dir(
    state: &Arc<AppState>,
    dir_id: &str,
    config: WebDavConfig,
    corpus_domain: &str,
) -> Result<serde_json::Value, String> {
    let connector = WebDavConnector::new(config);

    // WebDAV 文档量有限，spawn_blocking 内可物化 —— 简化增量判断与持锁窗口。
    let mut docs = Vec::new();
    {
        let mut sink: attune_core::ingest::DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector.fetch_documents(&mut sink).map_err(|e| e.to_string())?;
    }

    let mut total = 0usize;
    let mut new_files = 0usize;
    let mut updated_files = 0usize;
    let mut skipped_files = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut seen_refs: HashSet<String> = HashSet::new();

    for mut doc in docs {
        total += 1;
        seen_refs.insert(doc.source_ref.clone());
        // F-Pro：WebDAV 文档也享受领域前缀注入。
        if corpus_domain != "general" {
            doc.corpus_domain = Some(corpus_domain.to_string());
        }

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(e) => return Err(format!("vault locked: {e}")),
        };
        let store = vault.store();

        let marker = doc.modified_marker.clone().unwrap_or_default();
        let prior = store.get_indexed_file(&doc.source_ref).ok().flatten();
        let old_item_id: Option<String> = match &prior {
            Some(row) if row.file_hash == marker && !marker.is_empty() => {
                skipped_files += 1;
                continue;
            }
            Some(row) => {
                if let Some(old) = &row.item_id {
                    let _ = store.delete_item(old);
                    if let Err(e) = store.enqueue_reindex(old, "purge") {
                        log::warn!("webdav: enqueue_reindex(purge) failed for {old}: {e}");
                    }
                    if let Err(e) = store.record_signal_event("doc_update", old, None) {
                        log::debug!("webdav: record_signal_event failed for {old}: {e}");
                    }
                }
                row.item_id.clone()
            }
            None => None,
        };

        let result = match &old_item_id {
            Some(old) => ingest_document_replacing(store, &dek, &doc, old),
            None => ingest_document(store, &dek, &doc),
        };
        match result {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                new_files += 1;
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                updated_files += 1;
            }
            Ok(IngestOutcome::Duplicate { item_id }) => {
                let _ = store.upsert_indexed_file(dir_id, &doc.source_ref, &marker, &item_id);
                skipped_files += 1;
            }
            Ok(IngestOutcome::Skipped { reason }) => {
                skipped_files += 1;
                errors.push(format!("{}: {reason}", doc.source_ref));
            }
            Err(e) => errors.push(format!("{}: {e}", doc.source_ref)),
        }
        // 记录本次同步时间（best-effort）。
        let _ = store.touch_webdav_remote_sync(dir_id);
    }

    Ok(serde_json::json!({
        "total_files": total,
        "new_files": new_files,
        "updated_files": updated_files,
        "skipped_files": skipped_files,
        "errors": errors,
    }))
}
```

`rust/crates/attune-server/src/lib.rs` 加：

```rust
mod ingest_webdav;
```

`routes/remote.rs::bind_remote` 的 `spawn_blocking` 闭包体替换为调 `sync_webdav_dir`（闭包内逐文档逻辑全部删除，因为已搬进 `sync_webdav_dir`）：

```rust
    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let corpus_domain = body.corpus_domain.clone().unwrap_or_else(|| "general".into());
    let scan = tokio::task::spawn_blocking(move || {
        crate::ingest_webdav::sync_webdav_dir(&state_clone, &dir_id_clone, config, &corpus_domain)
    })
    .await
    .map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?
    .map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e})))
    })?;
```

> Task 7 重写后的 `bind_remote` 里那个手写逐文档循环此时被 `sync_webdav_dir` 取代 —— `routes/remote.rs` 顶部不再需要 `use std::collections::HashSet;` 和 `use attune_core::ingest::{...}` / `use attune_core::scanner_webdav::WebDavConnector`（仅保留 `WebDavConfig`）。按编译器 unused import 提示清理。

- [ ] **Step 4: 写 `start_webdav_sync_worker`**

`AppState` 加原子字段（参照 `reindex_worker_running` 的声明位置）：

```rust
    /// WebDAV 周期同步 worker 是否在运行（防重复启动）。
    pub webdav_sync_worker_running: std::sync::atomic::AtomicBool,
```

`AppState` 构造处给该字段初值 `std::sync::atomic::AtomicBool::new(false)`。

在 `state.rs` 加方法（紧跟 `start_reindex_worker` 之后）：

```rust
    /// 启动 WebDAV 周期同步 worker：每 15 分钟从 webdav_remotes 表读全部
    /// remote + 解密凭据，逐个增量重扫。原子 flag 防重入 + RAII guard 复位。
    pub fn start_webdav_sync_worker(state: std::sync::Arc<AppState>) {
        use std::sync::atomic::Ordering;
        if state
            .webdav_sync_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("WebDAV sync worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.webdav_sync_worker_running);

            tracing::info!("WebDAV sync worker started");
            loop {
                // vault 锁定则退出 —— 下次 unlock 会重新 start。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(15 * 60));

                // 从 webdav_remotes 表读全部已配置 remote + 解密凭据（snapshot 后释放锁）。
                let remotes: Vec<attune_core::store::webdav_remotes::WebDavRemoteRow> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let dek = match vault.dek_db() {
                        Ok(k) => k,
                        Err(_) => break, // vault 锁定 → 退出，下次 unlock 重启
                    };
                    vault.store().list_webdav_remotes(&dek).unwrap_or_default()
                };

                for remote in remotes {
                    let config = attune_core::scanner_webdav::WebDavConfig {
                        url: remote.url.clone(),
                        username: remote.username.clone(),
                        password: remote.password.clone(),
                        depth: remote.depth,
                    };
                    if let Err(e) = crate::ingest_webdav::sync_webdav_dir(
                        &state,
                        &remote.dir_id,
                        config,
                        &remote.corpus_domain,
                    ) {
                        tracing::warn!("WebDAV sync for dir {} failed: {e}", remote.dir_id);
                    }
                }
            }
            tracing::info!("WebDAV sync worker stopped (vault locked)");
        });
    }
```

> 凭据来源：`list_webdav_remotes` 已解密 `password` 回明文（Task 10），周期 worker 因此**对认证源也能自动同步** —— 决策 4 的核心收益。`WebDavRemoteRow` 不含 `Debug` 打印密码的风险点：实现者勿 `tracing::debug!("{:?}", remote)`，只打印 `dir_id` / `url`。

`routes/vault.rs` 中 3 处 `AppState::start_reindex_worker(state.clone());` 调用点旁边各加一行：

```rust
    crate::state::AppState::start_webdav_sync_worker(state.clone());
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo build -p attune-server && cargo test -p attune-server --lib webdav_sync_worker_flag`
Expected: 编译通过，测试 PASS。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-server/src/state.rs rust/crates/attune-server/src/routes/vault.rs rust/crates/attune-server/src/routes/remote.rs rust/crates/attune-server/src/ingest_webdav.rs rust/crates/attune-server/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(ingest): WebDAV 周期增量同步 worker

每 15 分钟从 webdav_remotes 表读全部 remote + 解密凭据，逐个
ETag 增量重扫。bind_remote 与周期 worker 共用 sync_webdav_dir。
认证源（Nextcloud/坚果云等）凭已持久化的加密凭据自动同步。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 12：全量回归 + 文档更新

**Files:**
- Modify: `rust/DEVELOP.md`（采集架构小节）
- Modify: `docs/TESTING.md`（如有采集相关测试矩阵条目）

- [ ] **Step 1: 全量测试**

Run: `cargo test -p attune-core && cargo test -p attune-server && cargo test -p attune`
Expected: 全绿。重点确认 `ingest_pipeline_test`、`webdav_remotes_test`、`scanner` 测试、`server_test` 中 upload/ingest/bind_remote 相关测试通过。

- [ ] **Step 2: 全量构建 + clippy**

Run: `cargo build --release -p attune-server && cargo clippy -p attune-core -p attune-server`
Expected: release 编译通过；clippy 不新增 warning（workspace baseline 是 24 个，不增即可）。

- [ ] **Step 3: 更新 DEVELOP.md**

在 `rust/DEVELOP.md` 找到描述采集 / 索引的小节（`grep -n "scanner\|采集\|ingest\|WebDAV" rust/DEVELOP.md`），加一段说明统一抽象：

```markdown
### 采集体系（Ingest）

所有入库路径统一走 `attune-core::ingest`：

- `SourceConnector` trait —— 一个采集源（本地文件夹 / WebDAV / 邮箱 / RSS / 云盘），
  通过回调 sink 逐个交出 `RawDocument`（含 domain / tags / corpus_domain 字段）。
- `ingest_document()` —— 唯一入库函数，走 parse → content_hash 判重 → insert
  （透传 domain/tags）→ breadcrumbs → embed(L1+L2，corpus_domain 注入 `[领域: X]`
  前缀)→ classify 五步，返回 `IngestOutcome`（Inserted / Duplicate / Updated / Skipped）。

新增采集源只需实现 `SourceConnector`，不碰 pipeline 内部。HTTP API
（`/api/v1/upload`、`/api/v1/ingest`、`/api/v1/index/*`）形态对外不变。

WebDAV remote 配置（含加密凭据）持久化在 `webdav_remotes` 表，`password`
走字段级 AES-256-GCM；周期同步 worker 每 15 分钟读回凭据对认证源自动增量重扫。
```

- [ ] **Step 4: Commit**

```bash
git add rust/DEVELOP.md docs/TESTING.md
git commit -m "$(cat <<'EOF'
docs(ingest): DEVELOP.md 记录统一采集抽象

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2：Email IMAP 采集源（task 蓝图级）

**目标：** 增加 `EmailConnector` —— 从 IMAP 邮箱拉邮件入库。MVP 只覆盖最常见场景，不做 OAuth、不做 push。

### 新增文件

| 文件 | 职责 |
|------|------|
| `rust/crates/attune-core/src/ingest/email.rs` | `EmailConnector` + `EmailConfig` + `ImapCredentials` |
| `rust/crates/attune-core/tests/ingest_email_test.rs` | mail-parser 解析单元测试（不连真 IMAP）|
| `rust/crates/attune-server/src/routes/email.rs` | `POST /api/v1/index/bind-email` route |

### crate 选型

```toml
async-imap = { version = "0.10", default-features = false, features = ["runtime-tokio"] }
mail-parser = "0.9"
```

`async-imap` 用 tokio runtime + rustls（确认无 `native-tls` 传递依赖，与 CLAUDE.md 纯 Rust TLS 约束一致）。`mail-parser` 纯 Rust，解析 MIME 邮件体 + 附件。

### `EmailConnector` 如何 impl `SourceConnector`

```rust
pub struct EmailConfig {
    pub host: String,           // imap.gmail.com
    pub port: u16,              // 993
    pub username: String,
    pub password: String,       // 应用专用密码 / IMAP 密码
    pub folders: Vec<String>,   // 默认 ["INBOX", "Sent"]
    pub since_uid: Option<u32>, // 增量：只拉 UID > since_uid
}

impl SourceConnector for EmailConnector {
    fn source_kind(&self) -> SourceKind { SourceKind::Email }
    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        // 单线程 tokio runtime 桥接（与 WebDavConnector::drive_blocking 同模式）：
        // 1. async-imap 连接 + login + select(folder)
        // 2. UID SEARCH "UID since_uid:*" 取增量 UID 列表
        // 3. 逐 UID FETCH (RFC822) 取原始邮件字节
        // 4. mail-parser 解析：
        //    - title  = Subject
        //    - content = text/plain body（无则 html_to_text(text/html)）
        //    - source_ref = Message-ID（稳定唯一，跨 folder 去重）
        //    - modified_marker = UID 字符串
        //    - metadata: from / date / folder
        // 5. 附件：每个附件单独产一份 RawDocument，content = 附件字节，
        //    source_ref = "{Message-ID}#att{n}"，parser 按附件文件名扩展名解析
        //    （复用现有 parse_bytes —— PDF/DOCX/图片附件自动 OCR）
    }
}
```

每封邮件 + 每个附件都是独立 `RawDocument`，逐个 `sink()` —— 大邮箱不物化。

### MVP 范围

- **认证**：用户名 + 密码（含 Gmail/Outlook 应用专用密码）。**不做 OAuth2**。
- **文件夹**：默认 `INBOX` + `Sent`，用户可配置。
- **增量**：按 IMAP UID。`indexed_files.file_hash` 存 `"{folder}:{uid}"`，下次 `UID SEARCH` 从 `max(uid)+1` 起。
- **去重**：`Message-ID` 作 `source_ref` → `content_hash` 双层（`ingest_document` 的 hash 短路兜底转发邮件）。
- **附件**：复用 `parser::parse_bytes`，自动走 OCR / DOCX / PDF 解析。附件大小超 20 MB 跳过。

### 配置项（`EmailConfig`，存 vault 加密）

`host` / `port` / `username` / `password` / `folders` / `since_uid`。密码必须经 vault 字段级 AES-GCM 加密存储 —— 不明文落盘。**直接复用 Phase 1 Task 10 `webdav_remotes` 表的加密模式**：新建 `email_accounts` 表，`password_enc` 列 `crypto::encrypt(dek, ...)`，CRUD 写法照搬 `store/webdav_remotes.rs`。Email 周期同步 worker（按 UID 增量）从该表读回解密凭据，与 WebDAV worker 同构。

### 验收标准

1. `EmailConnector` impl `SourceConnector`，`cargo test -p attune-core --test ingest_email_test` 通过（mail-parser 解析 Subject / body / 附件的离线测试，用固定 .eml fixture）。
2. `POST /api/v1/index/bind-email` 能连真实 IMAP（手测 Gmail 应用密码），拉 INBOX 邮件入库，`item_count` 增长。
3. 二次调用同邮箱：UID 增量生效，已入库邮件 `skipped`，只拉新邮件。
4. 带 PDF 附件的邮件：附件作为独立 item 入库且经 OCR / 文字层提取。
5. 邮件 item 的 L1+L2 embedding + classify 都入队（统一 pipeline 保证）。
6. IMAP 不可达 / 密码错 → route 返回结构化 `AppError`，不 panic。

---

## Phase 3：RSS 采集源（task 蓝图级）

**目标：** 增加 `RssConnector` —— 订阅 RSS / Atom feed，把每条 entry 入库。

### 新增文件

| 文件 | 职责 |
|------|------|
| `rust/crates/attune-core/src/ingest/rss.rs` | `RssConnector` + `RssConfig` + OPML 导入 |
| `rust/crates/attune-core/tests/ingest_rss_test.rs` | feed-rs 解析 + OPML 解析单元测试 |
| `rust/crates/attune-server/src/routes/rss.rs` | `POST /api/v1/index/bind-rss` + `POST /api/v1/index/import-opml` |

### crate 选型

```toml
feed-rs = "2"   # 纯 Rust，统一解析 RSS 2.0 / Atom / JSON Feed
```

OPML 导入用已有的 `scraper` 或轻量手写解析（OPML 是简单 XML，`<outline xmlUrl="...">`）。

### `RssConnector` 如何 impl `SourceConnector`

```rust
pub struct RssConfig {
    pub feed_url: String,
    pub seen_entry_ids: Vec<String>,  // 增量：已入库的 entry id
}

impl SourceConnector for RssConnector {
    fn source_kind(&self) -> SourceKind { SourceKind::Rss }
    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        // 单线程 tokio runtime 桥接：
        // 1. reqwest GET feed_url（复用 reqwest rustls）
        // 2. feed_rs::parser::parse(&bytes) → Feed
        // 3. 逐 entry：
        //    - source_ref = entry.id（GUID / link，稳定唯一）
        //    - title = entry.title
        //    - content = entry.content 或 entry.summary（HTML → html_to_text）
        //    - modified_marker = entry.updated / entry.published 的 rfc3339
        //    - uri = entry.links[0]
        //    - metadata: feed 标题 / author
        // 4. 已在 seen_entry_ids 里的 entry 跳过（增量）
    }
}
```

### OPML 导入

`POST /api/v1/index/import-opml` 收 OPML 文件 → 解析出所有 `xmlUrl` → 对每个 feed 创建一条 `bound_dir`（`rss:` 前缀）→ 各跑一次 `RssConnector`。

### 验收标准

1. `RssConnector` impl `SourceConnector`，`cargo test -p attune-core --test ingest_rss_test` 通过（用固定 RSS / Atom XML fixture）。
2. `POST /api/v1/index/bind-rss` 订阅一个真实 feed（如某博客 RSS），entry 入库，`item_count` 增长。
3. 二次调用同 feed：已入库 entry `skipped`，只拉新 entry。
4. `POST /api/v1/index/import-opml` 导入含多个 feed 的 OPML，每个 feed 各建一条 bound_dir 并完成首次抓取。
5. RSS entry item 的 L1+L2 embedding + classify 都入队。
6. feed URL 不可达 / 非法 XML → 结构化 `AppError`，不 panic。
7. 复用 Task 11 的周期 worker 模式：`start_rss_sync_worker` 周期重抓所有 `rss:` 目录。

---

## Phase 4：云盘采集源（task 蓝图级）

**目标：** 增加 `CloudDriveConnector` —— 经 `rclone` subprocess 桥接 Google Drive / Dropbox / OneDrive 等云盘。**不引入云厂商 SDK crate**，复用 rclone 的统一 remote 抽象。

### 新增文件

| 文件 | 职责 |
|------|------|
| `rust/crates/attune-core/src/ingest/cloud.rs` | `CloudDriveConnector` + `CloudConfig` |
| `rust/crates/attune-server/src/routes/cloud.rs` | `POST /api/v1/index/bind-cloud` |

### 技术选型

`rclone` 是跨平台单二进制（Win/Linux/macOS），支持 70+ 云存储后端。attune **不捆绑** rclone —— 检测系统 PATH（用已有的 `which` crate），缺失时引导用户安装。子进程调用模式与 whisper.cpp / poppler 一致（`std::process::Command`，跨平台）。

### `CloudDriveConnector` 如何 impl `SourceConnector`

```rust
pub struct CloudConfig {
    pub remote_name: String,   // rclone 配置里的 remote 名（如 "gdrive:")
    pub remote_path: String,   // remote 内子路径
    pub rclone_config_path: PathBuf,  // rclone.conf 路径（含云盘凭据）
}

impl SourceConnector for CloudDriveConnector {
    fn source_kind(&self) -> SourceKind { SourceKind::CloudDrive }
    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        // 1. `rclone lsjson --config <conf> <remote>:<path> -R` → JSON 文件清单
        //    （Path / Size / ModTime / Hashes）
        // 2. 按扩展名过滤（复用 is_supported_remote_ext 同款逻辑）
        // 3. 逐文件 `rclone cat --config <conf> <remote>:<path>/<file>` → 字节流
        //    （按需下载，不全量同步到本地磁盘 —— 不捆绑、不占空间）
        // 4. 产 RawDocument：
        //    - source_ref = "<remote>:<full-path>"
        //    - modified_marker = Hashes 里的 hash（缺失则 ModTime）
        //    - metadata: cloud provider 名
    }
}
```

### 凭据管理

云盘凭据在 `rclone.conf` 里（rclone 自己管 OAuth token 刷新）。attune 把 `rclone.conf` 路径存进 vault 加密配置 —— **不自己实现云厂商 OAuth**。用户先用 `rclone config` 配好 remote，attune 只引用。

### 验收标准

1. `CloudDriveConnector` impl `SourceConnector`，`cargo test -p attune-core --test ingest_cloud_test` 通过（mock `rclone lsjson` JSON 输出的解析测试）。
2. 系统无 rclone 时 `bind-cloud` 返回明确错误（引导安装），不 panic。
3. `POST /api/v1/index/bind-cloud` 对一个配好的 rclone remote 完成首次抓取，文件入库。
4. `rclone cat` 按需下载生效 —— 不在本地磁盘留全量副本。
5. 二次调用：hash / ModTime 增量生效。
6. 云盘文件 item 的 L1+L2 embedding + classify 都入队。
7. 复用周期 worker 模式：`start_cloud_sync_worker` 周期重扫 `cloud:` 目录。

---

## 风险与回滚

| 风险 | 缓解 |
|------|------|
| **迁移期破坏现有入库** | Phase 1 每个 Task 独立 commit、独立可编译可测。Task 3→4→9 各自迁一条路径，每步跑全量测试。任一 Task 出问题可 `git revert` 单个 commit 而不影响其它路径。 |
| **`reqwest_dav` 引入 `native-tls`/`openssl-sys`** | 决策 3 已核实 `reqwest_dav` 0.3 提供独立 `rustls-tls` feature 且不强制 TLS 后端 —— `default-features = false` + `rustls-tls` 干净规避。Task 5 Step 2 仍做 `Cargo.lock` 复查兜底。 |
| **HTTP API 形态漂移** | Task 3/4/7 各有「响应形态契约测试」。`/api/v1/upload` 的 `{id,title,chunks_queued,status}`、`/api/v1/ingest` 的 `{id,status,chunks_queued}`、`bind-remote` 的 `{status,dir_id,scan}` 都有断言锁定。`domain`/`tags` 透传由 `ingest_passes_through_domain_and_tags` 测试锁定。 |
| **`scanner` corpus_domain chunk 前缀注入丢失** | 决策 2 已落实：`RawDocument` 加 `corpus_domain` 字段，`ingest_document` 内部对 `!= general` 注入 `[领域: X]` 前缀 + 调 `set_item_corpus_domain`。`LocalFolderConnector` 由 `scanner.rs` 在构造 `RawDocument` 时从 `get_dir_corpus_domain` 回填。`ingest_injects_corpus_domain_prefix_into_chunks` 测试锁定。 |
| **WebDAV 周期 worker 拿不到认证凭据** | 决策 4 已落实：Task 10 新增 `webdav_remotes` 表（`password` 字段级 AES-256-GCM 加密），Task 11 worker 从表读回解密凭据 —— **认证源也能周期自动同步**。 |
| **`reqwest_dav` API 字段名与计划不符** | Task 6 明确写「字段名以实际 crate 为准，结构不变」。实现者按 docs.rs 校正。 |
| **`webdav_remotes.password` 明文泄漏** | `password_enc` 是 dek 加密 BLOB，与 `items.content` 同模式。`password_is_not_stored_in_plaintext` 测试断言密文列不含明文标记。`WebDavRemoteRow` 禁止 `Debug` 打印（Task 11 Step 4 注明）。 |

**每 Phase 可独立交付：** Phase 1 完成即「统一抽象 + WebDAV 修复（含认证源周期同步）」可发布。Phase 2/3/4 各自是独立 feature，互不依赖，可分别排期。Phase 2/3/4 都依赖 Phase 1 的 `SourceConnector` / `ingest_document`，但彼此之间无依赖。

---

## Self-Review

对照 spec 范围 + 产品负责人 4 个决策逐项自查的结果：

**1. Spec 覆盖：**
- ✅ 新增 `ingest/{connector,pipeline,mod}.rs` —— Task 1（connector + mod）、Task 2（pipeline）。`RawDocument` 现 11 个字段（原 8 + 决策 1/2 加的 `domain` / `tags` / `corpus_domain`）、`SourceConnector` 用回调 sink（非 `Vec`）、`ingest_document` 五步、`IngestOutcome` 四态 —— 全部落到 Task 1/2。`IngestOutcome` 的 `Skipped`（空内容）是 spec「Inserted/Duplicate/Updated」之外的必要补充，已明确。
- ✅ 4 条现有路径迁移 —— Task 3（ingest）、Task 4（upload）、Task 7（remote/bind_remote）、Task 9（scanner）。
- ✅ WebDAV 修复（`reqwest_dav` + async + ETag）—— Task 5（依赖）、Task 6（重写 connector）。Task 2 commit message + Task 6 文件头注释**明确点出**「L2 embedding + classify 缺陷由统一 pipeline 自动消失」。
- ✅ 增量同步调度 —— Task 10（凭据持久化）+ Task 11（周期 worker）。
- ✅ Phase 2（Email）/ 3（RSS）/ 4（云盘）—— 蓝图级，含新文件清单、crate 选型、`SourceConnector` impl 草图、MVP 范围、配置项、验收标准。
- ✅ writing-plans header、File Structure 小节、风险与回滚、Self-Review —— 齐全。

**2. 产品负责人 4 个决策逐条落实核对：**
- ✅ **决策 1（`domain` / `tags` 透传）**：`RawDocument` 加 `domain: Option<String>` + `tags: Option<Vec<String>>` 一等字段（Task 1 struct + 测试）；`ingest_document` 把它们透传给 `insert_item` 第 6/7 参数（Task 2 pipeline）；`/api/v1/ingest` 的 `IngestRequest` 保留 `domain` / `tags` 字段并经 `RawDocument` 透传（Task 3），对外行为不变。所有 `RawDocument` 构造点（Task 1 测试 ×2、Task 2 `md_doc`、Task 3 ingest、Task 4 upload、Task 6 webdav、Task 8 local）非 ingest 源全部传 `None`。回归测试 `ingest_passes_through_domain_and_tags`（Task 2）锁定。
- ✅ **决策 2（corpus_domain chunk 前缀注入）**：`RawDocument` 加 `corpus_domain: Option<String>` 字段；`ingest_document`（Task 2）内部当 `corpus_domain` 为 `Some(d)` 且 `d != "general"` 时，对 L1/L2 每个 chunk_text 注入 `[领域: d] ` 前缀再 `enqueue_embedding`，并调 `set_item_corpus_domain`。`LocalFolderConnector::new` 加 `corpus_domain` 参数（Task 8），`scanner.rs` 从 `get_dir_corpus_domain` 读出传入（Task 9）；其它源传 `None`，WebDAV 由 `sync_webdav_dir` 从 `webdav_remotes.corpus_domain` 回填（Task 11）。回归测试 `ingest_injects_corpus_domain_prefix_into_chunks` + `ingest_general_corpus_domain_skips_prefix`（Task 2）锁定。
- ✅ **决策 3（reqwest_dav TLS 合规）**：核实 `reqwest_dav` v0.3.3 `Cargo.toml` —— 提供 `rustls-tls = ["reqwest/rustls"]` feature，自身 `reqwest` 依赖 `default-features = false`，不强制 TLS 后端。计划保持用 `reqwest_dav`，Cargo.toml 写 `reqwest_dav = { version = "0.3", default-features = false, features = ["rustls-tls"] }`（**版本 0.3 不是 0.2**），不引入 `native-tls`/`openssl-sys`。结论写入开头「WebDAV TLS 合规核实结论」+ Task 5。**不需要**走「保留手写 parser」备选。
- ✅ **决策 4（WebDAV 凭据持久化纳入 Phase 1）**：新增 Task 10 —— `webdav_remotes` 表（`url` / `username` / `password_enc` / `depth` / `corpus_domain` / `updated_at` / `last_etag_sync`），`password` 走字段级 AES-256-GCM（`crypto::encrypt(dek, ...)`，与 `items.content` 同模式）。`bind-remote` 落库该配置；Task 11 周期 worker 从 `list_webdav_remotes` 读回解密凭据，**认证源也能自动增量重扫**。新增测试 `webdav_remotes_test.rs`（往返 / 明文不落盘 / list / 幂等 4 个测试）。

**3. 我自查中发现并已在计划内修正/标注的点：**
- **`SourceConnector::fetch_documents` 是同步契约，但 WebDAV/Email/RSS/云盘 I/O 是 async。** 未把 trait 改成 async（避免 `async-trait` 依赖 + trait object 复杂化）。统一解法：connector 内部用 `tokio::runtime::Builder::new_current_thread()` 桥接，调用方在 `spawn_blocking` 里调 —— Task 6 `WebDavConnector::drive_blocking` 是范本。
- **`ingest_document` 与「更新检测」的职责边界。** 增量判断 + 删旧由 caller 做，`ingest_document_replacing(…, old_item_id)` 接收 `old_item_id` 仅用于在 `IngestOutcome::Updated` 里透出。`ingest_document` 保持「这份文档怎么入库」单一职责。
- **OCR profile。** upload 原本用 `parse_bytes_with_profile`。Task 4 补了 `ingest_document_with_profile` 入口，避免丢 OCR profile 能力。Task 4 把 `ingest_document_inner` 签名加 `profile: Option<&str>`，`ingest_document` / `ingest_document_replacing` 内部调用补 `None` —— 与 Task 2 的 domain/tags/corpus_domain 逻辑正交，无冲突。
- **FTS 即时索引** 在 core 层做不了（拿不到 `FulltextIndex` 锁）。server 层薄壳 caller（upload/ingest）在拿到 `item_id` 后自己补 `fulltext.add_document`。upload 需要解析后 content，Task 4 改成入库后 `get_item` 取回。
- **测试脚手架不确定性。** `server_test.rs` 是否有 `unlocked_app`/`post_json` 无法 100% 确认 —— Task 3/7 给了 fallback。
- **`now_iso8601` / `peek_embed_queue_chunk_texts` 的实现不确定性。** Task 10 的时间戳工具函数名、Task 2 的 `embed_queue.chunk_text` 明文/密文存储形态，计划无法在不读对应实现的前提下确定 —— 两处都给了「实现者按实际二选一，结构不变」的明确指示。
- **方法签名核对。** `insert_item`（含 `domain` / `tags` 参数）/ `enqueue_embedding` / `enqueue_classify` / `enqueue_reindex` / `find_item_by_content_hash` / `compute_content_hash`（模块级自由函数）/ `upsert_chunk_breadcrumbs_from_content` / `get_indexed_file` / `upsert_indexed_file` / `record_signal_event` / `get_dir_corpus_domain` / `set_item_corpus_domain` / `get_item` / `get_tags_json` / `crypto::encrypt` / `crypto::decrypt` —— 全部已对照 `crates/attune-core/src/{store/*,crypto}.rs` 实际代码，签名一致。

**4. 类型与调用点一致性：**
- `RawDocument` 11 字段在所有 8 处构造点（Task 1 测试 ×2、Task 2 `md_doc`、Task 3、Task 4、Task 6、Task 8、Task 11 `sync_webdav_dir` 内 mutate）字段名/顺序一致。
- `ingest_document` / `ingest_document_replacing` / `ingest_document_with_profile` 在所有调用点（Task 3、Task 4、Task 6/7/11、Task 9）签名一致：`(store, dek, raw)` / `(store, dek, raw, old_item_id)` / `(store, dek, raw, profile)`。
- `LocalFolderConnector::new` 4 参数（`root` / `recursive` / `file_types` / `corpus_domain`）在 Task 8 测试 ×2 + Task 9 `scan_directory` 调用一致。
- `sync_webdav_dir(state, dir_id, config, corpus_domain)` 4 参数在 Task 11 的 `bind_remote` 与 `start_webdav_sync_worker` 两个调用点一致。
- `ingest/mod.rs` 的 `pub use` 在 Task 1/2/4 三次增量更新，每次给完整行，无悬空引用。
- Task 编号 1–12 连续无断号；Phase 1 = Task 1–12，决策 2/4 新增的 Task 10（凭据持久化）插在原 Task 9 后、worker 前，原「全量回归」从 Task 10 顺延为 Task 12。

**5. Placeholder 扫描：** Phase 1 所有 Task 含完整可编译 Rust 代码，无 TODO/TBD。「按实际 crate / 仓内实现校正」的点：`reqwest_dav` 字段名（Task 6）、`now_iso8601` 函数名（Task 10）、`embed_queue.chunk_text` 存储形态（Task 2）—— 均为客观不确定性，已给明确二选一指示。

---

## 已拍板决策记录（2026-05-18 产品负责人）

上一版计划末尾的 4 个存疑项已由产品负责人拍板，全部落实进 Phase 1，记录如下：

1. **`domain` / `tags` 透传 → 保留。** `RawDocument` 加 `domain` / `tags` 一等字段，`ingest_document` 透传给 `insert_item`。`/api/v1/ingest` 迁移后对外行为完全不变。落实于 Task 1（字段）/ Task 2（透传）/ Task 3（route）。

2. **corpus_domain chunk 前缀注入 → 保留。** `RawDocument` 加 `corpus_domain` 字段，`ingest_document` 对 `!= "general"` 给 L1/L2 每个 chunk_text 注入 `[领域: X] ` 前缀 + 调 `set_item_corpus_domain`。F-Pro 跨域防污染零收窄。落实于 Task 1/2/8/9。

3. **reqwest_dav TLS → 用 `reqwest_dav` 0.3 + rustls。** 核实 v0.3.3 `Cargo.toml` 确认提供 `rustls-tls` feature 且不强制 TLS 后端，`default-features = false` + `rustls-tls` 即纯 Rust rustls，符合 CLAUDE.md 硬约束。不走「保留手写 parser」备选。落实于开头核实结论 + Task 5。

4. **WebDAV 凭据持久化 → 纳入 Phase 1。** 新增 Task 10：`webdav_remotes` 表，`password` 字段级 AES-256-GCM 加密；`bind-remote` 落库，周期 worker（Task 11）读回解密凭据，认证源（Nextcloud / 坚果云等）可自动增量同步。Phase 2 的 Email `password` 复用同一加密表模式（`email_accounts` 表）。

**仍存疑点：** 无。4 个决策已全部转为计划内 Task，无遗留待拍板项。实现期可能遇到的客观不确定性（`reqwest_dav` 字段名、`now_iso8601` 函数名、`embed_queue.chunk_text` 存储形态）已在对应 Task 给出二选一处置指示，不需要产品决策。
