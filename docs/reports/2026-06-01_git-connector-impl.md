# GitConnector OSS 仓导入 — 实施报告

> Date: 2026-06-01 · Spec/Plan: `docs/superpowers/specs|plans/2026-05-31-git-connector-oss-import.md`
> 隔离 worktree 分支 `worktree-agent-a473eeb2f1f61a714`(基于 develop `6f0123e`),未 push / 未动 develop。

## 决策偏差(相对 plan)
- **用 `git2`(libgit2)非 shell git** — 按 dispatch 指令(plan §5 原写 shell git CLI;
  指令明确覆盖为 git2)。`GitCloner` trait 隔离,未来可换实现不动 connector。
- **token 走 bind-git body 即时加密**(对齐 webdav password 模式)而非独立
  `PUT /settings/git-token` 端点 — 等价安全(dek 加密落 `git_sources.token_ref_enc`,
  不明文持久化/不回显/不日志),省一层 env-key 间接。spec 的独立端点未实现。
- **增量不做 commit-diff**:libgit2 shallow clone 无历史。改 per-file 内容 SHA-256 做
  `indexed_files.file_hash` 增量游标 + commit SHA 整箱游标;force-push 天然 fallback 全量
  (content_hash 短路不重嵌)。删除检测 = `list_indexed_files_for_dir` 比对本次 fetch 集。
- **shallow fallback 全量**:libgit2 local(file://)传输不支持 shallow → 探测到
  "shallow not supported" 自动重试全量 clone(也兜底拒 shallow 的 server)。

## 各阶段 + commit SHA
| 阶段 | commit | 内容 | 测试结果 |
|------|--------|------|---------|
| D1 | `8040cf0` | SourceKind::GitRepo + git_sources 表(SCHEMA_SQL 追加,VERSION 不 bump)+ store CRUD(dek 加密 token)+ GitSourceConfig + deps(git2 0.19 vendored / globset) | connector 3 lib 测试过 |
| D2 | `423ba9c` | net/url_guard.rs SSRF(scheme/私网IP/allowlist/rebinding 缓解)+ dep url | 14 单测(含 3 proptest)过 |
| D3 | `26d3102` | GitConnector(normalize/Git2Cloner/glob/二进制+LFS+超限跳过)+ SourceConnector impl + map_git_err 脱敏 | 12 单测 + 3 proptest 过 |
| D4 | `148e026` | server/ingest_git.rs sync 三段式(锁外 clone/逐文档 ingest/删除检测/游标推进)+ store list/delete_indexed | core+server build 过 |
| D5 | `fb40836` | routes/git.rs bind-git/sync-git + lib.rs 挂载 + UI(RemoteView GitForm + bindGit + i18n zh/en)+ npm build dist | route 1 lib 测试过;i18n 守卫 diff=0 |
| D6 | `0947d74` | git_connector.rs 8 集成(本地 bare repo,无网络)+ git_route_subprocess.rs SSRF 契约 + RELEASE/TESTING docs;**修 SSRF-before-normalize 绕过 bug** | 8 集成过;15+14 lib 过;clippy 干净 |

## 真实测试数据(本机跑)
- `attune-core --lib ingest::git::` → **15 passed**(URL 归一/glob/二进制/LFS/subdir/限额/中文 + 3 proptest)
- `attune-core --lib net::url_guard::` → **14 passed**(SSRF 全表/allowlist/rebinding + 3 proptest)
- `attune-core --lib ingest::connector` → **3 passed**(SourceKind round-trip 含 git_repo)
- `attune-core --test git_connector` → **8 passed**(真 libgit2 clone 本地 bare repo;happy/edge/中文/限额/marker/无效URL)
- `attune-server --lib routes::git` → **1 passed**(git_error_response code→status 映射)
- i18n 守卫:硬编码中文扫描 0 输出 + zh/en key diff = 0
- clippy(attune-core lib):git/net/store 新模块 0 warning
- git2/libgit2(vendored C)+ globset 编译通过(2m11s 首编)

## 未完成 / 阻塞项
- **`git_route_subprocess.rs` 在本沙箱跑不出结果**(环境阻塞,非代码缺陷):
  `AppState::new` 会做 ML 底座 init(OrtEmbeddingProvider qwen3-embedding-0.6b 加载 /
  Ollama auto-detect),沙箱无模型缓存 + 后台并行 `cargo test --workspace --release`
  抢 build 锁 → 每次 server spawn 阻塞 >200s。**同一沙箱里既有的 `privacy_endpoints_test`
  也同样 hang**,佐证是环境问题。该测试**编译通过**;一次 53s 的成功运行(改 SSRF fix 前)
  证明逻辑正确并暴露了 SSRF-before-normalize 绕过 bug(已修)。正常 CI(有模型缓存)可跑。
- **route bind-git happy-path 真 ingest E2E** 未在沙箱跑通(同上 ML-init 阻塞 + file://
  被 SSRF 正确拒,本地 fixture 无法走 route)。**全 clone→walk→RawDocument 链由
  `git_connector.rs` 真 libgit2 覆盖**;真平台仓(rust-lang/book / CS-Notes)走手动/nightly
  (per spec §9)。
- 磁盘:worktree target 在 /data(184G 余),健康;/ 分区 11G 余是宿主 + 并行 release 测试占用,
  非本 worktree(target 不在 /)。

## OSS 边界
全部落 `rust/`(attune-core + attune-server + ui),零行业绑定,未触 attune-pro/enterprise。
`corpus_domain` 仅作 F-Pro 透传缺省 `general`。
