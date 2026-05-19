# RSS / Atom 采集源 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 attune 增加一个 `RssConnector`（impl `SourceConnector`）—— 订阅 RSS 0.x/1.0/2.0 / Atom / JSON Feed，把每条 feed entry 经统一 `ingest_document` pipeline 入库；配套 `rss_feeds` 配置表、HTTP API（feed CRUD + OPML 导入 + 手动刷新）、settings UI、后台周期拉取 worker。

**Architecture:** 复用 `attune_core::ingest` 已落地的统一抽象 —— `RssConnector` 不碰 pipeline 内部，只负责「拉取 feed XML → `feed-rs` 解析 → 逐 entry 产 `RawDocument`」。`SourceConnector::fetch_documents` 是同步契约而 HTTP 拉取是 async，照 `WebDavConnector::drive_blocking` 范式用单线程 `tokio` runtime 桥接，调用方在 `spawn_blocking` 里调。feed 列表 + 每 feed 的增量游标（`last_fetched` + 已见 GUID 集）落新表 `rss_feeds`（无凭据，**不加密**，但 CRUD 模式参考 `webdav_remotes`）。后台 `start_rss_sync_worker` 周期重抓，与 `start_webdav_sync_worker` 同范式。

**Tech Stack:** Rust 2021 / Axum 0.8 / rusqlite / tokio。新依赖：`feed-rs` 2.x（纯 Rust，统一解析 RSS/Atom/JSON Feed）、`opml` 1.x（纯 Rust，OPML 导入/导出）。HTTP 拉取复用已在 workspace 的 `reqwest`（rustls）。前端 Preact + signals + i18n（zh/en 同步）。

---

## 关键背景与约束（实现者必读）

**前置条件 —— 采集体系重构 Phase 1 已完成。** 本计划构建在 `docs/superpowers/plans/2026-05-18-ingest-unification.md` Phase 1 交付物之上：`attune_core::ingest` 模块（`connector.rs` / `pipeline.rs` / `local.rs`）已存在且 `SourceKind` 已含 `Rss` 变体。开工前用 `grep -n "Rss" rust/crates/attune-core/src/ingest/connector.rs` 确认。若 `ingest` 模块不存在，**先停下**让用户先跑完 unification Phase 1。

**真实代码事实（已与代码核对，实现者不要凭记忆改签名）：**

- `SourceKind` enum 已有 `Rss` 变体；`SourceKind::Rss.as_str() == "rss"`；`SourceKind::Rss.item_source_type() == "file"`。
- `RawDocument` 11 字段：`uri` / `title` / `content: Vec<u8>` / `mime_hint: Option<String>` / `source_kind` / `source_ref` / `modified_marker: Option<String>` / `domain: Option<String>` / `tags: Option<Vec<String>>` / `corpus_domain: Option<String>` / `metadata: HashMap<String,String>`。
- `SourceConnector` trait：`fn source_kind(&self) -> SourceKind` + `fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()>`。`DocumentSink<'a> = Box<dyn FnMut(RawDocument) + 'a>`。
- `ingest::ingest_document(store: &Store, dek: &Key32, raw: &RawDocument) -> Result<IngestOutcome>`；`ingest_document_replacing(store, dek, raw, old_item_id: &str)`。`IngestOutcome` 四态：`Inserted { item_id, chunks_enqueued }` / `Duplicate { item_id }` / `Updated { item_id, old_item_id }` / `Skipped { reason }`。
- `attune_core::error::{Result, VaultError}` 是 core 层错误类型。`VaultError::LlmUnavailable(String)` 是已有变体，被 `WebDavConnector` 复用作通用网络错误载体 —— RSS 沿用同模式（不新增变体，避免动 error.rs）。
- server 层错误是 `attune_server::error::{AppError, AppResult}`；`AppError: From<VaultError>`，`VaultError::LlmUnavailable` 落到 `AppError::Internal`（500）。Route 内对「feed URL 不可达 / 非法 XML」要显式 `.map_err(|e| AppError::BadGateway(...))` 归类到 502，不让它落 500。
- `Store::get_indexed_file(&self, path: &str) -> Result<Option<IndexedFileRow>>`；`IndexedFileRow { id, dir_id, path, file_hash, item_id: Option<String> }`。
- `Store::upsert_indexed_file(&self, dir_id, path, file_hash, item_id) -> Result<()>`。
- `Store::delete_item(&self, id: &str) -> Result<bool>`；`Store::enqueue_reindex(&self, item_id, action: &str) -> Result<()>`（`action` ∈ `{"purge","reindex"}`）。
- `Store::record_signal_event(&self, kind: &str, ref_id: &str, query: Option<&str>) -> Result<()>`（`kind` ∈ 已知集 `doc_create` / `doc_update` / `doc_delete` / …）。
- `Store::bind_directory(&self, path: &str, recursive: bool, file_types: &[&str]) -> Result<String>` —— 返回 `dir_id`，`path` UNIQUE。
- `Store` 持有 `pub(crate)` 的 `conn: rusqlite::Connection`（同 crate 内 `store/*.rs` 可访问，见 `webdav_remotes.rs` 直接用 `self.conn`）。
- schema 在 `store/mod.rs` 的 `SCHEMA_SQL` 常量里集中声明（`CREATE TABLE IF NOT EXISTS` —— 老 vault 下次 open 自动获得空表，无需独立 migration）。`webdav_remotes` 表即在此声明，`rss_feeds` 沿用同位置。
- WebDAV 范式参照：`scanner_webdav.rs::WebDavConnector`（同步 trait + tokio runtime 桥接 async）、`store/webdav_remotes.rs`（配置表 CRUD）、`attune-server/src/ingest_webdav.rs::sync_webdav_dir`（route + worker 共用入库逻辑）、`state.rs::start_webdav_sync_worker`（周期 worker + 原子 flag 防重入）。

**Lock ordering（防死锁，全程遵守）：** `vault.lock()` → `vectors.lock()` → `fulltext.lock()` → `embedding.lock()`。`RssConnector` 与 `ingest_document` 只碰 `Store`（SQL 连接），不碰 `VectorIndex` / `FulltextIndex`。`sync_rss_feed` 与 `sync_webdav_dir` 同设计：网络拉取**锁外**做，每个文档的 DB 写才短暂拿 `vault` 锁，写完即 drop guard。

**成本契约（CLAUDE.md「三层成本」）：** RSS 拉取属于 ⚡ 本地算力层 —— 建库阶段只跑到「entry 可被搜到 + 150 字存档摘要」。embedding / classify 由 `ingest_document` 入队后台处理。**不**在拉取阶段触发 LLM 深度分析。后台周期 worker 是「建库」性质，允许自动跑（与 webdav worker 一致），不违反「分析阶段等用户开口」。

**MVP 边界（蓝图级后续 task，不在本计划）：**
- **抓全文**：MVP 用 feed entry 自带的 `content` / `summary`。若 entry 只给摘要 + 链接，「跟链接抓全文」是后续增强 task（见文末「蓝图级后续 task」）。
- **feed HTTP Basic auth**：MVP 假设 feed 公开无凭据。少数私有 feed 的 auth 是后续增强 —— 届时 `rss_feeds` 表加 `password_enc` 列走 `webdav_remotes` 同款字段级加密。

**注释纪律：** 一个改动区域一条意图注释，写 WHY 不写 WHAT，禁止 `批次X` / `FIX-N` / `阶段Y` / `per reviewer` 这类过程标签。

**i18n 纪律：** 任何用户可见字符串走 `t()`；新增 key 必须同时写入 `i18n/zh.ts` 和 `i18n/en.ts`，两文件 key 集合永远一致。Task 9 末尾跑 grep 守卫。

**API 命名：** kebab-case。新 path：`/api/v1/rss/feeds`、`/api/v1/rss/import-opml`、`/api/v1/rss/feeds/{id}/refresh`。

---

## File Structure

### 新建文件

| 文件 | 职责 |
|------|------|
| `rust/crates/attune-core/src/ingest/rss.rs` | `RssConfig`、`RssConnector`（impl `SourceConnector`，feed-rs 解析 + GUID 增量过滤）、`parse_opml`（OPML → feed URL 列表） |
| `rust/crates/attune-core/src/store/rss_feeds.rs` | `rss_feeds` 表 CRUD：`RssFeedInput` / `RssFeedRow` + `upsert` / `get` / `list` / `delete` / `touch_sync` / `seen GUID` 读写 |
| `rust/crates/attune-core/tests/ingest_rss_test.rs` | feed-rs 解析（RSS 2.0 / Atom fixture）+ GUID 增量去重 + OPML 解析单元测试 |
| `rust/crates/attune-core/tests/rss_feeds_test.rs` | `rss_feeds` 表 CRUD + seen-GUID 累积往返集成测试 |
| `rust/crates/attune-server/src/routes/rss.rs` | HTTP API：feed CRUD + OPML 上传 + 手动刷新 |
| `rust/crates/attune-server/src/ingest_rss.rs` | `sync_rss_feed` 公共函数（route 手动刷新与周期 worker 共用） |
| `rust/crates/attune-server/ui/src/hooks/useRss.ts` | 前端 RSS API 封装（list / add / delete / import-opml / refresh） |

### 修改文件

| 文件 | 改动 |
|------|------|
| `rust/crates/attune-core/Cargo.toml` | 加 `feed-rs` 2.x、`opml` 1.x 依赖 |
| `rust/crates/attune-core/src/ingest/mod.rs` | 加 `pub mod rss; pub use rss::*;` |
| `rust/crates/attune-core/src/store/mod.rs` | 加 `pub mod rss_feeds;` + `rss_feeds` 表 `CREATE TABLE IF NOT EXISTS` |
| `rust/crates/attune-server/src/lib.rs` | 注册 5 条 RSS route；启动时调 `AppState::start_rss_sync_worker` |
| `rust/crates/attune-server/src/state.rs` | 加 `rss_sync_worker_running: AtomicBool` + `start_rss_sync_worker` 周期 worker |
| `rust/crates/attune-server/ui/src/views/RemoteView.tsx` | 加「添加 RSS」按钮 + RSS feed 列表行 + Add RSS / Import OPML 模态 + 「立即刷新」 |
| `rust/crates/attune-server/ui/src/i18n/zh.ts` | 加 `rss.*` key |
| `rust/crates/attune-server/ui/src/i18n/en.ts` | 加 `rss.*` key（与 zh 集合一致） |

---

## Phase 1：RssConnector + rss_feeds 表 + HTTP API + worker + UI

### Task 1：加 `feed-rs` + `opml` 依赖

**Files:**
- Modify: `rust/crates/attune-core/Cargo.toml`

- [ ] **Step 1: 在 `[dependencies]` 段加两个 crate**

在 `rust/crates/attune-core/Cargo.toml` 的 `[dependencies]` 段末尾追加：

```toml
# RSS / Atom / JSON Feed 统一解析（纯 Rust，无 C 绑定）。
feed-rs = "2"
# OPML 导入/导出（纯 Rust）—— 从其它 RSS 阅读器迁移 feed 列表。
opml = "1"
```

- [ ] **Step 2: 验证依赖可解析、无 native-tls 引入**

Run: `cd rust && cargo tree -p attune-core -i openssl-sys 2>&1 | head -3`
Expected: `error: package ID specification \`openssl-sys\` did not match any packages` —— 即未引入 OpenSSL。

Run: `cd rust && cargo build -p attune-core 2>&1 | tail -5`
Expected: `Finished` —— 依赖下载并编译通过。

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-core/Cargo.toml rust/Cargo.lock
git commit -m "$(cat <<'EOF'
chore(ingest): add feed-rs + opml deps for RSS source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2：`parse_opml` —— OPML → feed URL 列表

**Files:**
- Create: `rust/crates/attune-core/src/ingest/rss.rs`
- Modify: `rust/crates/attune-core/src/ingest/mod.rs`
- Test: `rust/crates/attune-core/tests/ingest_rss_test.rs`

- [ ] **Step 1: 写失败测试**

创建 `rust/crates/attune-core/tests/ingest_rss_test.rs`：

```rust
//! RSS 采集源单元测试 —— OPML 解析 / feed-rs 解析 / GUID 增量。

use attune_core::ingest::rss::{parse_opml, OpmlFeed};

const OPML_SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>My Subscriptions</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline type="rss" text="Rust Blog" title="Rust Blog"
        xmlUrl="https://blog.rust-lang.org/feed.xml"
        htmlUrl="https://blog.rust-lang.org/"/>
      <outline type="rss" text="LWN" xmlUrl="https://lwn.net/headlines/rss"/>
    </outline>
    <outline type="rss" text="Top Level" xmlUrl="https://example.com/feed"/>
  </body>
</opml>"#;

#[test]
fn parse_opml_extracts_nested_and_toplevel_feeds() {
    let feeds: Vec<OpmlFeed> = parse_opml(OPML_SAMPLE).expect("opml parses");
    assert_eq!(feeds.len(), 3, "嵌套 + 顶层 outline 的 xmlUrl 都要抽出来");
    let urls: Vec<&str> = feeds.iter().map(|f| f.xml_url.as_str()).collect();
    assert!(urls.contains(&"https://blog.rust-lang.org/feed.xml"));
    assert!(urls.contains(&"https://lwn.net/headlines/rss"));
    assert!(urls.contains(&"https://example.com/feed"));
    // title 缺失时回退 text。
    let lwn = feeds.iter().find(|f| f.xml_url.contains("lwn")).unwrap();
    assert_eq!(lwn.title, "LWN");
}

#[test]
fn parse_opml_skips_outline_without_xml_url() {
    // 纯分组 outline（无 xmlUrl）不应产出 feed。
    let opml = r#"<?xml version="1.0"?><opml version="2.0"><body>
      <outline text="Empty Group"/></body></opml>"#;
    let feeds = parse_opml(opml).expect("opml parses");
    assert!(feeds.is_empty(), "无 xmlUrl 的分组 outline 应跳过");
}

#[test]
fn parse_opml_rejects_malformed_xml() {
    let result = parse_opml("<opml><body><not-closed>");
    assert!(result.is_err(), "非法 XML 应返回 Err 而非 panic");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd rust && cargo test -p attune-core --test ingest_rss_test 2>&1 | tail -5`
Expected: 编译失败 —— `unresolved import attune_core::ingest::rss`。

- [ ] **Step 3: 创建 `ingest/rss.rs` 并实现 `parse_opml`**

创建 `rust/crates/attune-core/src/ingest/rss.rs`：

```rust
//! RSS / Atom / JSON Feed 采集源。
//!
//! `RssConnector` impl `SourceConnector`：拉取 feed XML → `feed-rs` 统一解析
//! → 逐 entry 产 `RawDocument`，由调用方走 `ingest_document` 入库。已见的
//! entry GUID（`<guid>` / Atom `<id>`）跳过，实现增量。
//!
//! `SourceConnector::fetch_documents` 是同步契约而 HTTP 拉取是 async ——
//! 照 `scanner_webdav::WebDavConnector` 范式用单线程 tokio runtime 桥接，
//! 调用方在 `spawn_blocking` 里调本 connector。

use std::collections::HashSet;

use crate::error::{Result, VaultError};
use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};

/// 从 OPML 抽出的一条 feed 订阅项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlFeed {
    /// feed XML 地址（OPML `outline/@xmlUrl`）。
    pub xml_url: String,
    /// 展示标题（`@title` 优先，缺失回退 `@text`，再缺失用 xml_url）。
    pub title: String,
}

/// 解析 OPML 文本，递归收集所有带 `xmlUrl` 的 outline。
///
/// OPML 允许 outline 任意层级嵌套（分组）—— `opml` crate 把 body 解析成
/// 树，这里深度优先遍历收所有叶子 feed。无 `xmlUrl` 的纯分组 outline 跳过。
pub fn parse_opml(opml_text: &str) -> Result<Vec<OpmlFeed>> {
    let doc = opml::OPML::from_str(opml_text)
        .map_err(|e| VaultError::InvalidInput(format!("opml parse: {e}")))?;
    let mut out = Vec::new();
    for outline in &doc.body.outlines {
        collect_opml_outline(outline, &mut out);
    }
    Ok(out)
}

/// 深度优先收集一个 outline 子树里的 feed。
fn collect_opml_outline(outline: &opml::Outline, out: &mut Vec<OpmlFeed>) {
    if let Some(xml_url) = &outline.xml_url {
        if !xml_url.trim().is_empty() {
            let title = outline
                .title
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    if outline.text.trim().is_empty() {
                        xml_url.clone()
                    } else {
                        outline.text.clone()
                    }
                });
            out.push(OpmlFeed { xml_url: xml_url.clone(), title });
        }
    }
    for child in &outline.outlines {
        collect_opml_outline(child, out);
    }
}
```

在文件顶部 `use` 之后补 `use std::str::FromStr;`（`opml::OPML::from_str` 需要 trait 在 scope）—— 实际上 `opml` crate 提供 `OPML::from_str` 关联函数，若编译报 `from_str` not found 则改用 `opml_text.parse::<opml::OPML>()`，二者等价，以 crate 实际 API 为准。

修改 `rust/crates/attune-core/src/ingest/mod.rs`，在现有 `pub use` 之后追加：

```rust
pub mod rss;
pub use rss::{OpmlFeed, RssConfig, RssConnector};
```

（`RssConfig` / `RssConnector` 在 Task 3 加入；本步先只导出 `rss` 模块本身可编译 —— 若 Task 3 未完成导致 `pub use rss::{...}` 引用未定义类型，先只写 `pub mod rss;`，Task 3 Step 5 再补全 `pub use`。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cd rust && cargo test -p attune-core --test ingest_rss_test parse_opml 2>&1 | tail -8`
Expected: `parse_opml_extracts_nested_and_toplevel_feeds`、`parse_opml_skips_outline_without_xml_url`、`parse_opml_rejects_malformed_xml` 三个 PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/rss.rs rust/crates/attune-core/src/ingest/mod.rs rust/crates/attune-core/tests/ingest_rss_test.rs
git commit -m "$(cat <<'EOF'
feat(ingest): parse OPML into feed URL list

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3：`RssConnector` —— feed-rs 解析 + GUID 增量产 `RawDocument`

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/rss.rs`
- Test: `rust/crates/attune-core/tests/ingest_rss_test.rs`

- [ ] **Step 1: 写失败测试**

在 `rust/crates/attune-core/tests/ingest_rss_test.rs` 末尾追加。`RssConnector` 暴露一个 `parse_feed_bytes` 关联函数，使解析逻辑可脱离网络单测：

```rust
use attune_core::ingest::rss::{RssConfig, RssConnector};
use attune_core::ingest::{RawDocument, SourceKind};

const RSS_2_0: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Sample Blog</title>
  <link>https://sample.example.com/</link>
  <item>
    <title>First Post</title>
    <link>https://sample.example.com/1</link>
    <guid>https://sample.example.com/1</guid>
    <pubDate>Mon, 12 May 2026 10:00:00 GMT</pubDate>
    <description>Body of first post.</description>
  </item>
  <item>
    <title>Second Post</title>
    <link>https://sample.example.com/2</link>
    <guid isPermaLink="false">tag:sample,2026:2</guid>
    <pubDate>Tue, 13 May 2026 10:00:00 GMT</pubDate>
    <description>Body of second post.</description>
  </item>
</channel></rss>"#;

const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Sample</title>
  <id>urn:uuid:feed-id</id>
  <entry>
    <title>Atom Entry One</title>
    <id>urn:uuid:entry-1</id>
    <link href="https://atom.example.com/1"/>
    <updated>2026-05-14T12:00:00Z</updated>
    <content type="html">&lt;p&gt;Atom body one&lt;/p&gt;</content>
  </entry>
</feed>"#;

#[test]
fn parse_feed_bytes_rss_2_0_yields_all_entries() {
    let cfg = RssConfig::new("https://sample.example.com/feed.xml");
    let connector = RssConnector::new(cfg);
    let docs = connector
        .parse_feed_bytes(RSS_2_0.as_bytes())
        .expect("rss parses");
    assert_eq!(docs.len(), 2, "两条 item 都要产出");
    let first = &docs[0];
    assert_eq!(first.source_kind, SourceKind::Rss);
    assert_eq!(first.title, "First Post");
    assert_eq!(first.source_ref, "https://sample.example.com/1", "source_ref = GUID");
    assert_eq!(first.uri, "https://sample.example.com/1", "uri = entry link");
    assert!(first.modified_marker.is_some(), "pubDate 转 rfc3339 作 marker");
    assert!(
        String::from_utf8_lossy(&first.content).contains("Body of first post"),
        "content 取 description"
    );
    // 非 permalink GUID 也作 source_ref。
    assert_eq!(docs[1].source_ref, "tag:sample,2026:2");
    // feed channel title 进 metadata。
    assert_eq!(first.metadata.get("feed_title").map(String::as_str), Some("Sample Blog"));
}

#[test]
fn parse_feed_bytes_atom_strips_html_to_text() {
    let cfg = RssConfig::new("https://atom.example.com/feed");
    let connector = RssConnector::new(cfg);
    let docs = connector.parse_feed_bytes(ATOM.as_bytes()).expect("atom parses");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].source_ref, "urn:uuid:entry-1", "Atom <id> 作 source_ref");
    let body = String::from_utf8_lossy(&docs[0].content);
    assert!(body.contains("Atom body one"), "正文文本保留");
    assert!(!body.contains("<p>"), "HTML 标签应被剥离");
}

#[test]
fn rss_connector_skips_seen_guids() {
    // seen_entry_ids 含第一条 GUID → fetch_documents 只交出第二条。
    let mut cfg = RssConfig::new("https://sample.example.com/feed.xml");
    cfg.seen_entry_ids = vec!["https://sample.example.com/1".to_string()];
    let connector = RssConnector::new(cfg);
    let all = connector.parse_feed_bytes(RSS_2_0.as_bytes()).unwrap();
    let fresh: Vec<&RawDocument> = all
        .iter()
        .filter(|d| !connector.is_seen(&d.source_ref))
        .collect();
    assert_eq!(fresh.len(), 1, "已见 GUID 过滤后只剩 1 条");
    assert_eq!(fresh[0].source_ref, "tag:sample,2026:2");
}

#[test]
fn parse_feed_bytes_rejects_garbage() {
    let cfg = RssConfig::new("https://x.example.com/feed");
    let connector = RssConnector::new(cfg);
    let result = connector.parse_feed_bytes(b"not a feed at all");
    assert!(result.is_err(), "非 feed 字节应返回 Err 而非 panic");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd rust && cargo test -p attune-core --test ingest_rss_test 2>&1 | tail -5`
Expected: 编译失败 —— `RssConfig` / `RssConnector` 未定义。

- [ ] **Step 3: 在 `ingest/rss.rs` 实现 `RssConfig` + `RssConnector`**

在 `rust/crates/attune-core/src/ingest/rss.rs` 末尾追加：

```rust
/// 单条 RSS feed 订阅配置 + 增量游标。
#[derive(Debug, Clone)]
pub struct RssConfig {
    /// feed XML 地址。
    pub feed_url: String,
    /// 已入库的 entry GUID 集 —— `fetch_documents` 跳过这些。
    pub seen_entry_ids: Vec<String>,
}

impl RssConfig {
    /// 新 feed，无任何已见 entry。
    pub fn new(feed_url: impl Into<String>) -> Self {
        Self { feed_url: feed_url.into(), seen_entry_ids: Vec::new() }
    }
}

/// RSS / Atom / JSON Feed 采集源。
pub struct RssConnector {
    config: RssConfig,
    /// `seen_entry_ids` 的 HashSet 视图，`is_seen` O(1) 查。
    seen: HashSet<String>,
}

/// 单个 feed 下载大小上限 —— feed XML 罕见超过几 MB，10 MB 足够且防滥用。
const MAX_FEED_BYTES: usize = 10 * 1024 * 1024;

impl RssConnector {
    pub fn new(config: RssConfig) -> Self {
        let seen = config.seen_entry_ids.iter().cloned().collect();
        Self { config, seen }
    }

    /// 该 GUID 是否已入库过。
    pub fn is_seen(&self, guid: &str) -> bool {
        self.seen.contains(guid)
    }

    /// 把 feed 原始字节解析成一组 `RawDocument`（**不**做增量过滤 ——
    /// 解析与过滤分离，方便单测）。RSS/Atom/JSON Feed 由 `feed-rs` 统一识别。
    pub fn parse_feed_bytes(&self, bytes: &[u8]) -> Result<Vec<RawDocument>> {
        let feed = feed_rs::parser::parse(bytes)
            .map_err(|e| VaultError::InvalidInput(format!("feed parse: {e}")))?;
        let feed_title = feed
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_default();
        let feed_author = feed.authors.first().map(|a| a.name.clone());

        let mut docs = Vec::with_capacity(feed.entries.len());
        for entry in &feed.entries {
            // GUID：feed-rs 把 RSS <guid> / Atom <id> 统一成 entry.id。
            // 极少数畸形 feed 缺 id 时回退第一个 link，再缺则跳过该 entry。
            let guid = if !entry.id.trim().is_empty() {
                entry.id.clone()
            } else if let Some(link) = entry.links.first() {
                link.href.clone()
            } else {
                log::warn!("rss: entry without id/link skipped in {}", self.config.feed_url);
                continue;
            };

            let title = entry
                .title
                .as_ref()
                .map(|t| t.content.clone())
                .unwrap_or_default();

            // 正文：content 优先（全文），否则 summary（摘要）。两者皆为 HTML。
            let raw_html = entry
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()))
                .unwrap_or_default();
            let body_text = html_to_text(&raw_html);
            if body_text.trim().is_empty() {
                // 纯摘要也没有的 entry：仍入库（标题可被搜到），正文用标题兜底。
                // "抓全文" 是蓝图级后续 task。
                log::debug!("rss: entry '{title}' has empty body, using title as content");
            }
            let content = if body_text.trim().is_empty() {
                title.clone()
            } else {
                body_text
            };

            // 增量标记：updated 优先，否则 published；feed-rs 给的是 DateTime<Utc>。
            let marker = entry
                .updated
                .or(entry.published)
                .map(|dt| dt.to_rfc3339());

            // uri：entry 第一个 link（缺失回退 GUID）。
            let uri = entry
                .links
                .first()
                .map(|l| l.href.clone())
                .unwrap_or_else(|| guid.clone());

            let mut metadata = std::collections::HashMap::new();
            if !feed_title.is_empty() {
                metadata.insert("feed_title".to_string(), feed_title.clone());
            }
            if let Some(author) = &feed_author {
                metadata.insert("feed_author".to_string(), author.clone());
            }
            metadata.insert("feed_url".to_string(), self.config.feed_url.clone());

            docs.push(RawDocument {
                uri,
                title,
                content: content.into_bytes(),
                // 正文已是纯文本 —— 让 parser 走纯文本分支（parse_filename 无扩展名）。
                mime_hint: Some("text/plain".to_string()),
                source_kind: SourceKind::Rss,
                source_ref: guid,
                modified_marker: marker,
                // RSS 源无来源域 / 用户标签 / corpus_domain（通用知识，general）。
                domain: None,
                tags: None,
                corpus_domain: None,
                metadata,
            });
        }
        Ok(docs)
    }
}

/// 极简 HTML → 纯文本：剥标签、解码常见实体、压缩空白。feed 正文是富文本片段，
/// 这里只要可被 embedding / 全文检索消费的纯文本，不追求结构保真。
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    // 压缩连续空白为单空格。
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl SourceConnector for RssConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::Rss
    }

    /// 拉取 feed → 解析 → 跳过已见 GUID → 逐条交给 sink。
    ///
    /// 同步契约桥接 async：单线程 tokio runtime 跑 reqwest GET。调用方
    /// （route / 周期 worker）须在 `spawn_blocking` 里调本方法。
    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("rss runtime: {e}")))?;
        let bytes = runtime.block_on(async { self.download_feed().await })?;
        let docs = self.parse_feed_bytes(&bytes)?;
        for doc in docs {
            if self.is_seen(&doc.source_ref) {
                continue;
            }
            sink(doc);
        }
        Ok(())
    }
}

impl RssConnector {
    /// 异步下载 feed XML 字节。10 MB 上限防滥用。
    async fn download_feed(&self) -> Result<Vec<u8>> {
        let client = reqwest::Client::builder()
            .user_agent("attune-rss/1.0")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("rss client: {e}")))?;
        let resp = client
            .get(&self.config.feed_url)
            .send()
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("rss fetch {}: {e}", self.config.feed_url)))?;
        if !resp.status().is_success() {
            return Err(VaultError::LlmUnavailable(format!(
                "rss fetch {}: HTTP {}",
                self.config.feed_url,
                resp.status()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("rss body {}: {e}", self.config.feed_url)))?;
        if bytes.len() > MAX_FEED_BYTES {
            return Err(VaultError::LlmUnavailable(format!(
                "rss feed too large: {} bytes (max {MAX_FEED_BYTES})",
                bytes.len()
            )));
        }
        Ok(bytes.to_vec())
    }
}
```

补全 `ingest/mod.rs` 的 `pub use`（若 Task 2 Step 3 只写了 `pub mod rss;`）：

```rust
pub mod rss;
pub use rss::{OpmlFeed, RssConfig, RssConnector};
```

注：`feed-rs` 2.x 的 `Entry` 字段名以 docs.rs 实际为准 —— 计划用的 `entry.id` / `entry.title.content` / `entry.content.body` / `entry.summary.content` / `entry.updated` / `entry.published` / `entry.links[].href` / `feed.title` / `feed.authors[].name` 是 2.x API。若某字段名不符（如 `body` vs `content`），按 docs.rs 校正，结构不变。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd rust && cargo test -p attune-core --test ingest_rss_test 2>&1 | tail -10`
Expected: 全部 7 个测试 PASS（3 个 OPML + 4 个 feed 解析/增量）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/rss.rs rust/crates/attune-core/src/ingest/mod.rs rust/crates/attune-core/tests/ingest_rss_test.rs
git commit -m "$(cat <<'EOF'
feat(ingest): RssConnector parses RSS/Atom and skips seen GUIDs

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4：`rss_feeds` 表 + CRUD

**Files:**
- Create: `rust/crates/attune-core/src/store/rss_feeds.rs`
- Modify: `rust/crates/attune-core/src/store/mod.rs`
- Test: `rust/crates/attune-core/tests/rss_feeds_test.rs`

- [ ] **Step 1: 在 `store/mod.rs` 的 `SCHEMA_SQL` 加 `rss_feeds` 表**

在 `rust/crates/attune-core/src/store/mod.rs` 的 `SCHEMA_SQL` 常量里，紧跟 `webdav_remotes` 表之后插入：

```sql
-- RSS / Atom feed 订阅。每个 feed 对应一条 bound_dirs(rss:* path) 记录。
-- 无凭据 → 不加密（与 webdav_remotes 的 password_enc 不同）。
-- seen_guids 是已入库 entry GUID 的 JSON 数组字符串，增量去重游标。
CREATE TABLE IF NOT EXISTS rss_feeds (
    dir_id        TEXT PRIMARY KEY REFERENCES bound_dirs(id) ON DELETE CASCADE,
    feed_url      TEXT UNIQUE NOT NULL,
    title         TEXT NOT NULL DEFAULT '',
    seen_guids    TEXT NOT NULL DEFAULT '[]',
    last_fetched  TEXT,
    created_at    TEXT NOT NULL
);
```

在 `store/mod.rs` 顶部模块声明区（`pub mod webdav_remotes;` 附近）加：

```rust
pub mod rss_feeds;
```

- [ ] **Step 2: 写失败测试**

创建 `rust/crates/attune-core/tests/rss_feeds_test.rs`：

```rust
//! rss_feeds 表 CRUD + seen-GUID 累积往返测试。

use attune_core::store::rss_feeds::RssFeedInput;
use attune_core::store::Store;
use tempfile::TempDir;

/// 建一个临时 Store（内存级隔离，每测独立 vault 文件）。
fn temp_store() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let store = Store::open(&db_path).expect("open store");
    (tmp, store)
}

#[test]
fn upsert_and_get_rss_feed_round_trips() {
    let (_tmp, store) = temp_store();
    let dir_id = store
        .bind_directory("rss:https://blog.example.com/feed.xml", false, &["rss"])
        .unwrap();
    let input = RssFeedInput {
        dir_id: dir_id.clone(),
        feed_url: "https://blog.example.com/feed.xml".into(),
        title: "Example Blog".into(),
    };
    store.upsert_rss_feed(&input).unwrap();

    let row = store.get_rss_feed(&dir_id).unwrap().expect("feed exists");
    assert_eq!(row.feed_url, "https://blog.example.com/feed.xml");
    assert_eq!(row.title, "Example Blog");
    assert!(row.seen_guids.is_empty(), "新 feed 无已见 GUID");
    assert!(row.last_fetched.is_none());
}

#[test]
fn list_rss_feeds_returns_all() {
    let (_tmp, store) = temp_store();
    for i in 0..3 {
        let url = format!("https://feed{i}.example.com/rss");
        let dir_id = store.bind_directory(&format!("rss:{url}"), false, &["rss"]).unwrap();
        store
            .upsert_rss_feed(&RssFeedInput {
                dir_id,
                feed_url: url,
                title: format!("Feed {i}"),
            })
            .unwrap();
    }
    let all = store.list_rss_feeds().unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn append_seen_guids_accumulates_and_dedups() {
    let (_tmp, store) = temp_store();
    let dir_id = store
        .bind_directory("rss:https://x.example.com/feed", false, &["rss"])
        .unwrap();
    store
        .upsert_rss_feed(&RssFeedInput {
            dir_id: dir_id.clone(),
            feed_url: "https://x.example.com/feed".into(),
            title: "X".into(),
        })
        .unwrap();

    store.append_seen_guids(&dir_id, &["g1".into(), "g2".into()]).unwrap();
    store.append_seen_guids(&dir_id, &["g2".into(), "g3".into()]).unwrap();

    let row = store.get_rss_feed(&dir_id).unwrap().unwrap();
    let mut guids = row.seen_guids.clone();
    guids.sort();
    assert_eq!(guids, vec!["g1", "g2", "g3"], "累积且去重，g2 不重复");
}

#[test]
fn touch_rss_feed_sets_last_fetched() {
    let (_tmp, store) = temp_store();
    let dir_id = store
        .bind_directory("rss:https://t.example.com/feed", false, &["rss"])
        .unwrap();
    store
        .upsert_rss_feed(&RssFeedInput {
            dir_id: dir_id.clone(),
            feed_url: "https://t.example.com/feed".into(),
            title: "T".into(),
        })
        .unwrap();
    store.touch_rss_feed_fetched(&dir_id).unwrap();
    let row = store.get_rss_feed(&dir_id).unwrap().unwrap();
    assert!(row.last_fetched.is_some(), "touch 后 last_fetched 应有值");
}

#[test]
fn delete_rss_feed_removes_row() {
    let (_tmp, store) = temp_store();
    let dir_id = store
        .bind_directory("rss:https://d.example.com/feed", false, &["rss"])
        .unwrap();
    store
        .upsert_rss_feed(&RssFeedInput {
            dir_id: dir_id.clone(),
            feed_url: "https://d.example.com/feed".into(),
            title: "D".into(),
        })
        .unwrap();
    store.delete_rss_feed(&dir_id).unwrap();
    assert!(store.get_rss_feed(&dir_id).unwrap().is_none(), "删除后查不到");
}
```

注：`Store::open` 的实际签名与构造方式以 `crates/attune-core/src/store/mod.rs` 为准 —— 若 `open` 需要额外参数（如加密 key），参照同目录已有集成测试（如 `webdav_remotes_test.rs`）的 `temp_store` 写法对齐，结构不变。

- [ ] **Step 3: 跑测试确认失败**

Run: `cd rust && cargo test -p attune-core --test rss_feeds_test 2>&1 | tail -5`
Expected: 编译失败 —— `attune_core::store::rss_feeds` / `RssFeedInput` 未定义。

- [ ] **Step 4: 创建 `store/rss_feeds.rs`**

创建 `rust/crates/attune-core/src/store/rss_feeds.rs`：

```rust
//! RSS feed 订阅持久化。
//!
//! 每个 feed 对应一条 `bound_dirs`（`rss:` 前缀 path）+ 一条 `rss_feeds`。
//! RSS 通常无凭据 —— 此表**不做字段加密**（与 `webdav_remotes.password_enc`
//! 不同），但 CRUD 形态参照 `webdav_remotes.rs`。`seen_guids` 是已入库 entry
//! GUID 的 JSON 数组，作增量去重游标。

use rusqlite::params;

use crate::error::{Result, VaultError};
use crate::store::Store;

/// 写入用的 RSS feed 配置（明文，调用方持有）。
#[derive(Debug, Clone)]
pub struct RssFeedInput {
    /// 关联的 bound_dirs.id。
    pub dir_id: String,
    /// feed XML 地址（UNIQUE）。
    pub feed_url: String,
    /// 展示标题。
    pub title: String,
}

/// 从表里读出的 RSS feed 配置。
#[derive(Debug, Clone)]
pub struct RssFeedRow {
    pub dir_id: String,
    pub feed_url: String,
    pub title: String,
    /// 已入库 entry 的 GUID 集（JSON 数组解码后）。
    pub seen_guids: Vec<String>,
    pub last_fetched: Option<String>,
}

impl Store {
    /// upsert 一条 RSS feed 配置。同 `dir_id` 已存在则更新 url/title
    /// （**保留** seen_guids / last_fetched —— 重新绑定不丢增量游标）。
    pub fn upsert_rss_feed(&self, input: &RssFeedInput) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO rss_feeds (dir_id, feed_url, title, seen_guids, last_fetched, created_at)
             VALUES (?1, ?2, ?3, '[]', NULL, ?4)
             ON CONFLICT(dir_id) DO UPDATE SET
                feed_url = excluded.feed_url,
                title = excluded.title",
            params![input.dir_id, input.feed_url, input.title, now],
        )?;
        Ok(())
    }

    /// 读单条 RSS feed 配置。
    pub fn get_rss_feed(&self, dir_id: &str) -> Result<Option<RssFeedRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, feed_url, title, seen_guids, last_fetched
             FROM rss_feeds WHERE dir_id = ?1",
        )?;
        let row = stmt
            .query_row(params![dir_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .ok();
        Ok(row.map(|(dir_id, feed_url, title, seen_json, last_fetched)| RssFeedRow {
            dir_id,
            feed_url,
            title,
            seen_guids: decode_guids(&seen_json),
            last_fetched,
        }))
    }

    /// 列出全部 RSS feed 配置（周期 worker / UI 列表用）。
    pub fn list_rss_feeds(&self) -> Result<Vec<RssFeedRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, feed_url, title, seen_guids, last_fetched
             FROM rss_feeds ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (dir_id, feed_url, title, seen_json, last_fetched) = row?;
            out.push(RssFeedRow {
                dir_id,
                feed_url,
                title,
                seen_guids: decode_guids(&seen_json),
                last_fetched,
            });
        }
        Ok(out)
    }

    /// 把新见到的 GUID 并入 seen_guids（累积 + 去重）。
    pub fn append_seen_guids(&self, dir_id: &str, new_guids: &[String]) -> Result<()> {
        if new_guids.is_empty() {
            return Ok(());
        }
        let existing = self
            .get_rss_feed(dir_id)?
            .map(|r| r.seen_guids)
            .unwrap_or_default();
        let mut set: std::collections::BTreeSet<String> = existing.into_iter().collect();
        for g in new_guids {
            set.insert(g.clone());
        }
        let merged: Vec<String> = set.into_iter().collect();
        let json = serde_json::to_string(&merged)
            .map_err(|e| VaultError::InvalidInput(format!("seen_guids encode: {e}")))?;
        self.conn.execute(
            "UPDATE rss_feeds SET seen_guids = ?1 WHERE dir_id = ?2",
            params![json, dir_id],
        )?;
        Ok(())
    }

    /// 记录某 feed 最近一次拉取时间。
    pub fn touch_rss_feed_fetched(&self, dir_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE rss_feeds SET last_fetched = ?1 WHERE dir_id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    /// 删除一条 RSS feed 配置（bound_dirs 由 caller 另行 unbind）。
    pub fn delete_rss_feed(&self, dir_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM rss_feeds WHERE dir_id = ?1", params![dir_id])?;
        Ok(())
    }
}

/// 解码 seen_guids JSON 数组；畸形 JSON 视为空集（不致命）。
fn decode_guids(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cd rust && cargo test -p attune-core --test rss_feeds_test 2>&1 | tail -10`
Expected: 全部 5 个测试 PASS。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-core/src/store/rss_feeds.rs rust/crates/attune-core/src/store/mod.rs rust/crates/attune-core/tests/rss_feeds_test.rs
git commit -m "$(cat <<'EOF'
feat(store): rss_feeds table with seen-GUID incremental cursor

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5：`sync_rss_feed` —— route 与 worker 共用的拉取入库逻辑

**Files:**
- Create: `rust/crates/attune-server/src/ingest_rss.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`（加 `mod ingest_rss;`）
- Test: `rust/crates/attune-server/tests/ingest_rss_sync_test.rs`

- [ ] **Step 1: 写失败测试**

创建 `rust/crates/attune-server/tests/ingest_rss_sync_test.rs`。该测试用一个本地 HTTP server 喂固定 feed XML，验证「首次拉取全入库 / 二次拉取增量」：

```rust
//! sync_rss_feed 集成测试 —— 本地 HTTP server 喂固定 feed，验证增量。

use std::sync::Arc;

use attune_server::test_support::unlocked_app_state;
use axum::http::StatusCode;

const FEED_V1: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Local Test Feed</title>
  <item><title>Post A</title><link>http://feed.local/a</link>
    <guid>http://feed.local/a</guid><description>Content A here.</description></item>
</channel></rss>"#;

const FEED_V2: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Local Test Feed</title>
  <item><title>Post A</title><link>http://feed.local/a</link>
    <guid>http://feed.local/a</guid><description>Content A here.</description></item>
  <item><title>Post B</title><link>http://feed.local/b</link>
    <guid>http://feed.local/b</guid><description>Content B here.</description></item>
</channel></rss>"#;

/// 起一个一次性 HTTP server，按调用次数先后返回 v1 然后 v2。
async fn spawn_feed_server() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/feed",
        axum::routing::get(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    ([("content-type", "application/rss+xml")], FEED_V1)
                } else {
                    ([("content-type", "application/rss+xml")], FEED_V2)
                }
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/feed")
}

#[tokio::test]
async fn sync_rss_feed_first_then_incremental() {
    let state = unlocked_app_state().await;
    let feed_url = spawn_feed_server().await;

    // 绑定 feed → bound_dir + rss_feeds 行。
    let dir_id = {
        let vault = state.vault.lock().unwrap();
        let store = vault.store();
        let dir_id = store
            .bind_directory(&format!("rss:{feed_url}"), false, &["rss"])
            .unwrap();
        store
            .upsert_rss_feed(&attune_core::store::rss_feeds::RssFeedInput {
                dir_id: dir_id.clone(),
                feed_url: feed_url.clone(),
                title: "Local Test Feed".into(),
            })
            .unwrap();
        dir_id
    };

    // 首次同步：1 条 entry 入库。
    let state_c = state.clone();
    let dir_c = dir_id.clone();
    let r1 = tokio::task::spawn_blocking(move || {
        attune_server::ingest_rss::sync_rss_feed(&state_c, &dir_c)
    })
    .await
    .unwrap()
    .expect("first sync ok");
    assert_eq!(r1["new_entries"].as_u64(), Some(1), "首次拉到 1 条");

    // 二次同步：v2 多 1 条，已见的 a 跳过。
    let state_c = state.clone();
    let dir_c = dir_id.clone();
    let r2 = tokio::task::spawn_blocking(move || {
        attune_server::ingest_rss::sync_rss_feed(&state_c, &dir_c)
    })
    .await
    .unwrap()
    .expect("second sync ok");
    assert_eq!(r2["new_entries"].as_u64(), Some(1), "二次只拉到新的 b");
    assert_eq!(r2["skipped_entries"].as_u64(), Some(1), "a 被增量跳过");

    let _ = StatusCode::OK; // 保留 import
}
```

注：`attune_server::test_support::unlocked_app_state` 的实际名称/路径以 `crates/attune-server/tests/` 现有测试为准 —— 若不存在该 helper，参照 `webdav` 或 `upload` 相关集成测试如何构造已解锁 `Arc<AppState>`，对齐结构。`vault.store()` 取 `Store` 引用、`vault.dek_db()` 取 `dek` 的用法见 `ingest_webdav.rs`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd rust && cargo test -p attune-server --test ingest_rss_sync_test 2>&1 | tail -5`
Expected: 编译失败 —— `attune_server::ingest_rss` 未定义。

- [ ] **Step 3: 创建 `ingest_rss.rs`**

创建 `rust/crates/attune-server/src/ingest_rss.rs`：

```rust
//! RSS 增量同步 —— refresh route 与周期 worker 共用的拉取入库逻辑。
//!
//! 设计与 `ingest_webdav::sync_webdav_dir` 对齐：网络拉取**锁外**做，每个
//! entry 的 DB 写才短暂拿 vault 锁。增量由 `rss_feeds.seen_guids` 驱动 ——
//! `RssConnector` 已在 `fetch_documents` 内跳过已见 GUID，本函数把本轮新见
//! 的 GUID 并回表。

use std::sync::Arc;

use attune_core::ingest::{ingest_document, DocumentSink, IngestOutcome, RawDocument};
use attune_core::ingest::rss::{RssConfig, RssConnector};
use attune_core::ingest::SourceConnector;

use crate::state::AppState;

/// 对一个 RSS feed 做一次增量拉取 + 入库。
///
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
/// 返回 `{total_entries, new_entries, skipped_entries, errors}`。
pub fn sync_rss_feed(state: &Arc<AppState>, dir_id: &str) -> Result<serde_json::Value, String> {
    // 读 feed 配置 + 增量游标（snapshot 后释放锁）。
    let feed_row = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .store()
            .get_rss_feed(dir_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("rss feed not found: {dir_id}"))?
    };

    let config = RssConfig {
        feed_url: feed_row.feed_url.clone(),
        seen_entry_ids: feed_row.seen_guids.clone(),
    };
    let connector = RssConnector::new(config);

    // 阶段 1：锁外做网络拉取 + 解析（connector 已跳过已见 GUID）。
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector
            .fetch_documents(&mut sink)
            .map_err(|e| e.to_string())?;
    }

    // 阶段 2：逐 entry 短暂持锁入库，写完即 drop guard。
    let mut total = 0usize;
    let mut new_entries = 0usize;
    let mut skipped_entries = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut fresh_guids: Vec<String> = Vec::new();

    for doc in docs {
        total += 1;
        let guid = doc.source_ref.clone();

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(e) => {
                errors.push(format!("{guid}: vault locked: {e}"));
                continue;
            }
        };
        let store = vault.store();

        // RSS entry 不可变（已发布的文章不改 GUID）—— 不做 replacing，
        // content_hash 短路若碰撞也只是 Duplicate，不重复入库。
        match ingest_document(store, &dek, &doc) {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                // indexed_files 以 GUID 作 path 键，记录 entry → item 映射。
                let _ = store.upsert_indexed_file(dir_id, &guid, &guid, &item_id);
                fresh_guids.push(guid);
                new_entries += 1;
            }
            Ok(IngestOutcome::Duplicate { .. }) | Ok(IngestOutcome::Skipped { .. }) => {
                // 内容重复或空 —— 仍记入 seen，避免下轮反复尝试。
                fresh_guids.push(guid);
                skipped_entries += 1;
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &guid, &guid, &item_id);
                fresh_guids.push(guid);
                new_entries += 1;
            }
            Err(e) => {
                errors.push(format!("{guid}: ingest {e}"));
            }
        }
        // vault guard 在此隐式 drop。
    }

    // 把本轮新见 GUID 并回增量游标 + 记录拉取时间（best-effort）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        let _ = store.append_seen_guids(dir_id, &fresh_guids);
        let _ = store.touch_rss_feed_fetched(dir_id);
    }

    Ok(serde_json::json!({
        "total_entries": total,
        "new_entries": new_entries,
        "skipped_entries": skipped_entries,
        "errors": errors,
    }))
}
```

在 `rust/crates/attune-server/src/lib.rs` 的模块声明区加（与 `mod ingest_webdav;` 相邻）：

```rust
pub mod ingest_rss;
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd rust && cargo test -p attune-server --test ingest_rss_sync_test 2>&1 | tail -10`
Expected: `sync_rss_feed_first_then_incremental` PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/ingest_rss.rs rust/crates/attune-server/src/lib.rs rust/crates/attune-server/tests/ingest_rss_sync_test.rs
git commit -m "$(cat <<'EOF'
feat(server): sync_rss_feed shared by refresh route and worker

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6：HTTP API —— feed CRUD + OPML 上传 + 手动刷新

**Files:**
- Create: `rust/crates/attune-server/src/routes/rss.rs`
- Modify: `rust/crates/attune-server/src/routes/mod.rs`（加 `pub mod rss;`）
- Modify: `rust/crates/attune-server/src/lib.rs`（注册 5 条 route）
- Test: `rust/crates/attune-server/tests/rss_api_test.rs`

- [ ] **Step 1: 写失败测试**

创建 `rust/crates/attune-server/tests/rss_api_test.rs`：

```rust
//! RSS HTTP API 形态契约测试 —— feed CRUD + OPML 上传响应形态。

use attune_server::test_support::{post_json, unlocked_app};
use axum::http::StatusCode;

#[tokio::test]
async fn list_feeds_empty_initially() {
    let app = unlocked_app().await;
    let (status, body) = app.get_json("/api/v1/rss/feeds").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["feeds"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn add_feed_rejects_non_http_url() {
    let app = unlocked_app().await;
    let (status, body) = post_json(
        &app,
        "/api/v1/rss/feeds",
        serde_json::json!({"url": "ftp://bad.example.com/feed"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad-request");
}

#[tokio::test]
async fn add_then_list_then_delete_feed() {
    let app = unlocked_app().await;
    // add —— 用一个不可达 URL（首次拉取允许失败，feed 仍登记）。
    let (status, body) = post_json(
        &app,
        "/api/v1/rss/feeds",
        serde_json::json!({"url": "https://unreachable.invalid/feed.xml"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let dir_id = body["dir_id"].as_str().expect("dir_id returned").to_string();
    assert_eq!(body["status"], "ok");

    // list —— 应含刚加的 feed。
    let (status, body) = app.get_json("/api/v1/rss/feeds").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["feeds"].as_array().map(|a| a.len()), Some(1));
    assert_eq!(body["feeds"][0]["feed_url"], "https://unreachable.invalid/feed.xml");

    // delete —— 200 且再 list 为空。
    let (status, _) = app
        .delete_json(&format!("/api/v1/rss/feeds/{dir_id}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = app.get_json("/api/v1/rss/feeds").await;
    assert_eq!(body["feeds"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn import_opml_creates_multiple_feeds() {
    let app = unlocked_app().await;
    let opml = r#"<?xml version="1.0"?><opml version="2.0"><body>
      <outline type="rss" text="A" xmlUrl="https://a.invalid/feed"/>
      <outline type="rss" text="B" xmlUrl="https://b.invalid/feed"/>
    </body></opml>"#;
    let (status, body) = app
        .post_multipart_file("/api/v1/rss/import-opml", "subs.opml", opml.as_bytes())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"].as_u64(), Some(2), "两个 feed 都登记");

    let (_, list) = app.get_json("/api/v1/rss/feeds").await;
    assert_eq!(list["feeds"].as_array().map(|a| a.len()), Some(2));
}
```

注：`test_support` 的 helper 名（`unlocked_app` / `post_json` / `get_json` / `delete_json` / `post_multipart_file`）以 `crates/attune-server/tests/` 现有测试为准。若缺 `post_multipart_file` / `delete_json`，参照 webdav `bind-remote` 测试 + upload multipart 测试现有 helper 对齐；缺失则在 `test_support` 补一个最小 helper（不算 placeholder —— 测试基建对齐）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd rust && cargo test -p attune-server --test rss_api_test 2>&1 | tail -5`
Expected: 编译失败 —— RSS route 未定义。

- [ ] **Step 3: 创建 `routes/rss.rs`**

创建 `rust/crates/attune-server/src/routes/rss.rs`：

```rust
//! RSS 订阅 HTTP API：feed CRUD + OPML 导入 + 手动刷新。
//!
//! feed 落 `bound_dirs`（`rss:` 前缀 path）+ `rss_feeds` 表。增量拉取入库
//! 逻辑在 `crate::ingest_rss::sync_rss_feed`，refresh route 与周期 worker 共用。

use std::sync::Arc;

use attune_core::ingest::rss::parse_opml;
use attune_core::store::rss_feeds::RssFeedInput;
use axum::extract::{Multipart, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// `POST /api/v1/rss/feeds` 请求体。
#[derive(Deserialize)]
pub struct AddFeedRequest {
    /// feed XML 地址。
    pub url: String,
    /// 可选展示标题（缺省时首次拉取后用 feed channel 标题回填）。
    pub title: Option<String>,
}

/// 校验 feed URL 是 http(s)。RSS feed 永远是 HTTP 资源。
fn validate_feed_url(url: &str) -> AppResult<()> {
    let u = url.trim();
    if u.starts_with("http://") || u.starts_with("https://") {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "feed url must be http(s): {url}"
        )))
    }
}

/// 登记一个 feed：建 bound_dir + rss_feeds 行。返回 dir_id。
/// 不在此处拉取 —— 首次拉取由 caller 显式触发（或周期 worker 兜底）。
fn register_feed(state: &SharedState, url: &str, title: &str) -> AppResult<String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?; // 确认 vault 已解锁。
    let store = vault.store();
    let dir_id = store
        .bind_directory(&format!("rss:{url}"), false, &["rss"])
        .map_err(AppError::from)?;
    store
        .upsert_rss_feed(&RssFeedInput {
            dir_id: dir_id.clone(),
            feed_url: url.to_string(),
            title: title.to_string(),
        })
        .map_err(AppError::from)?;
    Ok(dir_id)
}

/// `GET /api/v1/rss/feeds` —— 列出全部 feed。
pub async fn list_feeds(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    let feeds = vault.store().list_rss_feeds().map_err(AppError::from)?;
    let arr: Vec<serde_json::Value> = feeds
        .iter()
        .map(|f| {
            json!({
                "dir_id": f.dir_id,
                "feed_url": f.feed_url,
                "title": f.title,
                "entry_count": f.seen_guids.len(),
                "last_fetched": f.last_fetched,
            })
        })
        .collect();
    Ok(Json(json!({ "feeds": arr })))
}

/// `POST /api/v1/rss/feeds` —— 添加一个 feed 并立即做首次拉取。
///
/// 首次拉取失败（URL 不可达 / 非法 XML）**不**回滚 feed 登记 —— feed 已登记，
/// 周期 worker 会重试；响应里带 `first_fetch_error` 让 UI 提示。
pub async fn add_feed(
    State(state): State<SharedState>,
    Json(body): Json<AddFeedRequest>,
) -> AppResult<Json<serde_json::Value>> {
    validate_feed_url(&body.url)?;
    let url = body.url.trim().to_string();
    let title = body.title.clone().unwrap_or_default();
    let dir_id = register_feed(&state, &url, &title)?;

    // 首次拉取在 spawn_blocking 里跑（sync_rss_feed 是阻塞函数）。
    let state_c = state.clone();
    let dir_c = dir_id.clone();
    let fetch = tokio::task::spawn_blocking(move || {
        crate::ingest_rss::sync_rss_feed(&state_c, &dir_c)
    })
    .await
    .map_err(|e| AppError::Internal(format!("rss fetch join: {e}")))?;

    match fetch {
        Ok(scan) => Ok(Json(json!({
            "status": "ok",
            "dir_id": dir_id,
            "scan": scan,
        }))),
        Err(e) => Ok(Json(json!({
            "status": "ok",
            "dir_id": dir_id,
            "first_fetch_error": e,
        }))),
    }
}

/// `DELETE /api/v1/rss/feeds/{dir_id}` —— 删除一个 feed 订阅。
/// 已入库的 entry item 保留（与 webdav unbind 语义一致）。
pub async fn delete_feed(
    State(state): State<SharedState>,
    Path(dir_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    let store = vault.store();
    store.delete_rss_feed(&dir_id).map_err(AppError::from)?;
    // bound_dir 一并解绑（rss_feeds 行已删，ON DELETE CASCADE 反向不触发）。
    let _ = store.unbind_directory(&dir_id);
    Ok(Json(json!({ "status": "ok" })))
}

/// `POST /api/v1/rss/feeds/{dir_id}/refresh` —— 手动立即重抓一个 feed。
pub async fn refresh_feed(
    State(state): State<SharedState>,
    Path(dir_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let state_c = state.clone();
    let dir_c = dir_id.clone();
    let scan = tokio::task::spawn_blocking(move || {
        crate::ingest_rss::sync_rss_feed(&state_c, &dir_c)
    })
    .await
    .map_err(|e| AppError::Internal(format!("rss refresh join: {e}")))?
    .map_err(AppError::BadGateway)?;
    Ok(Json(json!({ "status": "ok", "scan": scan })))
}

/// `POST /api/v1/rss/import-opml` —— multipart 上传 OPML，批量登记 feed。
///
/// 每个 feed 各登记一条 bound_dir，但**不**在此逐个拉取（可能几十个 feed，
/// 同步拉会很慢）—— 登记后交给周期 worker 首轮兜底。
pub async fn import_opml(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> AppResult<Json<serde_json::Value>> {
    // 取第一个文件字段。
    let field = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
        .ok_or_else(|| AppError::BadRequest("no file in multipart".into()))?;
    let bytes = field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart body: {e}")))?;
    let opml_text = String::from_utf8_lossy(&bytes);

    let feeds = parse_opml(&opml_text).map_err(AppError::from)?;
    let mut imported = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for f in feeds {
        if validate_feed_url(&f.xml_url).is_err() {
            skipped.push(f.xml_url);
            continue;
        }
        match register_feed(&state, &f.xml_url, &f.title) {
            Ok(_) => imported += 1,
            Err(e) => skipped.push(format!("{}: {e}", f.xml_url)),
        }
    }
    Ok(Json(json!({
        "status": "ok",
        "imported": imported,
        "skipped": skipped,
    })))
}

/// 用于 `lib.rs` 路由装配 —— 把 `mut multipart` 等签名约束保持在本文件内。
pub fn _route_marker(_: &Arc<crate::state::AppState>) {}
```

注：`Store::unbind_directory` 的实际方法名以 `store/dirs.rs` 为准（webdav unbind route 用的同一个），若名为 `delete_bound_dir` 等按实际校正。`_route_marker` 仅占位说明用，实现期若不需要可删 —— 不影响其它代码。

- [ ] **Step 4: 在 `routes/mod.rs` + `lib.rs` 注册**

`rust/crates/attune-server/src/routes/mod.rs` 加（与其它 `pub mod` 并列）：

```rust
pub mod rss;
```

`rust/crates/attune-server/src/lib.rs` 在 `/api/v1/index/*` route 注册区附近追加 5 条（`use axum::routing::{get, post, delete};` 已在文件内）：

```rust
        .route("/api/v1/rss/feeds", get(routes::rss::list_feeds))
        .route("/api/v1/rss/feeds", post(routes::rss::add_feed))
        .route("/api/v1/rss/feeds/{dir_id}", delete(routes::rss::delete_feed))
        .route("/api/v1/rss/feeds/{dir_id}/refresh", post(routes::rss::refresh_feed))
        .route("/api/v1/rss/import-opml", post(routes::rss::import_opml))
```

注：Axum 0.8 path 参数语法是 `{dir_id}`（不是 0.7 的 `:dir_id`）—— 与现有 route 写法对齐确认。

- [ ] **Step 5: 跑测试确认通过**

Run: `cd rust && cargo test -p attune-server --test rss_api_test 2>&1 | tail -12`
Expected: 4 个测试全 PASS。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-server/src/routes/rss.rs rust/crates/attune-server/src/routes/mod.rs rust/crates/attune-server/src/lib.rs rust/crates/attune-server/tests/rss_api_test.rs
git commit -m "$(cat <<'EOF'
feat(server): RSS HTTP API — feed CRUD + OPML import + refresh

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7：后台周期拉取 worker

**Files:**
- Modify: `rust/crates/attune-server/src/state.rs`（加 `rss_sync_worker_running` 字段 + `start_rss_sync_worker`）
- Modify: `rust/crates/attune-server/src/lib.rs`（vault unlock 后启动 worker）

- [ ] **Step 1: 写失败测试**

在 `rust/crates/attune-server/src/state.rs` 的 `#[cfg(test)] mod tests` 里追加（参照已有的 `webdav_sync_worker_flag_prevents_double_start`）：

```rust
    #[test]
    fn rss_sync_worker_flag_prevents_double_start() {
        // 原子 flag：已在跑时再 start 应直接返回，不起第二个线程。
        let state = test_app_state();
        assert!(!state.rss_sync_worker_running.load(Ordering::SeqCst));
        state.rss_sync_worker_running.store(true, Ordering::SeqCst);
        // flag 为 true 时 start 应是 no-op（compare_exchange 失败）。
        AppState::start_rss_sync_worker(state.clone());
        assert!(state.rss_sync_worker_running.load(Ordering::SeqCst));
    }
```

注：`test_app_state()` helper 以 `state.rs` 现有 `mod tests` 里 `webdav_sync_worker_flag_prevents_double_start` 用的同一个为准。

- [ ] **Step 2: 跑测试确认失败**

Run: `cd rust && cargo test -p attune-server --lib rss_sync_worker 2>&1 | tail -5`
Expected: 编译失败 —— `rss_sync_worker_running` 字段 / `start_rss_sync_worker` 未定义。

- [ ] **Step 3: 在 `state.rs` 加字段 + worker**

在 `AppState` 结构体里，紧跟 `webdav_sync_worker_running` 字段加：

```rust
    /// RSS 周期拉取 worker 运行标志（原子防重入）。
    pub rss_sync_worker_running: AtomicBool,
```

在 `AppState` 的构造函数（`new` / `Default` —— 以 `webdav_sync_worker_running: AtomicBool::new(false)` 所在处为准）里同位置加：

```rust
            rss_sync_worker_running: AtomicBool::new(false),
```

在 `start_webdav_sync_worker` 之后加一个对称的 `start_rss_sync_worker`：

```rust
    /// 启动 RSS 周期拉取 worker：每 30 分钟从 rss_feeds 表读全部 feed，
    /// 逐个增量重抓。原子 flag 防重入 + RAII guard 复位。
    ///
    /// 周期取 30 分钟（比 WebDAV 的 15 分钟长）—— feed 更新频率通常以小时
    /// 计，30 分钟足够及时且对源站点友好（避免过于频繁的 polling）。
    pub fn start_rss_sync_worker(state: std::sync::Arc<AppState>) {
        if state
            .rss_sync_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("RSS sync worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.rss_sync_worker_running);

            tracing::info!("RSS sync worker started");
            loop {
                // vault 锁定则退出 —— 下次 unlock 会重新 start。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                // 读全部 feed 的 dir_id（snapshot 后释放锁）。
                let dir_ids: Vec<String> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    vault
                        .store()
                        .list_rss_feeds()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| f.dir_id)
                        .collect()
                };

                for dir_id in dir_ids {
                    tracing::info!("RSS sync: refreshing feed dir={dir_id}");
                    if let Err(e) = crate::ingest_rss::sync_rss_feed(&state, &dir_id) {
                        tracing::warn!("RSS sync for dir {dir_id} failed: {e}");
                    }
                }

                // unlock 后立即跑首轮，之后每 30 分钟一次。
                std::thread::sleep(std::time::Duration::from_secs(30 * 60));
            }
            tracing::info!("RSS sync worker stopped (vault locked)");
        });
    }
```

- [ ] **Step 4: 在 `lib.rs` vault unlock 后启动 worker**

找到 `lib.rs` 里调用 `AppState::start_webdav_sync_worker` 的位置（vault unlock 成功后），紧随其后加：

```rust
            AppState::start_rss_sync_worker(state.clone());
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cd rust && cargo test -p attune-server --lib rss_sync_worker 2>&1 | tail -6`
Expected: `rss_sync_worker_flag_prevents_double_start` PASS。

Run: `cd rust && cargo build -p attune-server 2>&1 | tail -3`
Expected: `Finished`。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-server/src/state.rs rust/crates/attune-server/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(server): periodic RSS sync worker (30-min interval)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8：前端 RSS API 封装

**Files:**
- Create: `rust/crates/attune-server/ui/src/hooks/useRss.ts`

- [ ] **Step 1: 创建 `useRss.ts`**

创建 `rust/crates/attune-server/ui/src/hooks/useRss.ts`。形态对齐 `useRemote.ts`：

```typescript
/** useRss · RSS / Atom feed 订阅管理 */
import { api } from '../store/api';
import { ApiError } from '../store/api';

export type RssFeed = {
  dir_id: string;
  feed_url: string;
  title: string;
  entry_count: number;
  last_fetched?: string | null;
};

export type RssActionResult = {
  ok: boolean;
  error?: string;
};

type ListResponse = { feeds: RssFeed[] };

export async function listRssFeeds(): Promise<RssFeed[]> {
  try {
    const res = await api.get<ListResponse>('/rss/feeds');
    return res.feeds ?? [];
  } catch {
    return [];
  }
}

export async function addRssFeed(url: string, title?: string): Promise<RssActionResult> {
  try {
    await api.post('/rss/feeds', { url, title: title ?? '' });
    return { ok: true };
  } catch (e: unknown) {
    return toResult(e);
  }
}

export async function deleteRssFeed(dirId: string): Promise<boolean> {
  try {
    await api.delete(`/rss/feeds/${encodeURIComponent(dirId)}`);
    return true;
  } catch {
    return false;
  }
}

export async function refreshRssFeed(dirId: string): Promise<RssActionResult> {
  try {
    await api.post(`/rss/feeds/${encodeURIComponent(dirId)}/refresh`, {});
    return { ok: true };
  } catch (e: unknown) {
    return toResult(e);
  }
}

/** OPML 文件上传 —— multipart/form-data。 */
export async function importOpml(file: File): Promise<{ ok: boolean; imported?: number; error?: string }> {
  try {
    const form = new FormData();
    form.append('file', file);
    const res = await api.postForm<{ imported: number }>('/rss/import-opml', form);
    return { ok: true, imported: res.imported };
  } catch (e: unknown) {
    const r = toResult(e);
    return { ok: false, error: r.error };
  }
}

function toResult(e: unknown): RssActionResult {
  if (e instanceof ApiError) {
    return { ok: false, error: extractErrorMessage(e.body) };
  }
  return { ok: false, error: e instanceof Error ? e.message : String(e) };
}

function extractErrorMessage(body: string): string {
  try {
    const parsed = JSON.parse(body) as { error?: string };
    return parsed.error?.trim() || body;
  } catch {
    return body;
  }
}
```

注：`api.postForm` 是 multipart 上传方法 —— 以 `store/api.ts` 实际 API 为准。若 `api` 无 `postForm`，参照 SidePanel / 文件上传现有 multipart 调用方式（`fetch` + `FormData` 直发），结构不变。

- [ ] **Step 2: 验证 TypeScript 编译**

Run: `cd rust/crates/attune-server/ui && npx tsc --noEmit 2>&1 | tail -5`
Expected: 无 error（或仅与本文件无关的既有 error）。

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-server/ui/src/hooks/useRss.ts
git commit -m "$(cat <<'EOF'
feat(ui): RSS feed API client hook

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9：RemoteView 加 RSS 管理 UI + i18n

**Files:**
- Modify: `rust/crates/attune-server/ui/src/views/RemoteView.tsx`
- Modify: `rust/crates/attune-server/ui/src/i18n/zh.ts`
- Modify: `rust/crates/attune-server/ui/src/i18n/en.ts`

- [ ] **Step 1: 在 `zh.ts` 加 `rss.*` key**

在 `rust/crates/attune-server/ui/src/i18n/zh.ts` 的 `remote.*` key 区块之后追加：

```typescript
  'rss.action.add': '添加 RSS',
  'rss.action.import_opml': '导入 OPML',
  'rss.section.title': 'RSS 订阅',
  'rss.empty.desc': '订阅博客 / 新闻 RSS，Attune 会定期抓取文章索引进知识库',
  'rss.modal.add.title': '添加 RSS 订阅',
  'rss.modal.import.title': '从 OPML 导入订阅',
  'rss.field.url_label': 'Feed 地址',
  'rss.field.url_placeholder': '例：https://blog.rust-lang.org/feed.xml',
  'rss.field.url_hint': 'RSS / Atom feed 的 XML 地址',
  'rss.field.title_label': '标题（可选）',
  'rss.field.title_placeholder': '留空则用 feed 自带标题',
  'rss.field.opml_label': 'OPML 文件',
  'rss.field.opml_hint': '从 Feedly / Miniflux 等阅读器导出的 .opml 文件',
  'rss.action.add_submit': '添加',
  'rss.action.import_submit': '导入',
  'rss.action.refresh': '立即刷新',
  'rss.row.entry_count': '已索引 {count} 篇',
  'rss.row.last_fetched': '上次抓取',
  'rss.row.never_fetched': '尚未抓取',
  'rss.row.delete': '取消订阅',
  'rss.confirm.delete': '取消订阅 {title}？已索引的文章保留。',
  'rss.toast.add_success': '已添加，开始首次抓取',
  'rss.toast.add_fail': '添加失败：{error}',
  'rss.toast.delete_success': '已取消订阅',
  'rss.toast.delete_fail': '取消订阅失败',
  'rss.toast.refresh_success': '已刷新',
  'rss.toast.refresh_fail': '刷新失败：{error}',
  'rss.toast.import_success': '已导入 {count} 个订阅',
  'rss.toast.import_fail': 'OPML 导入失败：{error}',
  'rss.toast.no_file': '请选择 OPML 文件',
```

- [ ] **Step 2: 在 `en.ts` 加同名 `rss.*` key**

在 `rust/crates/attune-server/ui/src/i18n/en.ts` 对应位置追加（key 集合必须与 zh 完全一致）：

```typescript
  'rss.action.add': 'Add RSS',
  'rss.action.import_opml': 'Import OPML',
  'rss.section.title': 'RSS Feeds',
  'rss.empty.desc': 'Subscribe to blog / news RSS — Attune fetches articles into your knowledge base periodically',
  'rss.modal.add.title': 'Add RSS Feed',
  'rss.modal.import.title': 'Import Feeds from OPML',
  'rss.field.url_label': 'Feed URL',
  'rss.field.url_placeholder': 'e.g. https://blog.rust-lang.org/feed.xml',
  'rss.field.url_hint': 'XML address of an RSS / Atom feed',
  'rss.field.title_label': 'Title (optional)',
  'rss.field.title_placeholder': 'Leave blank to use the feed's own title',
  'rss.field.opml_label': 'OPML File',
  'rss.field.opml_hint': 'An .opml file exported from Feedly / Miniflux etc.',
  'rss.action.add_submit': 'Add',
  'rss.action.import_submit': 'Import',
  'rss.action.refresh': 'Refresh now',
  'rss.row.entry_count': '{count} indexed',
  'rss.row.last_fetched': 'Last fetched',
  'rss.row.never_fetched': 'Not fetched yet',
  'rss.row.delete': 'Unsubscribe',
  'rss.confirm.delete': 'Unsubscribe from {title}? Indexed articles are kept.',
  'rss.toast.add_success': 'Added — first fetch started',
  'rss.toast.add_fail': 'Add failed: {error}',
  'rss.toast.delete_success': 'Unsubscribed',
  'rss.toast.delete_fail': 'Unsubscribe failed',
  'rss.toast.refresh_success': 'Refreshed',
  'rss.toast.refresh_fail': 'Refresh failed: {error}',
  'rss.toast.import_success': 'Imported {count} feeds',
  'rss.toast.import_fail': 'OPML import failed: {error}',
  'rss.toast.no_file': 'Please select an OPML file',
```

- [ ] **Step 3: 在 `RemoteView.tsx` 加 RSS 区块**

修改 `rust/crates/attune-server/ui/src/views/RemoteView.tsx`。

(a) 顶部 import 区追加：

```typescript
import {
  listRssFeeds,
  addRssFeed,
  deleteRssFeed,
  refreshRssFeed,
  importOpml,
} from '../hooks/useRss';
import type { RssFeed } from '../hooks/useRss';
```

(b) `RemoteView` 函数体内，`modal` signal 旁加 RSS 状态 + 加载逻辑：

```typescript
  const feeds = useSignal<RssFeed[]>([]);
  const rssModal = useSignal<null | 'add' | 'import'>(null);
```

把 `modal` 的类型扩成 `useSignal<null | 'local' | 'webdav'>(null)` 保持不变 —— RSS 用独立的 `rssModal` 避免互相干扰。

(c) `refresh()` 函数体内追加 feed 加载：

```typescript
  async function refresh() {
    loading.value = true;
    dirs.value = await listBoundDirs();
    feeds.value = await listRssFeeds();
    loading.value = false;
  }
```

(d) header 的按钮组里，`webdav` 按钮之后加 RSS 按钮：

```typescript
          <Button variant="secondary" size="sm" onClick={() => (rssModal.value = 'add')}>
            {`📡 ${t('rss.action.add')}`}
          </Button>
```

(e) 在 `dirs.value.map(...)` 列表区之后、`</div>` 主容器闭合之前，加 RSS feed 列表区块：

```typescript
      <section style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <h3 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}>
            {`📡 ${t('rss.section.title')}`}
          </h3>
          <Button variant="ghost" size="sm" onClick={() => (rssModal.value = 'import')}>
            {t('rss.action.import_opml')}
          </Button>
        </div>
        {feeds.value.length === 0 ? (
          <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
            {t('rss.empty.desc')}
          </div>
        ) : (
          feeds.value.map((f) => (
            <FeedRow
              key={f.dir_id}
              feed={f}
              onRefresh={async () => {
                const r = await refreshRssFeed(f.dir_id);
                if (r.ok) {
                  toast('success', t('rss.toast.refresh_success'));
                  await refresh();
                } else {
                  toast('error', t('rss.toast.refresh_fail', { error: r.error ?? '' }));
                }
              }}
              onDelete={async () => {
                if (!confirm(t('rss.confirm.delete', { title: f.title || f.feed_url }))) return;
                const ok = await deleteRssFeed(f.dir_id);
                if (ok) {
                  toast('success', t('rss.toast.delete_success'));
                  await refresh();
                } else {
                  toast('error', t('rss.toast.delete_fail'));
                }
              }}
            />
          ))
        )}
      </section>

      <Modal
        open={rssModal.value === 'add'}
        onClose={() => (rssModal.value = null)}
        title={t('rss.modal.add.title')}
      >
        <AddFeedForm
          onDone={async (result) => {
            rssModal.value = null;
            if (result.ok) {
              toast('success', t('rss.toast.add_success'));
              await refresh();
            } else {
              toast('error', t('rss.toast.add_fail', { error: result.error ?? '' }));
            }
          }}
        />
      </Modal>

      <Modal
        open={rssModal.value === 'import'}
        onClose={() => (rssModal.value = null)}
        title={t('rss.modal.import.title')}
      >
        <ImportOpmlForm
          onDone={async (result) => {
            rssModal.value = null;
            if (result.ok) {
              toast('success', t('rss.toast.import_success', { count: String(result.imported ?? 0) }));
              await refresh();
            } else if (result.error) {
              toast('error', t('rss.toast.import_fail', { error: result.error }));
            }
          }}
        />
      </Modal>
```

(f) 文件末尾追加三个组件 —— `FeedRow` / `AddFeedForm` / `ImportOpmlForm`：

```typescript
function FeedRow({
  feed: f,
  onRefresh,
  onDelete,
}: {
  feed: RssFeed;
  onRefresh: () => void;
  onDelete: () => void;
}): JSX.Element {
  return (
    <div
      style={{
        padding: 'var(--space-3) var(--space-4)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-3)',
      }}
    >
      <span aria-hidden="true" style={{ fontSize: 20 }}>
        📡
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text)',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {f.title || f.feed_url}
        </div>
        <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 2 }}>
          {t('rss.row.entry_count', { count: String(f.entry_count) })}
          {' · '}
          {f.last_fetched
            ? `${t('rss.row.last_fetched')}: ${new Date(f.last_fetched).toLocaleString()}`
            : t('rss.row.never_fetched')}
        </div>
      </div>
      <Button variant="ghost" size="sm" onClick={onRefresh}>
        {t('rss.action.refresh')}
      </Button>
      <Button variant="ghost" size="sm" onClick={onDelete}>
        {t('rss.row.delete')}
      </Button>
    </div>
  );
}

function AddFeedForm({
  onDone,
}: {
  onDone: (result: { ok: boolean; error?: string }) => void;
}): JSX.Element {
  const url = useSignal('');
  const title = useSignal('');
  const submitting = useSignal(false);

  async function submit() {
    if (!url.value.trim().startsWith('http')) return;
    submitting.value = true;
    const result = await addRssFeed(url.value.trim(), title.value.trim() || undefined);
    submitting.value = false;
    onDone(result);
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <Input
        label={t('rss.field.url_label')}
        value={url.value}
        onInput={(e) => (url.value = e.currentTarget.value)}
        placeholder={t('rss.field.url_placeholder')}
        hint={t('rss.field.url_hint')}
        autoFocus
        required
      />
      <Input
        label={t('rss.field.title_label')}
        value={title.value}
        onInput={(e) => (title.value = e.currentTarget.value)}
        placeholder={t('rss.field.title_placeholder')}
      />
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="ghost" onClick={() => onDone({ ok: false })}>
          {t('common.cancel')}
        </Button>
        <Button
          variant="primary"
          onClick={submit}
          loading={submitting.value}
          disabled={!url.value.trim().startsWith('http')}
        >
          {t('rss.action.add_submit')}
        </Button>
      </div>
    </div>
  );
}

function ImportOpmlForm({
  onDone,
}: {
  onDone: (result: { ok: boolean; imported?: number; error?: string }) => void;
}): JSX.Element {
  const file = useSignal<File | null>(null);
  const submitting = useSignal(false);

  async function submit() {
    if (!file.value) {
      toast('error', t('rss.toast.no_file'));
      return;
    }
    submitting.value = true;
    const result = await importOpml(file.value);
    submitting.value = false;
    onDone(result);
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)' }}>
        <span style={{ fontSize: 'var(--text-sm)', fontWeight: 500 }}>
          {t('rss.field.opml_label')}
        </span>
        <input
          type="file"
          accept=".opml,.xml,text/xml,text/x-opml"
          onChange={(e) => {
            const f = e.currentTarget.files?.[0] ?? null;
            file.value = f;
          }}
        />
        <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
          {t('rss.field.opml_hint')}
        </span>
      </label>
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="ghost" onClick={() => onDone({ ok: false })}>
          {t('common.cancel')}
        </Button>
        <Button
          variant="primary"
          onClick={submit}
          loading={submitting.value}
          disabled={!file.value}
        >
          {t('rss.action.import_submit')}
        </Button>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: i18n grep 守卫 —— 两条命令必须无输出**

Run:
```bash
cd rust/crates/attune-server/ui/src
grep -rnP "(toast\([^)]*'[^']*[\x{4e00}-\x{9fff}]|(title|placeholder|label|description|aria-label)=\"[^\"]*[\x{4e00}-\x{9fff}]|>[^<{]*[\x{4e00}-\x{9fff}])" --include="*.tsx" . | grep -v "/i18n/"
```
Expected: 无输出（RemoteView.tsx 不引入新硬编码中文）。

Run:
```bash
cd rust/crates/attune-server/ui/src
diff <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) \
     <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```
Expected: 无输出（zh / en key 集合完全一致）。

- [ ] **Step 5: 验证 TypeScript 编译 + 前端构建**

Run: `cd rust/crates/attune-server/ui && npx tsc --noEmit 2>&1 | tail -5`
Expected: 无 error。

Run: `cd rust/crates/attune-server/ui && npm run build 2>&1 | tail -5`
Expected: build 成功（嵌入式 UI 产物生成）。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-server/ui/src/views/RemoteView.tsx rust/crates/attune-server/ui/src/i18n/zh.ts rust/crates/attune-server/ui/src/i18n/en.ts
git commit -m "$(cat <<'EOF'
feat(ui): RSS feed management in RemoteView — add/import/refresh

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10：全量回归 + 文档同步

**Files:**
- Modify: `rust/crates/attune-core/README.md` 或 `DEVELOP.md`（采集源章节加 RSS）
- Modify: `rust/RELEASE.md`（changelog 加 RSS 条目）

- [ ] **Step 1: 跑 attune-core + attune-server 全量测试**

Run: `cd rust && cargo test -p attune-core 2>&1 | tail -15`
Expected: 全部 PASS（含新增 `ingest_rss_test` 7 个 + `rss_feeds_test` 5 个）。

Run: `cd rust && cargo test -p attune-server 2>&1 | tail -15`
Expected: 全部 PASS（含新增 `ingest_rss_sync_test` + `rss_api_test` + `rss_sync_worker` lib 测试）。

- [ ] **Step 2: clippy 零警告**

Run: `cd rust && cargo clippy -p attune-core -p attune-server --all-targets 2>&1 | tail -8`
Expected: 无 warning（新代码不引入 clippy 告警）。

- [ ] **Step 3: 跨平台编译冒烟（Windows target 不跑测试，只编译）**

Run: `cd rust && cargo build -p attune-core --target x86_64-pc-windows-gnu 2>&1 | tail -3`
Expected: `Finished`（`feed-rs` / `opml` 是纯 Rust，无跨平台风险；若本机无 windows-gnu toolchain 则跳过此步并在 commit message 注明）。

- [ ] **Step 4: 文档同步**

在 `rust/DEVELOP.md`（或 `rust/README.md`）的「采集源 / 数据接入」章节，把 RSS 列入已支持源：

```markdown
- **RSS / Atom 订阅**：订阅博客 / 新闻 feed，后台每 30 分钟增量抓取新文章入库；
  支持从 Feedly / Miniflux 等阅读器导出的 OPML 批量导入。
  Web UI 入口：Remote 页 → 「📡 RSS 订阅」。
```

在 `rust/RELEASE.md` 的 Unreleased / 下一版本 changelog 区加：

```markdown
### Added
- RSS / Atom feed 采集源：feed 订阅 CRUD、OPML 批量导入、后台周期增量抓取（每 30 分钟）。
```

- [ ] **Step 5: Commit**

```bash
git add rust/DEVELOP.md rust/RELEASE.md
git commit -m "$(cat <<'EOF'
docs: document RSS/Atom ingest source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## 蓝图级后续 task（不在本计划，记录待排期）

| 后续 task | 说明 | 触发条件 |
|-----------|------|----------|
| **抓全文** | feed entry 只给 `summary` + link 时，跟链接 GET 文章页 → 正文抽取（`readability` 风格）→ 替换 entry content。需处理反爬 / 付费墙。新建 `RssConnector` 的 `fetch_full_text` 开关 + per-feed 配置项。 | 用户反馈摘要太短、检索召回差 |
| **feed HTTP Basic auth** | 私有 feed（如 Miniflux 共享 feed、付费订阅）需鉴权。`rss_feeds` 表加 `username` / `password_enc` 列（字段级 AES-256-GCM，复用 `webdav_remotes` 加密模式），`RssConfig` 加凭据字段，`download_feed` 加 `.basic_auth()`。 | 用户提出私有 feed 需求 |
| **OPML 导出** | 反向 —— 把 attune 当前所有 feed 导出成 OPML，方便迁移到其它阅读器。`opml` crate 已支持构造 + 序列化。新增 `GET /api/v1/rss/export-opml`。 | 用户提出迁出需求 |
| **per-feed 抓取频率** | 不同 feed 更新频率差异大（新闻站 vs 月更博客）。`rss_feeds` 加 `interval_minutes` 列，worker 按各 feed 自己的周期调度。 | feed 数量多、polling 噪声明显 |
| **ETag / If-Modified-Since** | feed 拉取支持条件请求 —— server 返回 304 时整轮跳过解析，省带宽。`rss_feeds` 加 `etag` / `last_modified` 列。 | feed 数量多需降带宽 |

---

## 风险与回滚

| 风险 | 缓解 |
|------|------|
| **`feed-rs` 2.x 字段名与计划草图不符** | Task 3 Step 3 已注明「字段名以 docs.rs 实际为准，结构不变」。`entry.id` / `entry.content.body` / `entry.summary.content` / `entry.updated` 等是 2.x API；实现者按 docs.rs 校正。`parse_feed_bytes` 是纯函数，4 个解析测试（RSS 2.0 / Atom / 增量 / 垃圾输入）锁定行为。 |
| **`opml` crate API（`OPML::from_str` vs `parse`）不确定** | Task 2 Step 3 已给二选一指示。`parse_opml` 3 个测试（嵌套 / 无 xmlUrl / 非法 XML）锁定行为，与 crate 内部 API 形态无关。 |
| **RSS entry GUID 不稳定（部分 feed 每次拉取 GUID 变）** | 极少数畸形 feed 会这样 —— 后果是同一篇文章重复入库。但 `ingest_document` 的 content_hash 短路会把内容相同的判为 `Duplicate`，不重复写 item，仅多一次解析开销。可接受。`sync_rss_feed` 把 Duplicate 也记入 `seen_guids`，下轮跳过。 |
| **feed 站点把频繁 polling 视为滥用** | worker 周期取 30 分钟（比 webdav 15 分钟长），`download_feed` 带 30s timeout + 固定 `User-Agent`。后续 task「ETag / If-Modified-Since」「per-feed 频率」进一步降噪。 |
| **大 feed / 恶意超大 XML 爆内存** | `MAX_FEED_BYTES = 10 MB` 上限，`download_feed` 下载后二次校验字节数。 |
| **HTML 正文剥标签丢失结构** | MVP 的 `html_to_text` 是极简实现 —— 只保证可被 embedding / 全文检索消费的纯文本，不追求结构保真。这是 RSS 正文的合理取舍（feed 正文本就是富文本片段）。`parse_feed_bytes_atom_strips_html_to_text` 测试锁定。 |
| **迁移期破坏现有入库** | 本计划全部是**新增**（新文件 / 新表 / 新 route / 新 worker），不改 `ingest_document` / `scanner` / `upload` / WebDAV 任何现有路径。`rss_feeds` 表是 `CREATE TABLE IF NOT EXISTS`，老 vault 自动获得空表。任一 Task 出问题可 `git revert` 单个 commit 不影响其它路径。 |
| **worker 与手动 refresh 并发同一 feed** | 二者都通过 `sync_rss_feed`，每个 entry 短暂持 vault 锁；并发最坏情况是同一 entry 被两边都尝试入库，content_hash 短路保证只入一次。`append_seen_guids` 用 `BTreeSet` 合并幂等。可接受，不需额外锁。 |

**可独立交付：** 本计划是 ingest unification Phase 3，依赖 Phase 1 的 `SourceConnector` / `ingest_document`，与 Phase 2（Email）/ Phase 4（云盘）无依赖。10 个 Task 完成即「RSS/Atom 采集源」可发布。

---

## Self-Review

对照背景需求 + writing-plans 标准逐项自查：

**1. 需求覆盖：**
- ✅ `RssConnector` impl `SourceConnector` —— Task 3（`source_kind` + `fetch_documents`，tokio runtime 桥接 async，对齐 `WebDavConnector::drive_blocking`）。
- ✅ `rss_feeds` 表 + CRUD —— Task 4（`upsert` / `get` / `list` / `delete` / `touch` / `append_seen_guids`，schema 在 `SCHEMA_SQL`，无凭据故不加密，CRUD 形态参照 `webdav_remotes.rs`）。
- ✅ feed 拉取 + feed-rs 解析 + GUID 增量去重 —— Task 3（`download_feed` + `parse_feed_bytes` + `is_seen` 过滤）+ Task 5（`sync_rss_feed` 把新 GUID 并回 `seen_guids`）。
- ✅ 入库走 `ingest_document` —— Task 5（`sync_rss_feed` 对每份 `RawDocument` 调 `ingest_document`，不碰 pipeline 内部）。
- ✅ OPML 导入 —— Task 2（`parse_opml`，递归收集嵌套 outline）+ Task 6（`POST /rss/import-opml` multipart 上传）+ Task 9（UI `ImportOpmlForm`）。
- ✅ 后台周期拉取 worker —— Task 7（`start_rss_sync_worker`，30 分钟周期，原子 flag 防重入，对齐 `start_webdav_sync_worker`）。
- ✅ settings UI（feed 增删 + OPML 导入按钮 + 立即刷新）—— Task 9（`RemoteView` 加 RSS 区块 + `FeedRow` / `AddFeedForm` / `ImportOpmlForm`，i18n zh/en 同步）。
- ✅ HTTP API（feed CRUD + OPML 上传 + 手动刷新，kebab-case）—— Task 6（5 条 route：`GET/POST /rss/feeds`、`DELETE /rss/feeds/{id}`、`POST /rss/feeds/{id}/refresh`、`POST /rss/import-opml`）。
- ✅ 测试 —— Task 2/3（解析 + 增量单测，10 个）、Task 4（表 CRUD，5 个）、Task 5（sync 增量集成）、Task 6（API 形态契约，4 个）、Task 7（worker flag）。
- ✅ 「抓全文」「feed HTTP auth」作为蓝图级后续 task —— 「蓝图级后续 task」表 5 条。

**2. writing-plans 格式：** header（`# … Implementation Plan` + `> For agentic workers` + Goal / Architecture / Tech Stack）齐全；File Structure 新建/修改文件表齐全；每个 Task 是 bite-sized TDD（写失败测试 → 跑确认失败 → 实现 → 跑确认通过 → commit）；风险与回滚 + 蓝图级后续 task + Self-Review 齐全。

**3. Placeholder 扫描：** 所有 code step 含完整可编译 Rust / TypeScript，无 TODO/TBD。客观不确定性点（已给明确二选一指示，非 placeholder）：`feed-rs` 2.x 字段名（Task 3）、`opml` crate `from_str` vs `parse`（Task 2）、`Store::open` 签名（Task 4）、`test_support` helper 名（Task 5/6）、`Store::unbind_directory` 方法名（Task 6）、`api.postForm` 是否存在（Task 8）、`test_app_state` helper（Task 7）—— 全部指明「以仓内实际为准，结构不变」。

**4. 类型与调用点一致性：**
- `RssConfig` 字段（`feed_url` / `seen_entry_ids`）在 Task 3 定义、Task 5 构造一致。
- `RssConnector::new` / `parse_feed_bytes` / `is_seen` / `fetch_documents` 签名在 Task 3 定义、Task 3 测试 + Task 5 调用一致。
- `RssFeedInput`（`dir_id` / `feed_url` / `title` 3 字段）在 Task 4 定义，Task 5/6 构造一致。
- `RssFeedRow`（`dir_id` / `feed_url` / `title` / `seen_guids: Vec<String>` / `last_fetched`）在 Task 4 定义，Task 5/6 读取一致。
- `sync_rss_feed(state: &Arc<AppState>, dir_id: &str) -> Result<Value, String>` 在 Task 5 定义，Task 6（add/refresh route）+ Task 7（worker）调用一致。
- 5 条 route 路径在 Task 6 定义、Task 8 前端 hook 调用一致（`/rss/feeds`、`/rss/feeds/{id}`、`/rss/feeds/{id}/refresh`、`/rss/import-opml`）。
- `RssFeed` 前端类型（`dir_id` / `feed_url` / `title` / `entry_count` / `last_fetched`）在 Task 8 定义、与 Task 6 `list_feeds` 响应字段一致、Task 9 `FeedRow` 消费一致。
- i18n `rss.*` key 在 Task 9 Step 1（zh）与 Step 2（en）集合完全一致，Step 4 grep 守卫验证。
- Task 编号 1–10 连续无断号。

**5. 纪律核对：** Rust 跨平台（`feed-rs` / `opml` 纯 Rust，Task 10 Step 3 跨平台冒烟）；server 错误处理用 `AppError` / `AppResult` + `?`（Task 6 全程；feed 不可达显式归 `BadGateway` 502）；`sync_rss_feed` 是阻塞函数，route 在 `spawn_blocking` 里调，不阻塞 axum worker；lock ordering（只碰 `Store`，锁外做网络 I/O，每 entry 短暂持 vault 锁，对齐 `sync_webdav_dir`）；注释一处一条意图无过程标签；i18n zh/en key 集合一致 + grep 守卫；API kebab-case；commit message 末尾留 `Co-Authored-By` 行。

**仍需用户拍板的存疑点：** 无硬阻塞项。两点建议性确认（不阻塞实现，可在实现期或验收时定）：
1. **worker 周期 30 分钟** —— 计划取 30 分钟（webdav 是 15 分钟）以对源站点友好。若用户希望更快感知新文章可调短；建议保留 30 分钟，后续 task 提供 per-feed 频率。
2. **OPML 导入不逐个首抓** —— Task 6 `import_opml` 登记后不立即拉取（几十个 feed 同步拉会很慢），交周期 worker 首轮兜底（最坏 30 分钟后才有内容）。若用户希望导入后立即可见内容，可改为后台 `spawn` 一个一次性任务异步逐个拉。建议保留「交 worker 兜底」，实现简单且符合「建库阶段后台跑」的成本契约。
