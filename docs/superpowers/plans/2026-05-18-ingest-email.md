# Email 采集源（IMAP）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 attune 增加一个 `EmailConnector`——从 IMAP 邮箱拉邮件正文 + 文档类附件，走统一 `attune_core::ingest::ingest_document` 入库；含加密的 `email_accounts` 账户表、UID 增量抓取、`Message-ID` 去重、后台周期同步 worker、settings UI（账户增删 + 立即同步）、HTTP API（账户 CRUD + 手动触发）。

**Architecture:** 复用采集体系重构 Phase 1 已落地的 `SourceConnector` trait + `RawDocument` + `ingest_document()` 三件套。`EmailConnector` 实现 `SourceConnector`：同步 `fetch_documents` 契约内用单线程 tokio current-thread runtime 桥接 `async-imap` 的 async I/O——与 `WebDavConnector::drive_blocking` 完全同构。每封邮件 + 每个文档附件各产一份 `RawDocument` 经 `sink()` 逐个交出（大邮箱不物化）。账户凭据 `password` 走字段级 AES-256-GCM 加密落 `email_accounts` 表，与 `webdav_remotes` 同模式。周期同步 worker 与 `start_webdav_sync_worker` 同构。增量靠 IMAP UID（`indexed_files.file_hash` 存 `"{folder}:{uid}"`）。OAuth2 列为蓝图级后续 task，MVP 只做用户名 + 密码 / App Password。

**Tech Stack:** Rust 2021 / Axum 0.8 / rusqlite / tantivy / usearch。新依赖：`async-imap` 0.11（`default-features = false` + `runtime-tokio`，避免双 runtime）+ `mail-parser` 0.11（Stalwart 出品，RFC5322/MIME 解析）。TLS 走 rustls——`async-imap` 的 tls helper 用 `async-native-tls` 是其默认路径，**本计划改用 `tokio-rustls` 自建 TLS 连接器**喂给 `async-imap::Client::new`，确保不引入 `native-tls`/`openssl-sys`（CLAUDE.md「网络栈纯 Rust TLS」硬约束）。

---

## 关键背景与约束（实现者必读）

**地基已就位（采集体系重构 Phase 1 已完成）：** 以下类型/函数均已在仓库中，本计划直接复用，不要重新定义：

- `attune_core::ingest::SourceKind` —— enum，已含 `Email` 变体；`Email.as_str() == "email"`；`item_source_type() == "file"`。
- `attune_core::ingest::RawDocument` —— 字段：`uri` / `title` / `content: Vec<u8>` / `mime_hint: Option<String>` / `source_kind: SourceKind` / `source_ref: String` / `modified_marker: Option<String>` / `domain: Option<String>` / `tags: Option<Vec<String>>` / `corpus_domain: Option<String>` / `metadata: HashMap<String, String>`。`parse_filename()` 取 `source_ref` 末段供 parser 选解析器。
- `attune_core::ingest::{SourceConnector, DocumentSink}` —— `SourceConnector::fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()>` 是**同步**契约；`DocumentSink<'a> = Box<dyn FnMut(RawDocument) + 'a>`。单份文档可恢复错误（解析/下载失败）实现者吞掉记日志继续，只有源级致命错误（连不上/鉴权失败）才返回 `Err`。
- `attune_core::ingest::{ingest_document, ingest_document_replacing, IngestOutcome}` —— `ingest_document(store: &Store, dek: &Key32, raw: &RawDocument) -> Result<IngestOutcome>`；`IngestOutcome` 四态 `Inserted { item_id, chunks_enqueued } / Duplicate { item_id } / Updated { item_id, old_item_id } / Skipped { reason }`。`ingest_document` 内部已做 `content_hash` 短路判重、L1 章节 + L2 段落块 embedding 入队、`enqueue_classify`、breadcrumbs、`doc_create` 信号——**EmailConnector 一律不重复这些步骤**。

**`WebDavConnector` 是范本（必读 `attune-core/src/scanner_webdav.rs`）：** `EmailConnector` 在所有结构上照搬：
- `drive_blocking(&self, sink)` 用 `tokio::runtime::Builder::new_current_thread().enable_all().build()` 建单线程 runtime，`runtime.block_on(async { ... })` 桥接 async I/O。
- `fetch_documents` 同步实现里只调 `drive_blocking`。
- caller（route / 周期 worker）在 `spawn_blocking` 或独立线程里调 `fetch_documents`，不阻塞外层 async runtime。

**`webdav_remotes` 表是 `email_accounts` 表的范本（必读 `attune-core/src/store/webdav_remotes.rs`）：** `email_accounts` 照搬其 `WebDavRemoteInput`（明文写入）/ `WebDavRemoteRow`（解密读出）双结构 + `upsert` / `get` / `list` / `touch` + `debug_raw_*_enc` 测试 helper 模式；`password` 列 `password_enc BLOB` 经 `crypto::encrypt(dek, ...)`。

**`start_webdav_sync_worker` 是周期 worker 范本（必读 `attune-server/src/state.rs:751-813`）：** `email` 周期 worker 照搬：`AtomicBool` flag `compare_exchange` 防重入 + `FlagGuard` RAII drop 复位；`std::thread::spawn` 内 `loop`；`vault.state()` 锁定则 break（下次 unlock 重启）；snapshot 账户列表后释放锁；`std::thread::sleep(15 min)`；只 log `account_id`/`host` 不 log password。worker 在 `routes/vault.rs` 的 3 处 unlock 成功点启动（紧挨 `start_webdav_sync_worker` 调用加一行）。

**`sync_webdav_dir` 是同步入库函数范本（必读 `attune-server/src/ingest_webdav.rs`）：** `sync_email_account` 照搬其两阶段结构——阶段 1 锁外做全部网络 I/O（`fetch_documents` 物化到 `Vec<RawDocument>`）；阶段 2 逐文档**短暂**持 vault 锁做增量判断 + `ingest_document` + `upsert_indexed_file`，写完即 drop guard。返回 `serde_json::json!` 统计。

**真实代码事实（签名已与代码核对，不要凭记忆改）：**

- `parser::parse_bytes(data: &[u8], filename: &str) -> Result<(String, String)>` —— `(title, content)`。`ingest_document` 内部已调，`EmailConnector` 不直接调 parser。
- `Store::insert_item` / `find_item_by_content_hash` / `enqueue_embedding` / `enqueue_classify` —— 均由 `ingest_document` 内部调用，EmailConnector / connector 层不碰。
- `Store::get_indexed_file(&self, path: &str) -> Result<Option<IndexedFileRow>>`；`IndexedFileRow { id, dir_id, path, file_hash, item_id: Option<String> }`。
- `Store::upsert_indexed_file(&self, dir_id: &str, path: &str, file_hash: &str, item_id: &str) -> Result<()>`。
- `Store::delete_item(&self, id: &str) -> Result<bool>`；`Store::enqueue_reindex(&self, item_id: &str, action: &str) -> Result<()>`（`action` 必须 `"purge"` 或 `"reindex"`）。
- `Store::record_signal_event(&self, kind: &str, ref_id: &str, query: Option<&str>) -> Result<()>` —— `kind` 取已知集（`doc_create` 已由 `ingest_document` 内部发；`doc_update` 由 caller 在替换旧 item 时发）。
- `Store::bind_directory(&self, path: &str, recursive: bool, file_types: &[&str]) -> Result<String>` —— 返回 `dir_id`。`email_accounts` 用一条 `bound_dirs` 记录承载（`path` 形如 `email:{username}@{host}`），让 `indexed_files.dir_id` 外键有所归属。
- `Store::open_memory() -> Result<Store>`（`store/mod.rs:452`，集成测试用）。
- `crypto::encrypt(key: &Key32, plaintext: &[u8]) -> Result<Vec<u8>>` / `crypto::decrypt(key: &Key32, data: &[u8]) -> Result<Vec<u8>>`。`Key32::generate()`（`crypto.rs:18`，测试用）。
- `crate::store::now_iso8601()` —— `webdav_remotes.rs` 用它填 `updated_at`（与 `chrono::Utc::now().to_rfc3339()` 等价，沿用即可）。
- `attune_core::error::{Result, VaultError}` 是 core 层错误类型。`VaultError` 含 `LlmUnavailable(String)` 变体——`scanner_webdav.rs` 用它兜底网络类错误，本计划 IMAP 网络/鉴权错误也用 `VaultError::LlmUnavailable`（不新增变体，避免动 core error enum）。
- server 层错误：`attune_server::error::{AppError, AppResult}`，`AppError: From<VaultError>`。route 用 `AppResult<Json<T>>` + `?`；错误 JSON shape `{"error": msg, "code": kebab}`。
- `attune_core::async_fs` —— async handler 内禁止直接 `std::fs`（本计划 route 不碰 fs，无需用到，但若需要走它）。
- `AppState` 已有 `webdav_sync_worker_running: AtomicBool` 字段（`state.rs:73`）+ `AtomicBool::new(false)` 初始化（`state.rs:143`）；本计划新增 `email_sync_worker_running` 同位置加。
- 路由注册在 `attune-server/src/lib.rs`（`bind-remote` 在 `:213`）；`routes/mod.rs` 列 `pub mod` 清单（无 `email`，本计划加）。

**Lock ordering（防死锁，全程遵守）：** `vault.lock()` → `vectors.lock()` → `fulltext.lock()` → `embedding.lock()`。`ingest_document` 只碰 `Store`（SQL 连接），不碰 `VectorIndex` / `FulltextIndex`。`EmailConnector` / `sync_email_account` 全程只用 `Store`，不引入新锁顺序风险。`fetch_documents` 的网络 I/O 阶段**不持** vault 锁。

**i18n 纪律（强制）：** 新增 UI 字符串必须走 `t()`，且 `i18n/zh.ts` + `i18n/en.ts` 同时加同名 key、key 集合永远一致。现有 `remote.*` key 已成体系，新 key 用 `email.*` 命名空间。

**注释纪律：** 一个改动区域一条意图注释，写 WHY 不写 WHAT，禁止 `批次X` / `FIX-N` / `阶段Y` / `per reviewer` 这类过程标签。

**成本契约：** 遵守 attune「三层成本」原则——`ingest_document` 建库阶段只到「可被搜到 + 150 字摘要」（embedding 是第二层本地算力，自动跑；LLM 深度分析是第三层，等用户触发）。Email 周期 worker 只跑到入库，不触发任何 LLM 分析。

---

## File Structure

### 新建文件

| 文件 | 职责 |
|------|------|
| `rust/crates/attune-core/src/ingest/email.rs` | `EmailConnector` + `EmailConfig` + `ImapFetcher` trait（可注入抓取层）+ `MailMessage`（mail-parser 解析产物）+ `parse_email_bytes()`（纯函数，离线可测）|
| `rust/crates/attune-core/src/store/email_accounts.rs` | `email_accounts` 表加密持久化：`EmailAccountInput`（明文写入）/ `EmailAccountRow`（解密读出）+ CRUD（`upsert` / `get` / `list` / `delete` / `touch_sync` / `set_folder_uid`）|
| `rust/crates/attune-core/tests/email_accounts_test.rs` | `email_accounts` 表加解密往返 + 幂等 + folder UID 增量游标集成测试 |
| `rust/crates/attune-core/tests/ingest_email_test.rs` | `parse_email_bytes` 离线解析测试（`.eml` fixture）+ `EmailConnector` 经 mock `ImapFetcher` 驱动 sink 的测试 |
| `rust/crates/attune-core/tests/fixtures/email/plain.eml` | 纯文本邮件 fixture |
| `rust/crates/attune-core/tests/fixtures/email/with-attachment.eml` | 带 PDF 附件的 multipart 邮件 fixture |
| `rust/crates/attune-core/tests/fixtures/email/html-only.eml` | 仅 text/html 的邮件 fixture |
| `rust/crates/attune-server/src/ingest_email.rs` | `sync_email_account()` 公共同步函数——route 与周期 worker 共用的入库逻辑（照搬 `ingest_webdav.rs`）|
| `rust/crates/attune-server/src/routes/email.rs` | Email 账户 CRUD route + 手动同步触发 route |

### 修改文件

| 文件 | 改动 |
|------|------|
| `rust/crates/attune-core/Cargo.toml` | 加 `async-imap` 0.11（`default-features = false` + `runtime-tokio`）、`mail-parser` 0.11、`tokio-rustls`、`rustls`、`webpki-roots` 依赖 |
| `rust/crates/attune-core/src/ingest/mod.rs` | 加 `mod email; pub use email::*;` |
| `rust/crates/attune-core/src/store/mod.rs` | 加 `pub mod email_accounts;` + `email_accounts` 表 `CREATE TABLE IF NOT EXISTS` |
| `rust/crates/attune-server/src/routes/mod.rs` | 加 `pub mod email;` |
| `rust/crates/attune-server/src/lib.rs` | 注册 4 条 email route + `pub mod ingest_email;` |
| `rust/crates/attune-server/src/state.rs` | 加 `email_sync_worker_running: AtomicBool` 字段 + 初始化 + `start_email_sync_worker()` 方法 |
| `rust/crates/attune-server/src/routes/vault.rs` | 3 处 unlock 成功点各加一行 `start_email_sync_worker(state.clone())` |
| `rust/crates/attune-server/ui/src/i18n/zh.ts` | 加 `email.*` 中文 key |
| `rust/crates/attune-server/ui/src/i18n/en.ts` | 加 `email.*` 英文 key（与 zh 集合一致）|
| `rust/crates/attune-server/ui/src/hooks/useEmail.ts` | 新建：Email 账户 API 封装（list / add / delete / syncNow）|
| `rust/crates/attune-server/ui/src/views/RemoteView.tsx` | 在 Remote 视图加 Email 账户区块（账户列表 + 添加模态 + 立即同步按钮）|

---

## Phase 1：MVP（IMAP 用户名+密码采集，必做核心）

### Task 1：加 Email 依赖到 `attune-core/Cargo.toml`

**Files:**
- Modify: `rust/crates/attune-core/Cargo.toml`

- [ ] **Step 1: 加依赖**

在 `[dependencies]` 段 `reqwest_dav` 那一组之后加入（保留注释说明 TLS 选型 WHY）：

```toml
# Email IMAP 采集 —— async-imap 走 tokio runtime（与 WebDavConnector 同 runtime 桥接模式，
# 不引第二个 async runtime）；TLS 用 tokio-rustls 自建连接器喂给 async-imap，
# 避免 async-imap 默认的 async-native-tls → openssl-sys（违反纯 Rust TLS 硬约束）。
async-imap = { version = "0.11", default-features = false, features = ["runtime-tokio"] }
mail-parser = "0.11"
tokio-rustls = { version = "0.26", default-features = false, features = ["ring"] }
rustls = { version = "0.23", default-features = false, features = ["ring"] }
webpki-roots = "0.26"
```

- [ ] **Step 2: 验证依赖解析**

Run: `cargo fetch -p attune-core`
Expected: 成功拉取 `async-imap` / `mail-parser` / `tokio-rustls` 等，无版本冲突报错。

- [ ] **Step 3: 验证不引入 native-tls**

Run: `cargo tree -p attune-core -i native-tls 2>&1 | head -3`
Expected: 输出 `package ID specification 'native-tls' did not match any packages`（即依赖图中无 `native-tls`，符合纯 Rust TLS 约束）。

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-core/Cargo.toml rust/Cargo.lock
git commit -m "chore(deps): add async-imap + mail-parser for email ingest source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 2：`email_accounts` 表 DDL + 模块声明

**Files:**
- Modify: `rust/crates/attune-core/src/store/mod.rs`

- [ ] **Step 1: 加表 DDL**

在 `store/mod.rs` 的 schema DDL 字符串里、`webdav_remotes` 表 `CREATE TABLE IF NOT EXISTS` 块之后插入新表（`CREATE TABLE IF NOT EXISTS` —— 老 vault 下次 open 自动获得空表，与 `webdav_remotes` 同模式，无需独立 migration）：

```sql
-- Email IMAP 采集账户持久化。与 webdav_remotes 同模式：
-- bound_dirs(email:* path) 只记账户标识；周期同步 worker 要复用 IMAP 凭据
-- → 此表存完整账户配置。password_enc 是 AES-256-GCM 密文 BLOB（dek 加密，
-- 与 items.content 同模式）；folders 是逗号分隔的文件夹名列表。
CREATE TABLE IF NOT EXISTS email_accounts (
    dir_id        TEXT PRIMARY KEY REFERENCES bound_dirs(id) ON DELETE CASCADE,
    host          TEXT NOT NULL,
    port          INTEGER NOT NULL DEFAULT 993,
    username      TEXT NOT NULL,
    password_enc  BLOB NOT NULL,
    folders       TEXT NOT NULL DEFAULT 'INBOX,Sent',
    corpus_domain TEXT NOT NULL DEFAULT 'general',
    updated_at    TEXT NOT NULL,
    last_sync     TEXT
);

-- 每账户每文件夹的 IMAP UID 增量游标。下次 UID SEARCH 从 last_uid+1 起。
CREATE TABLE IF NOT EXISTS email_folder_uids (
    dir_id   TEXT NOT NULL REFERENCES email_accounts(dir_id) ON DELETE CASCADE,
    folder   TEXT NOT NULL,
    last_uid INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (dir_id, folder)
);
```

- [ ] **Step 2: 加模块声明**

在 `store/mod.rs` 顶部模块声明区（`pub mod webdav_remotes;` 那一行之后）加：

```rust
pub mod email_accounts;
```

- [ ] **Step 3: 验证编译**

Run: `cargo build -p attune-core`
Expected: 编译失败 —— `store/email_accounts.rs` 文件尚不存在，`pub mod email_accounts;` 找不到模块。这是预期的，Task 3 补上文件。

- [ ] **Step 4: 不单独 commit**

本 task 与 Task 3 合并 commit（模块声明与模块文件必须同时存在才能编译通过）。

---

### Task 3：`email_accounts` 表 CRUD（加密持久化）

**Files:**
- Create: `rust/crates/attune-core/src/store/email_accounts.rs`
- Test: `rust/crates/attune-core/tests/email_accounts_test.rs`

- [ ] **Step 1: 写失败测试**

`rust/crates/attune-core/tests/email_accounts_test.rs` 完整内容：

```rust
//! email_accounts 表加密持久化集成测试。

use attune_core::crypto::Key32;
use attune_core::store::email_accounts::EmailAccountInput;
use attune_core::store::Store;

fn sample_input(dir_id: &str) -> EmailAccountInput {
    EmailAccountInput {
        dir_id: dir_id.into(),
        host: "imap.gmail.com".into(),
        port: 993,
        username: "alice@gmail.com".into(),
        password: "app-specific-pw".into(),
        folders: vec!["INBOX".into(), "Sent".into()],
        corpus_domain: "general".into(),
    }
}

#[test]
fn upsert_then_get_round_trips_with_decrypted_password() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    store.upsert_email_account(&dek, &sample_input("dir-1")).unwrap();

    let got = store
        .get_email_account(&dek, "dir-1")
        .unwrap()
        .expect("account row exists");
    assert_eq!(got.host, "imap.gmail.com");
    assert_eq!(got.port, 993);
    assert_eq!(got.username, "alice@gmail.com");
    assert_eq!(got.password, "app-specific-pw", "password 必须能解密回明文");
    assert_eq!(got.folders, vec!["INBOX".to_string(), "Sent".to_string()]);
    assert_eq!(got.corpus_domain, "general");
}

#[test]
fn password_is_not_stored_in_plaintext() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = sample_input("dir-2");
    input.password = "PLAINTEXT_MARKER_XYZ".into();
    store.upsert_email_account(&dek, &input).unwrap();

    let raw = store.debug_raw_email_password_enc("dir-2").unwrap();
    assert!(!raw.is_empty(), "password_enc 应已写入");
    assert!(
        !raw.windows(20).any(|w| w == b"PLAINTEXT_MARKER_XYZ"),
        "password_enc 列绝不能含明文密码"
    );
}

#[test]
fn list_email_accounts_returns_all_configured() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    for i in 0..3 {
        store.upsert_email_account(&dek, &sample_input(&format!("dir-{i}"))).unwrap();
    }
    let all = store.list_email_accounts(&dek).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn upsert_is_idempotent_on_dir_id() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut input = sample_input("dir-x");
    store.upsert_email_account(&dek, &input).unwrap();
    input.host = "imap.fastmail.com".into();
    input.password = "new-pw".into();
    store.upsert_email_account(&dek, &input).unwrap();

    let all = store.list_email_accounts(&dek).unwrap();
    assert_eq!(all.len(), 1, "同 dir_id 二次 upsert 不新增行");
    let got = store.get_email_account(&dek, "dir-x").unwrap().unwrap();
    assert_eq!(got.host, "imap.fastmail.com");
    assert_eq!(got.password, "new-pw");
}

#[test]
fn delete_email_account_removes_row() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    store.upsert_email_account(&dek, &sample_input("dir-del")).unwrap();
    store.delete_email_account("dir-del").unwrap();
    assert!(store.get_email_account(&dek, "dir-del").unwrap().is_none());
}

#[test]
fn folder_uid_cursor_round_trips() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    store.upsert_email_account(&dek, &sample_input("dir-uid")).unwrap();

    assert_eq!(store.get_folder_uid("dir-uid", "INBOX").unwrap(), 0, "未设置时默认 0");
    store.set_folder_uid("dir-uid", "INBOX", 1234).unwrap();
    assert_eq!(store.get_folder_uid("dir-uid", "INBOX").unwrap(), 1234);
    store.set_folder_uid("dir-uid", "INBOX", 5678).unwrap();
    assert_eq!(store.get_folder_uid("dir-uid", "INBOX").unwrap(), 5678, "upsert 覆盖");
    assert_eq!(store.get_folder_uid("dir-uid", "Sent").unwrap(), 0, "不同 folder 独立");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --test email_accounts_test`
Expected: 编译失败 —— `store::email_accounts` 模块未定义。

- [ ] **Step 3: 写实现**

`rust/crates/attune-core/src/store/email_accounts.rs` 完整内容：

```rust
//! Email IMAP 采集账户持久化。
//!
//! 与 store/webdav_remotes.rs 同模式：周期同步 worker 要对邮箱自动按 UID
//! 增量重扫，必须能读回 IMAP 凭据。此表存每个 email: bound_dir 的完整账户
//! 配置，`password` 经字段级 AES-256-GCM 加密（dek，与 items.content 同模式）
//! 落 password_enc BLOB 列；明文密码绝不落盘。folder UID 增量游标单独存
//! email_folder_uids 表（每账户每文件夹一行）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

/// 写入用的 Email 账户配置（明文，调用方持有）。
#[derive(Debug, Clone)]
pub struct EmailAccountInput {
    /// 关联的 bound_dirs.id。
    pub dir_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 明文密码 / App Password；落库前由 `upsert_email_account` 用 dek 加密。
    pub password: String,
    /// 要同步的 IMAP 文件夹列表（默认 INBOX + Sent）。
    pub folders: Vec<String>,
    /// 语料领域（写入 RawDocument.corpus_domain，驱动 F-Pro 跨域防污染）。
    pub corpus_domain: String,
}

/// 从表里读出的 Email 账户配置（password 已解密回明文）。
#[derive(Debug, Clone)]
pub struct EmailAccountRow {
    pub dir_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub folders: Vec<String>,
    pub corpus_domain: String,
    pub last_sync: Option<String>,
}

/// folders 列表 ⇄ 逗号分隔字符串。空段过滤，避免 "INBOX,," 解析出空 folder。
fn join_folders(folders: &[String]) -> String {
    folders.join(",")
}
fn split_folders(s: &str) -> Vec<String> {
    s.split(',')
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(|f| f.to_string())
        .collect()
}

impl Store {
    /// upsert 一条 Email 账户配置。`password` 用 dek 加密成 BLOB 落盘。
    /// 同 `dir_id` 已存在则整行替换（幂等）。
    pub fn upsert_email_account(&self, dek: &Key32, input: &EmailAccountInput) -> Result<()> {
        let password_enc = crypto::encrypt(dek, input.password.as_bytes())?;
        let now = crate::store::now_iso8601();
        self.conn.execute(
            "INSERT INTO email_accounts
                (dir_id, host, port, username, password_enc, folders, corpus_domain, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(dir_id) DO UPDATE SET
                host=excluded.host,
                port=excluded.port,
                username=excluded.username,
                password_enc=excluded.password_enc,
                folders=excluded.folders,
                corpus_domain=excluded.corpus_domain,
                updated_at=excluded.updated_at",
            params![
                input.dir_id,
                input.host,
                input.port as i64,
                input.username,
                password_enc,
                join_folders(&input.folders),
                input.corpus_domain,
                now,
            ],
        )?;
        Ok(())
    }

    /// 读单条 Email 账户配置（password 解密回明文）。
    pub fn get_email_account(&self, dek: &Key32, dir_id: &str) -> Result<Option<EmailAccountRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync
             FROM email_accounts WHERE dir_id = ?1",
        )?;
        let row = stmt
            .query_row(params![dir_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })
            .ok();
        match row {
            None => Ok(None),
            Some((dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync)) => {
                let password = String::from_utf8(crypto::decrypt(dek, &password_enc)?)
                    .map_err(|e| VaultError::Crypto(format!("email password utf8: {e}")))?;
                Ok(Some(EmailAccountRow {
                    dir_id,
                    host,
                    port: port as u16,
                    username,
                    password,
                    folders: split_folders(&folders),
                    corpus_domain,
                    last_sync,
                }))
            }
        }
    }

    /// 列出全部 Email 账户配置（周期 worker 用，password 已解密）。
    pub fn list_email_accounts(&self, dek: &Key32) -> Result<Vec<EmailAccountRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync
             FROM email_accounts ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (dir_id, host, port, username, password_enc, folders, corpus_domain, last_sync) =
                row?;
            let password = String::from_utf8(crypto::decrypt(dek, &password_enc)?)
                .map_err(|e| VaultError::Crypto(format!("email password utf8: {e}")))?;
            out.push(EmailAccountRow {
                dir_id,
                host,
                port: port as u16,
                username,
                password,
                folders: split_folders(&folders),
                corpus_domain,
                last_sync,
            });
        }
        Ok(out)
    }

    /// 删除一条 Email 账户配置（email_folder_uids 经 ON DELETE CASCADE 一并清）。
    pub fn delete_email_account(&self, dir_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM email_accounts WHERE dir_id = ?1", params![dir_id])?;
        Ok(())
    }

    /// 记录某账户最近一次同步时间（周期 worker / 手动同步调用）。
    pub fn touch_email_account_sync(&self, dir_id: &str) -> Result<()> {
        let now = crate::store::now_iso8601();
        self.conn.execute(
            "UPDATE email_accounts SET last_sync = ?1 WHERE dir_id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    /// 读某账户某文件夹的 IMAP UID 增量游标（未设置回 0）。
    pub fn get_folder_uid(&self, dir_id: &str, folder: &str) -> Result<u32> {
        let uid: Option<i64> = self
            .conn
            .query_row(
                "SELECT last_uid FROM email_folder_uids WHERE dir_id = ?1 AND folder = ?2",
                params![dir_id, folder],
                |r| r.get(0),
            )
            .ok();
        Ok(uid.unwrap_or(0).max(0) as u32)
    }

    /// 写某账户某文件夹的 IMAP UID 增量游标（upsert）。
    pub fn set_folder_uid(&self, dir_id: &str, folder: &str, last_uid: u32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO email_folder_uids (dir_id, folder, last_uid)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(dir_id, folder) DO UPDATE SET last_uid=excluded.last_uid",
            params![dir_id, folder, last_uid as i64],
        )?;
        Ok(())
    }

    /// 仅供集成测试用：取 password_enc 原始密文字节（验证不含明文）。
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn debug_raw_email_password_enc(&self, dir_id: &str) -> Result<Vec<u8>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT password_enc FROM email_accounts WHERE dir_id = ?1",
                params![dir_id],
                |r| r.get(0),
            )
            .ok();
        Ok(blob.unwrap_or_default())
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --test email_accounts_test`
Expected: PASS（6 个测试全绿）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/store/mod.rs \
        rust/crates/attune-core/src/store/email_accounts.rs \
        rust/crates/attune-core/tests/email_accounts_test.rs
git commit -m "feat(store): add email_accounts encrypted persistence table

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 4：邮件解析纯函数 `parse_email_bytes`（含 fixtures）

**Files:**
- Create: `rust/crates/attune-core/src/ingest/email.rs`（先只写 `MailMessage` + `parse_email_bytes`，连接器 Task 6 补）
- Modify: `rust/crates/attune-core/src/ingest/mod.rs`
- Create: `rust/crates/attune-core/tests/fixtures/email/plain.eml`
- Create: `rust/crates/attune-core/tests/fixtures/email/with-attachment.eml`
- Create: `rust/crates/attune-core/tests/fixtures/email/html-only.eml`
- Test: `rust/crates/attune-core/tests/ingest_email_test.rs`（先只写解析测试）

- [ ] **Step 1: 写 fixtures**

`rust/crates/attune-core/tests/fixtures/email/plain.eml` 完整内容：

```
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Quarterly Notes
Message-ID: <plain-001@example.com>
Date: Mon, 18 May 2026 09:00:00 +0000
Content-Type: text/plain; charset=utf-8

This is the body of a plain text email.
It has two lines worth of useful content.
```

`rust/crates/attune-core/tests/fixtures/email/html-only.eml` 完整内容：

```
From: Carol <carol@example.com>
To: Bob <bob@example.com>
Subject: HTML Newsletter
Message-ID: <html-002@example.com>
Date: Mon, 18 May 2026 10:00:00 +0000
Content-Type: text/html; charset=utf-8

<html><body><h1>Heading</h1><p>Paragraph one.</p><p>Paragraph two.</p></body></html>
```

`rust/crates/attune-core/tests/fixtures/email/with-attachment.eml` 完整内容（附件是最小合法 PDF，base64 编码——下方 base64 块解码后是一个含文字 "Hello" 的单页 PDF，mail-parser 会按 `Content-Disposition` 文件名 `report.pdf` 还原字节）：

```
From: Dave <dave@example.com>
To: Bob <bob@example.com>
Subject: Report Attached
Message-ID: <att-003@example.com>
Date: Mon, 18 May 2026 11:00:00 +0000
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="BOUNDARY42"

--BOUNDARY42
Content-Type: text/plain; charset=utf-8

Please see the attached report.
--BOUNDARY42
Content-Type: application/pdf; name="report.pdf"
Content-Disposition: attachment; filename="report.pdf"
Content-Transfer-Encoding: base64

JVBERi0xLjQKMSAwIG9iago8PC9UeXBlL0NhdGFsb2cvUGFnZXMgMiAwIFI+PgplbmRvYmoKMiAw
IG9iago8PC9UeXBlL1BhZ2VzL0tpZHNbMyAwIFJdL0NvdW50IDE+PgplbmRvYmoKMyAwIG9iago8
PC9UeXBlL1BhZ2UvUGFyZW50IDIgMCBSL01lZGlhQm94WzAgMCAyMDAgMjAwXS9SZXNvdXJjZXM8
PC9Gb250PDwvRjEgNCAwIFI+Pj4+L0NvbnRlbnRzIDUgMCBSPj4KZW5kb2JqCjQgMCBvYmoKPDwv
VHlwZS9Gb250L1N1YnR5cGUvVHlwZTEvQmFzZUZvbnQvSGVsdmV0aWNhPj4KZW5kb2JqCjUgMCBv
YmoKPDwvTGVuZ3RoIDQ0Pj4Kc3RyZWFtCkJUIC9GMSAyNCBUZiAyMCAxMDAgVGQgKEhlbGxvKSBU
aiBFVAplbmRzdHJlYW0KZW5kb2JqCnhyZWYKMCA2CjAwMDAwMDAwMDAgNjU1MzUgZiAKMDAwMDAw
MDAwOSAwMDAwMCBuIAowMDAwMDAwMDU4IDAwMDAwIG4gCjAwMDAwMDAxMTUgMDAwMDAgbiAKMDAw
MDAwMDI0NSAwMDAwMCBuIAowMDAwMDAwMzIzIDAwMDAwIG4gCnRyYWlsZXIKPDwvU2l6ZSA2L1Jv
b3QgMSAwIFI+PgpzdGFydHhyZWYKNDE3CiUlRU9G
--BOUNDARY42--
```

- [ ] **Step 2: 写失败测试**

`rust/crates/attune-core/tests/ingest_email_test.rs` 完整内容（本 task 只写解析测试，连接器测试 Task 6 追加）：

```rust
//! Email 采集源测试 —— 解析层离线测试（不连真 IMAP）。

use attune_core::ingest::email::parse_email_bytes;

const PLAIN: &[u8] = include_bytes!("fixtures/email/plain.eml");
const HTML_ONLY: &[u8] = include_bytes!("fixtures/email/html-only.eml");
const WITH_ATTACHMENT: &[u8] = include_bytes!("fixtures/email/with-attachment.eml");

#[test]
fn parse_plain_email_extracts_subject_and_body() {
    let msg = parse_email_bytes(PLAIN).expect("plain email parses");
    assert_eq!(msg.subject, "Quarterly Notes");
    assert_eq!(msg.message_id, "<plain-001@example.com>");
    assert!(msg.body.contains("body of a plain text email"));
    assert!(msg.body.contains("two lines worth"));
    assert_eq!(msg.from.as_deref(), Some("alice@example.com"));
    assert!(msg.attachments.is_empty());
}

#[test]
fn parse_html_only_email_strips_tags() {
    let msg = parse_email_bytes(HTML_ONLY).expect("html email parses");
    assert_eq!(msg.subject, "HTML Newsletter");
    // text/html 剥标签后保留可读文本，不含 < > 标签。
    assert!(msg.body.contains("Heading"));
    assert!(msg.body.contains("Paragraph one"));
    assert!(!msg.body.contains("<h1>"));
    assert!(!msg.body.contains("<p>"));
}

#[test]
fn parse_email_with_attachment_extracts_pdf_bytes() {
    let msg = parse_email_bytes(WITH_ATTACHMENT).expect("multipart email parses");
    assert_eq!(msg.subject, "Report Attached");
    assert!(msg.body.contains("attached report"));
    assert_eq!(msg.attachments.len(), 1, "应提取 1 个附件");
    let att = &msg.attachments[0];
    assert_eq!(att.filename, "report.pdf");
    // PDF 魔数 %PDF —— 确认 base64 已正确解码回二进制。
    assert!(att.content.starts_with(b"%PDF"), "附件应是解码后的 PDF 字节");
}

#[test]
fn parse_invalid_bytes_returns_err() {
    // 完全不是邮件的字节 —— 解析应失败而非 panic。
    let result = parse_email_bytes(&[0xFF, 0xFE, 0x00, 0x01]);
    assert!(result.is_err());
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p attune-core --test ingest_email_test`
Expected: 编译失败 —— `attune_core::ingest::email` 模块未定义。

- [ ] **Step 4: 写实现**

`rust/crates/attune-core/src/ingest/email.rs` 完整内容（本 task 部分——`MailMessage` / `MailAttachment` / `parse_email_bytes` / `html_to_text`；`EmailConnector` 在 Task 6 追加到同文件）：

```rust
//! Email IMAP 采集源。
//!
//! EmailConnector 实现 SourceConnector：用单线程 tokio runtime 桥接 async-imap
//! 的 async I/O（与 scanner_webdav.rs::WebDavConnector::drive_blocking 同模式）。
//! 每封邮件 + 每个文档类附件各产一份 RawDocument，逐个交给 sink —— 大邮箱不物化。
//! 解析层（parse_email_bytes / MailMessage）是纯函数，离线可测，不依赖网络。

use crate::error::{Result, VaultError};

/// mail-parser 解析出的一封邮件（解析层产物，与 IMAP 抓取解耦）。
#[derive(Debug, Clone)]
pub struct MailMessage {
    /// 邮件主题（Subject header）。
    pub subject: String,
    /// 稳定唯一标识（Message-ID header）。缺失时由调用方用 "{folder}:{uid}" 兜底。
    pub message_id: String,
    /// 正文纯文本：text/plain 优先，否则 text/html 剥标签。
    pub body: String,
    /// 发件人地址（From header 第一个地址）。
    pub from: Option<String>,
    /// 发件日期（Date header 原始字符串）。
    pub date: Option<String>,
    /// 文档类附件（已按扩展名白名单过滤，已剔除超大附件）。
    pub attachments: Vec<MailAttachment>,
}

/// 一个文档类附件。
#[derive(Debug, Clone)]
pub struct MailAttachment {
    pub filename: String,
    pub content: Vec<u8>,
}

/// 附件大小上限（与本地 upload / WebDAV 一致，超限跳过）。
pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

/// 受支持的文档类附件扩展名（与 parser 支持集对齐，二进制媒体不入库）。
const SUPPORTED_ATTACHMENT_EXTS: &[&str] = &[
    "md", "txt", "py", "js", "ts", "rs", "go", "java", "pdf", "docx", "html", "htm", "csv",
    "rtf", "pptx", "xlsx", "png", "jpg", "jpeg",
];

fn is_supported_attachment(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    SUPPORTED_ATTACHMENT_EXTS.contains(&ext.as_str())
}

/// 极简 HTML → 纯文本：去标签、解最常见实体、压空白。
/// 不追求完美渲染，只要让 text/html-only 邮件可被检索 + 不在正文里塞标签噪声。
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 解析一封邮件的原始 RFC822 字节为 `MailMessage`。
///
/// 纯函数，不触网 —— IMAP 抓取层把 FETCH 到的字节喂进来。正文取 text/plain
/// 优先，缺失时取 text/html 剥标签；文档类附件按扩展名白名单 + 大小上限过滤。
pub fn parse_email_bytes(raw: &[u8]) -> Result<MailMessage> {
    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or_else(|| VaultError::LlmUnavailable("email parse failed".into()))?;

    let subject = parsed.subject().unwrap_or_default().to_string();
    let message_id = parsed.message_id().unwrap_or_default().to_string();
    let from = parsed
        .from()
        .and_then(|addr| addr.first())
        .and_then(|a| a.address())
        .map(|s| s.to_string());
    let date = parsed.date().map(|d| d.to_rfc3339());

    // 正文：text/plain 优先，否则第一个 text/html 剥标签。
    let body = parsed
        .body_text(0)
        .map(|t| t.to_string())
        .filter(|t| !t.trim().is_empty())
        .or_else(|| parsed.body_html(0).map(|h| html_to_text(&h)))
        .unwrap_or_default();

    // 附件：按扩展名白名单 + 大小上限过滤。
    let mut attachments = Vec::new();
    for att in parsed.attachments() {
        let filename = att.attachment_name().unwrap_or("attachment").to_string();
        if !is_supported_attachment(&filename) {
            continue;
        }
        let bytes = att.contents();
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            log::warn!("email: skip oversized attachment {filename} ({} bytes)", bytes.len());
            continue;
        }
        attachments.push(MailAttachment {
            filename,
            content: bytes.to_vec(),
        });
    }

    Ok(MailMessage {
        subject,
        message_id,
        body,
        from,
        date,
        attachments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_strips_tags_and_collapses_whitespace() {
        let html = "<p>Hello   &amp;   <b>World</b></p>\n<div>line two</div>";
        let text = html_to_text(html);
        assert_eq!(text, "Hello & World line two");
    }

    #[test]
    fn supported_attachment_filters_media() {
        assert!(is_supported_attachment("report.pdf"));
        assert!(is_supported_attachment("notes.md"));
        assert!(!is_supported_attachment("video.mp4"));
        assert!(!is_supported_attachment("archive.zip"));
    }
}
```

在 `rust/crates/attune-core/src/ingest/mod.rs` 加（放在已有 `pub use` 之后）：

```rust
pub mod email;
pub use email::{EmailConfig, EmailConnector, MailAttachment, MailMessage};
```

> 注：`EmailConfig` / `EmailConnector` 在本 task 时点尚未定义，Task 6 才补。为避免 Task 4 编译失败，本 task 的 `mod.rs` 改动**只加** `pub mod email;`，`pub use email::{...}` 那行在 Task 6 实现 `EmailConnector` 后再加。本 task 仅 `pub mod email;`。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p attune-core --test ingest_email_test`
Expected: PASS（4 个解析测试 + 2 个 email.rs 内联单元测试全绿）。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-core/src/ingest/email.rs \
        rust/crates/attune-core/src/ingest/mod.rs \
        rust/crates/attune-core/tests/ingest_email_test.rs \
        rust/crates/attune-core/tests/fixtures/email/
git commit -m "feat(ingest): add email parsing layer (parse_email_bytes + fixtures)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 5：`ImapFetcher` 抓取层抽象（可注入，让连接器离线可测）

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/email.rs`（追加 `ImapFetcher` trait + `FetchedMail` + `EmailConfig`）

**为什么要这个抽象：** `EmailConnector` 若直接调 `async-imap` 就无法离线测试（要真 IMAP server）。本 task 抽一个 `ImapFetcher` trait —— 「给定文件夹 + 起始 UID，返回 `(uid, raw_bytes)` 列表」。`EmailConnector` 依赖 trait，生产用 `RealImapFetcher`（Task 6 实现），测试用 `MockImapFetcher`（Task 6 测试里实现）。

- [ ] **Step 1: 写实现（追加到 `email.rs` 末尾，`#[cfg(test)] mod tests` 之前）**

```rust
/// Email 账户连接配置（明文，连接器持有；持久化由 store/email_accounts.rs 负责）。
#[derive(Debug, Clone)]
pub struct EmailConfig {
    /// IMAP 服务器主机名（如 imap.gmail.com）。
    pub host: String,
    /// IMAP over TLS 端口（标准 993）。
    pub port: u16,
    pub username: String,
    /// 明文密码 / App Password。
    pub password: String,
    /// 要同步的文件夹（默认 INBOX + Sent）。
    pub folders: Vec<String>,
}

impl EmailConfig {
    /// 文件夹列表为空时回退到默认 INBOX + Sent。
    pub fn effective_folders(&self) -> Vec<String> {
        if self.folders.is_empty() {
            vec!["INBOX".to_string(), "Sent".to_string()]
        } else {
            self.folders.clone()
        }
    }
}

/// 一封从 IMAP 抓回的邮件原始字节 + 其 UID。
#[derive(Debug, Clone)]
pub struct FetchedMail {
    pub uid: u32,
    pub raw: Vec<u8>,
}

/// IMAP 抓取层抽象 —— 把网络 I/O 与连接器逻辑解耦，让连接器离线可测。
///
/// 实现者负责连接 / 登录 / 选文件夹 / `UID SEARCH since_uid:* ` / 逐 UID FETCH。
/// 单封邮件的 FETCH 失败应吞掉记日志继续；只有源级致命错误（连不上 / 鉴权失败 /
/// 文件夹不存在）才返回 Err。
pub trait ImapFetcher {
    /// 抓取 `folder` 内 UID 严格大于 `since_uid` 的全部邮件。
    fn fetch_since(&self, folder: &str, since_uid: u32) -> Result<Vec<FetchedMail>>;
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo build -p attune-core`
Expected: PASS（纯类型定义，无测试，编译通过即可）。

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-core/src/ingest/email.rs
git commit -m "feat(ingest): add ImapFetcher trait for injectable email fetch layer

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 6：`EmailConnector` impl `SourceConnector` + `RealImapFetcher`

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/email.rs`（追加 `EmailConnector` + `RealImapFetcher`）
- Modify: `rust/crates/attune-core/src/ingest/mod.rs`（补 `pub use`）
- Test: `rust/crates/attune-core/tests/ingest_email_test.rs`（追加连接器测试）

- [ ] **Step 1: 写失败测试（追加到 `ingest_email_test.rs` 末尾）**

```rust
use attune_core::ingest::email::{EmailConfig, EmailConnector, FetchedMail, ImapFetcher};
use attune_core::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};
use std::collections::HashMap;

/// 离线 mock：按 folder 返回预置邮件，记录被请求的 since_uid。
struct MockImapFetcher {
    by_folder: HashMap<String, Vec<FetchedMail>>,
}

impl ImapFetcher for MockImapFetcher {
    fn fetch_since(&self, folder: &str, since_uid: u32) -> attune_core::error::Result<Vec<FetchedMail>> {
        let all = self.by_folder.get(folder).cloned().unwrap_or_default();
        // 模拟 IMAP UID SEARCH since_uid:* 语义 —— 只返回 UID > since_uid。
        Ok(all.into_iter().filter(|m| m.uid > since_uid).collect())
    }
}

fn config() -> EmailConfig {
    EmailConfig {
        host: "imap.example.com".into(),
        port: 993,
        username: "bob@example.com".into(),
        password: "pw".into(),
        folders: vec!["INBOX".into()],
    }
}

#[test]
fn connector_emits_one_rawdocument_per_email() {
    let mut by_folder = HashMap::new();
    by_folder.insert(
        "INBOX".to_string(),
        vec![FetchedMail { uid: 1, raw: PLAIN.to_vec() }],
    );
    let connector = EmailConnector::with_fetcher(config(), Box::new(MockImapFetcher { by_folder }));

    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 1);
    let doc = &docs[0];
    assert_eq!(doc.source_kind, SourceKind::Email);
    assert_eq!(doc.title, "Quarterly Notes");
    assert_eq!(doc.source_ref, "<plain-001@example.com>", "source_ref = Message-ID");
    assert_eq!(doc.modified_marker.as_deref(), Some("INBOX:1"), "增量标记 = folder:uid");
    assert_eq!(doc.metadata.get("from").map(String::as_str), Some("alice@example.com"));
    assert_eq!(doc.metadata.get("folder").map(String::as_str), Some("INBOX"));
}

#[test]
fn connector_emits_extra_rawdocument_per_attachment() {
    let mut by_folder = HashMap::new();
    by_folder.insert(
        "INBOX".to_string(),
        vec![FetchedMail { uid: 7, raw: WITH_ATTACHMENT.to_vec() }],
    );
    let connector = EmailConnector::with_fetcher(config(), Box::new(MockImapFetcher { by_folder }));

    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    // 1 封邮件正文 + 1 个 PDF 附件 = 2 份 RawDocument。
    assert_eq!(docs.len(), 2);
    let attachment_doc = docs
        .iter()
        .find(|d| d.source_ref.contains("#att"))
        .expect("attachment doc exists");
    assert_eq!(attachment_doc.source_ref, "<att-003@example.com>#att0");
    assert!(attachment_doc.content.starts_with(b"%PDF"));
    assert_eq!(attachment_doc.parse_filename(), "report.pdf");
}

#[test]
fn connector_respects_since_uid_increment() {
    let mut by_folder = HashMap::new();
    by_folder.insert(
        "INBOX".to_string(),
        vec![
            FetchedMail { uid: 1, raw: PLAIN.to_vec() },
            FetchedMail { uid: 2, raw: HTML_ONLY.to_vec() },
        ],
    );
    let mut cfg = config();
    // since_uid 通过 with_since 设置（连接器构造后注入每文件夹起始游标）。
    cfg.folders = vec!["INBOX".into()];
    let mut connector = EmailConnector::with_fetcher(cfg, Box::new(MockImapFetcher { by_folder }));
    connector.set_folder_since("INBOX", 1); // 只要 UID > 1

    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 1, "UID=1 被增量游标跳过，只剩 UID=2");
    assert_eq!(docs[0].title, "HTML Newsletter");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --test ingest_email_test`
Expected: 编译失败 —— `EmailConnector` 未定义。

- [ ] **Step 3: 写实现（追加到 `email.rs`，`#[cfg(test)] mod tests` 之前）**

```rust
use std::collections::HashMap;

use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};

/// IMAP 邮箱采集源。
///
/// 持有一个 `ImapFetcher`（生产 = RealImapFetcher，测试 = mock），逐文件夹按
/// UID 增量抓取，把每封邮件正文 + 每个文档类附件转成 RawDocument 交给 sink。
pub struct EmailConnector {
    config: EmailConfig,
    fetcher: Box<dyn ImapFetcher>,
    /// 每文件夹的 IMAP UID 增量起点（fetch_since 只取 UID 严格大于此值的邮件）。
    /// 由 caller（sync_email_account）在 fetch 前从 email_folder_uids 表注入。
    since_by_folder: HashMap<String, u32>,
}

impl EmailConnector {
    /// 用指定 fetcher 构造（测试注入 mock；生产传 RealImapFetcher）。
    pub fn with_fetcher(config: EmailConfig, fetcher: Box<dyn ImapFetcher>) -> Self {
        Self {
            config,
            fetcher,
            since_by_folder: HashMap::new(),
        }
    }

    /// 用生产 IMAP 抓取层构造（rustls TLS over async-imap）。
    pub fn new(config: EmailConfig) -> Self {
        let fetcher = Box::new(RealImapFetcher {
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            password: config.password.clone(),
        });
        Self::with_fetcher(config, fetcher)
    }

    /// 设置某文件夹的 UID 增量起点（caller 从 email_folder_uids 表读出后注入）。
    pub fn set_folder_since(&mut self, folder: &str, since_uid: u32) {
        self.since_by_folder.insert(folder.to_string(), since_uid);
    }

    /// 把一封邮件展开成 RawDocument 列表（正文 1 份 + 每附件 1 份）交给 sink。
    fn emit_mail(&self, folder: &str, fetched: &FetchedMail, sink: &mut DocumentSink<'_>) {
        let msg = match parse_email_bytes(&fetched.raw) {
            Ok(m) => m,
            Err(e) => {
                // 单封解析失败不致命：记日志、继续下一封。
                log::warn!("email: parse uid {} in {folder} failed: {e}", fetched.uid);
                return;
            }
        };

        // Message-ID 缺失时用 folder:uid 兜底作稳定唯一键。
        let msg_id = if msg.message_id.trim().is_empty() {
            format!("{folder}:{}", fetched.uid)
        } else {
            msg.message_id.clone()
        };
        let marker = format!("{folder}:{}", fetched.uid);

        let mut metadata = HashMap::new();
        if let Some(ref from) = msg.from {
            metadata.insert("from".to_string(), from.clone());
        }
        if let Some(ref date) = msg.date {
            metadata.insert("date".to_string(), date.clone());
        }
        metadata.insert("folder".to_string(), folder.to_string());

        // 正文 RawDocument。source_ref = Message-ID（跨 folder 去重稳定键）。
        // content 给一个 .txt 文件名，让 ingest_document 内的 parser 走纯文本分支。
        if !msg.body.trim().is_empty() {
            sink(RawDocument {
                uri: format!("imap://{}/{folder}/{}", self.config.host, fetched.uid),
                title: msg.subject.clone(),
                content: msg.body.clone().into_bytes(),
                mime_hint: Some("text/plain".to_string()),
                source_kind: SourceKind::Email,
                source_ref: format!("{msg_id}.txt"),
                modified_marker: Some(marker.clone()),
                domain: None,
                tags: None,
                corpus_domain: None,
                metadata: metadata.clone(),
            });
        }

        // 每个文档类附件单独一份 RawDocument。source_ref 带 #attN 后缀避免与
        // 正文 / 其它附件碰撞；parse_filename 取末段 → parser 按附件扩展名解析。
        for (idx, att) in msg.attachments.iter().enumerate() {
            let mut att_meta = metadata.clone();
            att_meta.insert("attachment_of".to_string(), msg_id.clone());
            sink(RawDocument {
                uri: format!(
                    "imap://{}/{folder}/{}/att{idx}",
                    self.config.host, fetched.uid
                ),
                title: format!("{} — {}", msg.subject, att.filename),
                content: att.content.clone(),
                mime_hint: None,
                source_kind: SourceKind::Email,
                source_ref: format!("{msg_id}#att{idx}/{}", att.filename),
                modified_marker: Some(format!("{marker}#att{idx}")),
                domain: None,
                tags: None,
                corpus_domain: None,
                metadata: att_meta,
            });
        }
    }
}

impl SourceConnector for EmailConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::Email
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        for folder in self.config.effective_folders() {
            let since = self.since_by_folder.get(&folder).copied().unwrap_or(0);
            // 单文件夹抓取失败不致命：记日志、继续下一个文件夹。
            let mails = match self.fetcher.fetch_since(&folder, since) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("email: fetch folder {folder} failed: {e}");
                    continue;
                }
            };
            for fetched in &mails {
                self.emit_mail(&folder, fetched, sink);
            }
        }
        Ok(())
    }
}

/// 生产 IMAP 抓取层 —— async-imap over tokio-rustls，单线程 runtime 桥接。
pub struct RealImapFetcher {
    host: String,
    port: u16,
    username: String,
    password: String,
}

impl RealImapFetcher {
    /// 建 rustls TLS 配置（webpki 根证书，纯 Rust，不引 native-tls）。
    fn tls_connector() -> tokio_rustls::TlsConnector {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg))
    }

    /// 异步连接 + 登录 + 抓取某文件夹 since_uid 之后的邮件。
    async fn fetch_async(&self, folder: &str, since_uid: u32) -> Result<Vec<FetchedMail>> {
        use futures::StreamExt;

        let tcp = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("imap connect: {e}")))?;
        let dns = rustls::pki_types::ServerName::try_from(self.host.clone())
            .map_err(|e| VaultError::LlmUnavailable(format!("imap server name: {e}")))?;
        let tls = Self::tls_connector()
            .connect(dns, tcp)
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("imap tls: {e}")))?;

        let client = async_imap::Client::new(tls);
        let mut session = client
            .login(&self.username, &self.password)
            .await
            .map_err(|(e, _)| VaultError::LlmUnavailable(format!("imap login: {e}")))?;

        // 选文件夹（不存在则视为该文件夹无邮件，返回空而非致命错误）。
        if session.select(folder).await.is_err() {
            let _ = session.logout().await;
            log::warn!("email: select folder {folder} failed, treating as empty");
            return Ok(Vec::new());
        }

        // UID SEARCH (since_uid+1):* —— 只要严格大于游标的 UID。
        let lower = since_uid.saturating_add(1);
        let uids = session
            .uid_search(format!("UID {lower}:*"))
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("imap uid search: {e}")))?;

        let mut out = Vec::new();
        for uid in uids {
            // IMAP server 对 "lower:*" 在无更高 UID 时会回 lower 自身 —— 二次过滤。
            if uid <= since_uid {
                continue;
            }
            let mut stream = match session.uid_fetch(uid.to_string(), "RFC822").await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("email: uid_fetch {uid} in {folder} failed: {e}");
                    continue;
                }
            };
            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => {
                        if let Some(body) = fetch.body() {
                            out.push(FetchedMail { uid, raw: body.to_vec() });
                        }
                    }
                    Err(e) => log::warn!("email: fetch stream uid {uid} error: {e}"),
                }
            }
        }
        let _ = session.logout().await;
        Ok(out)
    }
}

impl ImapFetcher for RealImapFetcher {
    fn fetch_since(&self, folder: &str, since_uid: u32) -> Result<Vec<FetchedMail>> {
        // SourceConnector::fetch_documents 是同步契约 —— 单线程 tokio runtime
        // 桥接内部 async I/O（与 WebDavConnector::drive_blocking 同模式）。
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("imap runtime: {e}")))?;
        runtime.block_on(self.fetch_async(folder, since_uid))
    }
}
```

> `RealImapFetcher::fetch_async` 用到 `futures::StreamExt`。`async-imap` 已传递依赖 `futures`，但为显式起见在 `attune-core/Cargo.toml` 的 `[dependencies]` 加 `futures = "0.3"`（若 `cargo build` 报 `futures` 未找到则加；`async-imap` 0.11 通常已 re-export，先不加，编译失败再补）。

补 `rust/crates/attune-core/src/ingest/mod.rs` 的 `pub use`（Task 4 留的 TODO 在此补齐）：

```rust
pub use email::{EmailConfig, EmailConnector, FetchedMail, ImapFetcher, MailAttachment, MailMessage};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --test ingest_email_test`
Expected: PASS（解析 4 个 + 连接器 3 个 + 内联单元 2 个全绿）。

- [ ] **Step 5: 跑全 core 测试确认无回归**

Run: `cargo test -p attune-core`
Expected: 全绿（含 `email_accounts_test` / `ingest_email_test` / 既有 213 测试）。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-core/src/ingest/email.rs \
        rust/crates/attune-core/src/ingest/mod.rs \
        rust/crates/attune-core/tests/ingest_email_test.rs
git commit -m "feat(ingest): implement EmailConnector with IMAP UID increment

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 7：`sync_email_account` 同步入库函数（server 层）

**Files:**
- Create: `rust/crates/attune-server/src/ingest_email.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`（加 `pub mod ingest_email;`）

**职责：** route 与周期 worker 共用的入库逻辑。照搬 `ingest_webdav.rs::sync_webdav_dir` 两阶段结构。

- [ ] **Step 1: 写实现**

`rust/crates/attune-server/src/ingest_email.rs` 完整内容：

```rust
//! Email 增量同步 —— bind-email route 与周期 worker 共用的入库逻辑。

use std::sync::Arc;

use attune_core::ingest::{
    ingest_document, ingest_document_replacing, DocumentSink, EmailConfig, EmailConnector,
    IngestOutcome, RawDocument, SourceConnector,
};

use crate::state::AppState;

/// 对一个 Email 账户做一次按 UID 增量的全文件夹同步。
///
/// `corpus_domain` 回填进每份 RawDocument，驱动 F-Pro 跨域防污染前缀注入。
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
///
/// 持锁设计：IMAP 网络抓取全程不持 vault 锁；每封邮件的 DB 写操作才短暂拿锁，
/// 写完即释放，避免后台 worker 在慢网络 / 大邮箱时阻塞前台请求。
pub fn sync_email_account(
    state: &Arc<AppState>,
    dir_id: &str,
    config: EmailConfig,
    corpus_domain: &str,
) -> Result<serde_json::Value, String> {
    // 阶段 0：从 email_folder_uids 表读每文件夹的 UID 增量游标，注入连接器。
    let mut connector = EmailConnector::new(config.clone());
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        for folder in config.effective_folders() {
            let since = vault.store().get_folder_uid(dir_id, &folder).unwrap_or(0);
            connector.set_folder_since(&folder, since);
        }
    }

    // 阶段 1：锁外做全部 IMAP 网络 I/O（connect + login + fetch），物化到 Vec。
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector
            .fetch_documents(&mut sink)
            .map_err(|e| e.to_string())?;
    }

    // 阶段 2：逐文档短暂持锁做去重判断 + DB 写，写完即 drop guard。
    let mut total = 0usize;
    let mut new_items = 0usize;
    let mut updated_items = 0usize;
    let mut skipped_items = 0usize;
    let mut errors: Vec<String> = Vec::new();
    // 每文件夹本轮见到的最大 UID —— 全部成功后推进增量游标。
    let mut max_uid: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for mut doc in docs {
        total += 1;
        doc.corpus_domain = Some(corpus_domain.to_string());

        // modified_marker 形如 "INBOX:123" 或 "INBOX:123#att0" —— 取 folder + uid。
        let marker = doc.modified_marker.clone().unwrap_or_default();
        let (folder, uid) = parse_marker(&marker);
        if let Some(uid) = uid {
            let entry = max_uid.entry(folder.clone()).or_insert(0);
            *entry = (*entry).max(uid);
        }
        let source_ref = doc.source_ref.clone();

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(e) => {
                errors.push(format!("{source_ref}: vault locked: {e}"));
                continue;
            }
        };
        let store = vault.store();

        // Message-ID 增量判断：indexed_files 已记录同 source_ref 则跳过
        // （ingest_document 内部的 content_hash 短路也会兜底转发邮件）。
        let existing = store.get_indexed_file(&source_ref).ok().flatten();
        if existing.is_some() {
            skipped_items += 1;
            continue;
        }

        let outcome = ingest_document(store, &dek, &doc);
        match outcome {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                new_items += 1;
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                updated_items += 1;
            }
            Ok(IngestOutcome::Duplicate { item_id }) => {
                // 内容与已有 item 撞 hash（转发邮件）—— 记 indexed_files 避免下轮重判。
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                skipped_items += 1;
            }
            Ok(IngestOutcome::Skipped { .. }) => {
                skipped_items += 1;
            }
            Err(e) => {
                errors.push(format!("{source_ref}: ingest {e}"));
            }
        }
        // vault guard 在此隐式 drop，下一封邮件前释放锁。
    }

    // 全部处理完毕后推进每文件夹的 UID 游标 + 记录 last_sync（best-effort）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let store = vault.store();
        for (folder, uid) in &max_uid {
            let prev = store.get_folder_uid(dir_id, folder).unwrap_or(0);
            if *uid > prev {
                let _ = store.set_folder_uid(dir_id, folder, *uid);
            }
        }
        let _ = store.touch_email_account_sync(dir_id);
    }

    // 留存为 ingest_document_replacing 的导入符号扫描（unused import 触发 warning）。
    let _ = ingest_document_replacing as fn(_, _, _, _) -> _;

    Ok(serde_json::json!({
        "total_documents": total,
        "new_items": new_items,
        "updated_items": updated_items,
        "skipped_items": skipped_items,
        "errors": errors,
    }))
}

/// 拆 modified_marker（"INBOX:123" / "INBOX:123#att0"）为 (folder, Option<uid>)。
/// 附件 marker 含 "#attN" 后缀，uid 仍取冒号后到 '#' 前的数字段。
fn parse_marker(marker: &str) -> (String, Option<u32>) {
    let (folder, rest) = match marker.split_once(':') {
        Some((f, r)) => (f.to_string(), r),
        None => return (marker.to_string(), None),
    };
    let uid_str = rest.split('#').next().unwrap_or(rest);
    (folder, uid_str.parse::<u32>().ok())
}

#[cfg(test)]
mod tests {
    use super::parse_marker;

    #[test]
    fn parse_marker_handles_plain_and_attachment() {
        assert_eq!(parse_marker("INBOX:42"), ("INBOX".to_string(), Some(42)));
        assert_eq!(parse_marker("Sent:7#att0"), ("Sent".to_string(), Some(7)));
        assert_eq!(parse_marker("garbage"), ("garbage".to_string(), None));
    }
}
```

> 说明：上面 `let _ = ingest_document_replacing as fn(...)` 是为避免「导入但未用」warning。**更干净的做法**是直接不导入 `ingest_document_replacing`——本函数的 Email 增量语义是「Message-ID 没见过才入库，见过就跳过」，不存在「同 Message-ID 内容变了要替换旧 item」的场景（邮件不可变）。**实现时请删掉 `ingest_document_replacing` 的导入和那行 `let _ = ...`**，`use` 行改为 `use attune_core::ingest::{ingest_document, DocumentSink, EmailConfig, EmailConnector, IngestOutcome, RawDocument, SourceConnector};`。此处保留注释是提醒 reviewer 这个决策。

- [ ] **Step 2: 改用干净 import（落实上面说明）**

把 `ingest_email.rs` 顶部 `use` 改为：

```rust
use attune_core::ingest::{
    ingest_document, DocumentSink, EmailConfig, EmailConnector, IngestOutcome, RawDocument,
    SourceConnector,
};
```

并删除函数体末尾 `let _ = ingest_document_replacing as fn(_, _, _, _) -> _;` 那一行及其上方注释。

- [ ] **Step 3: 注册模块**

在 `rust/crates/attune-server/src/lib.rs` 模块声明区（`mod ingest_webdav;` 附近）加：

```rust
pub mod ingest_email;
```

- [ ] **Step 4: 验证编译 + 跑单元测试**

Run: `cargo test -p attune-server ingest_email`
Expected: PASS（`parse_marker_handles_plain_and_attachment` 绿；编译通过）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/ingest_email.rs \
        rust/crates/attune-server/src/lib.rs
git commit -m "feat(server): add sync_email_account ingest function

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 8：Email HTTP API route（账户 CRUD + 手动同步）

**Files:**
- Create: `rust/crates/attune-server/src/routes/email.rs`
- Modify: `rust/crates/attune-server/src/routes/mod.rs`（加 `pub mod email;`）
- Modify: `rust/crates/attune-server/src/lib.rs`（注册 4 条 route）

**API 设计（kebab-case 路径）：**

| 方法 | 路径 | 职责 |
|------|------|------|
| `GET` | `/api/v1/index/email-accounts` | 列出已配置 Email 账户（不含密码）|
| `POST` | `/api/v1/index/bind-email` | 新增 / 更新一个账户并立即跑首轮同步 |
| `DELETE` | `/api/v1/index/email-accounts/{dir_id}` | 删除账户（已入库内容保留）|
| `POST` | `/api/v1/index/email-accounts/{dir_id}/sync` | 手动触发一次增量同步 |

- [ ] **Step 1: 写实现**

`rust/crates/attune-server/src/routes/email.rs` 完整内容：

```rust
//! Email IMAP 采集账户 route —— 账户 CRUD + 手动同步触发。

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use attune_core::ingest::EmailConfig;
use attune_core::store::email_accounts::EmailAccountInput;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// 默认同步文件夹 —— body 未给 folders 时用 INBOX + Sent。
fn default_folders() -> Vec<String> {
    vec!["INBOX".to_string(), "Sent".to_string()]
}

#[derive(Deserialize)]
pub struct BindEmailRequest {
    pub host: String,
    #[serde(default = "default_imap_port")]
    pub port: u16,
    pub username: String,
    pub password: String,
    #[serde(default = "default_folders")]
    pub folders: Vec<String>,
    #[serde(default)]
    pub corpus_domain: Option<String>,
}

fn default_imap_port() -> u16 {
    993
}

#[derive(Serialize)]
pub struct EmailAccountView {
    pub dir_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub folders: Vec<String>,
    pub corpus_domain: String,
    pub last_sync: Option<String>,
}

/// 校验账户输入。host / username / password 不能为空，port 必须非 0。
fn validate(req: &BindEmailRequest) -> Result<(), AppError> {
    if req.host.trim().is_empty() {
        return Err(AppError::BadRequest("host must not be empty".into()));
    }
    if req.username.trim().is_empty() {
        return Err(AppError::BadRequest("username must not be empty".into()));
    }
    if req.password.is_empty() {
        return Err(AppError::BadRequest("password must not be empty".into()));
    }
    if req.port == 0 {
        return Err(AppError::BadRequest("port must not be zero".into()));
    }
    Ok(())
}

/// GET /api/v1/index/email-accounts —— 列出已配置账户（不含密码）。
pub async fn list_email_accounts(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db()?;
    let rows = vault.store().list_email_accounts(&dek)?;
    let accounts: Vec<EmailAccountView> = rows
        .into_iter()
        .map(|r| EmailAccountView {
            dir_id: r.dir_id,
            host: r.host,
            port: r.port,
            username: r.username,
            folders: r.folders,
            corpus_domain: r.corpus_domain,
            last_sync: r.last_sync,
        })
        .collect();
    Ok(Json(serde_json::json!({ "accounts": accounts })))
}

/// POST /api/v1/index/bind-email —— 新增 / 更新账户并立即跑首轮同步。
pub async fn bind_email(
    State(state): State<SharedState>,
    Json(req): Json<BindEmailRequest>,
) -> AppResult<Json<serde_json::Value>> {
    validate(&req)?;
    let corpus_domain = req
        .corpus_domain
        .clone()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| "general".to_string());

    // 创建 / 复用 bound_dirs 记录（email: 前缀标记邮箱源）+ 落库加密账户配置。
    let dir_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let store = vault.store();
        let path = format!("email:{}@{}", req.username, req.host);
        let dir_id = store
            .bind_directory(&path, false, &["eml"])
            .map_err(|e| AppError::Internal(format!("bind email dir: {e}")))?;
        let input = EmailAccountInput {
            dir_id: dir_id.clone(),
            host: req.host.clone(),
            port: req.port,
            username: req.username.clone(),
            password: req.password.clone(),
            folders: req.folders.clone(),
            corpus_domain: corpus_domain.clone(),
        };
        store
            .upsert_email_account(&dek, &input)
            .map_err(|e| AppError::Internal(format!("persist email account: {e}")))?;
        dir_id
    };

    // 首轮同步在阻塞线程跑（IMAP 网络 I/O + DB 写）—— 不阻塞 axum worker。
    let config = EmailConfig {
        host: req.host.clone(),
        port: req.port,
        username: req.username.clone(),
        password: req.password.clone(),
        folders: req.folders.clone(),
    };
    let state_cloned = state.clone();
    let dir_cloned = dir_id.clone();
    let domain_cloned = corpus_domain.clone();
    let stats = tokio::task::spawn_blocking(move || {
        crate::ingest_email::sync_email_account(
            &state_cloned,
            &dir_cloned,
            config,
            &domain_cloned,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("email sync task join: {e}")))?
    .map_err(AppError::BadGateway)?;

    Ok(Json(serde_json::json!({
        "dir_id": dir_id,
        "sync": stats,
    })))
}

/// DELETE /api/v1/index/email-accounts/{dir_id} —— 删除账户（已入库内容保留）。
pub async fn delete_email_account(
    State(state): State<SharedState>,
    Path(dir_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db()?;
    let store = vault.store();
    store
        .delete_email_account(&dir_id)
        .map_err(|e| AppError::Internal(format!("delete email account: {e}")))?;
    // bound_dirs 记录一并解绑（email_folder_uids 经 ON DELETE CASCADE 已清）。
    let _ = store.unbind_directory(&dir_id);
    Ok(Json(serde_json::json!({ "deleted": dir_id })))
}

/// POST /api/v1/index/email-accounts/{dir_id}/sync —— 手动触发一次增量同步。
pub async fn sync_email_account_now(
    State(state): State<SharedState>,
    Path(dir_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let (config, corpus_domain) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        let row = vault
            .store()
            .get_email_account(&dek, &dir_id)?
            .ok_or_else(|| AppError::NotFound(format!("email account {dir_id}")))?;
        let config = EmailConfig {
            host: row.host,
            port: row.port,
            username: row.username,
            password: row.password,
            folders: row.folders,
        };
        (config, row.corpus_domain)
    };

    let state_cloned = state.clone();
    let dir_cloned = dir_id.clone();
    let stats = tokio::task::spawn_blocking(move || {
        crate::ingest_email::sync_email_account(
            &state_cloned,
            &dir_cloned,
            config,
            &corpus_domain,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("email sync task join: {e}")))?
    .map_err(AppError::BadGateway)?;

    Ok(Json(serde_json::json!({
        "dir_id": dir_id,
        "sync": stats,
    })))
}
```

> `unbind_directory` 假定存在于 `Store`（`scanner` / `index` route 已用）。若实际方法名不同，实现时 `grep -rn "fn unbind_directory\|fn unbind_dir" rust/crates/attune-core/src/store/` 核对后改名。

- [ ] **Step 2: 注册模块**

在 `rust/crates/attune-server/src/routes/mod.rs` 加（按字母序，`pub mod demo;` 之后）：

```rust
pub mod email;
```

- [ ] **Step 3: 注册 4 条 route**

在 `rust/crates/attune-server/src/lib.rs` 的 `bind-remote` route 那一行（`:213`）之后加：

```rust
        .route("/api/v1/index/email-accounts", get(routes::email::list_email_accounts))
        .route("/api/v1/index/bind-email", post(routes::email::bind_email))
        .route(
            "/api/v1/index/email-accounts/{dir_id}",
            axum::routing::delete(routes::email::delete_email_account),
        )
        .route(
            "/api/v1/index/email-accounts/{dir_id}/sync",
            post(routes::email::sync_email_account_now),
        )
```

> `get` / `post` 已在 `lib.rs` 顶部 `use axum::routing::{get, post}` 导入；`delete` 用全路径 `axum::routing::delete` 避免与 `Store::delete_*` 视觉混淆，无需改 import。

- [ ] **Step 4: 验证编译**

Run: `cargo build -p attune-server`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/email.rs \
        rust/crates/attune-server/src/routes/mod.rs \
        rust/crates/attune-server/src/lib.rs
git commit -m "feat(server): add email account CRUD + manual sync routes

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 9：Email 周期同步 worker

**Files:**
- Modify: `rust/crates/attune-server/src/state.rs`（加字段 + 初始化 + `start_email_sync_worker`）
- Modify: `rust/crates/attune-server/src/routes/vault.rs`（3 处 unlock 点启动 worker）

- [ ] **Step 1: 加 AtomicBool 字段**

在 `state.rs` 的 `AppState` struct 定义里、`webdav_sync_worker_running: AtomicBool,`（`:73`）那一行之后加：

```rust
    /// Email 周期同步 worker 运行标志（防重入）。
    pub email_sync_worker_running: AtomicBool,
```

在 `AppState` 构造里（`webdav_sync_worker_running: AtomicBool::new(false),` 附近）加：

```rust
            email_sync_worker_running: AtomicBool::new(false),
```

- [ ] **Step 2: 加 `start_email_sync_worker` 方法**

在 `state.rs` 的 `start_webdav_sync_worker` 方法（`:751-813`）之后加（结构照搬 `start_webdav_sync_worker`）：

```rust
    /// 启动 Email 周期同步 worker：每 15 分钟从 email_accounts 表读全部账户 +
    /// 解密凭据，逐个按 UID 增量同步。原子 flag 防重入 + RAII guard 复位。
    pub fn start_email_sync_worker(state: std::sync::Arc<AppState>) {
        if state
            .email_sync_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Email sync worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.email_sync_worker_running);

            tracing::info!("Email sync worker started");
            loop {
                // vault 锁定则退出 —— 下次 unlock 会重新 start。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                // 从 email_accounts 表读全部账户 + 解密凭据（snapshot 后释放锁）。
                let accounts: Vec<attune_core::store::email_accounts::EmailAccountRow> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let dek = match vault.dek_db() {
                        Ok(k) => k,
                        Err(_) => break,
                    };
                    vault.store().list_email_accounts(&dek).unwrap_or_default()
                };

                for account in accounts {
                    let config = attune_core::ingest::EmailConfig {
                        host: account.host.clone(),
                        port: account.port,
                        username: account.username.clone(),
                        password: account.password.clone(),
                        folders: account.folders.clone(),
                    };
                    // 只打印 dir_id / host / username，不 log password。
                    tracing::info!(
                        "Email sync: account dir={} host={} user={}",
                        account.dir_id,
                        account.host,
                        account.username
                    );
                    if let Err(e) = crate::ingest_email::sync_email_account(
                        &state,
                        &account.dir_id,
                        config,
                        &account.corpus_domain,
                    ) {
                        tracing::warn!(
                            "Email sync for account {} failed: {e}",
                            account.dir_id
                        );
                    }
                }

                // unlock 后立即跑首轮，之后每 15 分钟一次。
                std::thread::sleep(std::time::Duration::from_secs(15 * 60));
            }
            tracing::info!("Email sync worker stopped (vault locked)");
        });
    }
```

- [ ] **Step 3: 在 vault unlock 成功点启动 worker**

在 `rust/crates/attune-server/src/routes/vault.rs` 的 3 处 `start_webdav_sync_worker(state.clone());` 调用（`:73` / `:99` / `:204`）后各加一行：

```rust
    crate::state::AppState::start_email_sync_worker(state.clone());
```

- [ ] **Step 4: 验证编译**

Run: `cargo build -p attune-server`
Expected: PASS。

- [ ] **Step 5: 跑 server 全测试确认无回归**

Run: `cargo test -p attune-server`
Expected: 全绿。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-server/src/state.rs \
        rust/crates/attune-server/src/routes/vault.rs
git commit -m "feat(server): add periodic email sync worker

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 10：i18n key（zh + en）

**Files:**
- Modify: `rust/crates/attune-server/ui/src/i18n/zh.ts`
- Modify: `rust/crates/attune-server/ui/src/i18n/en.ts`

- [ ] **Step 1: 加 zh key**

在 `zh.ts` 的 `remote.*` key 区块末尾（`'remote.webdav.bind'` 那组之后）加：

```ts
  'email.section.title': '邮箱采集',
  'email.section.desc': '通过 IMAP 把邮件正文和文档附件自动索引进知识库',
  'email.action.add': '添加邮箱',
  'email.action.sync_now': '立即同步',
  'email.empty.title': '还没添加任何邮箱',
  'email.modal.add.title': '添加 IMAP 邮箱',
  'email.field.host': 'IMAP 服务器',
  'email.field.host_hint': '如 imap.gmail.com',
  'email.field.port': '端口',
  'email.field.username': '邮箱地址',
  'email.field.password': '密码 / 应用专用密码',
  'email.field.password_hint': 'Gmail / Outlook 建议用应用专用密码',
  'email.field.folders': '同步文件夹',
  'email.field.folders_hint': '逗号分隔，默认 INBOX,Sent',
  'email.field.bind': '添加并同步',
  'email.row.last_sync': '上次同步',
  'email.row.never_synced': '尚未同步',
  'email.row.folders': '文件夹',
  'email.row.delete': '删除',
  'email.confirm.delete': '删除邮箱 {username}？已索引的邮件保留，但不再自动同步。',
  'email.toast.add_success': '已添加，开始首次同步',
  'email.toast.add_fail': '添加失败：{error}',
  'email.toast.sync_success': '同步完成：新增 {count} 封',
  'email.toast.sync_fail': '同步失败：{error}',
  'email.toast.delete_success': '已删除邮箱',
  'email.toast.delete_fail': '删除失败',
  'email.error.unknown': '未知错误',
```

- [ ] **Step 2: 加 en key（key 集合必须与 zh 完全一致）**

在 `en.ts` 对应位置（`'remote.webdav.bind'` 那组之后）加：

```ts
  'email.section.title': 'Email Ingest',
  'email.section.desc': 'Auto-index email bodies and document attachments via IMAP',
  'email.action.add': 'Add Mailbox',
  'email.action.sync_now': 'Sync Now',
  'email.empty.title': 'No mailbox added yet',
  'email.modal.add.title': 'Add IMAP Mailbox',
  'email.field.host': 'IMAP Server',
  'email.field.host_hint': 'e.g. imap.gmail.com',
  'email.field.port': 'Port',
  'email.field.username': 'Email Address',
  'email.field.password': 'Password / App Password',
  'email.field.password_hint': 'Use an app-specific password for Gmail / Outlook',
  'email.field.folders': 'Folders to Sync',
  'email.field.folders_hint': 'Comma-separated; defaults to INBOX,Sent',
  'email.field.bind': 'Add and Sync',
  'email.row.last_sync': 'Last sync',
  'email.row.never_synced': 'Not synced yet',
  'email.row.folders': 'Folders',
  'email.row.delete': 'Delete',
  'email.confirm.delete': 'Delete mailbox {username}? Indexed emails are kept but no longer auto-synced.',
  'email.toast.add_success': 'Added; first sync started',
  'email.toast.add_fail': 'Add failed: {error}',
  'email.toast.sync_success': 'Sync done: {count} new',
  'email.toast.sync_fail': 'Sync failed: {error}',
  'email.toast.delete_success': 'Mailbox deleted',
  'email.toast.delete_fail': 'Delete failed',
  'email.error.unknown': 'Unknown error',
```

- [ ] **Step 3: 验证 zh / en key 集合一致**

Run:
```bash
cd rust/crates/attune-server/ui/src && diff \
  <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) \
  <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```
Expected: 无输出（两文件 key 集合完全一致）。

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-server/ui/src/i18n/zh.ts \
        rust/crates/attune-server/ui/src/i18n/en.ts
git commit -m "feat(ui): add email ingest i18n keys (zh + en)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 11：Email 账户 API 封装 `useEmail.ts`

**Files:**
- Create: `rust/crates/attune-server/ui/src/hooks/useEmail.ts`

- [ ] **Step 1: 写实现（照搬 `useRemote.ts` 的 `api` + `ApiError` 范式）**

`rust/crates/attune-server/ui/src/hooks/useEmail.ts` 完整内容：

```ts
/** useEmail · IMAP 邮箱采集账户管理 */
import { api } from '../store/api';
import { ApiError } from '../store/api';

export type EmailAccount = {
  dir_id: string;
  host: string;
  port: number;
  username: string;
  folders: string[];
  corpus_domain: string;
  last_sync?: string;
};

export type EmailSyncStats = {
  total_documents: number;
  new_items: number;
  updated_items: number;
  skipped_items: number;
  errors: string[];
};

export type EmailActionResult = {
  ok: boolean;
  error?: string;
  stats?: EmailSyncStats;
};

export type EmailAccountInput = {
  host: string;
  port: number;
  username: string;
  password: string;
  folders: string[];
};

type ListResponse = { accounts: EmailAccount[] };
type SyncResponse = { dir_id: string; sync: EmailSyncStats };

export async function listEmailAccounts(): Promise<EmailAccount[]> {
  try {
    const res = await api.get<ListResponse>('/index/email-accounts');
    return res.accounts ?? [];
  } catch {
    return [];
  }
}

export async function addEmailAccount(input: EmailAccountInput): Promise<EmailActionResult> {
  try {
    const res = await api.post<SyncResponse>('/index/bind-email', input);
    return { ok: true, stats: res.sync };
  } catch (e: unknown) {
    return { ok: false, error: toErrorMessage(e) };
  }
}

export async function deleteEmailAccount(dirId: string): Promise<boolean> {
  try {
    await api.delete(`/index/email-accounts/${encodeURIComponent(dirId)}`);
    return true;
  } catch {
    return false;
  }
}

export async function syncEmailAccount(dirId: string): Promise<EmailActionResult> {
  try {
    const res = await api.post<SyncResponse>(
      `/index/email-accounts/${encodeURIComponent(dirId)}/sync`,
      {},
    );
    return { ok: true, stats: res.sync };
  } catch (e: unknown) {
    return { ok: false, error: toErrorMessage(e) };
  }
}

function toErrorMessage(e: unknown): string {
  if (e instanceof ApiError) {
    try {
      const parsed = JSON.parse(e.body) as { error?: string };
      return parsed.error?.trim() || e.body;
    } catch {
      return e.body;
    }
  }
  return e instanceof Error ? e.message : String(e);
}
```

> `api.get` / `api.post` / `api.delete` 与 `ApiError` 来自 `../store/api`，与 `useRemote.ts` 同源。`api.post` 第二参为 request body；`/index/email-accounts/{dir}/sync` 无 body 传 `{}`。

- [ ] **Step 2: 验证 TypeScript 编译**

Run: `cd rust/crates/attune-server/ui && npm run build`
Expected: 构建通过（`useEmail.ts` 暂未被任何视图引用，仅类型检查它本身）。

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-server/ui/src/hooks/useEmail.ts
git commit -m "feat(ui): add email account API hook

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 12：Email 账户 UI 区块（接入 RemoteView）

**Files:**
- Modify: `rust/crates/attune-server/ui/src/views/RemoteView.tsx`

**职责：** 在 Remote 视图（已有本地 + WebDAV 目录管理）下方加一个 Email 账户区块——账户列表 + 添加模态 + 每账户「立即同步」与「删除」按钮。

- [ ] **Step 1: 加 import**

在 `RemoteView.tsx` 顶部 import 区，`import type { BoundDir } from '../hooks/useRemote';` 之后加：

```tsx
import {
  listEmailAccounts,
  addEmailAccount,
  deleteEmailAccount,
  syncEmailAccount,
} from '../hooks/useEmail';
import type { EmailAccount } from '../hooks/useEmail';
```

- [ ] **Step 2: 在 `RemoteView` 组件内加 Email 区块（追加到 return 的根 `<div>` 末尾，最后一个 `</Modal>` 之后、根 `</div>` 之前）**

```tsx
      <EmailSection />
```

- [ ] **Step 3: 在文件末尾加 `EmailSection` 组件 + `EmailAddForm` 子组件**

```tsx
function EmailSection(): JSX.Element {
  const accounts = useSignal<EmailAccount[]>([]);
  const loading = useSignal(true);
  const adding = useSignal(false);
  const syncing = useSignal<string | null>(null);

  async function refresh() {
    loading.value = true;
    accounts.value = await listEmailAccounts();
    loading.value = false;
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function handleSync(a: EmailAccount) {
    syncing.value = a.dir_id;
    const result = await syncEmailAccount(a.dir_id);
    syncing.value = null;
    if (result.ok) {
      toast('success', t('email.toast.sync_success', { count: result.stats?.new_items ?? 0 }));
      await refresh();
    } else {
      toast('error', t('email.toast.sync_fail', { error: result.error ?? t('email.error.unknown') }));
    }
  }

  async function handleDelete(a: EmailAccount) {
    if (!confirm(t('email.confirm.delete', { username: a.username }))) return;
    const ok = await deleteEmailAccount(a.dir_id);
    if (ok) {
      toast('success', t('email.toast.delete_success'));
      await refresh();
    } else {
      toast('error', t('email.toast.delete_fail'));
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <header style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <h3 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}>
            {`📬 ${t('email.section.title')}`}
          </h3>
          <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 2 }}>
            {t('email.section.desc')}
          </div>
        </div>
        <Button variant="primary" size="sm" onClick={() => (adding.value = true)}>
          {t('email.action.add')}
        </Button>
      </header>

      {loading.value ? (
        <div style={{ color: 'var(--color-text-secondary)' }}>{t('common.loading')}</div>
      ) : accounts.value.length === 0 ? (
        <EmptyState icon="📬" title={t('email.empty.title')} description={t('email.section.desc')} />
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
          {accounts.value.map((a) => (
            <div
              key={a.dir_id}
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
              <span aria-hidden="true" style={{ fontSize: 20 }}>📬</span>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}>
                  {a.username}
                </div>
                <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 2 }}>
                  {a.host} · {t('email.row.folders')}: {a.folders.join(', ')}
                  {' · '}
                  {t('email.row.last_sync')}:{' '}
                  {a.last_sync ? new Date(a.last_sync).toLocaleString() : t('email.row.never_synced')}
                </div>
              </div>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => void handleSync(a)}
                disabled={syncing.value === a.dir_id}
              >
                {syncing.value === a.dir_id ? t('common.loading') : t('email.action.sync_now')}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => void handleDelete(a)}>
                {t('email.row.delete')}
              </Button>
            </div>
          ))}
        </div>
      )}

      <Modal
        open={adding.value}
        onClose={() => (adding.value = false)}
        title={t('email.modal.add.title')}
        maxWidth={520}
      >
        <EmailAddForm
          onDone={async (result) => {
            adding.value = false;
            if (result.ok) {
              toast('success', t('email.toast.add_success'));
              await refresh();
            } else {
              toast('error', t('email.toast.add_fail', { error: result.error ?? t('email.error.unknown') }));
            }
          }}
        />
      </Modal>
    </div>
  );
}

function EmailAddForm({
  onDone,
}: {
  onDone: (result: { ok: boolean; error?: string }) => void;
}): JSX.Element {
  const host = useSignal('');
  const port = useSignal('993');
  const username = useSignal('');
  const password = useSignal('');
  const folders = useSignal('INBOX,Sent');
  const submitting = useSignal(false);

  async function submit() {
    if (!host.value.trim() || !username.value.trim() || !password.value) return;
    submitting.value = true;
    const result = await addEmailAccount({
      host: host.value.trim(),
      port: Number(port.value) || 993,
      username: username.value.trim(),
      password: password.value,
      folders: folders.value
        .split(',')
        .map((f) => f.trim())
        .filter((f) => f.length > 0),
    });
    submitting.value = false;
    onDone(result);
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <Input
        label={t('email.field.host')}
        placeholder={t('email.field.host_hint')}
        value={host.value}
        onInput={(e) => (host.value = (e.target as HTMLInputElement).value)}
      />
      <Input
        label={t('email.field.port')}
        value={port.value}
        onInput={(e) => (port.value = (e.target as HTMLInputElement).value)}
      />
      <Input
        label={t('email.field.username')}
        value={username.value}
        onInput={(e) => (username.value = (e.target as HTMLInputElement).value)}
      />
      <Input
        label={t('email.field.password')}
        type="password"
        placeholder={t('email.field.password_hint')}
        value={password.value}
        onInput={(e) => (password.value = (e.target as HTMLInputElement).value)}
      />
      <Input
        label={t('email.field.folders')}
        placeholder={t('email.field.folders_hint')}
        value={folders.value}
        onInput={(e) => (folders.value = (e.target as HTMLInputElement).value)}
      />
      <Button variant="primary" onClick={() => void submit()} disabled={submitting.value}>
        {submitting.value ? t('common.loading') : t('email.field.bind')}
      </Button>
    </div>
  );
}
```

> `Button` / `EmptyState` / `Modal` / `Input` / `toast` / `t` / `useSignal` / `useEffect` 已在 `RemoteView.tsx` 顶部导入（本计划 Step 1 只补 `useEmail` import）。`Input` 组件 props（`label` / `placeholder` / `type` / `value` / `onInput`）与 `useRemote` 的 `WebdavForm` 用法一致——若实际 `Input` API 不同，实现时参照 `RemoteView.tsx` 现有 `WebdavForm` 的 `<Input>` 用法对齐。

- [ ] **Step 4: 验证 TypeScript 构建**

Run: `cd rust/crates/attune-server/ui && npm run build`
Expected: 构建通过。

- [ ] **Step 5: i18n grep 守卫（确认无硬编码中文）**

Run:
```bash
cd rust/crates/attune-server/ui/src && \
grep -rnP "(toast\([^)]*'[^']*[\x{4e00}-\x{9fff}]|(title|placeholder|label|description|aria-label)=\"[^\"]*[\x{4e00}-\x{9fff}]|>[^<{]*[\x{4e00}-\x{9fff}])" --include="*.tsx" views/RemoteView.tsx | grep -v "/i18n/"
```
Expected: 无输出（RemoteView 新增代码无硬编码中文 UI 字面量）。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-server/ui/src/views/RemoteView.tsx
git commit -m "feat(ui): add email account section to remote view

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 13：端到端手测 + 文档同步

**Files:**
- Modify: `rust/README.md` / `rust/DEVELOP.md`（受影响章节）

- [ ] **Step 1: 全量编译 + 测试**

Run:
```bash
cargo build --workspace && cargo test -p attune-core && cargo test -p attune-server
```
Expected: 全绿。

- [ ] **Step 2: 真实 IMAP 手测（Gmail App Password）**

启动 server，浏览器打开 Web UI → Remote 视图 → Email Ingest 区块：
1. 点「添加邮箱」，填 `imap.gmail.com` / `993` / Gmail 地址 / App Password / `INBOX`。
2. 提交后应 toast「已添加，开始首次同步」，账户出现在列表，`last_sync` 有时间。
3. Knowledge 视图应能搜到 INBOX 邮件标题 / 正文。
4. 再点该账户「立即同步」——若无新邮件，toast 显示「新增 0 封」（UID 增量生效）。
5. 发一封带 PDF 附件的测试邮件给自己，再「立即同步」——附件应作为独立 item 入库且经文字层 / OCR 提取。
6. 填错密码添加另一账户——应 toast 结构化错误（`bad gateway` / `imap login` 文案），不白屏不 panic。

记录手测结果到 PR 描述（不写入仓库文档）。

- [ ] **Step 3: 文档同步**

在 `rust/README.md` 的「采集源 / Remote」相关章节加一句 Email IMAP 采集已支持；在 `rust/DEVELOP.md` 的采集体系章节补 `EmailConnector` 一行说明（与 `WebDavConnector` 并列）。保持简洁，不新增独立文档。

- [ ] **Step 4: Commit**

```bash
git add rust/README.md rust/DEVELOP.md
git commit -m "docs: note email IMAP ingest source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 2：OAuth2 认证（蓝图级后续 task，不在 MVP）

> 本 Phase **不展开到代码**——记录设计方向，待 MVP 上线、有真实用户需求后再起独立计划。

**目标：** Gmail / Outlook 用户无需生成 App Password，用 OAuth2 授权登录 IMAP（IMAP `AUTHENTICATE XOAUTH2`）。

**设计要点：**
- **授权流**：loopback redirect（`http://127.0.0.1:<随机端口>/oauth/callback`）+ PKCE。attune 本地起一个一次性 HTTP listener 收 authorization code，换 access token + refresh token。不需要公网回调 URL。
- **provider 配置**：Gmail（`https://accounts.google.com/o/oauth2/v2/auth` + scope `https://mail.google.com/`）、Outlook（Microsoft identity platform + scope `https://outlook.office.com/IMAP.AccessAsUser.All`）。client_id 内置（公开 OAuth client，PKCE 保证安全）。
- **token 存储**：`email_accounts` 表加 `oauth_refresh_token_enc BLOB` + `auth_type TEXT`（`password` / `oauth2`）列。`auth_type='oauth2'` 时 `password_enc` 留空，凭据走 refresh token；与现有字段级 AES-GCM 加密同模式。
- **token 刷新**：`RealImapFetcher` 在 `auth_type='oauth2'` 分支用 refresh token 换 access token（HTTP POST 到 provider token endpoint），再以 `AUTHENTICATE XOAUTH2` 登录 IMAP。`async-imap` 支持自定义 SASL authenticator。
- **新依赖评估**：`oauth2` crate（纯 Rust，rustls 可选）做授权流；不引 Google/MS SDK。
- **UI**：Email 添加模态加「认证方式」单选（密码 / Google 登录 / Microsoft 登录）；选 OAuth 时弹系统浏览器走授权，回调后自动回填账户。
- **不做**：服务端常驻 OAuth；多租户 client 管理；Gmail History API push（仍走 IMAP UID 轮询）。

**触发条件：** MVP 上线后收集到「App Password 门槛劝退用户」的真实反馈，再起 `docs/superpowers/plans/<date>-ingest-email-oauth2.md`。

---

## 风险与回滚

| 风险 | 缓解 | 回滚 |
|------|------|------|
| `async-imap` 0.11 传递依赖引入 `native-tls` / `openssl-sys` | Task 1 Step 3 `cargo tree -i native-tls` 显式校验；用 `default-features = false` + `tokio-rustls` 自建 TLS 喂连接器 | 若无法避免 native-tls：评估 `imap` crate（同步版）+ `rustls` 手搓连接，仍走 `drive_blocking` 桥接 |
| `mail-parser` 0.11 API 与计划假设不符（`body_text` / `attachments` / `attachment_name` 签名漂移） | 实现 Task 4 时先 `cargo doc --open -p mail-parser` 核对真实 API；解析逻辑全在 `parse_email_bytes` 一个纯函数内，离线测试快速暴露偏差 | 仅 `parse_email_bytes` 内部调整，不影响 `EmailConnector` / route / worker |
| 大邮箱首轮同步耗时长，前台请求超时 | `bind_email` 首轮同步在 `spawn_blocking` 跑；`fetch_documents` 逐封 `sink()` 不物化全部；增量游标保证二次同步只拉新邮件 | 若首轮仍过久：改 `bind_email` 立即返回 `dir_id` + 把首轮同步丢给后台 worker（不在 route 内 `await`）|
| IMAP server 对 `UID n:*` 在无更高 UID 时回 `n` 自身，导致重复处理 | `RealImapFetcher::fetch_async` 二次过滤 `uid <= since_uid`；`sync_email_account` 再靠 `indexed_files` 的 `source_ref` 命中跳过；`ingest_document` 的 `content_hash` 短路三重兜底 | 无需回滚，三层去重已覆盖 |
| 周期 worker 与手动同步并发跑同一账户，重复入库 | 三层去重（UID 游标 / `indexed_files` / `content_hash`）保证幂等——并发最坏只是重复 fetch，不重复入库 | 若需要严格互斥：给 `sync_email_account` 加 per-`dir_id` 的 `Mutex`，但 MVP 不必要 |
| `Store::unbind_directory` 实际方法名不符 | Task 8 Step 1 注释已提示 `grep` 核对 | 改 `delete_email_account` route 里的方法名 |

**整体回滚：** 本计划新增文件为主，对既有路径的修改仅「加依赖 / 加表 DDL / 加模块声明 / 加 route 注册 / 加 worker 字段与启动调用 / RemoteView 末尾加区块」——均为追加式。最坏情况 `git revert` 全部 commit 即可，不触碰既有 4 条入库路径与 WebDAV 采集逻辑。

---

## Self-Review

按 writing-plans 的 Self-Review checklist 自查（fresh-eyes 对照任务背景）：

**1. 需求覆盖**
- `EmailConnector` impl `SourceConnector` → Task 6 ✅
- `email_accounts` 加密表 + CRUD → Task 2（DDL）+ Task 3（CRUD）✅
- IMAP 连接/认证/UID 增量抓取 → Task 5（`ImapFetcher` 抽象）+ Task 6（`RealImapFetcher` + `UID SEARCH n:*`）✅
- mail-parser 正文 + 附件提取 → Task 4（`parse_email_bytes` + `html_to_text` + 附件白名单）✅
- 入库走 `ingest_document` → Task 7（`sync_email_account` 调 `ingest_document`，连接器不重复 pipeline）✅
- 后台周期同步 worker → Task 9（`start_email_sync_worker`，15 min，照搬 webdav worker）✅
- settings UI（账户增删 + 立即同步，i18n zh/en 同步）→ Task 10（i18n）+ Task 11（hook）+ Task 12（RemoteView 区块）✅
- HTTP API（账户 CRUD + 手动同步，kebab-case）→ Task 8（4 条 route：`email-accounts` GET / `bind-email` POST / `email-accounts/{id}` DELETE / `email-accounts/{id}/sync` POST）✅
- 测试（mock IMAP / 加解密往返 / 增量 UID / Message-ID 去重）→ `email_accounts_test`（加解密 + folder UID 游标）+ `ingest_email_test`（`parse_email_bytes` 离线 + `MockImapFetcher` 驱动 + `since_uid` 增量）+ `ingest_email.rs::parse_marker` 单元测试 ✅
- OAuth2 蓝图级后续 task（不展开代码）→ Phase 2 ✅

**2. Placeholder 扫描**：无 `TODO` / `TBD` / 「类似 Task N」/ 「适当处理错误」。每个 code step 给出完整可编译代码。Task 4 Step 4 的「`pub use` 在 Task 6 补」是**有意的依序约束**（`EmailConnector` 此时未定义），已在 Task 4 正文与 Task 6 Step 3 双向说明，不是 placeholder。Task 7 Step 1 故意先给「带多余 import」版本再 Step 2 收敛为干净 import——是为了让 reviewer 看到「邮件不可变 → 不需要 `ingest_document_replacing`」这个决策；实现者按 Step 2 落最终形态。

**3. 类型一致性**：
- `EmailConfig` 字段（`host`/`port`/`username`/`password`/`folders`）— Task 5 定义，Task 6 / Task 7 / Task 8 / Task 9 一致使用；`effective_folders()` Task 5 定义、Task 6 与 Task 7 调用。
- `EmailConnector::with_fetcher` / `new` / `set_folder_since` — Task 6 定义，测试（Task 6）与 `sync_email_account`（Task 7）一致调用。
- `EmailAccountInput` / `EmailAccountRow` — Task 3 定义，Task 8 / Task 9 一致使用；`EmailAccountRow` 含 `last_sync`，UI `EmailAccount` 类型（Task 11）字段对齐。
- `FetchedMail { uid, raw }` / `ImapFetcher::fetch_since` — Task 5 定义，Task 6 `RealImapFetcher` 与 `MockImapFetcher` 一致 impl。
- `MailMessage` / `MailAttachment` — Task 4 定义，`EmailConnector::emit_mail`（Task 6）一致使用。
- `sync_email_account` 返回 JSON 字段（`total_documents`/`new_items`/`updated_items`/`skipped_items`/`errors`）— Task 7 定义，UI `EmailSyncStats`（Task 11）字段对齐。
- route 路径 kebab-case：`email-accounts` / `bind-email` / `email-accounts/{dir_id}` / `email-accounts/{dir_id}/sync` — Task 8 注册，`useEmail.ts`（Task 11）一致调用。

发现并已修正：Task 11 `EmailSyncStats` 与 Task 7 `sync_email_account` 返回 JSON 字段名核对一致（`new_items` 非 `new_files`）；UI `email.toast.sync_success` 用 `result.stats?.new_items`，与之对齐。

**仍需用户拍板的存疑点**见报告。
