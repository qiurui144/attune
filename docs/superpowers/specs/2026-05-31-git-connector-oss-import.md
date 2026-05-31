# Spec B — GitConnector 开源仓导入

> Date: 2026-05-31
> Status: Draft（待评审）
> Owner: attune OSS（Rust 商用线 `rust/`）
> 产品线归属: **OSS attune**（零行业绑定，对任何个人通用用户有价值）— 不进 attune-pro
> 关联架构: `attune-core/src/ingest/`（`SourceConnector` 统一采集抽象）

---

## 0. 调研事实基线（落档前已读真实代码）

本 spec 设计建立在以下已存在的 connector 架构事实之上（`rust/crates/attune-core/src/ingest/`）：

- **`SourceConnector` trait**（`connector.rs`）签名是**同步** `&self`：
  ```rust
  pub trait SourceConnector {
      fn source_kind(&self) -> SourceKind;
      fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()>;
  }
  ```
  约定：实现者**不入库**；单份文档可恢复错误（解析/下载失败）吞掉记日志、继续下一份；只有**源级致命错误**（无法连接/鉴权失败）才返回 `Err`。
- **`DocumentSink<'a> = Box<dyn FnMut(RawDocument) + 'a>`** — 回调式而非返回 `Vec`，大源不一次性物化避免爆内存。
- **`RawDocument` 字段**（12 个）：`uri / title / content: Vec<u8>（原始字节，未解析）/ mime_hint / source_kind / source_ref / modified_marker: Option<String> / domain / tags / corpus_domain / metadata: HashMap`。
  - `parse_filename()` 取 `source_ref` 末段供 `parser::parse_bytes` 按扩展名选解析器（`mime_hint` 不参与路由 → **`source_ref` 必须带正确扩展名**）。
  - `modified_marker` 是增量判断键：本地=SHA-256；WebDAV=ETag；Email=UID；RSS=entry guid。
- **`SourceKind` enum**（5 变体）：`LocalFolder / WebDav / Email / Rss / CloudDrive`。`as_str()`（稳定字符串，新增 variant 必须同步加分支）+ `item_source_type()`（当前全归一 `"file"`）。
- **`ingest_document` / `ingest_document_replacing`**（`pipeline.rs`，唯一入库函数）：走 parse → 判重(content_hash 短路) → insert → breadcrumbs → embed(L1+L2) → classify 五步。返回 `IngestOutcome::{Inserted/Updated/Duplicate/Skipped}`。
- **scanner 驱动模式**（`scanner.rs::scan_directory`）：构造 connector → `fetch_documents` 收集 docs → 对每份用 `modified_marker` 比对 `indexed_files.file_hash` 决定 skip / `ingest_document(_replacing)` → `store.upsert_indexed_file(dir_id, source_ref, marker, item_id)` → `update_dir_last_scan`。这是新源**复用增量基础设施**的唯一正路。
- **远程源注册模式**（`routes/remote.rs::bind_remote`，WebDAV）：route 收 url+鉴权 → `store.bind_directory("webdav:<url>")` → 内联驱动 connector + ETag 增量。GitConnector 完全对齐此模式。

> 结论：**GitConnector 不发明新机制** —— 实现 `SourceConnector`，复用 `ingest_document` + `indexed_files` 增量 + `bind_remote` 风格 route。新增工作量集中在「clone + glob 过滤 + commit SHA 增量游标」。

---

## 1. 目标定位

**用户痛点**：开发者 / 学习者 / 任何个人通用用户的知识大量沉淀在 Git 仓库里（开源项目文档、个人笔记仓、团队 wiki repo、技术书源仓如 `rust-lang/book`）。当前 attune 只能导入本地文件夹 / WebDAV / 邮箱 / RSS —— 用户要把一个 GitHub 仓的 `docs/` 导进知识库，得先手动 `git clone` 再 bind 本地目录，且无法增量跟随上游更新。

**本 feature**：让 attune 直接「输入一个仓库 URL → 自动 clone → 按 glob 过滤 → 入库 → 后续增量跟随上游 commit」。

**与产品 positioning 对齐**：
- **零行业绑定**：导入 Git 仓对律师/医生/学者/工程师/任何用户同等有价值 → 属 OSS attune（per `oss-pro-strategy.md` v2 §4.3「对任何领域的个人通用用户都有价值」判据）。**不进 attune-pro**。
- **降低 token + 数据安全**（attune 本地优先北极星）：clone 与 ingest 全在用户本地完成，仓内容不出本机；导入阶段**不调任何 LLM**（per §8 成本契约）。
- **混合智能 / 本地优先**：嵌入走现有本地算力（Ollama / ORT），不引入云依赖。

---

## 2. 范围边界

### 2.1 本版本做（v1）

- 新 `SourceKind::GitRepo` 变体 + `GitConnector` 实现 `SourceConnector`。
- 支持的 URL 形态（统一识别 + 归一）：
  - **托管平台 HTTPS**：GitHub / GitLab / Gitea / Bitbucket 的 `https://host/owner/repo(.git)`。
  - **通用 git URL**：任意 `https://…/x.git`（任意自建 git host，只要 `git clone` 能拉）。
  - **兜底 raw / tarball URL**（无 git，或用户只给单文件）：`https://…/raw/…/file.md`（单文件）或 `https://…/archive/refs/heads/main.tar.gz`（tarball 解包）。
- clone 策略：**shallow**（`--depth 1`）+ 可选 **sparse-checkout**（限定子目录，仅拉 `subdir/` 子树）。
- glob 过滤：默认 include 知识类（`*.md / *.rst / *.txt / *.adoc / 常见源码扩展 / docs/**`），可配 `include` / `exclude` glob 列表；二进制 / 超限文件跳过。
- 分支 / ref 选择：默认仓默认分支，可指定 `branch` 或 `tag`。
- 子目录限定：`subdir`（如只导 `docs/`）。
- 增量同步：记录上次 ingest 的 **commit SHA** 游标；再次同步时 `git fetch` + `git diff <old>..<new> --name-status` 只 ingest 变更文件（A/M 入库，D 删除对应 item）。
- 私有仓 token：走环境变量 / 加密配置（per 全局 §1.4），**绝不硬编码 / 不写库明文 / 不进日志**。
- 限额：文件数上限、单文件大小上限、仓总大小上限（拒绝超大仓，per §7 / §8）。
- UI 入口：Settings → 数据源 加「从 Git 仓库导入」（URL + 分支 + 子目录 + glob）。
- 后端 endpoint：`POST /api/v1/index/bind-git`（对齐 `bind-remote` 风格）+ 增量同步路径。

### 2.2 本版本不做（写死，禁止 silent scope creep）

- ❌ SSH 协议 clone（`git@host:…`）—— v1 仅 HTTPS（避免 SSH key 管理 + 已知 host 复杂度）；推 v.next。
- ❌ OAuth App / GitHub App 授权流 —— v1 私有仓仅 PAT（personal access token）。
- ❌ Webhook 实时推送同步 —— v1 仅手动 / 定时 poll 增量。
- ❌ commit 历史 / issue / PR / wiki tab 导入 —— v1 仅**工作树文件**（默认分支 HEAD 文件内容）。
- ❌ Git LFS 大文件拉取 —— v1 跳过 LFS 指针文件（当文本跳过或忽略）。
- ❌ 双向同步 / push 回仓 —— attune 永远只读导入。
- ❌ monorepo 子模块（submodule）递归 —— v1 `git clone` 不带 `--recurse-submodules`。

### 2.3 后续版本

- v.next：SSH 协议、OAuth 授权流、webhook 实时同步、submodule、LFS。

---

## 3. 架构数据流

```
                    ┌─────────────── route 层 (attune-server) ───────────────┐
 用户 (Settings UI)  │  POST /api/v1/index/bind-git                            │
   repo URL ─────────┼─▶ ① URL 归一 + SSRF 白名单校验 (host allowlist / 拒内网) │
   branch/subdir     │  ② token 取自 env/加密配置 (不入 body 明文持久化)        │
   glob/token        │  ③ store.bind_directory("git:<normalized-url>#<ref>")   │
                    └────────────────────────┬───────────────────────────────┘
                                             │ dir_id + GitSourceConfig
                                             ▼
        ┌──────────────── attune-core::ingest::git::GitConnector ─────────────┐
        │  fetch_documents(&self, sink):                                       │
        │   repo URL ──▶ ④ 选策略:                                             │
        │        ├─ git host / 通用 git ──▶ shallow clone (--depth 1)          │
        │        │                          [+ sparse-checkout subdir]         │
        │        │                          ──▶ 临时目录 (tempfile::TempDir)   │
        │        └─ raw/tarball 兜底 ──▶ HTTP GET ──▶ 单文件 / 解包 tar         │
        │   ⑤ walk 工作树 ──▶ glob include/exclude 过滤                         │
        │                  ──▶ 二进制/超限/LFS 跳过                              │
        │   ⑥ 逐文件 ──▶ RawDocument{                                          │
        │        uri = "git://<host>/<owner>/<repo>@<sha>/<relpath>"           │
        │        content = 文件原始字节                                         │
        │        source_kind = GitRepo                                         │
        │        source_ref = "<repo-slug>/<relpath>"  (带扩展名 → parser 路由)│
        │        modified_marker = <blob SHA-1 或 commit SHA + relpath hash>   │
        │        metadata = {repo, branch, commit, host}                       │
        │      } ──▶ sink(doc)                                                 │
        └──────────────────────────┬───────────────────────────────────────────┘
                                   │ 每份 RawDocument
                                   ▼   (scanner 风格驱动循环, route 内联或 git worker)
        ┌──────────── 复用现有 ingest pipeline (零改动) ───────────────────────┐
        │  per doc:                                                            │
        │   marker 比对 indexed_files.file_hash ─▶ 未变 ⇒ skip                 │
        │   变更 ⇒ ingest_document / ingest_document_replacing                 │
        │          (parse → content_hash 判重 → insert → breadcrumbs           │
        │           → embed L1+L2 (本地算力) → classify)                       │
        │   ⇒ store.upsert_indexed_file(dir_id, source_ref, marker, item_id)   │
        │  循环后: store.update_dir_last_scan(dir_id)                          │
        │          + 持久化本次 commit SHA 游标 (增量起点)                       │
        └──────────────────────────────────────────────────────────────────────┘
                                   ▼
                         临时 clone 目录: TempDir drop 自动清理 (无残留)
```

**DB tables**（复用现有，最小新增）：

| 表 | 用途 | 改动 |
|----|------|------|
| `bound_dir`（现有） | 绑定源登记，`path = "git:<url>#<ref>"` | 复用，无 schema 改 |
| `indexed_files`（现有） | `(dir_id, source_ref, file_hash=marker, item_id)` 增量基础设施 | 复用，无 schema 改 |
| `git_sources`（**新增**） | `dir_id PK / url / host / branch / subdir / include_glob / exclude_glob / last_commit_sha / token_ref(env 键名，非明文) / max_files / max_bytes / last_synced_at` | 新表（per §10 migration） |

**cache layers**：clone 走临时目录（`tempfile::TempDir`），用完即 drop 清理（per 全局磁盘铁律）；增量靠 `last_commit_sha` + `indexed_files.file_hash`，不缓存仓内容。

---

## 4. 模块边界

| 层 | crate / file | 职责 | 新增/改动 |
|----|--------------|------|-----------|
| 采集抽象 | `attune-core/src/ingest/connector.rs` | `SourceKind::GitRepo` 变体 + `as_str()` 分支 | 改动（加 1 变体 + 1 分支） |
| 采集抽象 | `attune-core/src/ingest/mod.rs` | `pub mod git;` + re-export `GitConnector / GitSourceConfig` | 改动（导出） |
| **核心实现** | `attune-core/src/ingest/git.rs`（**新文件**） | `GitConnector` impl `SourceConnector`；URL 归一；clone 策略选择；glob 过滤；增量 diff；`GitCloner` trait（生产 = git CLI 子进程；测试 = mock） | 新增 |
| 调度 / 增量 | `attune-core/src/scanner_git.rs`（**新文件**，对齐 `scanner_webdav` / `scanner_rss`） | `sync_git_source(store, dek, dir_id)` 公共函数：构造 connector + 驱动循环 + commit SHA 游标持久化 + 删除消失文件 | 新增 |
| 存储 | `attune-core/src/store.rs`（或 store 子模块） | `git_sources` 表 CRUD：`create_git_source / get_git_source / update_git_cursor / list_git_sources` | 改动（新方法） |
| route | `attune-server/src/routes/git.rs`（**新文件**，对齐 `routes/remote.rs`） | `POST /api/v1/index/bind-git`（首次绑定+全量）/ `POST /api/v1/index/sync-git`（手动增量） | 新增 |
| route 注册 | `attune-server/src/routes/mod.rs`（或 router builder） | 挂载 git route | 改动 |
| URL 安全 | `attune-core/src/net/url_guard.rs`（**新或复用**） | SSRF host allowlist + 拒私有 IP / loopback / link-local；GitConnector 与 raw/tarball fetch 共用 | 新增/复用（per §11） |
| UI | `attune-server/ui/src/`（SettingsView 数据源 tab） | 「从 Git 仓库导入」表单 + i18n key（zh/en 同步，per i18n 铁律） | 改动 |

**跨仓边界**：纯 OSS attune 仓内改动，**不触 attune-pro / attune-enterprise**。

---

## 5. API 契约

### 5.1 `SourceKind` 新变体（core）

```rust
pub enum SourceKind {
    LocalFolder,
    WebDav,
    Email,
    Rss,
    CloudDrive,
    GitRepo,   // 新增
}
// as_str(): SourceKind::GitRepo => "git_repo"
// item_source_type(): 仍归一 "file"（兼容现有检索/分类加权）
```

### 5.2 `GitSourceConfig`（core，connector 构造输入）

```rust
pub struct GitSourceConfig {
    pub url: String,              // 用户输入原始 URL（归一前）
    pub branch: Option<String>,   // None = 默认分支
    pub subdir: Option<String>,   // None = 整仓；Some("docs") = sparse-checkout
    pub include_glob: Vec<String>,// 空 = 用默认知识类 glob
    pub exclude_glob: Vec<String>,
    pub corpus_domain: Option<String>, // F-Pro 透传，缺省 general（OSS 一般不填）
    pub token_ref: Option<String>,// env 变量名（如 "ATTUNE_GIT_TOKEN_<id>"）；非明文 token
    pub max_files: usize,         // 默认 5000
    pub max_file_bytes: u64,      // 默认 5 MiB
    pub max_total_bytes: u64,     // 默认 500 MiB（拒绝超大仓）
    pub last_commit_sha: Option<String>, // 增量起点；None = 全量首扫
}
```

`GitConnector::source_kind()` 返回 `SourceKind::GitRepo`；`fetch_documents` 行为见 §3 数据流 ④⑤⑥。

### 5.3 REST endpoints（kebab-case，per API 命名规范）

**`POST /api/v1/index/bind-git`** — 首次绑定 + 全量导入（对齐 `bind-remote`）

请求：
```json
{
  "url": "https://github.com/rust-lang/book",
  "branch": "main",            // 可选
  "subdir": "src",             // 可选，sparse-checkout
  "include_glob": ["**/*.md"], // 可选，空=默认知识类
  "exclude_glob": ["**/SUMMARY.md"],
  "corpus_domain": "general",  // 可选
  "max_files": 5000            // 可选，服务端 clamp
}
```
私有仓 token：**不进 body**。客户端先调 `PUT /api/v1/settings/git-token`（token 仅存加密配置，记 env 键名）；`bind-git` 通过 `token_ref` 关联。

响应（形态对齐 `bind-remote` 的 `scan` 字段）：
```json
{ "status": "ok",
  "dir_id": "git:https://github.com/rust-lang/book#main",
  "commit": "<sha>",
  "scan": { "total_files": 320, "new_files": 318, "updated_files": 0,
            "skipped_files": 2, "errors": 0 } }
```

**`POST /api/v1/index/sync-git`** — 手动增量同步
```json
{ "dir_id": "git:https://github.com/rust-lang/book#main" }
```
响应同上 `scan` 形态（new/updated/skipped/deleted 计数）。

**`PUT /api/v1/settings/git-token`** — 写私有仓 token（仅存加密配置，返回 `token_ref` 键名，不回显 token）。

错误响应统一 `AppError` JSON shape（per v0.6.3 约定）：`{"error": "<脱敏 msg>", "code": "<kebab>"}`。错误码见 §7。

---

## 6. 扩展点 / 插件接口

1. **`GitCloner` trait**（git.rs 内）抽象 clone/fetch/diff，生产实现走 git CLI 子进程（`std::process::Command`，跨平台 per Rust 跨平台规范），测试注入 mock —— **新 git host 无需新实现**（只要 git URL 能 clone）。
2. **URL 归一表驱动**：`host → (web_url_pattern, raw_url_pattern, tarball_url_pattern)` 映射表。加新托管平台（如 codeberg / sourcehut）= 加一行映射，不改 connector 主逻辑。
3. **raw/tarball fetcher** 抽象为 trait，与 RSS 的 `FeedFetcher` 同模式 → 后续协议（IPFS gateway / 其他归档格式）走同扩展点。
4. **glob 默认集** 抽到常量 + 可被 `GitSourceConfig` 覆盖 → 后续按 corpus_domain 给不同默认 glob 留口。
5. 复用 `SourceConnector` 统一抽象本身即扩展点：未来任意新源走同 `fetch_documents + sink` 契约，零 pipeline 改动。

---

## 7. 错误 + 边界 case

| 场景 | 处理 | exit/错误码 |
|------|------|-------------|
| 无效 / 非法 URL（非 http(s)、解析失败） | route 400 拒绝 | `invalid-git-url` |
| URL 指向内网 / loopback / link-local（SSRF） | route 400 拒绝（per §11 白名单） | `git-url-not-allowed` |
| 私有仓鉴权失败（401/403 from git） | 源级致命 → `fetch_documents` 返回 `Err`，route 502 | `git-auth-failed` |
| 仓不存在 / 404 | 源级致命 → `Err` | `git-repo-not-found` |
| 分支 / ref 不存在 | 源级致命 → `Err` | `git-ref-not-found` |
| 网络断 / clone 超时 | 源级致命 → `Err`（带超时，默认 120s）；已绑定源同步失败保留旧游标，不清数据 | `git-network-error` |
| 超大仓（超 `max_total_bytes`） | clone 后估算超限 → `Err` 提示用户加 subdir / 调小范围 | `git-repo-too-large` |
| 文件数超 `max_files` | 截断到上限 + 响应 `truncated: true` 警告（不致命） | （warn，scan.errors 不增） |
| 单文件超 `max_file_bytes` | 跳过该文件，记日志（可恢复，继续下一份） | （skip） |
| 二进制文件（非文本 mime / NUL 字节探测） | 跳过（不喂 sink），记日志 | （skip） |
| LFS 指针文件 | 跳过（v1 不拉 LFS） | （skip） |
| 单文件解析失败 | `ingest_document` 内吞掉 → `IngestOutcome::Skipped` / connector 记日志续下一份 | （skip，scan.errors++） |
| git CLI 不存在（环境缺 git） | 启动期 / 首次 bind 探测 → 友好提示「需安装 git」 | `git-cli-missing` |
| 增量 diff 出现删除（D） | `sync-git` 删除对应 item + `indexed_files` 行 | （deleted++） |
| 临时 clone 目录清理失败 | 记日志，不阻塞（TempDir best-effort） | （warn） |

**graceful degradation**：单文档级错误（解析/超限/二进制）一律吞掉续跑（per `SourceConnector` 约定）；仅源级（连接/鉴权/404/ref/超大）返回 `Err`。已绑定源同步失败**保留旧 `last_commit_sha` 游标**，不丢已入库数据。

---

## 8. 成本契约（三层归属）

| 阶段 | 资源层 | 谁买单 | UI 显示 |
|------|--------|--------|---------|
| URL 归一 + SSRF 校验 | 🆓 零成本（CPU 毫秒） | — | 即时 |
| clone / fetch（网络 IO） | 🆓 零成本（**网络带宽**，无 LLM / 无 GPU） | 用户带宽 | 「正在克隆…」进度 |
| glob 过滤 + 文件读 + parse + 分词 + BM25 入库 | 🆓 零成本（CPU） | — | scan 进度条 |
| embedding（L1 章节 + L2 段落块） | ⚡ 本地算力（GPU/NPU 秒级） | 用户本地算力 | 后台任务队列可见 + 可暂停（顶栏开关） |
| 150 字存档摘要（若现有 pipeline 触发） | ⚡ 本地算力 | 本地 | 后台队列 |
| **导入阶段 LLM 调用** | 💰 — | **明确：不调任何 LLM** | — |

**硬约束**（对齐成本契约最高优先原则）：
1. **导入 = 建库阶段，永不升第三层**。clone + ingest 只跑到「能被搜到 + 有存档摘要」；深度分析 / 批注 / 观点提取一律等用户在 Chat 显式开口（per 成本契约规则 1+2）。
2. **不在导入路径调云 API / LLM**。GitConnector 路径无任何 LLM endpoint 调用。
3. UI 在「从 Git 仓库导入」按钮旁标注 `~本地 · 网络克隆 + 本地嵌入`（零金钱成本），与其它数据源一致。
4. 超大仓限额（`max_total_bytes` / `max_files`）即成本护栏 —— 防一次性把巨型 monorepo 灌进本地算力队列。

---

## 9. 测试矩阵

**语料（真实仓，版本固化 per `docs/TESTING.md` 禁止随机测试数据）**：

| 语料仓 | pin（commit/tag） | 用途 |
|--------|-------------------|------|
| `rust-lang/book` | 固定 commit SHA | happy path（大量 `.md` in `src/`，子目录限定测试） |
| `CyC2018/CS-Notes` | 固定 commit SHA | 中文 `.md` + 图片二进制混合（二进制跳过 + i18n） |
| 自建 fixture bare repo（`tests/fixtures/git/`） | 仓内固定 | 增量 diff（A/M/D）、超限、空仓、私有 token mock |

**6 类下限覆盖**（per 全局 §6.1 + `docs/TESTING.md`）：

| 类型 | case |
|------|------|
| **happy path** | clone `rust-lang/book` → glob `**/*.md` → 文件数/标题/内容入库正确；sparse `src/` 子目录只拉子树 |
| **edge** | 空仓 / 仓只有二进制 / 单文件仓 / 超长路径名 / 仓内符号链接 / `subdir` 不存在 |
| **error** | 无效 URL / 404 仓 / ref 不存在 / 鉴权失败（mock 401）/ 网络超时（mock）/ git CLI 缺失 |
| **adversarial** | SSRF：`http://127.0.0.1` / `http://169.254.169.254`（云 metadata）/ `http://[::1]` / 内网 `http://192.168.x.x` / DNS rebinding 域名 → 全部拒绝（§11）；path traversal：仓内 `../` 路径名不得逃出 vault |
| **多源/并发** | 同时绑定多个 git 源；同一仓二次 bind（去重 / UNIQUE）；增量同步与全量扫描并发 lock ordering（per lock 顺序约定） |
| **资源耗尽** | 超 `max_files` 截断；超 `max_file_bytes` 跳过；超 `max_total_bytes` 拒绝；磁盘满时 TempDir 失败 graceful |
| **国际化** | `CS-Notes` 中文内容入库 + tantivy-jieba 分词；UI 表单 zh/en key 齐 |
| **降级** | embedding provider 不可用 → 仍入库可 BM25 搜到（embed 入队列待补，不阻塞导入） |

**测试代码组织**：
- 单元：`git.rs` 内 `#[cfg(test)]`（URL 归一、glob 过滤、增量 diff 解析、二进制探测）；`GitCloner` mock 注入。
- proptest：≥3（URL 归一幂等、glob 匹配、marker 稳定性）。
- 集成 E2E：`attune-core/tests/git_connector.rs`（真实 bare fixture repo，不走网络）+ `attune-server` subprocess 测 `bind-git` route。
- 回归 fixture：每修一个 bug 加 1（per Agent 验证铁律 / golden set 永久）。

**通过判据**：deterministic pass rate = 1.00；无网络依赖（CI 用本地 bare fixture，真实平台仓仅手动 / nightly soak 验证）。

---

## 10. 向后兼容

- **新增 `SourceKind::GitRepo` 是纯追加**，不改现有 5 变体语义；`as_str()` / `item_source_type()` 加分支不影响旧源。`item_source_type()` 仍归一 `"file"` → 现有检索 / 分类加权对 git 来源**透明无感**（旧 client / 旧检索逻辑零改动）。
- **`RawDocument` 字段不变** → GitConnector 复用既有 12 字段，pipeline 零改动。
- **`indexed_files` / `bound_dir` schema 不变** → git 源借道现有增量基础设施（`path = "git:<url>#<ref>"` 是已支持的字符串绑定，同 WebDAV 的 `"webdav:<url>"`）。
- **新表 `git_sources`** 走 migration（schema versioning）：`CREATE TABLE IF NOT EXISTS`；老 vault 升级时表为空、不影响既有数据；无 git 源的用户该表恒空。**无破坏性 migration / 无数据迁移**。
- **新 endpoint 纯追加**（`bind-git` / `sync-git` / `git-token`）→ 旧 Chrome 扩展 / 旧 client 不调即无影响。
- **回滚路径**：删 git 源 = 删 `bound_dir` 行 + 级联 `indexed_files` + `git_sources` 行（已入库 item 按用户选择保留或删，对齐解绑本地目录行为）。

---

## 11. 风险登记

| 风险 | 等级 | 缓解 |
|------|------|------|
| **SSRF（用户给的 URL 打内网）** | 🔴 高 | route 层 + connector + raw/tarball fetcher **共用** `url_guard`：① 仅允许 `http(s)`；② 解析 host → 解析 IP → **拒绝** loopback(127/::1) / private(10./172.16./192.168./fc00::) / link-local(169.254./fe80::，含云 metadata 169.254.169.254) / `0.0.0.0`；③ 默认 host allowlist（github.com / gitlab.com / gitea.* / bitbucket.org / 用户在 settings 显式加的自建 host）；④ 防 DNS rebinding：解析后用 IP 连接或连后复核 IP。**对齐现有 settings URL 校验路径**（与 WebDAV / RSS remote fetch 同一 guard，不重复造轮子）。adversarial 测试矩阵（§9）专项覆盖。 |
| **私有仓 token 泄露** | 🔴 高 | per 全局 §1.4：token 走 env / 加密配置（sops/0600），DB 只存 `token_ref` 键名；**不进 body 持久化 / 不进日志 / 不进 commit / 不回显**；git CLI 调用通过 credential helper 或 `https://<token>@host` 仅在进程内存拼装，子进程 env 传递不落盘。 |
| **path traversal（仓内恶意 `../` 路径）** | 🟡 中 | `source_ref` 用仓内相对路径归一（`Path::components` 拒 `..` / 绝对路径）；ingest 不按 source_ref 写本地文件（只入加密 DB），逃逸面小但仍校验。 |
| **超大仓 / monorepo 撑爆磁盘+算力** | 🟡 中 | shallow `--depth 1` + sparse subdir + `max_total_bytes`/`max_files`/`max_file_bytes` 三限额；clone 用 `TempDir` 即时清理（per 全局磁盘铁律红黄绿线）；超限 `git-repo-too-large` 拒绝。 |
| **git CLI 跨平台 / 缺失** | 🟡 中 | `std::process::Command`（不依赖 shell，per Rust 跨平台规范）；探测 git 存在性，缺失给 `git-cli-missing` 友好提示；Win/Linux 安装包说明依赖（git 常见预装，文档标注）。 |
| **增量游标漂移 / force-push 上游** | 🟡 中 | 上游 force-push 致 `git diff <old>..<new>` 失败 → fallback 全量重扫（fetch 全部 + 按 `indexed_files.file_hash` 逐文件比对，content_hash 短路兜底，不重复嵌入）；游标只在成功 ingest 后推进。 |
| **二进制 / 大文件误入库** | 🟢 低 | NUL 字节探测 + mime 推断 + 扩展名 glob 三重过滤；超 `max_file_bytes` 跳过。 |
| **clone 临时目录残留（磁盘）** | 🟢 低 | `tempfile::TempDir` RAII drop 清理；同步失败路径也走 drop（per 全局磁盘自查铁律，会话末 df 检查）。 |
| **并发 lock 死锁** | 🟢 低 | 复用现有 lock ordering（`vault → vectors → fulltext → embedding`，per v0.7 约定）；git worker 走 `enqueue_reindex` 间接路径，不自己直调 vectors/fulltext API。 |

---

## 附录 A — 评审检查清单（spec → plan 前）

- [ ] `SourceConnector` 同步 `&self` 签名约束已确认（GitConnector 内部如需 async clone，用单线程 runtime 桥接，对齐 Email 的 `RealImapFetcher` 模式）。
- [ ] SSRF `url_guard` 是复用现有 settings 校验还是新建？评审拍板（§4 标注「新或复用」）。
- [ ] `git_sources` 新表 schema 字段评审。
- [ ] 默认 glob 集合 + 三限额默认值评审（`max_files=5000 / max_file_bytes=5MiB / max_total_bytes=500MiB`）。
- [ ] 私有仓 token 存储路径（加密配置文件 vs env）与 settings 现有 secret 管理对齐确认。
- [ ] 评审通过 → invoke `superpowers:writing-plans` 出实施 plan（文件清单 + commit 分批 + GA 验收清单）。
