# 云盘采集源（rclone）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 attune 加一个 `CloudDriveConnector`，经 `rclone` 二进制子进程把 Google Drive / Dropbox / OneDrive 等 70+ 云盘后端接入统一 `ingest_document` 采集 pipeline，配 `cloud_remotes` 表持久化、HTTP API、settings UI 与后台周期同步 worker。

**Architecture:** `CloudDriveConnector` 实现 `attune_core::ingest::SourceConnector` —— 它不直接拼 `std::process::Command`，而是依赖一个可注入的 `RcloneRunner` trait（`lsjson` / `cat` / `version` 三个方法）。生产实现 `SystemRclone` spawn 真 rclone 进程；测试注入 `FakeRclone` 喂预置 JSON / 字节，无需装 rclone 即可跑全部单测。Connector 内部用 `rclone lsjson -R --files-only` 列文件、按扩展名 + 体积过滤、逐文件 `rclone cat` 取字节、产 `RawDocument` 交给 sink。server 层 `ingest_cloud.rs::sync_cloud_dir` 走与 `sync_webdav_dir` 完全同构的「锁外 I/O → 逐文档短暂持锁入库」两阶段，增量 dedup 用 rclone 给的文件 hash（缺失退回 ModTime）。`cloud_remotes` 表记录 `dir_id → rclone remote 名 + 子路径 + 增量游标`，不存 OAuth token（token 在 rclone 自己的 `rclone.conf` 里）。

**Tech Stack:** Rust 2021 / Axum 0.8 / rusqlite / serde_json。**不引入新 crate** —— rclone 走 `std::process::Command` 子进程；PATH 探测复用已在 `attune-core/Cargo.toml` 的 `which = "6"`。前端 Preact + signals + i18n（zh/en 双表）。

---

## 关键背景与约束（实现者必读）

**为什么这样设计：** 云盘后端各家 SDK / OAuth 流程都不一样，自己实装会爆炸式增加维护面。rclone 一个跨平台单二进制（`rclone` / Win 上 `rclone.exe`）已经把 70+ 后端统一成 `<remote>:<path>` 抽象，且自己管 OAuth token 刷新。attune 只做「消费一个用户已配好的 rclone remote」，不碰云厂商凭据。

**rclone 不捆绑进安装包**（二进制 ~50MB，太大）—— attune 启动 / 绑定云盘时探测系统 PATH，缺失则在 API / UI 给出明确的安装引导。「attune 内嵌引导 `rclone config`」和「rclone 二进制按需下载」是蓝图级后续 task（见文末 §蓝图级后续），本计划 MVP 要求用户先自行 `rclone config` 配好 remote。

**MVP 与已有 WebDAV 路径的同构关系：** 本计划几乎逐项对照 `2026-05-18-ingest-unification.md` 的 WebDAV 部分 —— `cloud_remotes` 表 ≈ `webdav_remotes` 表、`sync_cloud_dir` ≈ `sync_webdav_dir`、`start_cloud_sync_worker` ≈ `start_webdav_sync_worker`、`routes/cloud.rs` ≈ `routes/remote.rs`。实现者读不准时以 WebDAV 对应文件为范本（路径见下文每个 Task 的 `Files`）。

**真实代码事实（签名已与代码核对，实现者不要凭记忆改）：**

- `attune_core::ingest::{SourceConnector, RawDocument, SourceKind, DocumentSink}` —— `SourceKind::CloudDrive` 变体**已存在**，`as_str()` 返回 `"cloud_drive"`，无需改 `connector.rs`。
- `SourceConnector::fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()>` —— 同步契约；单文档可恢复错误吞掉记日志，源级致命错误返回 `Err`。
- `DocumentSink<'a> = Box<dyn FnMut(RawDocument) + 'a>`。
- `RawDocument` 字段：`uri` / `title` / `content: Vec<u8>` / `mime_hint: Option<String>` / `source_kind` / `source_ref` / `modified_marker: Option<String>` / `domain: Option<String>` / `tags: Option<Vec<String>>` / `corpus_domain: Option<String>` / `metadata: HashMap<String,String>`。`RawDocument::parse_filename()` 取 `source_ref` 末段。
- `ingest_document(store: &Store, dek: &Key32, raw: &RawDocument) -> Result<IngestOutcome>`；`ingest_document_replacing(store, dek, raw, old_item_id: &str) -> Result<IngestOutcome>`。
- `IngestOutcome` 四态：`Inserted { item_id, chunks_enqueued }` / `Duplicate { item_id }` / `Updated { item_id, old_item_id }` / `Skipped { reason }`。
- `Store::get_indexed_file(&self, path: &str) -> Result<Option<IndexedFileRow>>`；`IndexedFileRow { id, dir_id, path, file_hash, item_id: Option<String>, indexed_at }`。
- `Store::upsert_indexed_file(&self, dir_id: &str, path: &str, file_hash: &str, item_id: &str) -> Result<()>`。
- `Store::delete_item(&self, id: &str) -> Result<bool>`；`Store::enqueue_reindex(&self, item_id: &str, action: &str) -> Result<()>`（`action` 必须 `"purge"` 或 `"reindex"`）。
- `Store::record_signal_event(&self, kind: &str, ref_id: &str, query: Option<&str>) -> Result<()>`。
- `Store::bind_directory(&self, path: &str, recursive: bool, file_types: &[&str]) -> Result<String>` —— 返回 `dir_id`。WebDAV route 用 `format!("webdav:{url}")` 当 path；云盘用 `format!("cloud:{remote}:{path}")`。
- `crypto::encrypt(key: &Key32, plaintext: &[u8]) -> Result<Vec<u8>>` / `crypto::decrypt(key: &Key32, data: &[u8]) -> Result<Vec<u8>>` —— AES-256-GCM。
- `attune_core::error::{Result, VaultError}`（core 层）；`attune_server::error::{AppError, AppResult}`（server 层，`AppError: From<VaultError>`）。
- `attune_core::async_fs` —— async handler 内禁止直接 `std::fs`。本计划 server 层不读写文件，handler 主要做 DB + spawn_blocking。
- WebDAV 范本里 `is_supported_remote_ext(filename) -> bool` 与 `MAX_REMOTE_FILE_BYTES = 20 * 1024 * 1024` 定义在 `scanner_webdav.rs` —— 本计划在 `cloud.rs` 内**自带**同款常量与函数（不跨模块引用，避免 `scanner_webdav` 成为 `cloud` 的依赖；两者职责独立）。
- `VaultError` 没有专门的「外部进程失败」变体；本计划统一用 `VaultError::LlmUnavailable(String)`（WebDAV 范本对网络错误也用这个）承载 rclone 子进程错误。

**Lock ordering（防死锁，全程遵守）：** `vault.lock()` → `vectors.lock()` → `fulltext.lock()` → `embedding.lock()`。`CloudDriveConnector` / `ingest_document` 只碰 `Store`，不碰 `VectorIndex` / `FulltextIndex`。`sync_cloud_dir` 与 `sync_webdav_dir` 同构：网络 I/O（rclone 子进程）全程**不持锁**，每个文档的 DB 写才短暂拿 `vault` 锁，写完即 drop。

**注释纪律：** 一个改动区域一条意图注释，写 WHY 不写 WHAT，禁止 `批次X` / `FIX-N` / `阶段Y` / `Round M` / `per reviewer` 这类过程标签。

**API kebab-case：** 新增 path 一律 kebab，如 `/api/v1/index/bind-cloud`、`/api/v1/index/cloud-remotes`、`/api/v1/index/sync-cloud`。

**i18n：** 所有 UI 可见字符串走 `t()`，新 key 必须**同时**写入 `i18n/zh.ts` 与 `i18n/en.ts`，两表 key 集合完全一致。

**跨平台：** rclone 二进制名 Win 上是 `rclone.exe`、其它平台 `rclone`；用 `cfg!(windows)` 选名。`std::process::Command` 本身跨平台。`rclone.conf` 路径用 `PathBuf`，不硬编码分隔符。

---

## File Structure

### 新建文件

| 文件 | 职责 |
|------|------|
| `rust/crates/attune-core/src/ingest/rclone.rs` | `RcloneRunner` trait（`version` / `lsjson` / `cat`）+ 生产实现 `SystemRclone`（spawn 真子进程）+ `RcloneError` + `rclone_binary_name()` + `RcloneFileEntry`（lsjson 单条解析结构）|
| `rust/crates/attune-core/src/ingest/cloud.rs` | `CloudConfig` + `CloudDriveConnector<R: RcloneRunner>` impl `SourceConnector` + 云盘扩展名/体积过滤常量 |
| `rust/crates/attune-core/src/store/cloud_remotes.rs` | `cloud_remotes` 表 CRUD：`CloudRemoteInput` / `CloudRemoteRow` + `upsert_cloud_remote` / `get_cloud_remote` / `list_cloud_remotes` / `touch_cloud_remote_sync` / `delete_cloud_remote` |
| `rust/crates/attune-server/src/ingest_cloud.rs` | `sync_cloud_dir` —— bind route 与周期 worker 共用的两阶段增量入库逻辑 |
| `rust/crates/attune-server/src/routes/cloud.rs` | `bind_cloud` / `list_cloud_remotes` / `unbind_cloud` / `sync_cloud` / `rclone_status` 五个 handler |
| `rust/crates/attune-core/tests/cloud_connector_test.rs` | `CloudDriveConnector` 集成测试（FakeRclone 注入：lsjson 解析 / 扩展名过滤 / 体积过滤 / cat 取字节 / rclone 缺失降级）|
| `rust/crates/attune-core/tests/cloud_remotes_test.rs` | `cloud_remotes` 表 CRUD + 增量游标往返测试 |

### 修改文件

| 文件 | 改动 |
|------|------|
| `rust/crates/attune-core/src/ingest/mod.rs` | 加 `pub mod rclone;` `pub mod cloud;` + re-export `CloudDriveConnector` / `CloudConfig` / `RcloneRunner` / `SystemRclone` / `RcloneError` |
| `rust/crates/attune-core/src/store/mod.rs` | 加 `pub mod cloud_remotes;` + `cloud_remotes` 表 `CREATE TABLE IF NOT EXISTS` |
| `rust/crates/attune-server/src/lib.rs` | `pub(crate) mod ingest_cloud;` + 注册 5 条 cloud 路由 |
| `rust/crates/attune-server/src/routes/mod.rs` | `pub mod cloud;` |
| `rust/crates/attune-server/src/state.rs` | 加 `cloud_sync_worker_running: AtomicBool` 字段 + `start_cloud_sync_worker` 方法；在已有 `start_webdav_sync_worker` 启动点旁一并启动 |
| `rust/crates/attune-server/ui/src/hooks/useRemote.ts` | 加 `CloudInput` / `CloudRemote` 类型 + `bindCloud` / `listCloudRemotes` / `syncCloud` / `unbindCloud` / `getRcloneStatus` |
| `rust/crates/attune-server/ui/src/views/RemoteView.tsx` | 「添加云盘」按钮 + `CloudForm` 模态 + 云盘行的「立即同步」按钮 + rclone 未安装提示条 |
| `rust/crates/attune-server/ui/src/i18n/zh.ts` | 加 `remote.cloud.*` / `remote.action.add_cloud` 等 key |
| `rust/crates/attune-server/ui/src/i18n/en.ts` | 同上 key 的英文值 |

---

## Task 1：`RcloneRunner` trait + `RcloneError` + rclone 二进制名

**Files:**
- Create: `rust/crates/attune-core/src/ingest/rclone.rs`
- Test: 同文件 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写失败测试**

在新文件 `rust/crates/attune-core/src/ingest/rclone.rs` 末尾写：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_name_matches_platform() {
        let name = rclone_binary_name();
        if cfg!(windows) {
            assert_eq!(name, "rclone.exe");
        } else {
            assert_eq!(name, "rclone");
        }
    }

    #[test]
    fn rclone_error_display_is_human_readable() {
        let e = RcloneError::NotInstalled;
        assert!(e.to_string().contains("rclone"));
        let e2 = RcloneError::CommandFailed("remote not found".into());
        assert!(e2.to_string().contains("remote not found"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib ingest::rclone 2>&1 | tail -20`
Expected: 编译失败 —— `rclone_binary_name` / `RcloneError` 未定义。

- [ ] **Step 3: 写最小实现**

在 `rclone.rs` 顶部写（放在 `#[cfg(test)]` 之前）：

```rust
//! rclone 子进程驱动抽象。
//!
//! `CloudDriveConnector` 不直接拼 `std::process::Command` —— 它依赖 `RcloneRunner`
//! trait，生产用 `SystemRclone` spawn 真进程，测试注入 `FakeRclone` 喂预置数据，
//! 无需装 rclone 即可跑全部单测。三个方法对应三个 rclone 子命令：
//! `version`（探测）/ `lsjson`（列文件 JSON）/ `cat`（取单文件字节）。

use std::process::Command;

use crate::error::{Result, VaultError};

/// rclone 子进程错误。`CloudDriveConnector` / route 层据此区分「未安装」（引导安装）
/// 与「调用失败」（remote 不存在 / 网络错误，展示给用户）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RcloneError {
    /// 系统 PATH 找不到 rclone 二进制。
    NotInstalled,
    /// rclone 进程跑起来了但退出码非 0，载荷是 stderr 摘要。
    CommandFailed(String),
    /// 进程能跑但输出不是预期格式（lsjson 不是合法 JSON 等）。
    BadOutput(String),
}

impl std::fmt::Display for RcloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RcloneError::NotInstalled => {
                write!(f, "rclone is not installed or not on PATH")
            }
            RcloneError::CommandFailed(msg) => write!(f, "rclone command failed: {msg}"),
            RcloneError::BadOutput(msg) => write!(f, "rclone produced unexpected output: {msg}"),
        }
    }
}

impl std::error::Error for RcloneError {}

impl From<RcloneError> for VaultError {
    fn from(e: RcloneError) -> Self {
        VaultError::LlmUnavailable(e.to_string())
    }
}

/// 当前平台的 rclone 可执行文件名。Windows 是 `rclone.exe`，其它平台 `rclone`。
pub fn rclone_binary_name() -> &'static str {
    if cfg!(windows) {
        "rclone.exe"
    } else {
        "rclone"
    }
}

/// 对 rclone 子进程的最小抽象。`CloudDriveConnector` 只通过此 trait 调 rclone，
/// 因此测试可注入 fake 实现。所有方法同步阻塞 —— caller（connector / route）
/// 已在 `spawn_blocking` 或独立线程内运行。
pub trait RcloneRunner: Send + Sync {
    /// 探测 rclone 是否可用，返回版本行（如 `rclone v1.66.0`）。
    /// 找不到二进制时返回 `RcloneError::NotInstalled`。
    fn version(&self) -> std::result::Result<String, RcloneError>;

    /// `rclone lsjson <remote>:<path> --recursive --files-only --config <conf>`
    /// 的原始 stdout（调用方负责 JSON 解析）。
    fn lsjson(&self, config_path: &str, remote: &str, path: &str)
        -> std::result::Result<Vec<u8>, RcloneError>;

    /// `rclone cat <remote>:<path> --config <conf>` 的原始 stdout（文件字节）。
    fn cat(&self, config_path: &str, remote: &str, path: &str)
        -> std::result::Result<Vec<u8>, RcloneError>;
}

/// 把 rclone 子进程错误装回 core 层 `Result`，供 `SourceConnector` 契约使用。
pub(crate) fn rclone_result<T>(r: std::result::Result<T, RcloneError>) -> Result<T> {
    r.map_err(|e| VaultError::from(e))
}

/// `Command` 占位 —— 在 Task 2 实装 `SystemRclone` 时使用 `Command`，
/// 此处先 `use` 防 unused（Task 2 会替换为真实使用）。
#[allow(unused_imports)]
use Command as _RcloneCommandMarker;
```

> 说明：`_RcloneCommandMarker` 只是为了让 Task 1 单独编译时 `use Command` 不报 unused。Task 2 实装 `SystemRclone` 后会真正用到 `Command`，届时**删除**这个 marker 行（Task 2 Step 3 已含删除）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib ingest::rclone 2>&1 | tail -20`
Expected: 编译报错 —— `ingest/mod.rs` 还没 `pub mod rclone;`。先做 Step 5。

- [ ] **Step 5: 在 mod.rs 挂载模块**

修改 `rust/crates/attune-core/src/ingest/mod.rs`，在 `pub mod local;` 下一行加：

```rust
pub mod rclone;
```

再次 Run: `cargo test -p attune-core --lib ingest::rclone 2>&1 | tail -20`
Expected: PASS（`binary_name_matches_platform` + `rclone_error_display_is_human_readable` 两个 ok）。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-core/src/ingest/rclone.rs rust/crates/attune-core/src/ingest/mod.rs
git commit -m "feat(ingest): RcloneRunner trait + RcloneError for cloud-drive source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2：`SystemRclone` —— 生产实现（spawn 真子进程）

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/rclone.rs`

- [ ] **Step 1: 写失败测试**

在 `rclone.rs` 的 `#[cfg(test)] mod tests` 里追加：

```rust
    #[test]
    fn system_rclone_version_reports_not_installed_for_bogus_path() {
        // 指向一个绝不存在的二进制名 —— 必须干净返回 NotInstalled，不 panic。
        let r = SystemRclone::with_binary("attune-rclone-does-not-exist-xyz");
        match r.version() {
            Err(RcloneError::NotInstalled) => {}
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }

    #[test]
    fn system_rclone_default_uses_platform_binary_name() {
        let r = SystemRclone::default();
        assert_eq!(r.binary, rclone_binary_name());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib ingest::rclone 2>&1 | tail -20`
Expected: 编译失败 —— `SystemRclone` 未定义。

- [ ] **Step 3: 写实现**

先**删除** Task 1 末尾的 marker 行：

```rust
/// `Command` 占位 —— 在 Task 2 实装 `SystemRclone` 时使用 `Command`，
/// 此处先 `use` 防 unused（Task 2 会替换为真实使用）。
#[allow(unused_imports)]
use Command as _RcloneCommandMarker;
```

然后在 `rclone.rs` 中 `RcloneRunner` trait 定义之后、`#[cfg(test)]` 之前插入：

```rust
/// rclone 单文件下载体积上限 —— 与 WebDAV 采集一致（大文件不远程拉取）。
pub const MAX_CLOUD_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// 生产 `RcloneRunner`：spawn 系统 PATH 上的 rclone 二进制。
pub struct SystemRclone {
    /// rclone 可执行文件名或绝对路径（默认平台名，测试可覆盖成不存在的名字）。
    pub binary: String,
}

impl Default for SystemRclone {
    fn default() -> Self {
        Self { binary: rclone_binary_name().to_string() }
    }
}

impl SystemRclone {
    /// 用指定二进制名/路径构造（测试用；生产用 `default()`）。
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self { binary: binary.into() }
    }

    /// 跑一条 rclone 子命令，成功返回 stdout 字节。
    ///
    /// 失败映射：spawn 失败（二进制不存在）→ `NotInstalled`；退出码非 0 →
    /// `CommandFailed(stderr 摘要)`。stderr 截断到 512 字节防超长日志。
    fn run(&self, args: &[&str]) -> std::result::Result<Vec<u8>, RcloneError> {
        let output = Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|_| RcloneError::NotInstalled)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if stderr.len() > 512 {
                stderr.truncate(512);
            }
            Err(RcloneError::CommandFailed(stderr.trim().to_string()))
        }
    }
}

impl RcloneRunner for SystemRclone {
    fn version(&self) -> std::result::Result<String, RcloneError> {
        let out = self.run(&["version"])?;
        let text = String::from_utf8_lossy(&out);
        // rclone version 首行形如 "rclone v1.66.0"。
        let first = text.lines().next().unwrap_or("").trim().to_string();
        if first.is_empty() {
            Err(RcloneError::BadOutput("empty version output".into()))
        } else {
            Ok(first)
        }
    }

    fn lsjson(&self, config_path: &str, remote: &str, path: &str)
        -> std::result::Result<Vec<u8>, RcloneError>
    {
        // remote:path 形式 —— rclone 用 ':' 分隔 remote 名与路径。
        let target = format!("{remote}:{path}");
        self.run(&[
            "lsjson",
            &target,
            "--recursive",
            "--files-only",
            "--config",
            config_path,
        ])
    }

    fn cat(&self, config_path: &str, remote: &str, path: &str)
        -> std::result::Result<Vec<u8>, RcloneError>
    {
        let target = format!("{remote}:{path}");
        self.run(&["cat", &target, "--config", config_path])
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib ingest::rclone 2>&1 | tail -20`
Expected: PASS（4 个 test 全 ok）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/rclone.rs
git commit -m "feat(ingest): SystemRclone — production rclone subprocess driver

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3：`rclone lsjson` JSON 解析 —— `RcloneFileEntry`

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/rclone.rs`

- [ ] **Step 1: 写失败测试**

在 `#[cfg(test)] mod tests` 追加：

```rust
    #[test]
    fn parse_lsjson_extracts_files_and_markers() {
        // rclone lsjson 真实输出形态：数组，每条含 Path/Name/Size/IsDir/ModTime/Hashes。
        let json = br#"[
          {"Path":"docs/a.md","Name":"a.md","Size":42,"IsDir":false,
           "ModTime":"2026-05-18T08:00:00.000Z","Hashes":{"sha1":"abc123"}},
          {"Path":"docs","Name":"docs","Size":-1,"IsDir":true,
           "ModTime":"2026-05-18T07:00:00.000Z"},
          {"Path":"docs/b.txt","Name":"b.txt","Size":17,"IsDir":false,
           "ModTime":"2026-05-18T09:00:00.000Z","Hashes":{}}
        ]"#;
        let entries = parse_lsjson(json).expect("parse ok");
        // IsDir=true 的条目被过滤掉，只剩两个文件。
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "docs/a.md");
        assert_eq!(entries[0].size, 42);
        // Hashes 非空 → marker 取 hash。
        assert_eq!(entries[0].modified_marker(), "abc123");
        // Hashes 为空 {} → marker 退回 ModTime。
        assert_eq!(entries[1].modified_marker(), "2026-05-18T09:00:00.000Z");
    }

    #[test]
    fn parse_lsjson_rejects_garbage() {
        let r = parse_lsjson(b"not json at all");
        assert!(matches!(r, Err(RcloneError::BadOutput(_))));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib ingest::rclone 2>&1 | tail -20`
Expected: 编译失败 —— `parse_lsjson` / `RcloneFileEntry` 未定义。

- [ ] **Step 3: 写实现**

在 `rclone.rs` 中 `MAX_CLOUD_FILE_BYTES` 常量之后插入：

```rust
/// `rclone lsjson` 输出里的一条文件记录（目录已在解析时过滤）。
#[derive(Debug, Clone)]
pub struct RcloneFileEntry {
    /// remote 内相对路径（lsjson 的 `Path` 字段，如 `docs/a.md`）。
    pub path: String,
    /// 文件字节数（lsjson 的 `Size`；目录是 -1，已过滤）。
    pub size: u64,
    /// 内容 hash —— rclone 给的首选 hash 算法值（sha1 / md5，按 backend 而定）。
    /// 部分 backend 不给 hash，此时为空。
    pub hash: String,
    /// 修改时间 RFC3339 字符串（lsjson 的 `ModTime`）。
    pub mod_time: String,
}

impl RcloneFileEntry {
    /// 增量标记：优先用内容 hash（最可靠），backend 不给 hash 时退回 ModTime。
    /// 即便 ModTime fallback 误判「已变」触发重入库，`ingest_document` 内部的
    /// content_hash 短路会把结果判为 `Duplicate`，不会重复写入。
    pub fn modified_marker(&self) -> String {
        if self.hash.is_empty() {
            self.mod_time.clone()
        } else {
            self.hash.clone()
        }
    }
}

/// lsjson 原始 JSON 字段（serde 直接 deserialize；命名匹配 rclone 输出）。
#[derive(serde::Deserialize)]
struct LsjsonRaw {
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "Size", default)]
    size: i64,
    #[serde(rename = "IsDir", default)]
    is_dir: bool,
    #[serde(rename = "ModTime", default)]
    mod_time: String,
    #[serde(rename = "Hashes", default)]
    hashes: std::collections::HashMap<String, String>,
}

/// 解析 `rclone lsjson` stdout 成文件列表（目录条目过滤掉）。
///
/// `Hashes` 是 `{算法: 值}` map —— 取任意一个非空值（rclone 通常只给一种），
/// 优先级 sha1 > md5 > 其它，保证同一 backend 多次调用 marker 稳定。
pub fn parse_lsjson(stdout: &[u8]) -> std::result::Result<Vec<RcloneFileEntry>, RcloneError> {
    let raw: Vec<LsjsonRaw> = serde_json::from_slice(stdout)
        .map_err(|e| RcloneError::BadOutput(format!("lsjson not valid JSON: {e}")))?;
    let mut out = Vec::new();
    for r in raw {
        if r.is_dir {
            continue;
        }
        // hash 选取：sha1 优先，其次 md5，再其次任意第一个非空值。
        let hash = r
            .hashes
            .get("sha1")
            .or_else(|| r.hashes.get("md5"))
            .or_else(|| r.hashes.values().find(|v| !v.is_empty()))
            .cloned()
            .unwrap_or_default();
        out.push(RcloneFileEntry {
            path: r.path,
            size: r.size.max(0) as u64,
            hash,
            mod_time: r.mod_time,
        });
    }
    Ok(out)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib ingest::rclone 2>&1 | tail -20`
Expected: PASS（6 个 test 全 ok）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/rclone.rs
git commit -m "feat(ingest): parse rclone lsjson output into RcloneFileEntry

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4：`CloudConfig` + 云盘扩展名/体积过滤

**Files:**
- Create: `rust/crates/attune-core/src/ingest/cloud.rs`
- Modify: `rust/crates/attune-core/src/ingest/mod.rs`

- [ ] **Step 1: 写失败测试**

新建 `rust/crates/attune-core/src/ingest/cloud.rs`，先写文件骨架 + 测试：

```rust
//! 云盘采集源 —— 经 rclone 子进程桥接 Google Drive / Dropbox / OneDrive 等。
//!
//! `CloudDriveConnector` 不直接 spawn 进程，依赖泛型 `RcloneRunner`：生产传
//! `SystemRclone`，测试传 `FakeRclone`。走 `rclone lsjson` 列文件、按扩展名 +
//! 体积过滤、逐文件 `rclone cat` 取字节、产 `RawDocument`。增量标记用 rclone
//! 给的内容 hash（缺失退回 ModTime）。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Result;
use crate::ingest::rclone::{
    parse_lsjson, rclone_result, RcloneRunner, MAX_CLOUD_FILE_BYTES,
};
use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};

/// 云盘受支持的扩展名 —— 与 WebDAV 采集对齐的子集（二进制媒体不远程拉取）。
const SUPPORTED_CLOUD_EXTS: &[&str] = &[
    "md", "txt", "py", "js", "ts", "rs", "go", "java", "pdf", "docx", "html", "htm", "csv",
    "rtf", "pptx", "xlsx",
];

/// 判断文件名扩展名是否属于受支持的云盘采集类型。
pub fn is_supported_cloud_ext(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    SUPPORTED_CLOUD_EXTS.contains(&ext.as_str())
}

/// 一个云盘采集目标的连接配置。
#[derive(Debug, Clone)]
pub struct CloudConfig {
    /// rclone.conf 里的 remote 名（不含尾部冒号，如 `gdrive`）。
    pub remote_name: String,
    /// remote 内子路径（`/` 或空表示 remote 根；不含前导 remote 名）。
    pub remote_path: String,
    /// rclone.conf 文件路径（含各 remote 的 OAuth token，rclone 自己管刷新）。
    pub rclone_config_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_ext_filter_matches_webdav_set() {
        assert!(is_supported_cloud_ext("report.pdf"));
        assert!(is_supported_cloud_ext("notes.MD"));
        assert!(!is_supported_cloud_ext("movie.mp4"));
        assert!(!is_supported_cloud_ext("photo.jpg"));
    }

    #[test]
    fn cloud_config_holds_remote_and_conf_path() {
        let cfg = CloudConfig {
            remote_name: "gdrive".into(),
            remote_path: "Knowledge".into(),
            rclone_config_path: PathBuf::from("/home/u/.config/attune/rclone.conf"),
        };
        assert_eq!(cfg.remote_name, "gdrive");
        assert_eq!(cfg.rclone_config_path.file_name().unwrap(), "rclone.conf");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib ingest::cloud 2>&1 | tail -20`
Expected: 编译失败 —— `ingest/mod.rs` 未挂 `cloud` 模块。

- [ ] **Step 3: 在 mod.rs 挂载**

修改 `rust/crates/attune-core/src/ingest/mod.rs`，在 `pub mod rclone;` 下一行加：

```rust
pub mod cloud;
```

并在 `pub use pipeline::{...}` 之后加 re-export：

```rust
pub use cloud::{is_supported_cloud_ext, CloudConfig, CloudDriveConnector};
pub use rclone::{RcloneError, RcloneRunner, SystemRclone};
```

> 注意：`CloudDriveConnector` 此刻尚未定义（Task 5 才定义）。为让 Task 4 单独编译通过，**本步先只加 `is_supported_cloud_ext, CloudConfig`** 这两个已存在的符号，`CloudDriveConnector` 留到 Task 5 Step 3 再补进 re-export。即 Task 4 此处写：

```rust
pub use cloud::{is_supported_cloud_ext, CloudConfig};
pub use rclone::{RcloneError, RcloneRunner, SystemRclone};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib ingest::cloud 2>&1 | tail -20`
Expected: PASS（2 个 test ok）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/cloud.rs rust/crates/attune-core/src/ingest/mod.rs
git commit -m "feat(ingest): CloudConfig + cloud-drive ext/size filter

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5：`CloudDriveConnector` impl `SourceConnector`

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/cloud.rs`
- Modify: `rust/crates/attune-core/src/ingest/mod.rs`

- [ ] **Step 1: 写失败测试**

在 `cloud.rs` 的 `#[cfg(test)] mod tests` 追加（含一个 `FakeRclone` —— 这是后续所有 connector 测试的注入桩）：

```rust
    /// 测试用 rclone 桩 —— 喂预置 lsjson JSON 与按路径映射的文件字节。
    struct FakeRclone {
        installed: bool,
        lsjson_out: Vec<u8>,
        files: HashMap<String, Vec<u8>>,
    }

    impl crate::ingest::rclone::RcloneRunner for FakeRclone {
        fn version(&self) -> std::result::Result<String, crate::ingest::rclone::RcloneError> {
            if self.installed {
                Ok("rclone v1.66.0 (fake)".into())
            } else {
                Err(crate::ingest::rclone::RcloneError::NotInstalled)
            }
        }
        fn lsjson(&self, _conf: &str, _remote: &str, _path: &str)
            -> std::result::Result<Vec<u8>, crate::ingest::rclone::RcloneError>
        {
            if !self.installed {
                return Err(crate::ingest::rclone::RcloneError::NotInstalled);
            }
            Ok(self.lsjson_out.clone())
        }
        fn cat(&self, _conf: &str, _remote: &str, path: &str)
            -> std::result::Result<Vec<u8>, crate::ingest::rclone::RcloneError>
        {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| crate::ingest::rclone::RcloneError::CommandFailed(
                    format!("no such file: {path}"),
                ))
        }
    }

    fn fake_config() -> CloudConfig {
        CloudConfig {
            remote_name: "gdrive".into(),
            remote_path: "".into(),
            rclone_config_path: PathBuf::from("/tmp/fake-rclone.conf"),
        }
    }

    #[test]
    fn connector_emits_supported_files_only() {
        let lsjson = br#"[
          {"Path":"a.md","Name":"a.md","Size":10,"IsDir":false,
           "ModTime":"2026-05-18T08:00:00Z","Hashes":{"sha1":"h-a"}},
          {"Path":"b.mp4","Name":"b.mp4","Size":99,"IsDir":false,
           "ModTime":"2026-05-18T08:00:00Z","Hashes":{"sha1":"h-b"}}
        ]"#;
        let mut files = HashMap::new();
        files.insert("a.md".to_string(), b"# heading\n\nbody".to_vec());
        let rclone = FakeRclone { installed: true, lsjson_out: lsjson.to_vec(), files };
        let connector = CloudDriveConnector::new(fake_config(), rclone);

        let mut emitted: Vec<RawDocument> = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| emitted.push(d));
            connector.fetch_documents(&mut sink).unwrap();
        }
        // 只 a.md 入选（b.mp4 扩展名被过滤）。
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].source_kind, SourceKind::CloudDrive);
        assert_eq!(emitted[0].source_ref, "gdrive:a.md");
        assert_eq!(emitted[0].modified_marker.as_deref(), Some("h-a"));
        assert_eq!(emitted[0].content, b"# heading\n\nbody");
    }

    #[test]
    fn connector_skips_oversized_files() {
        let big = MAX_CLOUD_FILE_BYTES + 1;
        let lsjson = format!(
            r#"[{{"Path":"huge.pdf","Name":"huge.pdf","Size":{big},"IsDir":false,
                 "ModTime":"2026-05-18T08:00:00Z","Hashes":{{"sha1":"h"}}}}]"#
        );
        let rclone = FakeRclone {
            installed: true,
            lsjson_out: lsjson.into_bytes(),
            files: HashMap::new(),
        };
        let connector = CloudDriveConnector::new(fake_config(), rclone);
        let mut count = 0usize;
        {
            let mut sink: DocumentSink<'_> = Box::new(|_| count += 1);
            connector.fetch_documents(&mut sink).unwrap();
        }
        assert_eq!(count, 0);
    }

    #[test]
    fn connector_fails_cleanly_when_rclone_missing() {
        let rclone = FakeRclone { installed: false, lsjson_out: vec![], files: HashMap::new() };
        let connector = CloudDriveConnector::new(fake_config(), rclone);
        let mut sink: DocumentSink<'_> = Box::new(|_| {});
        let r = connector.fetch_documents(&mut sink);
        assert!(r.is_err());
    }

    #[test]
    fn connector_continues_when_one_file_fetch_fails() {
        // a.md 有字节、c.txt 在 files map 里缺失 → cat 失败但不中断枚举。
        let lsjson = br#"[
          {"Path":"a.md","Name":"a.md","Size":5,"IsDir":false,
           "ModTime":"2026-05-18T08:00:00Z","Hashes":{"sha1":"h-a"}},
          {"Path":"c.txt","Name":"c.txt","Size":5,"IsDir":false,
           "ModTime":"2026-05-18T08:00:00Z","Hashes":{"sha1":"h-c"}}
        ]"#;
        let mut files = HashMap::new();
        files.insert("a.md".to_string(), b"hello".to_vec());
        let rclone = FakeRclone { installed: true, lsjson_out: lsjson.to_vec(), files };
        let connector = CloudDriveConnector::new(fake_config(), rclone);
        let mut emitted: Vec<RawDocument> = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| emitted.push(d));
            connector.fetch_documents(&mut sink).unwrap();
        }
        // c.txt fetch 失败被吞掉，a.md 仍正常交出。
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].source_ref, "gdrive:a.md");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib ingest::cloud 2>&1 | tail -20`
Expected: 编译失败 —— `CloudDriveConnector` 未定义。

- [ ] **Step 3: 写实现**

在 `cloud.rs` 中 `CloudConfig` 结构体之后、`#[cfg(test)]` 之前插入：

```rust
/// 云盘采集源。泛型于 `RcloneRunner` —— 生产 `R = SystemRclone`，测试注入 fake。
pub struct CloudDriveConnector<R: RcloneRunner> {
    config: CloudConfig,
    rclone: R,
}

impl<R: RcloneRunner> CloudDriveConnector<R> {
    pub fn new(config: CloudConfig, rclone: R) -> Self {
        Self { config, rclone }
    }

    /// rclone.conf 路径的字符串形式（子命令 `--config` 参数）。
    fn config_path_str(&self) -> String {
        self.config.rclone_config_path.to_string_lossy().into_owned()
    }

    /// 列出 remote 内全部受支持文件（目录 / 不支持扩展名 / 超大文件已过滤）。
    fn list_files(&self) -> Result<Vec<crate::ingest::rclone::RcloneFileEntry>> {
        let conf = self.config_path_str();
        let stdout = rclone_result(self.rclone.lsjson(
            &conf,
            &self.config.remote_name,
            &self.config.remote_path,
        ))?;
        let all = rclone_result(parse_lsjson(&stdout))?;
        let mut out = Vec::new();
        for entry in all {
            let filename = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            if !is_supported_cloud_ext(filename) {
                continue;
            }
            if entry.size > MAX_CLOUD_FILE_BYTES {
                log::warn!(
                    "cloud: skip oversized {} ({} bytes)",
                    entry.path,
                    entry.size
                );
                continue;
            }
            out.push(entry);
        }
        Ok(out)
    }
}

impl<R: RcloneRunner> SourceConnector for CloudDriveConnector<R> {
    fn source_kind(&self) -> SourceKind {
        SourceKind::CloudDrive
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let conf = self.config_path_str();
        // list 失败是源级致命错误（rclone 缺失 / remote 不存在）→ 返回 Err。
        let entries = self.list_files()?;
        for entry in entries {
            // 单文件下载失败不致命：记日志、继续下一个。
            let bytes = match self.rclone.cat(&conf, &self.config.remote_name, &entry.path) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("cloud: fetch {} failed: {e}", entry.path);
                    continue;
                }
            };
            // rclone 可能对 Size 谎报，下载后二次校验防超大内容整体入内存。
            if bytes.len() as u64 > MAX_CLOUD_FILE_BYTES {
                log::warn!("cloud: skip {} — actual size exceeds limit", entry.path);
                continue;
            }
            // source_ref / uri 用 "<remote>:<path>" —— 同一 remote 内稳定唯一键。
            let source_ref = format!("{}:{}", self.config.remote_name, entry.path);
            let mut metadata = HashMap::new();
            metadata.insert("cloud_remote".into(), self.config.remote_name.clone());
            sink(RawDocument {
                uri: source_ref.clone(),
                title: String::new(),
                content: bytes,
                mime_hint: None,
                source_kind: SourceKind::CloudDrive,
                source_ref,
                modified_marker: Some(entry.modified_marker()),
                // 云盘源无来源域 / 用户标签；corpus_domain 由 route 层从
                // cloud_remotes 表读出后回填（见 Task 9 的 sync_cloud_dir）。
                domain: None,
                tags: None,
                corpus_domain: None,
                metadata,
            });
        }
        Ok(())
    }
}
```

并在 `rust/crates/attune-core/src/ingest/mod.rs` 把 `CloudDriveConnector` 补进 re-export：

```rust
pub use cloud::{is_supported_cloud_ext, CloudConfig, CloudDriveConnector};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib ingest::cloud 2>&1 | tail -20`
Expected: PASS（6 个 test ok：`cloud_ext_filter_matches_webdav_set` / `cloud_config_holds_remote_and_conf_path` / `connector_emits_supported_files_only` / `connector_skips_oversized_files` / `connector_fails_cleanly_when_rclone_missing` / `connector_continues_when_one_file_fetch_fails`）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/cloud.rs rust/crates/attune-core/src/ingest/mod.rs
git commit -m "feat(ingest): CloudDriveConnector implements SourceConnector via rclone

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6：`cloud_remotes` 表 schema

**Files:**
- Modify: `rust/crates/attune-core/src/store/mod.rs`

- [ ] **Step 1: 写失败测试**

在 `rust/crates/attune-core/src/store/mod.rs` 末尾的 `#[cfg(test)]` 区域（若有多个 test mod，加到文件最末新建一个）追加：

```rust
#[cfg(test)]
mod tests_cloud_remotes_schema {
    use crate::store::Store;

    #[test]
    fn cloud_remotes_table_exists_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("v.db");
        let store = Store::open_for_test(&db);
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='cloud_remotes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "cloud_remotes table must be created on open");
    }
}
```

> 说明：`Store::open_for_test` 是否存在请先 `grep -n "open_for_test\|fn open" rust/crates/attune-core/src/store/mod.rs` 确认。若仓库用别的测试构造方式（如 `Store::open(path)` + 直接传 dek），按 `tests_indexed_files`（store/mod.rs 第 1201 行附近已有的测试 mod）现成范式照抄它的 store 构造代码。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --lib tests_cloud_remotes_schema 2>&1 | tail -20`
Expected: FAIL —— `cloud_remotes` 表不存在，`COUNT(*)` 返回 0。

- [ ] **Step 3: 写实现**

在 `rust/crates/attune-core/src/store/mod.rs` 的 `SCHEMA_SQL` 字符串里，紧跟 `webdav_remotes` 表定义之后插入：

```sql
-- 云盘 remote 配置持久化。bound_dirs(cloud:* path) 只记 remote 名；
-- 周期同步 worker 要复用配置自动重扫 → 此表存完整配置。
-- 不存 OAuth token —— token 在 rclone 自己的 rclone.conf 里（rclone 管刷新）。
-- rclone_config_path 是 rclone.conf 的文件系统路径（非敏感，明文存）。
CREATE TABLE IF NOT EXISTS cloud_remotes (
    dir_id             TEXT PRIMARY KEY REFERENCES bound_dirs(id) ON DELETE CASCADE,
    remote_name        TEXT NOT NULL,
    remote_path        TEXT NOT NULL DEFAULT '',
    rclone_config_path TEXT NOT NULL,
    corpus_domain      TEXT NOT NULL DEFAULT 'general',
    updated_at         TEXT NOT NULL,
    last_sync          TEXT
);
```

> `CREATE TABLE IF NOT EXISTS` 让老 vault 下次 open 自动获得空表，无需独立 migration（与 `webdav_remotes` 同模式，见 store/mod.rs 第 75 行注释）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --lib tests_cloud_remotes_schema 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/store/mod.rs
git commit -m "feat(store): cloud_remotes table schema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7：`cloud_remotes` CRUD

**Files:**
- Create: `rust/crates/attune-core/src/store/cloud_remotes.rs`
- Create: `rust/crates/attune-core/tests/cloud_remotes_test.rs`
- Modify: `rust/crates/attune-core/src/store/mod.rs`

- [ ] **Step 1: 写失败测试**

新建 `rust/crates/attune-core/tests/cloud_remotes_test.rs`：

```rust
//! cloud_remotes 表 CRUD + 增量游标往返集成测试。

use attune_core::store::cloud_remotes::CloudRemoteInput;
use attune_core::store::Store;

/// 建一个临时 vault store，绑定一个 cloud: 目录拿到 dir_id。
fn setup() -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_for_test(&dir.path().join("v.db"));
    let dir_id = store
        .bind_directory("cloud:gdrive:Knowledge", false, &["md", "txt"])
        .unwrap();
    (dir, store, dir_id)
}

#[test]
fn upsert_then_get_roundtrips() {
    let (_d, store, dir_id) = setup();
    let input = CloudRemoteInput {
        dir_id: dir_id.clone(),
        remote_name: "gdrive".into(),
        remote_path: "Knowledge".into(),
        rclone_config_path: "/home/u/.config/attune/rclone.conf".into(),
        corpus_domain: "tech".into(),
    };
    store.upsert_cloud_remote(&input).unwrap();

    let row = store.get_cloud_remote(&dir_id).unwrap().expect("row exists");
    assert_eq!(row.remote_name, "gdrive");
    assert_eq!(row.remote_path, "Knowledge");
    assert_eq!(row.rclone_config_path, "/home/u/.config/attune/rclone.conf");
    assert_eq!(row.corpus_domain, "tech");
    assert!(row.last_sync.is_none());
}

#[test]
fn upsert_is_idempotent_on_dir_id() {
    let (_d, store, dir_id) = setup();
    let mut input = CloudRemoteInput {
        dir_id: dir_id.clone(),
        remote_name: "gdrive".into(),
        remote_path: "A".into(),
        rclone_config_path: "/tmp/rclone.conf".into(),
        corpus_domain: "general".into(),
    };
    store.upsert_cloud_remote(&input).unwrap();
    input.remote_path = "B".into();
    store.upsert_cloud_remote(&input).unwrap();

    let all = store.list_cloud_remotes().unwrap();
    assert_eq!(all.len(), 1, "same dir_id must replace, not duplicate");
    assert_eq!(all[0].remote_path, "B");
}

#[test]
fn touch_sync_sets_last_sync() {
    let (_d, store, dir_id) = setup();
    store
        .upsert_cloud_remote(&CloudRemoteInput {
            dir_id: dir_id.clone(),
            remote_name: "dropbox".into(),
            remote_path: String::new(),
            rclone_config_path: "/tmp/rclone.conf".into(),
            corpus_domain: "general".into(),
        })
        .unwrap();
    store.touch_cloud_remote_sync(&dir_id).unwrap();
    let row = store.get_cloud_remote(&dir_id).unwrap().unwrap();
    assert!(row.last_sync.is_some(), "touch must set last_sync");
}

#[test]
fn delete_removes_row() {
    let (_d, store, dir_id) = setup();
    store
        .upsert_cloud_remote(&CloudRemoteInput {
            dir_id: dir_id.clone(),
            remote_name: "onedrive".into(),
            remote_path: String::new(),
            rclone_config_path: "/tmp/rclone.conf".into(),
            corpus_domain: "general".into(),
        })
        .unwrap();
    store.delete_cloud_remote(&dir_id).unwrap();
    assert!(store.get_cloud_remote(&dir_id).unwrap().is_none());
    assert_eq!(store.list_cloud_remotes().unwrap().len(), 0);
}
```

> 若 `Store::open_for_test` 不存在，按 Task 6 Step 1 说明改用仓库现有测试构造方式。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --test cloud_remotes_test 2>&1 | tail -20`
Expected: 编译失败 —— `store::cloud_remotes` 模块不存在。

- [ ] **Step 3: 写实现**

新建 `rust/crates/attune-core/src/store/cloud_remotes.rs`：

```rust
//! 云盘 remote 配置持久化。
//!
//! 周期同步 worker 要对每个 cloud: bound_dir 自动增量重扫，必须能读回配置。
//! 此表存 rclone remote 名 + 子路径 + rclone.conf 路径 + 增量游标。
//! 不存 OAuth token —— token 在 rclone 自己的 rclone.conf 里（rclone 管刷新），
//! 因此本表全字段明文，无需字段级加密（区别于 webdav_remotes.password_enc）。

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

/// 写入用的云盘 remote 配置。
#[derive(Debug, Clone)]
pub struct CloudRemoteInput {
    /// 关联的 bound_dirs.id。
    pub dir_id: String,
    /// rclone.conf 里的 remote 名（不含尾部冒号）。
    pub remote_name: String,
    /// remote 内子路径（空 = remote 根）。
    pub remote_path: String,
    /// rclone.conf 文件路径。
    pub rclone_config_path: String,
    /// 语料领域（写入 RawDocument.corpus_domain，驱动 F-Pro 跨域防污染）。
    pub corpus_domain: String,
}

/// 从表里读出的云盘 remote 配置。
#[derive(Debug, Clone)]
pub struct CloudRemoteRow {
    pub dir_id: String,
    pub remote_name: String,
    pub remote_path: String,
    pub rclone_config_path: String,
    pub corpus_domain: String,
    pub last_sync: Option<String>,
}

impl Store {
    /// upsert 一条云盘 remote 配置。同 `dir_id` 已存在则整行替换（幂等）。
    pub fn upsert_cloud_remote(&self, input: &CloudRemoteInput) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO cloud_remotes
                (dir_id, remote_name, remote_path, rclone_config_path,
                 corpus_domain, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(dir_id) DO UPDATE SET
                remote_name=excluded.remote_name,
                remote_path=excluded.remote_path,
                rclone_config_path=excluded.rclone_config_path,
                corpus_domain=excluded.corpus_domain,
                updated_at=excluded.updated_at",
            params![
                input.dir_id,
                input.remote_name,
                input.remote_path,
                input.rclone_config_path,
                input.corpus_domain,
                now,
            ],
        )?;
        Ok(())
    }

    /// 读单条云盘 remote 配置。
    pub fn get_cloud_remote(&self, dir_id: &str) -> Result<Option<CloudRemoteRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, remote_name, remote_path, rclone_config_path,
                    corpus_domain, last_sync
             FROM cloud_remotes WHERE dir_id = ?1",
        )?;
        let row = stmt
            .query_row(params![dir_id], |r| {
                Ok(CloudRemoteRow {
                    dir_id: r.get(0)?,
                    remote_name: r.get(1)?,
                    remote_path: r.get(2)?,
                    rclone_config_path: r.get(3)?,
                    corpus_domain: r.get(4)?,
                    last_sync: r.get(5)?,
                })
            })
            .ok();
        Ok(row)
    }

    /// 列出全部云盘 remote 配置（周期 worker 用）。
    pub fn list_cloud_remotes(&self) -> Result<Vec<CloudRemoteRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT dir_id, remote_name, remote_path, rclone_config_path,
                    corpus_domain, last_sync
             FROM cloud_remotes ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CloudRemoteRow {
                dir_id: r.get(0)?,
                remote_name: r.get(1)?,
                remote_path: r.get(2)?,
                rclone_config_path: r.get(3)?,
                corpus_domain: r.get(4)?,
                last_sync: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 记录某 remote 最近一次增量同步时间。
    pub fn touch_cloud_remote_sync(&self, dir_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE cloud_remotes SET last_sync = ?1 WHERE dir_id = ?2",
            params![now, dir_id],
        )?;
        Ok(())
    }

    /// 删除一条云盘 remote 配置（unbind 时调用）。
    pub fn delete_cloud_remote(&self, dir_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM cloud_remotes WHERE dir_id = ?1",
            params![dir_id],
        )?;
        Ok(())
    }
}
```

并在 `rust/crates/attune-core/src/store/mod.rs` 加模块声明（紧跟 `pub mod webdav_remotes;` 之后）：

```rust
pub mod cloud_remotes;
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --test cloud_remotes_test 2>&1 | tail -20`
Expected: PASS（4 个 test ok）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/store/cloud_remotes.rs rust/crates/attune-core/src/store/mod.rs rust/crates/attune-core/tests/cloud_remotes_test.rs
git commit -m "feat(store): cloud_remotes CRUD with incremental sync cursor

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8：`CloudDriveConnector` 端到端集成测试（FakeRclone → 入库）

**Files:**
- Create: `rust/crates/attune-core/tests/cloud_connector_test.rs`

- [ ] **Step 1: 写失败测试**

新建 `rust/crates/attune-core/tests/cloud_connector_test.rs` —— 验证 connector 产出的 `RawDocument` 走完 `ingest_document` 真正入库：

```rust
//! CloudDriveConnector → ingest_document 端到端集成测试。
//!
//! 用 FakeRclone 注入预置 lsjson + 文件字节，验证：连接器枚举的 RawDocument
//! 经 ingest_document 入库后产生 Inserted；二次同等内容判 Duplicate。

use std::collections::HashMap;
use std::path::PathBuf;

use attune_core::ingest::rclone::{RcloneError, RcloneRunner};
use attune_core::ingest::{
    ingest_document, CloudConfig, CloudDriveConnector, DocumentSink, IngestOutcome, RawDocument,
};
use attune_core::store::Store;

/// 复用与单测同款的 rclone 桩。
struct FakeRclone {
    lsjson_out: Vec<u8>,
    files: HashMap<String, Vec<u8>>,
}

impl RcloneRunner for FakeRclone {
    fn version(&self) -> std::result::Result<String, RcloneError> {
        Ok("rclone v1.66.0 (fake)".into())
    }
    fn lsjson(&self, _c: &str, _r: &str, _p: &str)
        -> std::result::Result<Vec<u8>, RcloneError>
    {
        Ok(self.lsjson_out.clone())
    }
    fn cat(&self, _c: &str, _r: &str, path: &str)
        -> std::result::Result<Vec<u8>, RcloneError>
    {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| RcloneError::CommandFailed(format!("no file {path}")))
    }
}

fn fake_connector() -> CloudDriveConnector<FakeRclone> {
    let lsjson = br#"[
      {"Path":"guide.md","Name":"guide.md","Size":24,"IsDir":false,
       "ModTime":"2026-05-18T08:00:00Z","Hashes":{"sha1":"hash-guide"}}
    ]"#;
    let mut files = HashMap::new();
    files.insert(
        "guide.md".to_string(),
        b"# Cloud Guide\n\nfirst paragraph body text".to_vec(),
    );
    CloudDriveConnector::new(
        CloudConfig {
            remote_name: "gdrive".into(),
            remote_path: String::new(),
            rclone_config_path: PathBuf::from("/tmp/fake-rclone.conf"),
        },
        FakeRclone { lsjson_out: lsjson.to_vec(), files },
    )
}

#[test]
fn cloud_document_ingests_as_inserted_then_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let (store, dek) = Store::open_unlocked_for_test(&dir.path().join("v.db"));

    let connector = fake_connector();
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        use attune_core::ingest::SourceConnector;
        let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
        connector.fetch_documents(&mut sink).unwrap();
    }
    assert_eq!(docs.len(), 1);

    // 首次入库 → Inserted。
    let outcome = ingest_document(&store, &dek, &docs[0]).unwrap();
    match outcome {
        IngestOutcome::Inserted { chunks_enqueued, .. } => {
            assert!(chunks_enqueued >= 1, "L1+L2 chunks must be enqueued");
        }
        other => panic!("expected Inserted, got {other:?}"),
    }

    // 同内容再入库 → content_hash 短路判 Duplicate。
    let outcome2 = ingest_document(&store, &dek, &docs[0]).unwrap();
    assert!(matches!(outcome2, IngestOutcome::Duplicate { .. }));
}
```

> `Store::open_unlocked_for_test` 返回 `(Store, Key32)` 是假定名 —— 先 `grep -rn "open_unlocked_for_test\|fn open.*Key32\|dek_db" rust/crates/attune-core/src/store/` 与 `rust/crates/attune-core/tests/ingest_pipeline_test.rs` 确认仓库里 ingest 集成测试**实际**怎么拿到 `(Store, dek)`，照抄它的构造代码。`ingest_pipeline_test.rs` 是 ingest-unification 计划产出的文件，必然有现成范式。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-core --test cloud_connector_test 2>&1 | tail -20`
Expected: FAIL —— 测试构造函数名不匹配（按 Step 1 注释修正）或断言失败。修正构造代码后应能编译。

- [ ] **Step 3: 修正测试构造代码使其通过**

按 `ingest_pipeline_test.rs` 的真实 store 构造方式替换 `Store::open_unlocked_for_test` 调用。本 Task 无生产代码改动 —— `CloudDriveConnector` 与 `ingest_document` 已在前序 Task 完成，此处只是端到端验证。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-core --test cloud_connector_test 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/tests/cloud_connector_test.rs
git commit -m "test(ingest): cloud connector end-to-end ingest integration

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9：`sync_cloud_dir` —— 两阶段增量入库

**Files:**
- Create: `rust/crates/attune-server/src/ingest_cloud.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`

- [ ] **Step 1: 写失败测试**

`sync_cloud_dir` 依赖 `AppState`（重型），用注入的 `RcloneRunner` 直接做单测较绕。本 Task 的测试放在文件内，验证一个**纯逻辑**辅助函数 `cloud_incremental_decision` —— 它把「indexed_files 里旧 marker」与「lsjson 新 marker」比对，决定 skip / insert / replace。新建 `rust/crates/attune-server/src/ingest_cloud.rs`，先写：

```rust
//! 云盘增量同步 —— bind-cloud route 与周期 worker 共用的入库逻辑。
//!
//! 与 `ingest_webdav::sync_webdav_dir` 同构：网络 I/O（rclone 子进程）全程不持
//! vault 锁；每个文档的 DB 写才短暂拿锁，写完即释放。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_skips_when_marker_unchanged() {
        let d = cloud_incremental_decision(Some("hash-x"), "hash-x");
        assert_eq!(d, IncrementalDecision::Skip);
    }

    #[test]
    fn decision_inserts_when_no_prior_record() {
        let d = cloud_incremental_decision(None, "hash-x");
        assert_eq!(d, IncrementalDecision::Insert);
    }

    #[test]
    fn decision_replaces_when_marker_changed() {
        let d = cloud_incremental_decision(Some("hash-old"), "hash-new");
        assert_eq!(d, IncrementalDecision::Replace);
    }

    #[test]
    fn decision_inserts_when_prior_marker_empty() {
        // 旧记录 marker 为空（未 backfill）视为需重新入库。
        let d = cloud_incremental_decision(Some(""), "hash-x");
        assert_eq!(d, IncrementalDecision::Replace);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-server ingest_cloud 2>&1 | tail -20`
Expected: 编译失败 —— `cloud_incremental_decision` / `IncrementalDecision` 未定义、`ingest_cloud` 未在 `lib.rs` 挂载。

- [ ] **Step 3: 写实现**

在 `ingest_cloud.rs` 的 `#[cfg(test)]` 之前写完整实现：

```rust
use std::sync::Arc;

use attune_core::ingest::{
    ingest_document, ingest_document_replacing, CloudConfig, CloudDriveConnector, DocumentSink,
    IngestOutcome, RawDocument, SourceConnector, SystemRclone,
};

use crate::state::AppState;

/// 单个云盘文件的增量入库决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncrementalDecision {
    /// marker 未变 → 跳过。
    Skip,
    /// 无旧记录 → 新入库。
    Insert,
    /// marker 变了（或旧 marker 为空）→ 替换旧 item。
    Replace,
}

/// 比对旧 marker（indexed_files.file_hash）与新 marker（lsjson hash/ModTime），
/// 得出该文件的入库决策。旧 marker 为空（未 backfill）按需重新入库。
pub fn cloud_incremental_decision(
    prior_marker: Option<&str>,
    new_marker: &str,
) -> IncrementalDecision {
    match prior_marker {
        None => IncrementalDecision::Insert,
        Some(old) if old.is_empty() => IncrementalDecision::Replace,
        Some(old) if old == new_marker => IncrementalDecision::Skip,
        Some(_) => IncrementalDecision::Replace,
    }
}

/// 对一个云盘 remote 做一次增量同步。
///
/// `corpus_domain` 回填进每份 `RawDocument`，驱动 F-Pro 跨域防污染前缀注入。
/// 阻塞函数 —— caller 必须在 `spawn_blocking` 或独立线程里调。
///
/// 持锁设计：rclone 子进程 list + cat 全程**不持** vault 锁；每个文档的 DB 写
/// 操作才短暂拿锁，写完即释放，避免后台 worker 阻塞前台请求。
pub fn sync_cloud_dir(
    state: &Arc<AppState>,
    dir_id: &str,
    config: CloudConfig,
    corpus_domain: &str,
) -> Result<serde_json::Value, String> {
    let connector = CloudDriveConnector::new(config, SystemRclone::default());

    // 阶段 1：锁外做全部 rclone I/O（lsjson + 逐文件 cat），物化到 Vec。
    let mut docs: Vec<RawDocument> = Vec::new();
    {
        let mut sink: DocumentSink<'_> = Box::new(|doc| docs.push(doc));
        connector
            .fetch_documents(&mut sink)
            .map_err(|e| e.to_string())?;
    }

    // 阶段 2：逐文档短暂持锁做增量判断 + DB 写，写完即 drop guard。
    let mut total = 0usize;
    let mut new_files = 0usize;
    let mut updated_files = 0usize;
    let mut skipped_files = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for mut doc in docs {
        total += 1;
        doc.corpus_domain = Some(corpus_domain.to_string());

        let source_ref = doc.source_ref.clone();
        let marker = doc.modified_marker.clone().unwrap_or_default();
        let filename = source_ref.rsplit('/').next().unwrap_or(&source_ref).to_string();

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(e) => {
                errors.push(format!("{filename}: vault locked: {e}"));
                continue;
            }
        };
        let store = vault.store();

        let existing = store.get_indexed_file(&source_ref).ok().flatten();
        let prior_marker = existing.as_ref().map(|ex| ex.file_hash.as_str());
        let decision = cloud_incremental_decision(prior_marker, &marker);

        if decision == IncrementalDecision::Skip {
            skipped_files += 1;
            continue;
        }

        // 内容已变（或首次）：Replace 时先删旧 item + 入队 purge + 记信号。
        let old_item_id: Option<String> = if decision == IncrementalDecision::Replace {
            existing.as_ref().and_then(|ex| {
                ex.item_id.as_ref().map(|id| {
                    let _ = store.delete_item(id);
                    if let Err(e) = store.enqueue_reindex(id, "purge") {
                        tracing::warn!("sync_cloud_dir: enqueue_reindex(purge) {id}: {e}");
                    }
                    if let Err(e) = store.record_signal_event("doc_update", id, None) {
                        tracing::debug!("sync_cloud_dir: record_signal_event {id}: {e}");
                    }
                    id.clone()
                })
            })
        } else {
            None
        };

        let outcome = if let Some(ref old_id) = old_item_id {
            ingest_document_replacing(store, &dek, &doc, old_id)
        } else {
            ingest_document(store, &dek, &doc)
        };

        match outcome {
            Ok(IngestOutcome::Inserted { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                if old_item_id.is_some() {
                    updated_files += 1;
                } else {
                    new_files += 1;
                }
            }
            Ok(IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &marker, &item_id);
                updated_files += 1;
            }
            Ok(IngestOutcome::Duplicate { .. }) | Ok(IngestOutcome::Skipped { .. }) => {
                skipped_files += 1;
            }
            Err(e) => {
                errors.push(format!("{filename}: ingest {e}"));
            }
        }
        // vault guard 在此隐式 drop。
    }

    // 全部处理完后更新 last_sync（best-effort）。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.store().touch_cloud_remote_sync(dir_id);
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

并在 `rust/crates/attune-server/src/lib.rs` 紧跟 `pub(crate) mod ingest_webdav;` 之后加：

```rust
pub(crate) mod ingest_cloud;
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-server ingest_cloud 2>&1 | tail -20`
Expected: PASS（4 个 decision test ok）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/ingest_cloud.rs rust/crates/attune-server/src/lib.rs
git commit -m "feat(server): sync_cloud_dir — two-phase incremental cloud ingest

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10：`routes/cloud.rs` —— bind / list / unbind / sync / rclone-status

**Files:**
- Create: `rust/crates/attune-server/src/routes/cloud.rs`
- Modify: `rust/crates/attune-server/src/routes/mod.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`

- [ ] **Step 1: 写失败测试**

云盘 route 全部依赖 `SharedState` + spawn_blocking，integration 测试重；本 Task 测一个纯函数 `validate_remote_name`（防注入 rclone 命令参数）。在 `routes/cloud.rs` 末尾写：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_name_accepts_plain_identifiers() {
        assert!(validate_remote_name("gdrive").is_ok());
        assert!(validate_remote_name("my-drive_2").is_ok());
    }

    #[test]
    fn remote_name_rejects_colon_and_whitespace() {
        // 冒号会被 rclone 当 remote:path 分隔符；空白可能拆成多参数。
        assert!(validate_remote_name("gdrive:secret").is_err());
        assert!(validate_remote_name("two words").is_err());
        assert!(validate_remote_name("").is_err());
        assert!(validate_remote_name("--config").is_err());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-server routes::cloud 2>&1 | tail -20`
Expected: 编译失败 —— `routes::cloud` 模块不存在。

- [ ] **Step 3: 写实现**

新建 `rust/crates/attune-server/src/routes/cloud.rs`：

```rust
//! 云盘采集 HTTP 路由 —— rclone 桥接的 bind / list / unbind / sync / 状态探测。

use std::path::PathBuf;

use attune_core::ingest::rclone::{RcloneRunner, SystemRclone};
use attune_core::ingest::CloudConfig;
use attune_core::store::cloud_remotes::CloudRemoteInput;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// remote 名校验 —— 只允许字母数字 / `-` / `_`。
///
/// rclone 用 `:` 分隔 remote 与 path，含 `:` 会被误解析；含空白可能在
/// 参数数组里被拆开；以 `-` 开头会被当成 flag。一律拒绝。
fn validate_remote_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("remote name must not be empty".into());
    }
    if name.starts_with('-') {
        return Err("remote name must not start with '-'".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("remote name may only contain letters, digits, '-' and '_'".into());
    }
    Ok(())
}

/// rclone.conf 的默认存放位置 —— attune 配置目录下。
/// 用户用 `rclone config --config <此路径>` 配 remote，attune 据此消费。
fn default_rclone_config_path() -> PathBuf {
    let base = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("attune");
    base.join("rclone.conf")
}

#[derive(Deserialize)]
pub struct BindCloudRequest {
    /// rclone remote 名（不含尾部冒号）。
    pub remote_name: String,
    /// remote 内子路径（缺省 = remote 根）。
    #[serde(default)]
    pub remote_path: String,
    /// rclone.conf 路径；缺省用 attune 配置目录下的默认路径。
    pub rclone_config_path: Option<String>,
    /// 语料领域，缺省 general。
    pub corpus_domain: Option<String>,
}

#[derive(serde::Serialize)]
pub struct CloudRemoteView {
    pub dir_id: String,
    pub remote_name: String,
    pub remote_path: String,
    pub corpus_domain: String,
    pub last_sync: Option<String>,
}

#[derive(serde::Serialize)]
pub struct RcloneStatusView {
    /// rclone 是否在系统 PATH 上可用。
    pub installed: bool,
    /// 可用时的版本行（如 `rclone v1.66.0`）。
    pub version: Option<String>,
    /// attune 默认 rclone.conf 路径（前端展示给用户写 `rclone config`）。
    pub config_path: String,
}

/// GET /api/v1/index/rclone-status — 探测系统是否装了 rclone。
pub async fn rclone_status(State(_state): State<SharedState>) -> AppResult<Json<RcloneStatusView>> {
    // 探测是阻塞子进程调用，放 spawn_blocking。
    let probe = tokio::task::spawn_blocking(|| SystemRclone::default().version())
        .await
        .map_err(|e| AppError::Internal(format!("rclone probe join: {e}")))?;
    let (installed, version) = match probe {
        Ok(v) => (true, Some(v)),
        Err(_) => (false, None),
    };
    Ok(Json(RcloneStatusView {
        installed,
        version,
        config_path: default_rclone_config_path().to_string_lossy().into_owned(),
    }))
}

/// POST /api/v1/index/bind-cloud — 绑定一个 rclone remote 并首次同步入库。
pub async fn bind_cloud(
    State(state): State<SharedState>,
    Json(body): Json<BindCloudRequest>,
) -> AppResult<Json<serde_json::Value>> {
    validate_remote_name(&body.remote_name).map_err(AppError::BadRequest)?;

    // rclone 未安装直接拒绝并引导（不进入 bind 流程）。
    let rclone_ok = tokio::task::spawn_blocking(|| SystemRclone::default().version())
        .await
        .map_err(|e| AppError::Internal(format!("rclone probe join: {e}")))?;
    if rclone_ok.is_err() {
        return Err(AppError::ServiceUnavailable(
            "rclone is not installed — install rclone and run `rclone config` first".into(),
        ));
    }

    let config_path = body
        .rclone_config_path
        .clone()
        .unwrap_or_else(|| default_rclone_config_path().to_string_lossy().into_owned());
    let corpus_domain = body
        .corpus_domain
        .clone()
        .unwrap_or_else(|| "general".into());

    // 创建/复用 bound_dirs 记录（cloud: 前缀标记云盘目录）+ 落库 cloud_remotes。
    let dir_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db().map_err(AppError::from)?;
        let store = vault.store();
        let path = format!("cloud:{}:{}", body.remote_name, body.remote_path);
        let dir_id = store
            .bind_directory(&path, false, &["md", "txt"])
            .map_err(AppError::from)?;
        store
            .upsert_cloud_remote(&CloudRemoteInput {
                dir_id: dir_id.clone(),
                remote_name: body.remote_name.clone(),
                remote_path: body.remote_path.clone(),
                rclone_config_path: config_path.clone(),
                corpus_domain: corpus_domain.clone(),
            })
            .map_err(AppError::from)?;
        dir_id
    };

    // 首次同步是阻塞 I/O —— 在 spawn_blocking 里跑。
    let config = CloudConfig {
        remote_name: body.remote_name.clone(),
        remote_path: body.remote_path.clone(),
        rclone_config_path: PathBuf::from(config_path),
    };
    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let domain_clone = corpus_domain.clone();
    let scan = tokio::task::spawn_blocking(move || {
        crate::ingest_cloud::sync_cloud_dir(&state_clone, &dir_id_clone, config, &domain_clone)
    })
    .await
    .map_err(|e| AppError::Internal(format!("cloud sync join: {e}")))?
    .map_err(AppError::BadGateway)?;

    Ok(Json(json!({ "dir_id": dir_id, "scan": scan })))
}

/// GET /api/v1/index/cloud-remotes — 列出已绑定的云盘 remote。
pub async fn list_cloud_remotes(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(AppError::from)?;
    let rows = vault.store().list_cloud_remotes().map_err(AppError::from)?;
    let view: Vec<CloudRemoteView> = rows
        .into_iter()
        .map(|r| CloudRemoteView {
            dir_id: r.dir_id,
            remote_name: r.remote_name,
            remote_path: r.remote_path,
            corpus_domain: r.corpus_domain,
            last_sync: r.last_sync,
        })
        .collect();
    Ok(Json(json!({ "remotes": view })))
}

#[derive(Deserialize)]
pub struct CloudDirQuery {
    pub dir_id: String,
}

/// POST /api/v1/index/sync-cloud?dir_id=... — 手动触发一次云盘增量同步。
pub async fn sync_cloud(
    State(state): State<SharedState>,
    Query(q): Query<CloudDirQuery>,
) -> AppResult<Json<serde_json::Value>> {
    // 读出该 remote 配置（snapshot 后释放锁）。
    let row = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db().map_err(AppError::from)?;
        vault
            .store()
            .get_cloud_remote(&q.dir_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::NotFound(format!("no cloud remote for dir {}", q.dir_id)))?
    };
    let config = CloudConfig {
        remote_name: row.remote_name.clone(),
        remote_path: row.remote_path.clone(),
        rclone_config_path: PathBuf::from(row.rclone_config_path.clone()),
    };
    let state_clone = state.clone();
    let dir_id = q.dir_id.clone();
    let domain = row.corpus_domain.clone();
    let scan = tokio::task::spawn_blocking(move || {
        crate::ingest_cloud::sync_cloud_dir(&state_clone, &dir_id, config, &domain)
    })
    .await
    .map_err(|e| AppError::Internal(format!("cloud sync join: {e}")))?
    .map_err(AppError::BadGateway)?;
    Ok(Json(json!({ "scan": scan })))
}

/// DELETE /api/v1/index/unbind-cloud?dir_id=... — 解绑云盘 remote。
/// 已索引内容保留（与本地 / WebDAV unbind 行为一致），仅删配置 + 停止同步。
pub async fn unbind_cloud(
    State(state): State<SharedState>,
    Query(q): Query<CloudDirQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(AppError::from)?;
    let store = vault.store();
    store.delete_cloud_remote(&q.dir_id).map_err(AppError::from)?;
    // bound_dirs 记录一并删除，不再被周期 rescan worker 触及。
    let _ = store.unbind_directory_by_id(&q.dir_id);
    Ok(Json(json!({ "ok": true })))
}
```

> `store.unbind_directory_by_id` 是假定名 —— 先 `grep -rn "unbind\|fn delete.*dir\|bound_dir" rust/crates/attune-core/src/store/` 找现成的「按 id 删 bound_dir」函数（`routes/index.rs::unbind_directory` 一定调过某个）。若不存在按 id 删的函数，改用 `routes/index.rs::unbind_directory` 实际用的那个签名；`ON DELETE CASCADE` 已让 `cloud_remotes` 行随 `bound_dirs` 删除，因此即使只删 bound_dir 也安全（`delete_cloud_remote` 仍保留作显式清理）。

在 `rust/crates/attune-server/src/routes/mod.rs` 加：

```rust
pub mod cloud;
```

在 `rust/crates/attune-server/src/lib.rs` 的路由注册区，紧跟 `.route("/api/v1/index/bind-remote", ...)` 之后加 5 行：

```rust
        .route("/api/v1/index/bind-cloud", post(routes::cloud::bind_cloud))
        .route("/api/v1/index/cloud-remotes", get(routes::cloud::list_cloud_remotes))
        .route("/api/v1/index/sync-cloud", post(routes::cloud::sync_cloud))
        .route("/api/v1/index/unbind-cloud", delete(routes::cloud::unbind_cloud))
        .route("/api/v1/index/rclone-status", get(routes::cloud::rclone_status))
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-server routes::cloud 2>&1 | tail -20`
Expected: PASS（2 个 test ok）。

- [ ] **Step 5: 整体编译确认**

Run: `cargo build -p attune-server 2>&1 | tail -15`
Expected: 编译成功（无 error；warning 容忍）。

- [ ] **Step 6: Commit**

```bash
git add rust/crates/attune-server/src/routes/cloud.rs rust/crates/attune-server/src/routes/mod.rs rust/crates/attune-server/src/lib.rs
git commit -m "feat(server): cloud-drive HTTP routes (bind/list/unbind/sync/status)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11：`start_cloud_sync_worker` —— 后台周期同步

**Files:**
- Modify: `rust/crates/attune-server/src/state.rs`

- [ ] **Step 1: 写失败测试**

在 `state.rs` 已有的 `#[cfg(test)] mod tests`（含 `webdav_sync_worker_flag_prevents_double_start`，约第 1443 行）追加一个同款 flag 测试：

```rust
    #[test]
    fn cloud_sync_worker_flag_prevents_double_start() {
        // 与 webdav 同款：原子 flag 防止周期 worker 被重复启动。
        let flag = std::sync::atomic::AtomicBool::new(false);
        // 首次抢占成功。
        assert!(flag
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok());
        // 二次抢占失败（worker 已在跑）。
        assert!(flag
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p attune-server cloud_sync_worker_flag 2>&1 | tail -15`
Expected: PASS —— 此测试只验证 `AtomicBool` 语义，本身不依赖新代码（它是行为契约文档化）。先确认它编译跑通；真正的生产代码改动在 Step 3。

> 说明：周期 worker 是 `std::thread::spawn` + 15 分钟 `sleep` 的长跑循环，无法在单测里真跑。`webdav_sync_worker_flag_prevents_double_start` 在仓库里就是这种「验证 flag 语义」的契约测试 —— 照抄它的形态即可。

- [ ] **Step 3: 写实现**

(3a) 在 `AppState` 结构体里，紧挨 `webdav_sync_worker_running: AtomicBool,` 字段加：

```rust
    /// 防 `start_cloud_sync_worker` 被重复启动的原子 flag（与 webdav 同款）。
    pub cloud_sync_worker_running: AtomicBool,
```

(3b) 在 `AppState` 的构造处（`webdav_sync_worker_running: AtomicBool::new(false),` 那一行旁）加：

```rust
            cloud_sync_worker_running: AtomicBool::new(false),
```

(3c) 在 `start_webdav_sync_worker` 方法之后新增 `start_cloud_sync_worker` —— 结构与 webdav 版逐行对应，只把数据源换成 `list_cloud_remotes` + `sync_cloud_dir`：

```rust
    /// 启动云盘周期同步 worker：每 15 分钟从 cloud_remotes 表读全部 remote，
    /// 逐个增量重扫。原子 flag 防重入 + RAII guard 复位。
    pub fn start_cloud_sync_worker(state: std::sync::Arc<AppState>) {
        if state
            .cloud_sync_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("cloud sync worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.cloud_sync_worker_running);

            tracing::info!("cloud sync worker started");
            loop {
                // vault 锁定则退出 —— 下次 unlock 会重新 start。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                // 读全部已配置 cloud remote（snapshot 后释放锁）。
                let remotes: Vec<attune_core::store::cloud_remotes::CloudRemoteRow> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if vault.dek_db().is_err() {
                        break; // vault 锁定 → 退出，下次 unlock 重启
                    }
                    vault.store().list_cloud_remotes().unwrap_or_default()
                };

                for remote in remotes {
                    let config = attune_core::ingest::CloudConfig {
                        remote_name: remote.remote_name.clone(),
                        remote_path: remote.remote_path.clone(),
                        rclone_config_path: std::path::PathBuf::from(
                            remote.rclone_config_path.clone(),
                        ),
                    };
                    tracing::info!(
                        "cloud sync: scanning dir={} remote={}",
                        remote.dir_id,
                        remote.remote_name
                    );
                    if let Err(e) = crate::ingest_cloud::sync_cloud_dir(
                        &state,
                        &remote.dir_id,
                        config,
                        &remote.corpus_domain,
                    ) {
                        tracing::warn!("cloud sync for dir {} failed: {e}", remote.dir_id);
                    }
                }

                // unlock 后立即跑首轮，之后每 15 分钟一次。
                std::thread::sleep(std::time::Duration::from_secs(15 * 60));
            }
            tracing::info!("cloud sync worker stopped (vault locked)");
        });
    }
```

(3d) 在 `start_webdav_sync_worker` 的所有**调用点**（`grep -rn "start_webdav_sync_worker" rust/crates/attune-server/src/` 找出 —— 通常在 vault unlock 后启动后台 worker 的地方）旁，每处补一行：

```rust
        AppState::start_cloud_sync_worker(state.clone());
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p attune-server cloud_sync_worker_flag 2>&1 | tail -15`
Expected: PASS。
Run: `cargo build -p attune-server 2>&1 | tail -15`
Expected: 编译成功。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/state.rs
git commit -m "feat(server): start_cloud_sync_worker — periodic cloud rescan

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12：i18n key（zh + en）

**Files:**
- Modify: `rust/crates/attune-server/ui/src/i18n/zh.ts`
- Modify: `rust/crates/attune-server/ui/src/i18n/en.ts`

- [ ] **Step 1: 加 zh.ts key**

在 `rust/crates/attune-server/ui/src/i18n/zh.ts` 的 `remote.*` 区块（`'remote.webdav.bind'` 之后）插入：

```ts
  'remote.action.add_cloud': '添加云盘',
  'remote.modal.cloud.title': '绑定云盘（rclone）',
  'remote.toast.bind_cloud_success': '已绑定，开始首次同步',
  'remote.toast.bind_cloud_fail': '云盘绑定失败：{error}',
  'remote.toast.sync_cloud_success': '同步完成',
  'remote.toast.sync_cloud_fail': '同步失败：{error}',
  'remote.cloud.remote_name': 'rclone Remote 名',
  'remote.cloud.remote_name_hint': '在 rclone config 里配置的 remote 名，不含冒号（如 gdrive）',
  'remote.cloud.remote_path': '子路径',
  'remote.cloud.remote_path_hint': '相对 remote 根目录；留空表示根目录',
  'remote.cloud.corpus_domain': '语料领域',
  'remote.cloud.bind': '绑定',
  'remote.cloud.sync_now': '立即同步',
  'remote.cloud.rclone_missing_title': '未检测到 rclone',
  'remote.cloud.rclone_missing_desc': '云盘采集需要系统已安装 rclone。请先安装 rclone 并运行 rclone config 配置远程盘。',
  'remote.cloud.rclone_ok': 'rclone 已就绪：{version}',
  'remote.cloud.rclone_config_hint': '用 rclone config --config {path} 配置好 remote 后再绑定',
```

- [ ] **Step 2: 加 en.ts key（key 集合必须与 zh.ts 完全一致）**

在 `rust/crates/attune-server/ui/src/i18n/en.ts` 对应 `remote.*` 区块插入：

```ts
  'remote.action.add_cloud': 'Add Cloud Drive',
  'remote.modal.cloud.title': 'Bind Cloud Drive (rclone)',
  'remote.toast.bind_cloud_success': 'Bound — first sync started',
  'remote.toast.bind_cloud_fail': 'Cloud drive bind failed: {error}',
  'remote.toast.sync_cloud_success': 'Sync complete',
  'remote.toast.sync_cloud_fail': 'Sync failed: {error}',
  'remote.cloud.remote_name': 'rclone Remote Name',
  'remote.cloud.remote_name_hint': 'The remote name configured in rclone config, without colon (e.g. gdrive)',
  'remote.cloud.remote_path': 'Subpath',
  'remote.cloud.remote_path_hint': 'Relative to the remote root; leave empty for the root',
  'remote.cloud.corpus_domain': 'Corpus Domain',
  'remote.cloud.bind': 'Bind',
  'remote.cloud.sync_now': 'Sync Now',
  'remote.cloud.rclone_missing_title': 'rclone Not Detected',
  'remote.cloud.rclone_missing_desc': 'Cloud drive ingest requires rclone installed on the system. Install rclone and run rclone config to set up your remote first.',
  'remote.cloud.rclone_ok': 'rclone ready: {version}',
  'remote.cloud.rclone_config_hint': 'Configure the remote with rclone config --config {path}, then bind',
```

- [ ] **Step 3: 验证 key 集合一致**

Run:
```bash
cd rust/crates/attune-server/ui/src && diff <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```
Expected: 无输出（两表 key 完全一致）。

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-server/ui/src/i18n/zh.ts rust/crates/attune-server/ui/src/i18n/en.ts
git commit -m "feat(ui): i18n keys for cloud-drive ingest source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13：前端 `useRemote.ts` —— 云盘 API 封装

**Files:**
- Modify: `rust/crates/attune-server/ui/src/hooks/useRemote.ts`

- [ ] **Step 1: 加类型与函数**

在 `rust/crates/attune-server/ui/src/hooks/useRemote.ts` 的 `bindWebdav` 之后插入：

```ts
export type CloudInput = {
  remote_name: string;
  remote_path: string;
  corpus_domain: string;
};

export type CloudRemote = {
  dir_id: string;
  remote_name: string;
  remote_path: string;
  corpus_domain: string;
  last_sync?: string;
};

export type RcloneStatus = {
  installed: boolean;
  version?: string;
  config_path: string;
};

export async function getRcloneStatus(): Promise<RcloneStatus> {
  try {
    return await api.get<RcloneStatus>('/index/rclone-status');
  } catch {
    return { installed: false, config_path: '' };
  }
}

export async function bindCloud(input: CloudInput): Promise<RemoteActionResult> {
  try {
    await api.post('/index/bind-cloud', input);
    return { ok: true };
  } catch (e: unknown) {
    if (e instanceof ApiError) {
      return { ok: false, error: extractErrorMessage(e.body) };
    }
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function listCloudRemotes(): Promise<CloudRemote[]> {
  try {
    const res = await api.get<{ remotes: CloudRemote[] }>('/index/cloud-remotes');
    return res.remotes ?? [];
  } catch {
    return [];
  }
}

export async function syncCloud(dirId: string): Promise<RemoteActionResult> {
  try {
    await api.post(`/index/sync-cloud?dir_id=${encodeURIComponent(dirId)}`, {});
    return { ok: true };
  } catch (e: unknown) {
    if (e instanceof ApiError) {
      return { ok: false, error: extractErrorMessage(e.body) };
    }
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function unbindCloud(dirId: string): Promise<boolean> {
  try {
    await api.delete(`/index/unbind-cloud?dir_id=${encodeURIComponent(dirId)}`);
    return true;
  } catch {
    return false;
  }
}
```

> 注意：`syncCloud` / `bindCloud` 用到 `api.post` 的第二参数 —— 确认 `store/api.ts` 的 `post` 签名是否允许空 body `{}`；若 `post` 强制要 body，传 `{}` 即可（云盘 sync 无 body）。`api.get` / `api.delete` 用法照抄文件内已有的 `listBoundDirs` / `unbindDir`。

- [ ] **Step 2: 类型检查**

Run: `cd rust/crates/attune-server/ui && npm run typecheck 2>&1 | tail -15`（若无 `typecheck` 脚本，用 `npx tsc --noEmit`）
Expected: 无 error。

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-server/ui/src/hooks/useRemote.ts
git commit -m "feat(ui): cloud-drive API client in useRemote hook

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14：前端 `RemoteView.tsx` —— 云盘绑定 UI

**Files:**
- Modify: `rust/crates/attune-server/ui/src/views/RemoteView.tsx`

- [ ] **Step 1: 改 import + modal 类型 + 拉 rclone 状态**

(1a) 把 import 块的 `useRemote` 导入扩成：

```tsx
import {
  listBoundDirs,
  bindLocalDir,
  bindWebdav,
  unbindDir,
  bindCloud,
  listCloudRemotes,
  syncCloud,
  unbindCloud,
  getRcloneStatus,
} from '../hooks/useRemote';
import type { BoundDir, CloudRemote, RcloneStatus } from '../hooks/useRemote';
```

(1b) 在 `RemoteView` 组件体内、`modal` signal 旁加：

```tsx
  const cloudRemotes = useSignal<CloudRemote[]>([]);
  const rclone = useSignal<RcloneStatus | null>(null);
```

(1c) 把 `modal` 的类型从 `null | 'local' | 'webdav'` 改成 `null | 'local' | 'webdav' | 'cloud'`：

```tsx
  const modal = useSignal<null | 'local' | 'webdav' | 'cloud'>(null);
```

(1d) 把 `refresh` 改成同时拉云盘列表，并在 `useEffect` 里探测 rclone：

```tsx
  async function refresh() {
    loading.value = true;
    dirs.value = await listBoundDirs();
    cloudRemotes.value = await listCloudRemotes();
    loading.value = false;
  }

  useEffect(() => {
    void refresh();
    void getRcloneStatus().then((s) => (rclone.value = s));
  }, []);
```

- [ ] **Step 2: header 加「添加云盘」按钮 + rclone 状态提示条**

在 header 的按钮组里，`add_webdav` 按钮之后加：

```tsx
          <Button variant="secondary" size="sm" onClick={() => (modal.value = 'cloud')}>
            {`🗄 ${t('remote.action.add_cloud')}`}
          </Button>
```

并在 `<header>` 之后、加载判断之前插入 rclone 状态条（rclone 缺失时红条提示，就绪时绿条）：

```tsx
      {rclone.value && !rclone.value.installed && (
        <div
          style={{
            padding: 'var(--space-3)',
            background: 'var(--color-danger-bg, #fdecea)',
            border: '1px solid var(--color-danger, #e57373)',
            borderRadius: 'var(--radius-md)',
            fontSize: 'var(--text-sm)',
          }}
        >
          <strong>{t('remote.cloud.rclone_missing_title')}</strong>
          <div style={{ marginTop: 4 }}>{t('remote.cloud.rclone_missing_desc')}</div>
        </div>
      )}
```

- [ ] **Step 3: 渲染云盘行 + 「立即同步」**

在已绑定目录列表的 `dirs.value.map(...)` 之后，追加云盘 remote 行渲染：

```tsx
      {cloudRemotes.value.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
          {cloudRemotes.value.map((c) => (
            <CloudRow
              key={c.dir_id}
              remote={c}
              onSync={async () => {
                const r = await syncCloud(c.dir_id);
                if (r.ok) {
                  toast('success', t('remote.toast.sync_cloud_success'));
                  await refresh();
                } else {
                  toast('error', t('remote.toast.sync_cloud_fail', { error: r.error ?? t('remote.error.unknown') }));
                }
              }}
              onUnbind={async () => {
                if (!confirm(t('remote.confirm.unbind', { path: `${c.remote_name}:${c.remote_path}` }))) return;
                const ok = await unbindCloud(c.dir_id);
                if (ok) {
                  toast('success', t('remote.toast.unbind_success'));
                  await refresh();
                } else {
                  toast('error', t('remote.toast.unbind_fail'));
                }
              }}
            />
          ))}
        </div>
      )}
```

- [ ] **Step 4: 加 cloud Modal**

在 webdav `<Modal>` 之后加：

```tsx
      <Modal
        open={modal.value === 'cloud'}
        onClose={() => (modal.value = null)}
        title={t('remote.modal.cloud.title')}
        maxWidth={520}
      >
        <CloudForm
          rclone={rclone.value}
          onDone={async (result) => {
            modal.value = null;
            if (result.ok) {
              toast('success', t('remote.toast.bind_cloud_success'));
              await refresh();
            } else {
              toast('error', t('remote.toast.bind_cloud_fail', { error: result.error ?? t('remote.error.unknown') }));
            }
          }}
        />
      </Modal>
```

- [ ] **Step 5: 加 `CloudRow` + `CloudForm` 组件**

在文件末尾（`WebdavForm` 之后）加：

```tsx
function CloudRow({
  remote: c,
  onSync,
  onUnbind,
}: {
  remote: CloudRemote;
  onSync: () => void;
  onUnbind: () => void;
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
        🗄
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontFamily: 'var(--font-mono)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text)',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {`${c.remote_name}:${c.remote_path}`}
        </div>
        <div
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            marginTop: 2,
          }}
        >
          {`${t('remote.cloud.corpus_domain')}: ${c.corpus_domain}`}
          {c.last_sync && ` · ${t('remote.row.last_scan')}: ${new Date(c.last_sync).toLocaleString()}`}
        </div>
      </div>
      <Button variant="secondary" size="sm" onClick={onSync}>
        {t('remote.cloud.sync_now')}
      </Button>
      <Button variant="ghost" size="sm" onClick={onUnbind}>
        {t('remote.row.unbind')}
      </Button>
    </div>
  );
}

function CloudForm({
  rclone,
  onDone,
}: {
  rclone: RcloneStatus | null;
  onDone: (result: { ok: boolean; error?: string }) => void;
}): JSX.Element {
  const remoteName = useSignal('');
  const remotePath = useSignal('');
  const corpusDomain = useSignal('general');
  const submitting = useSignal(false);

  async function submit() {
    submitting.value = true;
    const result = await bindCloud({
      remote_name: remoteName.value.trim(),
      remote_path: remotePath.value.trim(),
      corpus_domain: corpusDomain.value,
    });
    submitting.value = false;
    onDone(result);
  }

  // rclone 未安装时禁用绑定 —— 后端也会拒，但前端先挡省一次往返。
  const rcloneReady = rclone?.installed === true;
  const canSubmit = rcloneReady && remoteName.value.trim().length > 0;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      {rcloneReady ? (
        <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
          {t('remote.cloud.rclone_ok', { version: rclone?.version ?? '' })}
          <div style={{ marginTop: 2 }}>
            {t('remote.cloud.rclone_config_hint', { path: rclone?.config_path ?? '' })}
          </div>
        </div>
      ) : (
        <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-danger, #e57373)' }}>
          {t('remote.cloud.rclone_missing_desc')}
        </div>
      )}
      <Input
        label={t('remote.cloud.remote_name')}
        value={remoteName.value}
        onInput={(e) => (remoteName.value = e.currentTarget.value)}
        placeholder="gdrive"
        autoFocus
        required
        hint={t('remote.cloud.remote_name_hint')}
      />
      <Input
        label={t('remote.cloud.remote_path')}
        value={remotePath.value}
        onInput={(e) => (remotePath.value = e.currentTarget.value)}
        hint={t('remote.cloud.remote_path_hint')}
      />
      <Input
        label={t('remote.cloud.corpus_domain')}
        value={corpusDomain.value}
        onInput={(e) => (corpusDomain.value = e.currentTarget.value)}
        placeholder="general"
      />
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="ghost" onClick={() => onDone({ ok: false })}>
          {t('common.cancel')}
        </Button>
        <Button
          variant="primary"
          onClick={submit}
          loading={submitting.value}
          disabled={!canSubmit}
        >
          {t('remote.cloud.bind')}
        </Button>
      </div>
    </div>
  );
}
```

- [ ] **Step 6: 类型检查 + 硬编码中文守卫**

Run:
```bash
cd rust/crates/attune-server/ui && npx tsc --noEmit 2>&1 | tail -15
cd src && grep -rnP "(toast\([^)]*'[^']*[\x{4e00}-\x{9fff}]|(title|placeholder|label|description|aria-label)=\"[^\"]*[\x{4e00}-\x{9fff}]|>[^<{]*[\x{4e00}-\x{9fff}])" --include="RemoteView.tsx" views/
```
Expected: tsc 无 error；grep 无输出（RemoteView 内零硬编码中文）。

- [ ] **Step 7: Commit**

```bash
git add rust/crates/attune-server/ui/src/views/RemoteView.tsx
git commit -m "feat(ui): cloud-drive bind modal + sync-now in RemoteView

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 15：全量编译 + 测试 + 文档同步

**Files:**
- Modify: `rust/README.md` / `rust/DEVELOP.md` / `rust/RELEASE.md`（受影响段落）

- [ ] **Step 1: 全量编译**

Run: `cd rust && cargo build 2>&1 | tail -15`
Expected: workspace 整体编译成功。

- [ ] **Step 2: 全量测试**

Run: `cd rust && cargo test -p attune-core -p attune-server 2>&1 | tail -30`
Expected: 全绿。重点确认本计划新增的 6 个测试文件/mod：`ingest::rclone` / `ingest::cloud` / `tests_cloud_remotes_schema` / `cloud_remotes_test` / `cloud_connector_test` / `ingest_cloud` 的 decision 测试 / `routes::cloud` / `cloud_sync_worker_flag`。

- [ ] **Step 3: 前端构建**

Run: `cd rust/crates/attune-server/ui && npm run build 2>&1 | tail -15`
Expected: Vite 构建成功（嵌入式 UI 进 `include_str!`）。

- [ ] **Step 4: 文档同步**

按受影响范围更新（只动相关段落，不新增文档文件）：
- `rust/README.md`：采集源列表加「云盘（rclone）」一项。
- `rust/DEVELOP.md`：「采集体系」段补云盘 connector 说明 + rclone 未捆绑、需用户自行安装的前置条件。
- `rust/RELEASE.md`：changelog 加一行 `feat: 云盘采集源（rclone 桥接 Google Drive / Dropbox / OneDrive 等）`。

- [ ] **Step 5: Commit**

```bash
git add rust/README.md rust/DEVELOP.md rust/RELEASE.md
git commit -m "docs: document cloud-drive (rclone) ingest source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## 风险与回滚

| 风险 | 缓解 | 回滚 |
|------|------|------|
| `Store::open_for_test` / `open_unlocked_for_test` 等测试构造函数名与本计划假定不符 | Task 6 / 7 / 8 已注明：动手前先 `grep` 仓库现成范式（`tests_indexed_files` / `ingest_pipeline_test.rs`）照抄 | 测试文件独立，改构造代码不影响生产代码 |
| `unbind_directory_by_id` 函数不存在 | Task 10 已注明 `grep` `routes/index.rs::unbind_directory` 找真实签名；`cloud_remotes` 有 `ON DELETE CASCADE` 兜底 | 仅 unbind 行为，删 route handler 即回滚 |
| rclone 输出格式跨版本漂移（lsjson 字段改名） | `LsjsonRaw` 用 `#[serde(default)]` 容忍缺字段；`parse_lsjson` 失败返回 `BadOutput` 不 panic | 解析逻辑集中在 `rclone.rs::parse_lsjson` 单点，易改 |
| rclone 子进程在慢网络下长时间阻塞 worker | `sync_cloud_dir` 全程锁外 I/O，不阻塞前台请求；周期 worker 在独立 `std::thread` | worker flag 可让其自然退出（vault lock 时） |
| 大文件 / 二进制文件经 `rclone cat` 整体进内存 | 双层防护：lsjson `Size` 预过滤 + 下载后 `bytes.len()` 二次校验；扩展名白名单挡二进制 | 常量 `MAX_CLOUD_FILE_BYTES` 单点可调 |
| rclone remote 名注入命令参数 | `validate_remote_name` 拒绝 `:` / 空白 / `-` 前缀；rclone 参数以数组传（非 shell 拼接） | 校验函数单点 |
| 整个 feature 需回滚 | 全部新代码在新文件 + 少量挂载点；按 Task 逆序 `git revert` | `cloud_remotes` 表 `CREATE TABLE IF NOT EXISTS` 残留空表无害 |

**分支策略**：本计划在 `feature/ingest-cloud` 分支开发，每 Task 一 commit；完成后 squash merge 进 `develop`，merge 后删 feature 分支。不直接动 `main`。

---

## Self-Review

**1. Spec 覆盖核对**（背景「计划要覆盖」逐项 → Task）：

| Spec 要求 | 落在 |
|-----------|------|
| `CloudDriveConnector` impl `SourceConnector`（rclone 子进程驱动） | Task 1-2（RcloneRunner/SystemRclone）+ Task 5（Connector）|
| `cloud_remotes` 表 + CRUD | Task 6（schema）+ Task 7（CRUD）|
| rclone 检测 + 未安装引导 | Task 1（`RcloneError::NotInstalled`）+ Task 10（`rclone_status` / `bind_cloud` 拒绝）+ Task 14（UI 红条 + 表单禁用）|
| `rclone lsjson` 解析 + `rclone cat` 取文件 | Task 3（`parse_lsjson`）+ Task 2（`cat`）+ Task 5（connector 串联）|
| 增量 dedup | Task 3（`modified_marker` hash/ModTime）+ Task 9（`cloud_incremental_decision`）|
| 入库走 `ingest_document` | Task 8（端到端）+ Task 9（`sync_cloud_dir`）|
| 后台周期同步 worker | Task 11（`start_cloud_sync_worker`）|
| settings UI（remote 配置 + rclone 状态 + 立即同步，i18n zh/en） | Task 12（i18n）+ Task 13（API client）+ Task 14（UI）|
| HTTP API（remote CRUD + 手动同步，kebab-case） | Task 10（5 条 kebab 路由）|
| 测试（mock rclone / lsjson 解析 / 增量 / rclone 缺失降级） | Task 1/3/5（FakeRclone + 解析 + 缺失）+ Task 8（端到端）+ Task 9（增量决策）+ Task 7（CRUD）|
| 大文件跳过 + 子进程错误健壮 | Task 5（双层体积过滤 + 单文件失败吞掉）+ Task 2（spawn/退出码映射）|
| 「内嵌 rclone config 引导」「rclone 按需下载」蓝图级 | 文末 §蓝图级后续 |

无遗漏。

**2. Placeholder 扫描**：无 TBD / TODO / 「类似 Task N」。所有 code step 含完整可编译 Rust/TS。三处「先 grep 确认仓库真实函数名」（`open_for_test` / `open_unlocked_for_test` / `unbind_directory_by_id`）是**有意**的——这些是已存在但本计划无法 100% 确认精确签名的仓库 API，已在 Task 内联给出 grep 命令 + 照抄范本，不属于 placeholder（不是「待补充逻辑」，是「核对既有 API 名」）。

**3. 类型一致性核对**：
- `RcloneRunner` 三方法签名（`version`/`lsjson`/`cat`）Task 1 定义，Task 2（SystemRclone）/ Task 5（FakeRclone）/ Task 8（FakeRclone）实现完全一致。
- `RcloneFileEntry { path, size, hash, mod_time }` + `modified_marker()` Task 3 定义，Task 5 connector 用 `entry.path` / `entry.size` / `entry.modified_marker()` 一致。
- `CloudConfig { remote_name, remote_path, rclone_config_path }` Task 4 定义，Task 5/8/9/10/11 构造时字段一致。
- `CloudRemoteInput` / `CloudRemoteRow` 字段 Task 7 定义，Task 10（`upsert_cloud_remote`/`list_cloud_remotes`/`get_cloud_remote`）+ Task 11（`list_cloud_remotes` → `CloudRemoteRow`）一致。
- `IncrementalDecision { Skip, Insert, Replace }` + `cloud_incremental_decision` Task 9 定义并自用。
- HTTP path kebab-case：`bind-cloud` / `cloud-remotes` / `sync-cloud` / `unbind-cloud` / `rclone-status` —— Task 10 路由注册与 Task 13 前端 `api.*` 调用字符串完全一致。
- i18n key：Task 12 定义的 `remote.cloud.*` / `remote.action.add_cloud` / `remote.toast.bind_cloud_*` 等，Task 14 `t()` 调用全部命中；zh/en 同 key。

无不一致。

**存疑点（需用户/实现期拍板）**：
1. **rclone.conf 默认路径** —— 本计划用 `dirs::config_dir()/attune/rclone.conf`。若 attune 已有统一「配置目录」约定（vault 同级目录等），实现时应改用那个，并让 `default_rclone_config_path` 复用既有 helper。
2. **MVP 不内嵌 `rclone config`** —— 用户须先在终端自行 `rclone config --config <attune路径>` 配好 remote。这是 spec 明确的 MVP 边界；UI 已给出 `config_path` 提示。若产品希望首版就降低这一摩擦，需把「蓝图级后续 task 1」提前。

---

## 蓝图级后续 task（不在本计划交付范围，留作后续 sprint）

1. **attune 内嵌引导 `rclone config`** —— 在 UI 里以子进程交互方式驱动 `rclone config` 的 OAuth 流程（rclone 支持 `rclone authorize` + `rclone config create` 非交互子命令），让用户不必开终端。需设计 OAuth 回调端口 / 浏览器拉起 / 进度反馈。
2. **rclone 二进制按需下载** —— rclone 未安装时，attune 从 rclone 官方发布页按平台下载对应二进制到 attune 数据目录（校验 SHA256），`SystemRclone::with_binary` 指向它。需处理下载进度 / 校验 / 跨平台解压（zip）。
3. **云盘大文件流式分块** —— 当前 >20MB 直接跳过；后续可对超大文档走 `rclone cat` 流式 + 分块入库，不整体进内存。
