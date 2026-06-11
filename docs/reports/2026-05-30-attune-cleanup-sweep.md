# attune 主仓 release-clean 冗余清理 sweep — 2026-05-30

worktree 隔离（基于 origin/develop d121cba）。每类清理后真跑 `cargo test -p attune-core --lib` 验证（不信自报）。保守第一：拿不准是否冗余一律保留。

## 测试基线（真跑，非自报）

| 阶段 | 命令 | 结果 |
|------|------|------|
| 清理前基线 | `cargo test -p attune-core --lib` | **1499 passed; 0 failed; 1 ignored** |
| 核心 src 注释清理后 | 同上 | 1499 passed; 0 failed |
| 全量注释 sweep 后 | 同上 | **1499 passed; 0 failed; 1 ignored** |
| clippy lib+bins | `cargo clippy -p attune-core -p attune-server -p attune-cli --lib --bins -- -D warnings` | **0 warnings（干净）** |

测试数清理前后一致：**1499 / 0 failed**。零回归。

## 1. 冗余注释清理（§4.1）— 唯一执行的清理类别

**只做了一件事：剥离过程性标签前缀，保留 WHY 正文。** 38 文件，+151/-160 行，全部为注释/字符串编辑，**零逻辑行改动**（git diff 验证：所有变更行均为 `//` `///` `//!` `--` 注释或错误消息/assert 字符串；`SCORE_CUTOFF` 值仍为 0.001 未改，`use sha2::Sha256` 行体未改）。

剥离的过程性标签类型（这些是 PR/commit 内容，污染代码 diff）：
- `R<N> [P/S/F]<N> fix:` / `R<N> E2E fix (P0):` / `R<N> 补测:` / `R<N> 验收:`（review round 编号）
- `per reviewer <ID>:` / `per R<NN> P<N>:`（reviewer 轮次）
- `Round B/D E2E fix:`（E2E round 标签）
- `v0.7 sprint` / `v0.7 next sprint`（sprint 标签，module doc + schema 注释）
- `W3 batch A` / `W4-005` / `FIX-9` / `FEAT-2`（批次/feature 编号）
- 错误消息 + assert 消息里的 `(R6 P1-4 fix: ...)` → 收敛为纯语义（如 `(typo guard)`、`ref_id too long (max 128)`）

**保留的 WHY 正文**（剥前缀后内容全留）：lock ordering、并发 race 防护、cache 失效原因、事务原子性、加密落盘承诺、defense-in-depth bound、UTF-8 char boundary、graceful degradation 等。

涉及文件（按 crate）：
- attune-core src: items / signals / queue / store/queue / feedback / chunker / chat / search / lib / browse_signals / store/mod / state_migration / annotations / chunk_breadcrumbs / auto_bookmarks / reader / skill_eval / entity_graph / report / web_search_browser
- attune-core tests: memory_moat_integration / agent_gate_orchestrator / rag_w2 / rag_w3 / migration_roundtrip / model_boundary_audit / perf_reindex_bench
- attune-server: lib / middleware / state / routes/{items,upload,chat,search,annotations,browse_signals,errors}
- attune-cli: main

## 2. 死代码/未接入 — 审计后零删除（保守保留）

- `#[allow(dead_code)]` / `#[allow(unused_imports)]` ~18 处：均为**有意抑制**（如 attune-accounts `license/activated_at 待 endpoint 实装`、index.rs/mcp_client.rs 字段、annotations/signals 的条件 import）。clippy `-D warnings` 已干净 → 无真正未用代码。**保留。**
- 注释掉的代码块扫描：`resource_governor/mod.rs:6` 的 `//   let g = ...` 是 module doc 的 **API 用法示例**（合法文档，非 dead code）。其余 grep 命中均为 WHY 注释续行，无真正 commented-out 死代码。**零删除。**

## 3. 冗余文档（§3.2 白名单）— 审计后零改动（结构已合规）

- docs/ 顶层 22 个 .md：均为单主题 feature doc（DEPLOY/INSTALL/PRIVACY/SECURITY/TESTING/UPGRADING/VERSIONING + wizard-flow/updater/mcp-integration 等），符合白名单 `<feature-area>.md`。
- `find docs/ -name "*.zh.md"` 除 README 外 **0 个**（无双语漂移）。
- 无 `*-tasks.md` / `*-todo.md` / `v*-release-notes.md` 违规命名。
- `docs/reports/` 29 份 one-off 报告：§1.1.7 明确 `docs/reports/` 是 sprint 产物**合法归档位**，非违规。**保留。**
- `docs/superpowers/plans/` 2 份（2026-05-28 kb-bench / web-plugin）：无法确认对应 feature 是否已 ship-merge；删 active plan 属破坏性 → 按"拿不准则保留"**保留。**

## 保留决策汇总（为何留）

| 项 | 决策 | 理由 |
|----|------|------|
| `阶段 0/1/2:` 注释（ingest_email/rss/webdav）| 保留 | 是**运行时执行阶段**（锁外 IO → 持锁写 DB 的并发 invariant），非 dev 过程标签 |
| `#[allow(dead_code)]` 全部 | 保留 | 有意抑制 + clippy 干净；删除会破坏编译或丢失意图 |
| docs/reports/ + plans/ + specs/ | 保留 | sanctioned 位置 / 无法确认 stale / 本 session 报告 |
| 多行 WHY 注释正文 | 保留 | 剥前缀后全留 — lock ordering / 安全 bound / cache 失效原因等非显然约束 |

## commit

- branch: `cleanup-sweep`（worktree，基于 origin/develop d121cba）
- merge → develop + push（HTTPS）后 SHA 见 commit log
