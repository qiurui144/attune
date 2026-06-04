---
name: rss-cloud-ingest-connectors
version: v0.1.0-spec
status: DRAFT
date: 2026-06-01
authors: qiurui144 + Claude (spec-analyst)
template_version: 1
---

# Spec: RSS feed connector 加固 + 云盘 (cloud-drive via rclone) ingest connector

> 在已统一的 `SourceConnector` + `ingest_document` 框架上，把 RSS 采集源补到生产级安全/测试下限，并新实装一个经 rclone 桥接的云盘采集源，让个人用户能把 RSS 订阅与云盘目录接入 attune vault。

## 0. 目录 (TOC)

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误 + 边界 case](#7-错误--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [Appendix A: 代码勘查事实表（grounding ground-truth）](#appendix-a-代码勘查事实表grounding-ground-truth)

## 1. 目标定位

### 用户痛点
个人知识库用户的"知识来源"不只是本地文件 —— 大量增量知识活在 (a) RSS/Atom 订阅（博客、LWN、arXiv feed、release notes）和 (b) 云盘（Google Drive / Dropbox / OneDrive 上的文档目录）。attune 当前只能接入本地文件夹 / WebDAV / 邮箱 / Git 仓 / Chrome 扩展，无法把这两类高价值增量源纳入 vault，用户得手动下载再 upload，违背"被动捕获、主动进化"的产品定位。

### 与产品 positioning 对齐
- attune = 个人知识库 + 记忆增强；"自动捕获" 是核心叙事。多一个被动增量源 = 记忆护城河更宽。
- 通用能力：RSS / 云盘对**任何领域**的个人用户都有价值（程序员订 release feed、研究者订 arXiv、普通用户同步 Dropbox 文档）—— 符合 OSS attune 边界（CLAUDE.md「OSS attune 边界规则」：一个功能进 OSS 当且仅当对任何领域个人用户都有价值）。
- 成本契约对齐：fetch + parse 走零成本/本地算力层，embed 走本地算力层，**建库阶段绝不升级到 LLM**。

### 与 CLAUDE.md 规则映射
| 规则 | 本 spec 落点 |
|------|-------------|
| 全局 §3.1 11 节 spec-first | 本文档 |
| 全局 §1.4 secrets | 云盘凭据 / feed URL 经 `url_enc` 字段级 AES-256-GCM 加密落库（复用 rss_feeds 现有 `url_enc` 模式） |
| 全局 §2.2 用户视角 | §9 含端到端「新增云盘源 → vault 出现文档」黑盒 case |
| 全局 §6.1 6 类测试下限 | §9 测试矩阵 |
| 项目 OSS 边界 | §2 写死：零行业绑定、不调 attune-enterprise |
| 项目「成本感知三层」 | §8 成本契约 |
| 项目 Lock ordering（vault→vectors→fulltext→embedding）| §11 R9 + §3 持锁设计 |
| git.rs SSRF 教训（拒绝须 400 非 502）| §7 错误码 + §11 R2 |

## 2. 范围边界

> **本 sprint 版本归属**：作为一个 distinct deliverable minor（建议 `v1.1.x` 系采集能力增量，确切 minor 号按 merge 顺序确定，per 项目 §7.1.7「版本号 merge 时刻确定」）。

### ✅ 本 sprint 做（写死）

**A. RSS connector — 加固 + 补测试下限（不是 greenfield，见 Appendix A）**
RSS 在 core (`ingest/rss.rs`) + server (`ingest_rss.rs` / `routes/rss.rs`) + DB (`rss_feeds` 表) + 周期 worker (`start_rss_sync_worker`) **已端到端实装并可用**。本 sprint 对 RSS 的真实工作严格限定为三件：
1. **SSRF 加固**：`routes/rss.rs::validate()` 当前只校验 http/https scheme，**未调 `url_guard::validate_outbound_url`**。补上 SSRF 校验（拒内网 / link-local / 元数据端点），拒绝须返回 **400 `ssrf-rejected`（非 502）** —— 复刻 git.rs 已验证的修法。在 worker 直调路径 (`sync_rss_feed`) 也加一道（防绕过 route）。
2. **补 6 类测试矩阵**：现有 8 个单元测试只覆盖 happy/edge（RSS2/Atom 解析、dedup、304）。缺 adversarial（XXE / billion-laughs / SSRF feed URL）、error（畸形 XML / 网络 5xx）、concurrent（同 feed 并发 poll）、resource（超大 feed body OOM 上限）。补齐到下限。
3. **feed body 体积上限**：`RealFeedFetcher` 当前 `resp.bytes()` 无上限 —— 加 `max_feed_bytes`（默认 16 MiB）防 OOM。

**B. 云盘 connector — 全新实装（greenfield）**
1. 新模块 `ingest/cloud_drive.rs`：`CloudDriveConnector impl SourceConnector`（`source_kind() == SourceKind::CloudDrive`），经 **rclone subprocess** 桥接（类比 whisper.cpp / poppler 子进程模式，见 Appendix A）。
2. 抓取层抽象 `RcloneRunner` trait（生产 = `RealRcloneRunner` 调 `rclone lsjson` + `rclone cat`；测试注入 mock），与 RSS 的 `FeedFetcher` 同模式 —— 离线可测。
3. 增量 marker：用 rclone `lsjson` 返回的 `ModTime` + `Size`（无 hash 时）或 `Hashes.SHA-1`（有则优先）拼成 `modified_marker`，喂 `indexed_files` 去重。
4. server 侧 `ingest_cloud.rs` + `routes/cloud_drive.rs`：add / list / delete / patch / poll 五路由 + 周期 worker（复用 RSS 三段式：锁内读 cursor → 锁外跑 rclone I/O → 逐文档短暂持锁 ingest）。
5. DB 表 `cloud_drive_remotes`（结构对齐 `rss_feeds` / `webdav_remotes`，凭据字段 `rclone_config_enc` 加密）。
6. rclone binary 缺失兜底：graceful `Result::Err`（`rclone-not-found`）+ 用户友好 message，**不 panic**；UI 显示"未检测到 rclone，云盘源不可用，请安装 rclone"。

### ❌ 本 sprint 不做（写死，silent scope creep = bug）
- **Email connector 任何改动** —— `ingest/email.rs` 已存在且本 sprint 不碰。
- **Telegram / 其他 IM 源 / Notion / 微信收藏** —— 推 v.next。
- **任何行业插件 / 行业打标 / law-patent-sales 绑定** —— OSS 边界，零行业绑定。
- **rclone 自身的安装 / 打包分发 / 自动下载** —— 本 sprint 只检测+调用已安装的 rclone；打包推 v.next（与 whisper/poppler 的捆绑策略一并规划）。
- **OAuth 流程内置（在 attune 内引导用户授权云盘）** —— 本 sprint 依赖用户**自行 `rclone config` 配好 remote**，attune 只引用 remote 名 + 加密存储其 config；内置 OAuth 引导推 v.next。
- **RSS 重写为 async / 引入新 HTTP 栈 / 改 SourceConnector trait 签名** —— trait 与 reqwest blocking 模式保持不变。
- **任何 LLM-driven 抽取/分类 agent** —— 本 sprint 全程 deterministic，§9 显式声明不引入 LLM agent。

### ⏭️ 推迟到 v.next
- 内置 rclone 二进制捆绑 + 跨平台分发（含 K3 riscv64 镜像）。
- 云盘 OAuth 内置引导。
- 按 source_type 细分检索加权（当前 `item_source_type()` 全归一 `"file"`，本 sprint 保持）。

## 3. 架构数据流

### 总览（两条采集源共用统一 ingest 五步）

```
                            ┌─────────────────── attune-server ──────────────────────┐
  用户                       │                                                          │
  ─────                      │  routes/rss.rs            routes/cloud_drive.rs (新)     │
  POST /sources/rss/feeds ──►│  ├─ validate()+SSRF(新)   ├─ validate()+SSRF             │
  POST /sources/cloud/... ──►│  └─ spawn_blocking ──┐    └─ spawn_blocking ──┐          │
                             │                      ▼                        ▼          │
  周期 worker (各源独立) ────►│  ingest_rss.rs        ingest_cloud.rs (新)               │
                             │  sync_rss_feed()      sync_cloud_remote()                │
                             │   阶段0 锁内读cursor    阶段0 锁内读cursor+rclone配置       │
                             │   阶段1 锁外HTTP抓取     阶段1 锁外 rclone subprocess        │
                             │   阶段2 逐doc持锁ingest  阶段2 逐doc持锁ingest              │
                             └──────────┬───────────────────────┬──────────────────────┘
                                        │  RawDocument          │  RawDocument
                                        ▼                       ▼
        ┌──────────────────── attune-core::ingest ────────────────────────┐
        │ RssConnector            CloudDriveConnector (新)                  │
        │  FeedFetcher trait       RcloneRunner trait (新)                  │
        │  (RealFeedFetcher        (RealRcloneRunner: rclone lsjson/cat     │
        │   reqwest blocking)       MockRcloneRunner 离线测)                │
        │        │                        │                                 │
        │        └────────► fetch_documents(sink) ◄───────┘                 │
        │                          │ 每份 RawDocument                       │
        │                          ▼                                        │
        │              ingest_document(store, dek, &doc)  ← 唯一入库函数      │
        │   ┌──────────────────────────────────────────────────────────┐   │
        │   │ 1 parse_bytes  2 content_hash 短路判重  3 insert_item      │   │
        │   │ 4 breadcrumbs  5 embed(L1 章节 + L2 段落)  → classify      │   │
        │   └──────────────────────────────────────────────────────────┘   │
        └──────────────────────────────────────────────────────────────────┘
                                        │
                                        ▼
                   SQLite (字段级 AES-256-GCM)  +  tantivy FTS  +  usearch HNSW
```

### 增量 / 去重三层防线（两源一致）
1. **源级条件抓取**：RSS = HTTP 条件 GET（ETag / If-Modified-Since → 304 整箱跳过）；云盘 = rclone `lsjson` 比对 `modified_marker`（ModTime/Size/SHA-1），未变跳过。
2. **`indexed_files` 短路**：同 `source_ref` 已记录 → 跳过 ingest（`get_indexed_file`）。
3. **`content_hash` 短路**：`ingest_document` 内 `find_item_by_content_hash`（O(1) 索引查询，**非** O(N) 扫描，见 §11 R1）。

### DB tables
- `rss_feeds`（**已存在，本 sprint 不改 schema**）：见 Appendix A。
- `cloud_drive_remotes`（**新**）：
  ```sql
  CREATE TABLE IF NOT EXISTS cloud_drive_remotes (
      id                    TEXT PRIMARY KEY,
      name                  TEXT NOT NULL DEFAULT '',
      remote_name           TEXT NOT NULL,          -- rclone remote 名（如 "gdrive:"）
      remote_path           TEXT NOT NULL DEFAULT '', -- remote 内子路径（如 "Docs/kb"）
      rclone_config_enc     BLOB NOT NULL,          -- 该 remote 的 rclone.conf 片段，AES-256-GCM
      last_cursor           TEXT,                   -- 上次同步的最大 ModTime（增量起点）
      last_polled_at        TEXT,
      poll_interval_minutes INTEGER NOT NULL DEFAULT 360,
      max_file_bytes        INTEGER NOT NULL DEFAULT 67108864, -- 单文件 64 MiB 上限
      include_glob          TEXT NOT NULL DEFAULT '',  -- 复用 git.rs glob 模式（逗号分隔）
      exclude_glob          TEXT NOT NULL DEFAULT '',
      enabled               INTEGER NOT NULL DEFAULT 1,
      created_at            TEXT NOT NULL,
      updated_at            TEXT NOT NULL
  );
  CREATE INDEX IF NOT EXISTS idx_cloud_remotes_enabled_polled
      ON cloud_drive_remotes(enabled, last_polled_at);
  ```
- `indexed_files`（**已存在，不改**）：两源共用，`source_ref` 去重。

### 状态机（单 remote 一次 sync_cloud_remote 调用，对齐 sync_rss_feed）
```
   read cursor+config (锁内)
        │
        ▼
   rclone lsjson (锁外) ──► 网络/进程错误 ──► touch_polled_at + Err  ("rclone-exec-failed")
        │ ok
        ▼
   diff: ModTime > last_cursor 的文件列表
        │ 空 ──► touch_polled_at + Ok{new:0}
        ▼ 非空
   for each new file:
     rclone cat (锁外, 受 max_file_bytes 限) ──► RawDocument
        │
        ▼ (逐文件短暂持锁)
     get_indexed_file 短路 → 跳过 ;  否则 ingest_document → upsert_indexed_file
        │
        ▼
   写回 last_cursor = max(ModTime) + last_polled_at  (锁内)
```

## 4. 模块边界

### attune-core（`rust/crates/attune-core/`）
| 文件 | 改动 |
|------|------|
| `src/ingest/rss.rs` | **改**：`RealFeedFetcher` 加 `max_feed_bytes` 上限；补 adversarial/resource 单元测试 |
| `src/ingest/cloud_drive.rs` | **新建**：`CloudDriveConnector` / `RcloneRunner` trait / `RealRcloneRunner` / `MockRcloneRunner` / `parse_lsjson` 纯函数 |
| `src/ingest/mod.rs` | **改**：`pub mod cloud_drive;` + `pub use cloud_drive::{...}` |
| `src/ingest/connector.rs` | **不改**（`SourceKind::CloudDrive` 已存在；trait 不动） |
| `src/store/cloud_drive_remotes.rs` | **新建**：`CloudDriveRemoteRow` / `CloudDriveRemoteInput` + CRUD（对齐 `store/rss_feeds.rs`） |
| `src/store/mod.rs` | **改**：加 `cloud_drive_remotes` CREATE TABLE + `pub mod cloud_drive_remotes;` |
| `src/net/url_guard.rs` | **不改**（复用 `validate_outbound_url`） |

### attune-server（`rust/crates/attune-server/`）
| 文件 | 改动 |
|------|------|
| `src/routes/rss.rs` | **改**：`validate()` 内加 `url_guard::validate_outbound_url` 调用，拒绝映射 400 |
| `src/ingest_rss.rs` | **改**：`sync_rss_feed` 阶段 1 前加 SSRF 二次校验（防 worker 绕过 route） |
| `src/ingest_cloud.rs` | **新建**：`sync_cloud_remote(state, remote_id)` 三段式 |
| `src/routes/cloud_drive.rs` | **新建**：5 路由（GET/POST/DELETE/PATCH/POST-poll），挂 `/api/v1/sources/cloud/remotes` |
| `src/routes/mod.rs` | **改**：`pub mod cloud_drive;` + nest 路由 |
| `src/state.rs` | **改**：加 `start_cloud_sync_worker`（对齐 `start_rss_sync_worker`），unlock 后启动 |
| `src/routes/vault.rs` | **改**：unlock 路径加 `start_cloud_sync_worker(state.clone())`（三处，对齐 RSS） |

### 跨仓边界
- **零跨仓依赖**。不调 attune-enterprise / attune-pro。rclone 是外部系统二进制（subprocess），非 crate 依赖。
- 新增 Rust crate 依赖：`feed-rs`（RSS 已用，不新增）；云盘**不新增 crate**（rclone 走 `std::process::Command`，lsjson 用现有 `serde_json` 解析）。

## 5. API 契约

### RSS（已存在，本 sprint 仅 §7 错误行为变更，**无 schema 变更**）
`POST /api/v1/sources/rss/feeds` 行为不变，但 SSRF 命中时改返回 400（见 §7）。typed schema（现状）：
```rust
struct AddFeedRequest { url: String, name: Option<String>, poll_interval_minutes: Option<i64> }
```

### 云盘（新）— REST endpoints
挂载前缀：`/api/v1/sources/cloud/remotes`（kebab-case 路径段，per 项目 API 命名规范）。

```typescript
// POST /api/v1/sources/cloud/remotes  — 新增云盘源并立即首轮 poll
interface AddCloudRemoteRequest {
  remote_name: string;          // rclone remote 名，必须以 ':' 结尾或裸名，如 "gdrive:" / "dropbox:"
  remote_path?: string;         // remote 内子路径，默认 ""（根）
  rclone_config: string;        // 该 remote 的 rclone.conf 片段（[gdrive]\n type=drive\n ...），落库前加密
  name?: string;                // 展示名
  poll_interval_minutes?: number; // 默认 360
  include_glob?: string[];      // 默认知识类（复用 git.rs DEFAULT_INCLUDE）
  exclude_glob?: string[];
  max_file_bytes?: number;      // 默认 67108864 (64 MiB)
}
interface AddCloudRemoteResponse {
  id: string;
  poll: { status: "ok" | "not_modified" | "error"; total_files: number; new_files: number; skipped: number; errors: string[] };
}

// GET /api/v1/sources/cloud/remotes — 列出（rclone_config 不回传，仅元数据）
interface CloudRemoteListItem {
  id: string; name: string; remote_name: string; remote_path: string;
  last_polled_at: string | null; poll_interval_minutes: number; enabled: boolean;
}
type ListCloudRemotesResponse = CloudRemoteListItem[];

// DELETE /api/v1/sources/cloud/remotes/:id — 删除源（已 ingest 的 item 保留，对齐 RSS 删除语义）
// → 200 { deleted: true }

// PATCH /api/v1/sources/cloud/remotes/:id — 改 enabled / poll_interval_minutes
interface PatchCloudRemoteRequest { enabled?: boolean; poll_interval_minutes?: number; }

// POST /api/v1/sources/cloud/remotes/:id/poll — 手动触发一次增量同步（与周期 worker 共用 sync_cloud_remote）
interface PollCloudRemoteResponse { status: "ok" | "not_modified" | "error"; total_files: number; new_files: number; skipped: number; errors: string[]; }
```

### core trait（新）
```rust
/// 云盘抓取层抽象 —— 与 FeedFetcher / ImapFetcher 同模式，离线可测。
pub trait RcloneRunner: Send + Sync {
    /// `rclone lsjson <remote><path> --recursive --files-only` → 解析后的文件清单。
    fn lsjson(&self, remote: &str, path: &str) -> Result<Vec<RcloneFile>>;
    /// `rclone cat <remote><path>/<file>`，受 max_bytes 上限（超限 → Err "cloud-file-too-large"）。
    fn cat(&self, remote: &str, full_path: &str, max_bytes: u64) -> Result<Vec<u8>>;
}

/// lsjson 一行（只保留 ingest 必需字段）。
pub struct RcloneFile {
    pub path: String,        // remote 内相对路径
    pub size: u64,
    pub mod_time: String,    // RFC3339
    pub sha1: Option<String>,// Hashes.SHA-1（有则用作 marker；无则 size+mod_time）
    pub is_dir: bool,
}

pub struct CloudDriveConnector { /* config + Box<dyn RcloneRunner> + RefCell cursor */ }
impl SourceConnector for CloudDriveConnector { /* source_kind = CloudDrive; fetch_documents */ }
```

### CLI
本 sprint 不新增 CLI 子命令（云盘源经 Web UI / REST 管理；CLI 推 v.next 与其它源统一）。

## 6. 扩展点 / 插件接口

- **加新采集源的通用模式（本 spec 即第 5、6 个源的实证）**：(1) `SourceKind` 加 variant；(2) `ingest/<src>.rs` 实现 `SourceConnector` + 一个 Fetcher/Runner trait（保证离线可测）；(3) `store/<src>.rs` 加凭据表（加密字段）；(4) `ingest_<src>.rs` 三段式（锁外 I/O）；(5) `routes/<src>.rs` 五路由；(6) `start_<src>_sync_worker`。本 sprint 严格沿用，不发明新模式。
- **抓取层注入点**：`RcloneRunner` / `FeedFetcher` 是 mock 注入边界 —— 测试不依赖网络/外部进程。
- **配置覆盖位置**：每源一行 DB 配置（`poll_interval_minutes` / `include_glob` / `exclude_glob` / `max_file_bytes`），无全局 config 文件改动。
- **rclone backend 透明**：attune 不感知 remote 是 Google Drive 还是 Dropbox —— rclone remote 名抽象掉，新增任何 rclone 支持的 backend（S3 / OneDrive / SFTP …）零代码改动。

## 7. 错误 + 边界 case

### 错误码（kebab-case，经 `AppError` → JSON `{"error","code"}`）
| code | HTTP | 触发 | 备注 |
|------|------|------|------|
| `url-empty` | 400 | feed/remote URL 空 | RSS 已有 |
| `url-scheme-invalid` | 400 | 非 http/https feed URL | RSS 已有 |
| `ssrf-rejected` | **400** | feed URL / 解析出的 IP 命中内网/link-local/元数据端点 | **RSS 新增**；复刻 git.rs（拒绝必须 400 非 502 —— code 从 message 扫描提取，不取 ':' 首段） |
| `rclone-not-found` | 503 | 系统未安装 rclone | graceful degrade，UI 提示安装 |
| `rclone-config-invalid` | 400 | 提供的 rclone.conf 片段无法解析 / remote 名非法 | |
| `rclone-exec-failed` | 502 | rclone subprocess 非零退出（鉴权失败 / remote 不可达） | touch_polled_at 防 tight-loop |
| `cloud-remote-not-found` | 404 | poll/delete 不存在的 remote id | |
| `cloud-file-too-large` | — | 单文件超 `max_file_bytes` | 单文件级跳过+记日志，不中断整源 |
| `feed-too-large` | — | feed body 超 `max_feed_bytes`(16 MiB) | **RSS 新增**；单 feed 级 Err |
| `feed-parse-failed` | 502 | feed-rs 解析失败（畸形 XML） | RSS 已有（经 VaultError） |

### 边界 case 矩阵
| 场景 | 期望行为 |
|------|---------|
| feed 返回 304 | 不 emit 文档，仅 touch_polled_at（已实现） |
| feed entry 无 guid 无 link | 跳过该 entry（已实现） |
| feed body 空但 title 非空 | 用 title 兜底当正文（已实现） |
| 云盘 remote 为空目录 | new_files=0，正常 Ok |
| 云盘文件名含路径穿越 `../../etc/passwd` | `source_ref` 用 rclone 相对 path，不拼本地 fs 路径 → 不触达本地文件系统；仍校验 path 不含 `..` 段，命中则跳过 |
| rclone 未安装 | `rclone-not-found` 503，源标 unavailable，**不 panic** |
| rclone remote 鉴权过期 | `rclone-exec-failed` 502，touch_polled_at，源保留待用户重配 |
| 同一文件 ModTime 未变 | indexed_files 短路跳过 |
| feed URL = `http://169.254.169.254/latest/meta-data/` | `ssrf-rejected` 400，**不发起请求** |

### graceful degradation
- 单 entry / 单文件可恢复错误（解析失败 / 单文件下载失败）→ connector 吞掉 + 记日志 + 继续下一份（`SourceConnector` 契约要求）。
- 源级致命错误（连不上 / 鉴权失败）→ 返回 `Err`，worker `touch_polled_at` 后下次重试，不阻塞其它源。
- rclone 缺失 → 整个云盘源类型 disable，RSS / 本地 / WebDAV / Email / Git 不受影响。

## 8. 成本契约

### 三层成本归属
| 操作 | 成本层 | 触发 | 备注 |
|------|--------|------|------|
| RSS 条件 GET + feed 解析 | 🆓 零成本（CPU/网络 ms~s） | 周期 worker 自动 | 304 时近乎零 |
| 云盘 `rclone lsjson` + `cat` | 🆓 零成本（CPU/subprocess + 网络） | 周期 worker 自动 | rclone 自身不收费；用户云盘流量归用户 |
| parse_bytes（PDF/DOCX/MD） | 🆓 零成本 | ingest 内 | |
| embed L1+L2（章节+段落向量） | ⚡ 本地算力（GPU/NPU 秒级） | 建库阶段自动 | 本地 Ollama / ORT，零 API 费 |
| classify（tag/cluster） | ⚡ 本地算力 | 建库阶段自动 | 基础分类，非 LLM |
| **LLM 深度分析** | 💰 时间/金钱 | **用户显式触发** | **本 sprint 建库阶段绝不触发**；§9 声明不引入 LLM agent |

### 磁盘开销
- 每条 RSS entry / 云盘文件 = 1 item BLOB（加密原文）+ 向量（usearch f16）+ FTS 索引。估算：典型 feed entry ~2-8 KB 原文，云盘文档可达 MB 级（受 `max_file_bytes` 64 MiB 上限）。
- 新增 DB 表 `cloud_drive_remotes`：每源 ~1 KB（含加密 config）。
- **无新增大模型 / 二进制捆绑**（rclone 由用户自装，本 sprint 不打包）。

### token 估算
- **本 sprint 引入 0 LLM token**（fetch/parse/embed/classify 全本地）。embed 用本地底座，无云端 token。
- 若用户事后对 RSS/云盘入库文档**显式**发起 Chat/AI 批注 → 走既有 Chat 成本路径，UI 已显示 `~tok·$`，不在本 spec 新增成本面。

### wall-clock 估算（实现侧，非 LLM）
- 单 RSS feed poll：304 时 <1s；200 时 1-5s（含解析+ingest，取决于 entry 数）。
- 单云盘 remote sync：lsjson <2s（千文件级）；每文件 cat+ingest 取决于文件大小，受 worker governor 限速。

### audit 命令（用户可跑）
```bash
# 查 vault 内两源入库的 item 数 + 占用（需 vault 已 unlock）
sqlite3 ~/.attune/vault/data.db \
  "SELECT source_type, COUNT(*) FROM items WHERE is_deleted=0 GROUP BY source_type; \
   SELECT COUNT(*) FROM rss_feeds; SELECT COUNT(*) FROM cloud_drive_remotes;"
```

## 9. 测试矩阵

> **本 sprint 不引入任何 LLM-driven 抽取/分类 agent** —— 两个 connector 都是 deterministic（HTTP/subprocess + 字节解析）。因此 §9 走 deterministic 6 类下限矩阵，**不触发** CLAUDE.md「Agent 验证铁律」的 3-tier 模型矩阵要求。通过判据：deterministic 路径 PASS rate = 1.00。multi-seed 不适用（无随机/LLM 方差），但并发 case 用固定多线程数复跑 N=3 确认无 flake。

| 类 | 源 | worked case（输入 → 期望 → 判据） | 工具 |
|----|----|----|----|
| **happy** | RSS | 标准 RSS2 feed 2 entries → emit 2 RawDocument，HTML 已剥 → `docs.len()==2 && !contains("<p>")` | 单元（已有 5 个，复用） |
| **happy** | Cloud | mock lsjson 返回 3 个 .md 文件 → emit 3 RawDocument，source_ref 含相对路径 → `docs.len()==3` | 单元（新 MockRcloneRunner） |
| **edge** | RSS | Atom content vs summary 优先级 / 空 body 用 title 兜底 / 304 不 emit | 单元（已有，复用） |
| **edge** | Cloud | 空目录 lsjson `[]` → new_files=0；ModTime 未变文件 → indexed_files 短路 | 单元（新） |
| **error** | RSS | 畸形 XML `b"not xml"` → `parse_feed_bytes` Err `feed-parse-failed`；mock fetch 5xx → 源级 Err，worker touch_polled_at | 单元（parse 已有；5xx 新增） |
| **error** | Cloud | rclone 退出码非零 → `rclone-exec-failed`；rclone 不存在 → `rclone-not-found` 503 不 panic | 单元 + 集成（mock Command + 缺失探测） |
| **adversarial** | RSS | (a) XXE：feed XML 含 `<!ENTITY xxe SYSTEM "file:///etc/passwd">` → feed-rs 不解析外部实体（验证不读本地文件）；(b) billion-laughs 嵌套实体 → 受解析器/体积上限保护不 OOM；(c) SSRF：feed URL=`http://127.0.0.1:18900/` / `http://169.254.169.254/` → `ssrf-rejected` **400**，不发请求 | 单元（XXE/laughs）+ 集成（SSRF via url_guard，注入固定 DNS resolve） |
| **adversarial** | Cloud | (a) 文件名路径穿越 `../../../etc/passwd` → source_ref 不触达本地 fs，含 `..` 段跳过；(b) rclone_config 注入 shell 元字符 → 用 `Command` arg 数组传参（非 shell 拼接），不触发命令注入 | 单元 + 集成 |
| **concurrent** | RSS | 同 feed_id 两线程同时 sync_rss_feed → indexed_files 短路保证不重复入库；N=3 复跑无 flake | 集成（多线程） |
| **concurrent** | Cloud | 两 remote worker 并发 + 前台 add → 验证 Lock ordering（vault→vectors→fulltext→embedding）无死锁；N=3 | 集成（test_support harness） |
| **resource** | RSS | feed body 32 MiB（超 16 MiB 上限）→ `feed-too-large` Err，不 OOM；进程 RSS 内存上界可观测 | 单元（构造大 body）|
| **resource** | Cloud | 单文件 128 MiB（超 64 MiB 上限）→ `cloud-file-too-large` 单文件跳过，整源继续 | 单元（mock cat 返回大 buf）|

### 集成 E2E（≥1，黑盒用户视角）
- `tests/cloud_drive_subprocess.rs`：起 test server → add cloud remote（mock RcloneRunner via test seam）→ poll → 验证 vault `items` 表出现对应文档 → search 命中。对齐 `git_subprocess` / `rss` 现有集成测试风格。

### 回归 fixture
- SSRF 修复必须附 reproducer：一个固定 case「feed URL 169.254.169.254 → 400 ssrf-rejected」永久进测试集（防回归到 502）。
- 每修一个 bug 加 1 个 fixture（per 项目 Agent 验证铁律「回归 fixture」精神，虽非 agent）。

## 10. 向后兼容

### SemVer / schema versioning
- `SourceKind::CloudDrive` enum variant **已存在**（无 enum 变更，向后兼容）。
- 新增 DB 表 `cloud_drive_remotes` 用 `CREATE TABLE IF NOT EXISTS`（与既有 migration 模式一致）—— 老 vault 升级时自动建表，**不影响**既有 `items` / `rss_feeds` / `indexed_files`。
- `items.source_type` 仍归一 `"file"`（`item_source_type()` 不改）—— 既有检索/分类加权逻辑零感知，向后兼容。
- RSS REST schema **无变更**；唯一行为变化是 SSRF 命中从（曾可能的）502 → 400 —— 对正常用户透明，仅影响恶意/误配 URL 的错误码。

### 老 client 行为
- Chrome 扩展 / 旧 Web UI 不感知云盘源（新增 UI 面板属前端增量，旧前端无该面板但 REST 向后兼容）。
- 老 vault（无 `cloud_drive_remotes` 表）首次 unlock → 自动建表，无云盘源即空表，行为同今天。

### migration path — worked example
**Before（v1.0.x，云盘未实装）**：
```
items 表：仅含 local_folder / webdav / email / rss / git_repo / ingest 来源，source_type 全为 "file"。
无 cloud_drive_remotes 表。
```
**After（本 sprint，用户新增一个 Google Drive 源后）**：
```sql
-- 1) 新建源（API 落库，url/config 加密）
INSERT INTO cloud_drive_remotes(id, name, remote_name, remote_path, rclone_config_enc, poll_interval_minutes, enabled, created_at, updated_at)
VALUES ('cr_01', 'My GDrive Docs', 'gdrive:', 'KnowledgeBase', <encrypted-blob>, 360, 1, '2026-06-01T..', '2026-06-01T..');

-- 2) 首轮 poll 后，每个云盘文档入 items（source_type 仍 "file"，与本地文档检索权重一致）
--    indexed_files 记 source_ref = "cr_01#KnowledgeBase/notes/a.md"，marker = "<sha1或modtime+size>"
SELECT id, title, source_type FROM items WHERE id IN (SELECT item_id FROM indexed_files WHERE source_id='cr_01');
-- → ('it_99', 'a.md', 'file')   ← 与本地导入文档无差别，既有检索/分类逻辑零改动可用
```
**回滚**：删除该云盘源（`DELETE /sources/cloud/remotes/cr_01`）→ 行删除，已 ingest 的 item 保留（与 RSS 删除语义一致）；`DROP TABLE cloud_drive_remotes` 不影响其它源（仅丢失云盘配置，items 仍在）。

## 11. 风险登记

| # | 风险 | 概率 | 影响 | 缓解 |
|---|------|------|------|------|
| R1 | **4-path → 5-path 去重 O(N²) 放大**：新增云盘成为第 5 条入库 path（RCA 已发现 4-path 去重 O(N²)），可能让全量首次同步在大 vault 上变慢 | Med | High | 云盘走 `indexed_files`(source_ref，索引查询 O(1)) + `find_item_by_content_hash`(content_hash 唯一索引 O(1))，**不引入任何对 items 全表的 O(N) 扫描**；§9 加 1000-file 规模 case 验证 sync 不是 O(N²)；本 sprint 不修既有 4-path RCA 但保证不放大（不新增扫描型 path） |
| R2 | **SSRF feed/cloud URL**：用户/恶意配置把 feed URL 指向内网 (`127.0.0.1` / `169.254.169.254` 元数据端点 / 内部服务) | High | High | RSS route + worker 双重 `url_guard::validate_outbound_url`；拒绝返回 **400 `ssrf-rejected`**（复刻 git.rs 已验证修法：code 从 message 扫描提取不取 ':' 首段，避免错返 502）；云盘经 rclone 不直连任意 URL，但 remote 配置仍校验 |
| R3 | **rclone subprocess 依赖缺失**：目标机未装 rclone | High | Med | 启动/首次 add 时探测 `rclone version`；缺失 → `rclone-not-found` 503 + UI 友好提示，整个云盘源 disable，**graceful Result::Err 不 panic**；其它 5 源不受影响 |
| R4 | **增量 marker 设计错导致重复入库 / 漏入库**：云盘 ModTime 精度/时区不一致，或 SHA-1 缺失时 size+modtime 碰撞 | Med | Med | marker 优先 `Hashes.SHA-1`，无则 `size + mod_time(RFC3339)` 组合；三层去重防线兜底（content_hash 短路保证内容相同不重复入库）；§9 加「ModTime 未变跳过」case |
| R5 | **并发多源 fetch 死锁**：云盘 worker + RSS worker + 前台 add 同时持锁 | Med | High | 严守 Lock ordering `vault→vectors→fulltext→embedding`（项目铁律）；沿用 RSS 三段式（锁外 I/O，逐文档短暂持锁）；§9 concurrent case N=3 验证无死锁 |
| R6 | **rclone 命令注入**：rclone_config / remote_name / path 含 shell 元字符 | Med | High | 全程用 `std::process::Command` arg 数组传参，**绝不** shell 字符串拼接；remote_name / path 白名单校验（拒 `..` / `;` / `\|` / 反引号）；config 写临时文件（0600）经 `--config` 传，不进 argv |
| R7 | **XXE / billion-laughs 恶意 feed XML** | Med | High | feed-rs 默认不解析外部实体（验证）；`max_feed_bytes`(16 MiB) 上限挡 billion-laughs 内存放大；§9 adversarial case 显式覆盖 |
| R8 | **OOM：超大 feed body / 超大云盘文件** | Med | High | RSS `max_feed_bytes`(16 MiB) + 云盘 `max_file_bytes`(64 MiB) 双上限；超限单 feed/文件级 Err 跳过不中断整源；DocumentSink 回调模式避免一次性物化全部文档 |
| R9 | **凭据泄露**：feed URL（含 token 的私有 feed）/ rclone config（含 OAuth refresh token）明文落库或进日志 | Med | High | `url_enc` / `rclone_config_enc` 字段级 AES-256-GCM（复用 rss_feeds 现有模式）；日志只打 feed_id/remote_id 不打 URL/config（per §1.4 secrets：永不 echo / 不写 log） |
| R10 | **路径穿越**：云盘文件名/路径含 `../` 试图触达本地 fs | Low | Med | source_ref 用 rclone remote 相对 path（非本地 fs 路径），ingest_document 不按 source_ref 写本地文件；含 `..` 段的 path 跳过；§9 adversarial case 覆盖 |
| R11 | **worker tight-loop**：broken feed / 不可达云盘反复重试烧 CPU/网络 | Med | Med | 失败路径 `touch_polled_at`（RSS 已有，云盘对齐），按 `poll_interval_minutes` 退避；governor 限速沿用 |
| R12 | **大目录云盘 lsjson 一次性物化爆内存** | Low | Med | lsjson 结果是元数据（path+size+modtime，每条 ~200B），千文件 ~200KB 可接受；文件内容经 DocumentSink 逐个 cat+ingest，不一次性下载全部；超 10万文件级推 v.next 流式 lsjson |
| R13 | **跨平台 rclone 二进制差异**（Win/Linux/riscv64-K3 路径与可用性） | Med | Med | 用 `Command::new("rclone")`（PATH 查找，跨平台）；K3 镜像 v.next 再决定是否预装；缺失走 R3 graceful 路径；本 sprint 不打包 rclone（§2 写死推迟） |

---

## Appendix A: 代码勘查事实表（grounding ground-truth）

> ⚠️ 本 sprint 起点与「背景认知」有一处重要出入，已逐条核实代码（spec-analyst 亲自 Read，非盲信）：

| 主张 | 核实结果 | 证据 |
|------|---------|------|
| 「RSS 仅 staged，未实装」 | **错**。RSS 已端到端实装且可用 | `ingest/rss.rs`(586行，`RssConnector` impl + 8 单元测试)；`ingest_rss.rs`(`sync_rss_feed` 三段式)；`routes/rss.rs`(5 路由)；`state.rs`(`start_rss_sync_worker`)；`store/rss_feeds.rs` + `rss_feeds` 表 |
| RSS connector 是 stub | **否**，是生产级：条件 GET / entry dedup / HTML 剥离 / 304 处理俱全 | `rss.rs:73-347` |
| RSS 有 SSRF 校验 | **无**！`routes/rss.rs::validate()` 只查 http/https scheme，未调 `url_guard` | `routes/rss.rs:51-60`（仅 scheme）vs `ingest_git.rs:74`（git 调 `check_ssrf`） |
| `SourceKind::CloudDrive` 存在 | **是**，enum variant + as_str 已就位 | `connector.rs:17,30` |
| 云盘 connector 实装 | **无**，零实现文件 / 零 route / 零 DB 表 | grep 无 `cloud_drive.rs` / `ingest_cloud` / `cloud_drive_remotes` |
| `sync/webdav.rs` 是云盘 | **否**，是独立 WebDAV sync 路径（`SourceKind::WebDav`），与 rclone 云盘无关 | `sync/webdav.rs` |
| SSRF 拒绝须 400 非 502 | **确认**，git 历史踩坑已修 | `routes/git.rs:54-56` 注释 |
| 统一入库五步 | **确认**：parse → content_hash 短路 → insert → breadcrumbs → embed(L1+L2) → classify | `ingest/pipeline.rs:4,82-99` |
| content_hash 短路是 O(N²) 源 | **否**，`find_item_by_content_hash` 是索引查询；4-path O(N²) RCA 指别处，本 sprint 不放大 | `pipeline.rs:99` |
| 复用 SSRF guard API | `url_guard::validate_outbound_url(url, allow_hosts, resolve)` | `net/url_guard.rs` + `ingest/git.rs:361` |
| 子进程桥接先例 | whisper.cpp(asr.rs) / poppler(parser.rs) / PP-OCR(ocr/) 均 subprocess | grep `Command::new` |

**结论对范围的影响**：RSS 部分从「实装」缩为「**SSRF 加固 + 补 6 类测试下限 + 体积上限**」三项确定性工作（不编造 greenfield 工作量）；云盘是唯一真 greenfield 部分。
