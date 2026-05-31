# Implementation Plan — GitConnector 开源仓导入

> Date: 2026-05-31
> Spec: `docs/superpowers/specs/2026-05-31-git-connector-oss-import.md`（已批准）
> 产品线: **OSS attune**（`rust/`），零行业绑定 — **不进 attune-pro**
> 估期: 6 个工作阶段（D1–D6），单 worktree 串行
> Status: Draft（待评审）

---

## 0. 落档前代码复核（plan 文件 path 已对真实代码核实）

> 本 plan 所有 path 均已 `Read`/`grep` 验证存在。spec 与现实的偏差已在下表修正，作为本 plan 的权威依据。

| spec 写法 | 真实情况（已核实） | plan 采用 |
|-----------|-------------------|-----------|
| `SourceConnector` 同步 `&self`，`DocumentSink = Box<dyn FnMut>` | ✅ 与 `connector.rs:96-107` 一致 | 照 spec |
| 调度参照 `scanner_webdav` / `scanner_rss` | ⚠️ **无 `scanner_rss.rs`**。RSS/WebDAV 的「route + 周期 worker 共用入库逻辑」实际在 **server 端** `attune-server/src/ingest_rss.rs` / `ingest_webdav.rs`（非 core scanner） | git 同步逻辑落 **`attune-server/src/ingest_git.rs`**（对齐 `ingest_rss.rs`），**不**新建 core `scanner_git.rs` |
| migration 文件 | ⚠️ **无独立 migration 文件机制**。schema 是 `store/mod.rs::SCHEMA_SQL` 常量（`execute_batch` 于 open 时跑），新表用 `CREATE TABLE IF NOT EXISTS` 自动建；`PRAGMA user_version`（`SCHEMA_VERSION=1`）仅用于「学习态」语义迁移 | `git_sources` 表追加进 `SCHEMA_SQL` 常量；**SCHEMA_VERSION 不动**（纯追加表无需 bump，老 vault 下次 open 自动得空表） |
| SSRF `url_guard`「新或复用」 | ⚠️ **现无任何 SSRF/url guard**。RSS route 明确「不校验 DNS/可达性」（`routes/rss.rs:50`） | **新建** `attune-core/src/net/url_guard.rs`（评审拍板：新建，无可复用件） |
| store 方法 | webdav 模式：`store/webdav_remotes.rs`，方法签名带 `dek: &Key32`（`upsert_webdav_remote(&self, dek, input)`） | `git_sources` 表 CRUD 落 **`store/git_sources.rs`**，token 字段走 `dek` 加密同 webdav |
| route 注册 | `routes/mod.rs` 不注册；router 在 **`attune-server/src/lib.rs`**（`lib.rs:247` 挂 `/api/v1/index/bind-remote`） | git route 在 `lib.rs` 挂载 |
| bind 接口 | `store/dirs.rs::bind_directory(&self, path, recursive, file_types)` → 返回 dir_id String | git 用 `bind_directory("git:<url>#<ref>", false, &[...])` |
| UI 数据源入口 | `ui/src/views/RemoteView.tsx`（modal `'local' \| 'webdav'`）+ `ui/src/hooks/useRemote.ts`（`bindWebdav` POST `/index/bind-remote`）+ i18n `i18n/{en,zh}.ts`（flat key，如 `'remote.action.add_webdav'`） | git 入口扩 `RemoteView` modal 加 `'git'` + `useRemote.ts` 加 `bindGit` + i18n 加 `remote.*.git.*` key（zh/en 同步） |

---

## 1. 阶段日历（D1–D6，串行）

| 阶段 | 主题 | 关键交付 | 依赖 |
|------|------|---------|------|
| **D1** | 数据底座 | `SourceKind::GitRepo` 变体 + `git_sources` 表（进 SCHEMA_SQL）+ `store/git_sources.rs` CRUD + `GitSourceConfig` 结构 | — |
| **D2** | SSRF guard | `net/url_guard.rs`（host allowlist + 拒私网/loopback/link-local + 防 rebinding）+ 单测 + proptest | — |
| **D3** | GitConnector | `ingest/git.rs`：URL 归一表 + `GitCloner` trait（生产=git CLI 子进程 / 测试=mock）+ shallow/sparse clone + glob 过滤 + 二进制/超限跳过 + `RawDocument` 产出 | D1, D2 |
| **D4** | 增量 + 调度 | `ingest/git.rs` 增量 diff（commit SHA 游标，`git diff <old>..<new> --name-status`，force-push fallback 全量）+ **`attune-server/src/ingest_git.rs`**（bind/sync 共用驱动循环，对齐 `ingest_rss.rs`）+ 游标持久化 + 删除消失文件 | D3 |
| **D5** | route + UI | `routes/git.rs`（`bind-git` / `sync-git` / `git-token`）+ `lib.rs` 挂载 + `RemoteView` git modal + `useRemote.ts::bindGit` + i18n zh/en key | D4 |
| **D6** | E2E + 验收 | bare-repo fixture 集成测 + 真仓（`rust-lang/book` / `CS-Notes` pin）手动/nightly 验证 + GA 验收清单逐条勾 + RELEASE.md | D5 |

---

## 2. 文件清单（每阶段具体 path）

### D1 — 数据底座
- 改 `rust/crates/attune-core/src/ingest/connector.rs`
  - `SourceKind` 加 `GitRepo` 变体；`as_str()` 加 `GitRepo => "git_repo"` 分支（含同步现有 `source_kind_as_str_round_trips` 测试数组）
- 改 `rust/crates/attune-core/src/store/mod.rs`
  - `SCHEMA_SQL` 常量追加 `CREATE TABLE IF NOT EXISTS git_sources (...)`（字段见 §3）+ `CREATE INDEX IF NOT EXISTS idx_git_sources_synced ...`
  - `pub mod git_sources;`（mod.rs 顶部 mod 声明区，对齐 `pub mod webdav_remotes;`）
- 新 `rust/crates/attune-core/src/store/git_sources.rs`
  - `GitSourceInput` / `GitSourceRow` 结构 + `upsert_git_source(&self, dek, input)` / `get_git_source(&self, dek, dir_id)` / `list_git_sources(&self, dek)` / `update_git_cursor(&self, dir_id, commit_sha)` / `touch_git_synced(&self, dir_id)`（token_ref 字段经 `dek` 加密，对齐 webdav password）
- 新 `rust/crates/attune-core/src/ingest/git.rs`（仅本阶段放结构体骨架）
  - `pub struct GitSourceConfig { ... }`（spec §5.2 全字段）
- 改 `rust/crates/attune-core/src/ingest/mod.rs`
  - `pub mod git;` + re-export `pub use git::{GitConnector, GitSourceConfig};`

### D2 — SSRF guard
- 新 `rust/crates/attune-core/src/net/mod.rs`（`pub mod url_guard;`）+ 改 `attune-core/src/lib.rs` 加 `pub mod net;`
- 新 `rust/crates/attune-core/src/net/url_guard.rs`
  - `pub fn validate_outbound_url(url: &str, allowlist: &[String]) -> Result<Url>`：① 仅 http(s)；② 解析 host→IP，拒 loopback(`127.0.0.0/8`,`::1`)/private(`10.`,`172.16/12`,`192.168/16`,`fc00::/7`)/link-local(`169.254/16`,`fe80::/10`,含 `169.254.169.254`)/`0.0.0.0`；③ host allowlist（默认 github.com/gitlab.com/bitbucket.org/gitea.* + settings 自建 host）；④ 防 rebinding：返回已解析 IP 供调用方按 IP 连接
  - `#[cfg(test)]`：SSRF 拒绝表 + allowlist + proptest（归一幂等）

### D3 — GitConnector 核心
- 改 `rust/crates/attune-core/src/ingest/git.rs`
  - `pub trait GitCloner { fn clone_shallow(...); fn fetch(...); fn diff_name_status(...); }`
  - `struct CliGitCloner`（`std::process::Command("git")`，跨平台无 shell；探测 git 存在性 → `git-cli-missing`；token 仅进程内存拼 `https://<token>@host`）
  - `fn normalize_url(raw) -> NormalizedRepo`（host 映射表：github/gitlab/gitea/bitbucket → web/raw/tarball pattern；表驱动，加平台=加一行）
  - `pub struct GitConnector { config, cloner, guard_allowlist }`，`impl SourceConnector`：
    - `source_kind() -> SourceKind::GitRepo`
    - `fetch_documents(&self, sink)`：调 url_guard → clone（shallow + 可选 sparse subdir，到 `tempfile::TempDir`）→ walk 工作树 → glob include/exclude（默认知识类常量）→ 二进制(NUL 探测)/超 `max_file_bytes`/LFS 指针跳过 → 逐文件构 `RawDocument`（`source_ref` 带正确扩展名 → parser 路由）→ `sink(doc)`；源级错误（鉴权/404/ref/超大/超时）返回 `Err`
  - `#[cfg(test)]`：URL 归一、glob 匹配、二进制探测、路径 `..` traversal 拒绝；`MockGitCloner` 注入；proptest（归一幂等 / glob / marker 稳定性 ≥3）

### D4 — 增量 + 调度
- 改 `rust/crates/attune-core/src/ingest/git.rs`
  - 增量：config 带 `last_commit_sha` 时 `fetch` + `diff_name_status(old, new)` → A/M 产 doc、D 标删；force-push 致 diff 失败 → fallback 全量（按 `indexed_files.file_hash` + content_hash 短路兜底）
- 新 `rust/crates/attune-server/src/ingest_git.rs`（对齐 `ingest_rss.rs` / `ingest_webdav.rs`）
  - `pub async fn sync_git_source(state, dir_id) -> ScanResult`：取 `git_sources` 配置 → 构 `GitConnector` → scanner 风格驱动循环（`fetch_documents` → 每 doc 比对 `indexed_files.file_hash` → `ingest_document`/`ingest_document_replacing` → `upsert_indexed_file`）→ 处理 D（删 item + `indexed_files` 行）→ `update_dir_last_scan` + `update_git_cursor`（成功才推进游标）→ `touch_git_synced`
  - lock ordering 复用 `vault→vectors→fulltext→embedding`；走 `enqueue_reindex` 间接路径不直调 vectors/fulltext

### D5 — route + UI
- 新 `rust/crates/attune-server/src/routes/git.rs`（对齐 `routes/remote.rs`）
  - `pub async fn bind_git(...)`：url_guard 校验 → token 取自加密配置（不入 body 持久化）→ `store.bind_directory("git:<url>#<ref>", false, &[...])` → `store.upsert_git_source(dek, ...)` → `ingest_git::sync_git_source`（全量）→ 返回 `{status, dir_id, commit, scan}`
  - `pub async fn sync_git(...)`：按 dir_id 增量
  - `pub async fn put_git_token(...)`：写加密配置，返回 `token_ref` 键名（不回显 token）
  - 错误走 `AppError` JSON shape，错误码 per spec §7（`invalid-git-url` / `git-url-not-allowed` / `git-auth-failed` / `git-repo-not-found` / `git-ref-not-found` / `git-network-error` / `git-repo-too-large` / `git-cli-missing`）
- 改 `rust/crates/attune-server/src/lib.rs`
  - 加 `.route("/api/v1/index/bind-git", post(routes::git::bind_git))` + `sync-git` + `.route("/api/v1/settings/git-token", put(routes::git::put_git_token))`
- 改 `rust/crates/attune-server/src/routes/mod.rs`：`pub mod git;`
- 改 `rust/crates/attune-server/ui/src/hooks/useRemote.ts`
  - `GitInput` type + `bindGit(input)` POST `/index/bind-git` + `syncGit(dirId)` POST `/index/sync-git`
- 改 `rust/crates/attune-server/ui/src/views/RemoteView.tsx`
  - modal 类型加 `'git'`；顶栏加按钮 `t('remote.action.add_git')`；git 表单（url + branch + subdir + include/exclude glob + 可选 token）
- 改 `rust/crates/attune-server/ui/src/i18n/en.ts` + `i18n/zh.ts`（**两文件 key 集合必须一致**，per 项目 i18n 铁律）
  - 新 key：`remote.action.add_git` / `remote.modal.git.title` / `remote.modal.git.url` / `remote.modal.git.branch` / `remote.modal.git.subdir` / `remote.modal.git.glob` / `remote.modal.git.token` / `remote.modal.git.cost_hint`（`~本地 · 网络克隆 + 本地嵌入`）/ `remote.toast.git_bind_success` / `remote.toast.git_bind_fail` / `remote.toast.git_sync_success` / `items.source.git`

### D6 — 测试 + 验收
- 新 `rust/crates/attune-core/tests/git_connector.rs`（bare fixture，无网络）
- 新 `rust/crates/attune-core/tests/fixtures/git/`（脚本生成 bare repo：A/M/D 增量、空仓、二进制混合、私有 token mock）
- 新 `rust/crates/attune-server/tests/git_route_subprocess.rs`（subprocess 测 `bind-git` route）
- 改 `docs/TESTING.md`（git 语料 pin 表）+ `RELEASE.md`（feature 节 + Known Limitations: SSH/OAuth/webhook/LFS/submodule 不做）

---

## 3. `git_sources` 表 schema（进 SCHEMA_SQL）

```sql
CREATE TABLE IF NOT EXISTS git_sources (
    dir_id          TEXT PRIMARY KEY,          -- = bound_dirs.id ("git:<url>#<ref>")
    url             TEXT NOT NULL,             -- 归一后 URL
    host            TEXT NOT NULL,
    branch          TEXT,                      -- NULL = 默认分支
    subdir          TEXT,                      -- NULL = 整仓
    include_glob    TEXT NOT NULL DEFAULT '',  -- JSON array；'' = 默认知识类
    exclude_glob    TEXT NOT NULL DEFAULT '',
    corpus_domain   TEXT,
    token_ref_enc   BLOB,                      -- dek 加密的 env 键名，非明文 token；NULL = 公开仓
    max_files       INTEGER NOT NULL DEFAULT 5000,
    max_file_bytes  INTEGER NOT NULL DEFAULT 5242880,
    max_total_bytes INTEGER NOT NULL DEFAULT 524288000,
    last_commit_sha TEXT,                      -- 增量游标；NULL = 未首扫
    last_synced_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_git_sources_synced ON git_sources(last_synced_at);
```

`SCHEMA_VERSION` **保持 1**（纯追加表，老 vault open 时自动建空表，无破坏性迁移）。

---

## 4. commit 分批（单一职责）

| # | 阶段 | message |
|---|------|---------|
| 1 | D1 | `feat(ingest): add SourceKind::GitRepo + git_sources table + GitSourceConfig` |
| 2 | D1 | `feat(store): git_sources CRUD with encrypted token_ref` |
| 3 | D2 | `feat(net): url_guard SSRF allowlist (reject private/loopback/link-local)` |
| 4 | D3 | `feat(ingest): GitConnector — normalize/clone/glob/skip-binary via GitCloner trait` |
| 5 | D4 | `feat(ingest): git incremental sync — commit SHA cursor + force-push fallback` |
| 6 | D4 | `feat(server): ingest_git shared bind/sync drive loop (mirrors ingest_rss)` |
| 7 | D5 | `feat(server): bind-git / sync-git / git-token routes + mount` |
| 8 | D5 | `feat(ui): Git import modal in RemoteView + bindGit + i18n zh/en` |
| 9 | D6 | `test(ingest): GitConnector bare-repo fixtures + route subprocess (6-class)` |
| 10 | D6 | `docs: TESTING git corpus pins + RELEASE git-connector notes` |

> 个人/私有仓：测试通过后 develop 直 commit（per 项目 git push 权限）。涉 ingest pipeline 触点的 commit 走 §5.2 两轮 code review。

---

## 5. 风险登记（spec §11 继承 + 实施新增）

### 继承自 spec §11
SSRF（🔴）/ token 泄露（🔴）/ path traversal（🟡）/ 超大仓（🟡）/ git CLI 跨平台缺失（🟡）/ 游标漂移 force-push（🟡）/ 二进制误入（🟢）/ TempDir 残留（🟢）/ 并发死锁（🟢）— 缓解见 spec。

### 实施期新增

| 风险 | 等级 | 缓解 |
|------|------|------|
| **git2 crate vs shell git** | 🟡 中 | **决策：用 git CLI 子进程（`std::process::Command`）非 `git2`/libgit2**。理由：(a) libgit2 引入 C 依赖 + 跨平台编译复杂（per 跨平台规范 usearch/rusqlite 已是 C 负担）；(b) 子进程隔离崩溃面；(c) shallow/sparse/diff 用 CLI flag 直接表达。代价：依赖系统 git → 探测缺失给 `git-cli-missing` 友好提示 + 安装包文档标注。`GitCloner` trait 隔离，未来可换 git2 实现不动 connector |
| **大仓 clone 内存/磁盘峰值** | 🟡 中 | shallow `--depth 1` + sparse subdir 限子树；`max_total_bytes` clone 后估算超限即 `Err`；逐文件读+`sink` 流式不全物化（连 `DocumentSink` 回调约定）；`TempDir` RAII 清理 + 会话末 `df` 自查（全局磁盘铁律） |
| **私有仓 token 存储**（全局 §1.4） | 🔴 高 | token **不进 body 持久化 / 不进 DB 明文 / 不进日志 / 不回显**；DB 仅存 `dek` 加密的 `token_ref` 键名；git CLI 调用仅在进程内存拼 `https://<token>@host`，子进程 env 传递不落盘；错误 msg 脱敏（不含 token 子串） |
| **subprocess env 不传**（历史踩坑 #146 agent_runner） | 🟡 中 | git 子进程显式构 env（`GIT_TERMINAL_PROMPT=0` 禁交互、credential 走内存 URL）；D6 subprocess 集成测真起子进程验证，非 mock |
| **i18n zh/en key 漂移**（项目 i18n 铁律） | 🟡 中 | D5 完成后跑项目 i18n 守卫 grep（`diff zh/en key 集合` + 硬编码中文扫描）必 0 输出才提交 |

---

## 6. GA 验收清单（可勾选 — D6 逐条本机真验）

- [ ] 无效 URL（非 http(s) / 解析失败）→ `bind-git` 返回 400 `invalid-git-url`
- [ ] SSRF 拒内网：`http://127.0.0.1` / `http://169.254.169.254` / `http://[::1]` / `http://192.168.x.x` 全部 400 `git-url-not-allowed`
- [ ] DNS rebinding 域名（解析到内网 IP）→ 拒绝
- [ ] 私有仓鉴权：mock 401 → `git-auth-failed` 502；正确 token（env）→ 成功导入；token 不出现在响应/日志/DB 明文
- [ ] 404 仓 → `git-repo-not-found`；不存在 ref → `git-ref-not-found`
- [ ] 增量：二次 `sync-git` 仅 ingest 变更文件（A/M 入库、D 删 item），未变文件 skip；游标只在成功后推进
- [ ] force-push 上游 → diff 失败 fallback 全量重扫，不重复嵌入（content_hash 短路）
- [ ] 二进制文件（图片/NUL）跳过、不喂 sink；LFS 指针跳过
- [ ] 超 `max_file_bytes` 跳过；超 `max_files` 截断 + `truncated` 警告；超 `max_total_bytes` → `git-repo-too-large` 拒绝
- [ ] path traversal：仓内 `../` 路径名不逃出 vault
- [ ] **真仓**：`rust-lang/book` glob `**/*.md` + sparse `src/` 导入成功，标题/内容正确，BM25 + 向量可检索
- [ ] **真仓 i18n**：`CyC2018/CS-Notes` 中文 `.md` 入库 + tantivy-jieba 分词可搜，图片二进制跳过
- [ ] 导入路径**零 LLM 调用**（成本契约 §8）；UI 按钮标注 `~本地 · 网络克隆 + 本地嵌入`
- [ ] embedding provider 不可用时仍入库可 BM25 搜到（embed 入队列不阻塞）
- [ ] git CLI 缺失环境 → `git-cli-missing` 友好提示，不 panic
- [ ] `TempDir` clone 目录用后清理（成功 + 失败路径都清）；会话末 `df` 无残留
- [ ] i18n 守卫 grep 0 输出（zh/en key 齐 + 无硬编码中文）
- [ ] `cargo test --workspace` 全过 + `cargo clippy -- -D warnings` 干净；deterministic pass rate = 1.00

---

## 7. 测试策略（6 类下限 — per 全局 §6.1 + `docs/TESTING.md`）

| 类型 | case | 工具/位置 |
|------|------|-----------|
| **happy** | `rust-lang/book`（pin commit）clone+glob+入库；sparse `src/` 只拉子树 | bare fixture（CI）+ 真仓（手动/nightly） |
| **edge** | 空仓 / 全二进制仓 / 单文件仓 / 超长路径 / 符号链接 / `subdir` 不存在 | `tests/fixtures/git/` bare repo |
| **error** | 无效 URL / 404 / ref 不存在 / 鉴权失败(mock 401) / 网络超时(mock) / git CLI 缺失 | mock `GitCloner` + route subprocess |
| **adversarial** | SSRF 全表（127/169.254.169.254/[::1]/192.168/rebinding）拒绝；path traversal `../` | `url_guard` 单测 + connector 测 |
| **多源/并发** | 多 git 源并存 / 同仓二次 bind 去重 / 增量与全量并发 lock ordering | 集成测 |
| **资源耗尽** | 超 max_files 截断 / 超 max_file_bytes 跳过 / 超 max_total_bytes 拒绝 / 磁盘满 TempDir graceful | fixture + 限额注入 |
| **国际化** | `CS-Notes` 中文入库 + jieba 分词；UI zh/en key 齐 | 真仓 + i18n 守卫 |
| **降级** | embedding 不可用 → BM25 仍可搜（embed 入队列不阻塞） | mock embedding provider |

- 单元：`ingest/git.rs` + `net/url_guard.rs` 内 `#[cfg(test)]`；proptest ≥3（URL 归一幂等 / glob 匹配 / marker 稳定性）。
- 集成：`attune-core/tests/git_connector.rs`（bare fixture，**无网络**，CI 可跑）。
- E2E：`attune-server/tests/git_route_subprocess.rs`（真起子进程验 env 传递，per #146 教训）。
- 真平台仓（`rust-lang/book` / `CS-Notes`）固定 commit SHA，仅手动 / nightly soak（CI 默认不依赖网络）。
- 回归 fixture：每修一 bug 加 1 入 fixture 永久（per Agent 验证铁律）。

---

## 8. OSS 边界确认

✅ **GitConnector 全部落 OSS attune（`rust/` core + server + ui）**，零行业绑定 — 对任何个人通用用户（开发者/学习者/律师/医生/学者）同等有价值，符合 `oss-pro-strategy.md` v2 §4.3「对任何领域的个人通用用户都有价值」判据。
❌ **不触 attune-pro / attune-enterprise**：不调其 API、不复用其代码、`corpus_domain` 字段仅作 F-Pro 透传缺省 `general`（OSS 一般不填），不引入任何行业 prompt/schema。

---

## 9. 评审待拍板项（plan → 实施前）

1. **git CLI 子进程 vs git2 crate** — 本 plan 决策子进程（§5），评审确认。
2. **`net/url_guard.rs` 新建**（现无可复用件）— 评审确认新建 + 默认 host allowlist 集合。
3. **`SCHEMA_VERSION` 保持 1**（纯追加表）vs bump — 本 plan 主张不 bump，评审确认。
4. **token 存储**：`dek` 加密 `token_ref` 键名进 `git_sources.token_ref_enc`（对齐 webdav password 加密）vs 纯 env — 评审确认与现有 secret 管理对齐。
5. 默认 glob 集合 + 三限额默认值（5000 / 5MiB / 500MiB）。
