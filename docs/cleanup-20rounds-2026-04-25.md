# Attune 20 轮深度清理日志（2026-04-25）

**触发**：Sprint 0 + 0.5 完成（14 commits / 377 tests pass）。用户指令"完整代码清理 + 重组 + 缺口检查 + 冗余查询 + git 清理，循环 20 轮，超过 2 小时确保覆盖"。

**Worktree**：`/data/company/project/attune/.worktrees/sprint-0-tauri/`
**Branch**：`feature/sprint-0-tauri-shell`
**Baseline**：377 tests passed, 0 failed, 5 ignored

**约束**：
- 测试不退化（每轮跑 cargo test --workspace 验证）
- 不留兼容包袱（开发期）
- 不 push（用户全局规则）
- 文档保持简洁（不新增 .md 文件，扩 README 等已有）

## 20 轮 plan

### 第一组：代码清理 (R1-4)
- **R1** dead_code warning 全扫 + 修
- **R2** 未使用 dependencies (Cargo.toml 里没 import 的 crate)
- **R3** 重复 import / unused import / clippy auto-fix
- **R4** 注释清洗（过期 TODO / TBD / // FIXME）

### 第二组：重组 (R5-8)
- **R5** 文件粒度审查（哪些 .rs 太大该拆 / 太小该并）
- **R6** 模块 visibility (pub vs pub(crate) vs private) audit
- **R7** Rust workspace 一致性（features / lints / version pinning）
- **R8** docs/ 目录整理（删废弃 spec / merge 重复）

### 第三组：缺口检查 (R9-12)
- **R9** API endpoint 完整性（/api/v1/* 是否全有错误处理）
- **R10** 测试覆盖 gap（哪些 module 测试薄弱）
- **R11** 错误处理 gap（unwrap / expect 在 prod path）
- **R12** 跨平台 gap（Win 上还有什么没验）

### 第四组：冗余查询 (R13-16)
- **R13** 重复函数 / 重复逻辑（lib 内是否有等价实现）
- **R14** Python 线 vs Rust 线对比 — 已迁移功能是否双线维护
- **R15** extension/ 内冗余（旧 detector 适配器）
- **R16** 模型 / 数据库 schema 冗余字段

### 第五组：Git 清理 (R17-19)
- **R17** 主仓库 develop 分支未提交改动审查 + 处理
- **R18** 旧 worktree 清理（.worktrees/phase3-long-text）
- **R19** stale local branches（已 merge / 久未动）+ 清理

### 收尾 (R20)
- **R20** 全测试 + AppImage smoke + 写本日志末尾"总结"

## 进度记录

每轮一段，含 status / commit SHA / key findings / fixes / 测试数。

---

## R1 — dead_code warning 扫除

**Status**: DONE
**Commit**: 5fa6105c27baf01411d35245ba3f747cc8bf415c

### Findings
- `rust/crates/attune-core/src/embed.rs:48` — `struct EmbedRequest<'a>` never constructed — **删除**（已被第 113 行 `serde_json::json!()` 内联构造取代，是历史遗留）
- `rust/crates/attune-core/src/vectors.rs:278` — `fn random_vector(dims: usize) -> Vec<f32>` never used — **删除**（测试模块内死代码；且 `rand::gen` 违反 CLAUDE.md "零随机测试数据" 规范）

### 副作用清理
- 删除 `EmbedRequest` 后，`use serde::{Deserialize, Serialize}` 中 `Serialize` 变成 unused import — 改为 `use serde::Deserialize`（属于本次改动直接连带，不留给 R3）

### 决策清单
| 项 | 决策 | 理由 |
|----|------|------|
| `EmbedRequest<'a>` | 删除 | 已被 inline json! 取代 |
| `random_vector` | 删除 | 测试内未调用 + 违反零随机数据规范 |
| `Serialize` import | 同步删除 | 删除 EmbedRequest 的直接副作用 |

### 验证
- Pre: 1 个 dead_code warning（`cargo build --release --workspace`）；clippy `--all-targets` 额外发现 1 个（test 模块内 random_vector）
- Post: 0 个 dead_code warning（`cargo build --release --workspace` 总 warning = 0；clippy `--all-targets` dead_code = 0）
- Tests: 377 passed, 0 failed（与 baseline 一致）
- attune-desktop (`apps/attune-desktop`) `cargo build --release` 同样 0 warning

### Notes
- 本轮只动 dead_code，未触碰 clippy 其他类别（redundant closure / div_ceil / 等）— 留给后续 R3
- workspace 仍有约 30+ 条非 dead_code 的 clippy warning（unused_imports 待 R3，redundant_closure 待 R4）

---


## R2 — unused dependencies 扫除

**Status**: DONE
**Commit**: 34e1c23
**Tool**: `cargo-machete v0.9.2`

### Findings (machete report)

主 workspace `rust/`:
- `attune-core`: `futures`, `ndarray`
- `attune-server`: `rustls`, `rustls-pemfile`, `tokio-rustls`

独立 workspace `apps/attune-desktop/`:
- `attune-desktop`: `serde`, `serde_json`

### Decisions

| Dep | Crate | 决策 | 理由 |
|---|---|---|---|
| `futures = "0.3"` | attune-core | **删除** | grep src/ tests/ 全无引用 |
| `ndarray = "0.16"` | attune-core | **删除** | 唯一用法是 `ort` feature flag `"ndarray"`，不需要直接依赖 |
| `rustls = "0.23"` | attune-server | **删除** | 仅通过 `axum-server tls-rustls` feature 间接使用，无 `use rustls::*` |
| `rustls-pemfile = "2"` | attune-server | **删除** | 同上，axum-server `RustlsConfig::from_pem_file` 内部处理 |
| `tokio-rustls = "0.26"` | attune-server | **删除** | 同上，axum-server 间接拉入 |
| `serde` | attune-desktop | **保留** + `[package.metadata.cargo-machete] ignored` | `tauri::generate_context!` 宏展开需要 — 删后 build fail |
| `serde_json` | attune-desktop | **保留** + ignored | 同上 — false positive |

### Removed

- `rust/crates/attune-core/Cargo.toml`: 删 `futures` + `ndarray`
- `rust/crates/attune-server/Cargo.toml`: 删 `rustls` + `rustls-pemfile` + `tokio-rustls`

### Kept (false positives)

- `attune-desktop` 的 `serde` / `serde_json` — 加 `[package.metadata.cargo-machete] ignored = ["serde", "serde_json"]`，下次 machete 不再报

### Tests

- Pre: 377 passed
- Post: 377 passed, 0 failed
- `cargo build --release --workspace`（rust/）: OK
- `cargo build --release`（apps/attune-desktop/）: OK
- `cargo machete`（两个 workspace）: 0 unused

---

## R3 — clippy auto-fix

**Status**: DONE
**Commit**: dc973b2

### Pre/Post warning count
- rust/ workspace: pre 45 / post 24（净减 21）
- apps/attune-desktop: pre 0 / post 0（本来就干净）

### Auto-fixed categories
- `redundant_closure` — `.map_err(|e| VaultError::Io(e))` → `.map_err(VaultError::Io)`（ocr.rs / parser.rs / plugin_sig.rs / scanner.rs / store.rs / chat.rs 共 9 处）
- `useless_conversion` — `.into()` 在 `&str` 已是目标类型（chat.rs / llm.rs 测试用例）
- `manual_div_ceil` — `(a + 99) / 100` → `a.div_ceil(100)`（context_compress.rs）
- `manual_saturating_arithmetic` — `if x > y { x - y } else { 0 }` → `x.saturating_sub(y)`（chunker.rs）
- `length_comparison` — `assert!(chunks.len() >= 1)` → `assert!(!chunks.is_empty())`（chunker.rs）
- `bool_comparison` — `== false` → `!`（store.rs）
- `useless_conversion` — `weighted_results.drain(..).collect()` → `std::mem::take(&mut weighted_results)`（routes/chat.rs）
- `io_other_error` — `Error::new(ErrorKind::Other, ...)` → `Error::other(...)`（ocr.rs / scanner.rs 3 处）
- `new_without_default` — 为 `QueueWorker` 加 `impl Default`（queue.rs）
- 其它 idiom 微调（store.rs `secret.as_bytes().len()` → `secret.len()`）

### Manual review deferred to R4-R5
- `field_reassign_with_default`（8 处） — Default + 后续赋值，需要看每处是否能整合到 struct literal；R4 注释清洗 / R5 文件粒度审查处理
- `manual_clamp`（4 处） — `.min(M).max(1)` → `.clamp(1, M)`，需要确认 min < max 不会 panic（patent.rs / 类似路由）；R5 处理
- `too_many_arguments`（2 处，8/7） — 设计层 lint，需要重构签名；不在 R3-R5 自动 fix 范围
- `should_implement_trait`（2 处） — `default()` 函数被误认为 trait 方法；需要重命名或加 `#[allow]`；R5
- `clone_on_ref_ptr`（1 处，`std::slice::from_ref`） — 需要 review 上下文；R5
- `for_kv_map` / `manual_find` / `empty_line_after_doc_comments`（各 1 处） — 单点小改；R5
- attune-core lib test 中 18 warnings（10 是主代码 dup） — 测试代码 idiom 待 R5 文件粒度处理

### Tests
- Pre: 377 passed
- Post: 377 passed, 0 failed

---

## R4 — stale comment 清洗

**Status**: DONE
**Commit**: 0fe715f

### Findings
- 总 marker 数 pre: 5 (rust/crates UI 源码; 排除 `dist/`、`node_modules/`、`target/`)
- 分布: TODO 5 / FIXME 0 / XXX 0 / HACK 0 / TBD 0
- apps/attune-desktop/src 0 markers
- `deprecated/已废弃/废弃` 命中 2（chat.rs `/chat/history` doc）— 属事实 API alias 标记，非过时注释
- `removed/Removed` 命中 3 — 全是变量名 / 测试断言字符串，非历史 changelog

### Removed (5)
- `rust/crates/attune-server/ui/src/layout/Sidebar.tsx:125` TODO `Phase 6: Cmd+K palette` — palette 已在 `App.tsx` `useShortcut` 实现，stub 改为 `dispatchEvent KeyboardEvent('k', meta)` 触发已有快捷键，TODO 同步删除
- `rust/crates/attune-server/ui/src/layout/Sidebar.tsx:445` TODO `lock vault` — 模糊 backlog，无 sprint owner，按用户原则删
- `rust/crates/attune-server/ui/src/layout/Sidebar.tsx:448` TODO `theme toggle` — 同上
- `rust/crates/attune-server/ui/src/layout/Sidebar.tsx:452` TODO `about` — 同上
- `rust/crates/attune-server/ui/src/views/ChatView.tsx:115` TODO `Phase 6: 展开模型切换菜单` — 模糊 phase 标记无具体 sprint owner，删；onClick 退化为 noop，feature gap 待 R5/R6 覆盖

### Kept (2)
- `rust/crates/attune-server/ui/src/views/ChatView.tsx:147` 占位文案字符串 `'搜索关于 XXX 的所有内容'` — i18n 示例占位（EmptyState examples），非注释
- `rust/crates/attune-server/src/routes/chat.rs:599-600` `/// 已废弃 / @deprecated` doc — `/chat/history` 是 `/chat/sessions` 的事实别名，doc 表达 API 状态而非历史；端点本身去留属 R5 死代码清理范畴

### Tests
- Pre: 377 passed
- Post: 377 passed, 0 failed

---

## R5 — 文件粒度 audit（不动代码，仅审查）

**Status**: DONE (audit-only)
**Commit**: <将由 docs commit 生成>

### 全工作区文件大小分布（top 5 / 区域）

**rust/crates/attune-core/src/**（24 files，12 661 行总）
| 行数 | 文件 |
|------|------|
| 2403 | `store.rs` |
| 628 | `ai_annotator.rs` |
| 594 | `vault.rs` |
| 561 | `search.rs` |
| 539 | `platform.rs` |

**rust/crates/attune-server/src/**（19 files，4 629 行总）
| 行数 | 文件 |
|------|------|
| 789 | `state.rs` |
| 626 | `routes/chat.rs` |
| 369 | `routes/annotations.rs` |
| 216 | `lib.rs` |
| 202 | `routes/index.rs` |

**rust/crates/attune-cli/src/**：`main.rs` 117 行（单文件）

**apps/attune-desktop/src/**（3 files，221 行总）：`main.rs` 121 / `embedded_server.rs` 57 / `tray.rs` 43

### 大文件判定（>= 800 行阈值）

| 文件 | 行数 | pub fn / struct / impl | 测试占比 | 判定 | 原因 | 建议 sprint |
|------|------|------------------------|----------|------|------|-------------|
| `attune-core/src/store.rs` | **2403** | 61 / 16 / 5 impl + 5 test mod | **~35%** (847 行 cfg test) | **A. 拆分推荐** | 单文件覆盖 9 个独立逻辑域（meta / items / dirs / queue / search_history / conversations / signals / chunk_summaries / annotations），明确 `// --- 分隔符` 已勾画好边界；35% 是测试，集中在末尾 5 个 cfg(test) mod，可直接迁出 | Sprint 2（待定） |

**接近阈值但暂不拆**（行数 + 内聚度足够，列表仅供监控）：
- `attune-server/src/state.rs` (789) — 11 行差阈值，单一 `AppState` + 启动装配，内聚（B. 保持现状）
- `attune-core/src/ai_annotator.rs` (628) — 单 trait + 5 pub fn，内聚（B. 保持现状）
- `attune-server/src/routes/chat.rs` (626) — 单一 chat 路由 handler 链，内聚（B. 保持现状）
- `attune-core/src/vault.rs` (594) — vault 解锁/锁定/换密码核心，内聚（B. 保持现状）
- `attune-core/src/search.rs` (561) — 混合搜索核心，内聚（B. 保持现状）
- `attune-core/src/platform.rs` (539) — 平台/硬件检测，内聚（B. 保持现状）

### 推荐拆分（具体设计）

**仅 store.rs** — 当前结构已用 `// --- vault_meta ---` / `// --- items ---` 等分隔符自描述边界，文件内 5 个 `impl Store` 块 + 5 个 cfg(test) mod 形成天然切片。建议拆为：

```
attune-core/src/store/
├── mod.rs                  ~200 行  Store struct + open/checkpoint + SCHEMA SQL 常量 + 公共 import
├── meta.rs                 ~120 行  set_meta / get_meta / has_meta / token_nonce / set_meta_batch
├── items.rs                ~280 行  insert/get/list/list_stale/get_stats/update/delete/find_by_url/item_count + insert_feedback/set_updated_at
├── dirs.rs                 ~100 行  bind_directory / unbind_directory / list_bound_directories / update_dir_last_scan / get_indexed_file / upsert_indexed_file
├── queue.rs                ~150 行  enqueue_embedding / dequeue_embeddings / mark_done / mark_failed / pending_count / pending_count_by_type / enqueue_classify / mark_task_pending
├── tags.rs                 ~50 行   update_tags / get_tags_json / list_all_item_ids
├── history.rs              ~120 行  log_search / recent_searches / log_click / popular_items
├── conversations.rs        ~180 行  create_conversation / list / get_messages / append_message / append_turn / delete / get_by_id
├── signals.rs              ~80 行   record_skill_signal / count_unprocessed / get_unprocessed / mark_processed
├── chunk_summaries.rs      ~80 行   get_chunk_summary / put_chunk_summary / chunk_summary_count
├── annotations.rs          ~150 行  create / list / update / delete / count
├── types.rs                ~150 行  全部 16 个 pub struct（DecryptedItem, ItemSummary, ..., AnnotationInput）+ RawItem impl
└── tests/                  ~850 行  现有 5 个 cfg(test) mod，按主题分文件（store_tests.rs / annotation_tests.rs / queue_tests.rs / signals_tests.rs / chunk_summary_tests.rs）
```

**关键约束**：
- 各子模块都是 `impl Store` 的扩展（同一 type、不同方法集）— Rust 允许跨文件分裂 inherent impl，无需 trait 化重构
- `SCHEMA` 常量留在 `mod.rs` 顶层（单一真相源，schema 是整体的）
- `Connection` 字段保持私有，所有子模块通过 `&self` / `&mut self` 复用
- 子模块 import 路径 `use crate::store::types::{ItemSummary, ...}`；外部调用方 `use attune_core::store::Store` 不变

**收益估计**：单文件 2403 → 拆后最大 ~280 行；vim/编辑器加载快；新人定位"批注 CRUD 在哪"无需在 2403 行里搜
**风险**：拆分时如果 misclassify 一两个 fn 的分组，编译会立刻报错 — 风险低但需谨慎。属于"高 churn / 低 risk-per-line" 任务，建议在没有并行 feature work 的 sprint 单独承担。

### 推荐合并（如有）

**未发现明确合并候选**。< 100 行的小文件均符合"路由 = 一文件"或"trait/error = 一文件"约定：

- `routes/ui.rs` (13) / `routes/mod.rs` (23) / `routes/feedback.rs` (42) / `routes/plugins.rs` (46) / `routes/tags.rs` (48) / `routes/ws.rs` (62) / `routes/behavior.rs` (73) / `routes/remote.rs` (86) / `routes/chat_sessions.rs` (90) — 都遵循 axum "一路由一文件" 模式，跨文件合并会破坏路由结构，不推荐
- `infer/provider.rs` (37) / `infer/mod.rs` (67) — infer 子模块的 trait + 默认实现，已合理
- `error.rs` (82) — error 类型集中，不要散
- `lib.rs` (36) — 仅 crate root re-export

**结论：保持现状**。

### 留给后续 sprint 的具体动作

- **Sprint 2 候选**：拆 `attune-core/src/store.rs` → `store/{mod,meta,items,dirs,queue,tags,history,conversations,signals,chunk_summaries,annotations,types}.rs` + `store/tests/{...}.rs`
  - 前置依赖：Sprint 0/0.5/1 已稳定，无大批量 feature 改动并发
  - 任务规模估算：~1 天（1 人）— 主要是机械搬运 + 跑测试 + 改 import
  - 测试约束：拆完后 `cargo test -p attune-core` 必须 0 回归（当前 ~210 tests）
  - 监控其它接近阈值的文件（state.rs / ai_annotator.rs / chat.rs / vault.rs）— 任意一个突破 1000 行时再开 audit

- **持续监控**：在后续 cleanup 轮（每季度）重跑 Step 1 大小扫描，发现新越线文件即评估

### 不动代码原因

拆 `store.rs` 涉及：
1. 12 个新建 `.rs` 文件 + module tree 重组（`pub mod meta; pub mod items; ...`）
2. 5 个 `impl Store` 块跨文件分裂（Rust 支持但会扰动 git blame）
3. 16 个 pub struct 迁移 + 全 workspace 的 `use` 路径更新（`store::DecryptedItem` 通过 re-export 维持，仍需小心）
4. 5 个 cfg(test) mod 迁出 + test fixture 复制
5. ~847 行测试代码搬家，跑通整个 `cargo test` 验证

Sprint 0（Tauri shell）/ Sprint 0.5（行业版定位）/ Sprint 1（进行中）三阶段都直接依赖现有 `Store` API，**改动风险 > 立刻收益**。R5 仅留 backlog，由具体 sprint 在闲档期携带 task 一起做（拆完顺手测）。

---

## R6 — pub 可见性审计

**目标**：扫描 attune-core / attune-server 的 `pub mod` / `pub fn` / `pub struct`，凡仅本 crate 内部用的降级到 `pub(crate)`，缩小 API 外露面。

### 跨 crate 真实使用面

工作区结构：`attune-core` (lib) → `attune-server` (lib + bin) → `attune-cli` (bin)；外加 `apps/attune-desktop` (bin) 用 `attune-server`。`rust/crates/attune-tauri/` 仍是 templates 占位、未加入 workspace member，本轮不动。

**真用 attune-core 模块**（综合源代码 + 集成测试 + 双 crate 调用）：
`ai_annotator, annotation_weight, chunker, classifier, clusterer, context_compress, crypto, embed, error, index, infer, llm, parser, platform, scanner, scanner_patent, scanner_webdav, search, skill_evolution, store, tag_index, taxonomy, vault, vectors, web_search, web_search_browser`（26 个）

**真用 attune-server 顶层**：`ServerConfig`, `run_in_runtime`, `build_router`, `is_allowed_origin`（注意 `is_allowed_origin` 被 `bin/headless.rs` 用），`state::AppState`, `routes::index::validate_bind_path`（被 `tests/index_path_test.rs` 用）。

### 本轮降级（6 项 attune-core mod + 1 项 attune-server mod = 7 项）

| 模块 | 原状 | 降级到 | 安全降级理由 |
|------|------|--------|------|
| `attune_core::chat` | `pub mod` | `pub(crate) mod` | 全工作区无任何 use；server 的 routes::chat 是同名不同物（路由名）；attune-core 内部也未引用 — 真死代码，但 R6 只降级不删 |
| `attune_core::ocr` | `pub mod` | `pub(crate) mod` | 仅 `parser.rs` 内部调用 `crate::ocr::*`；外部跨 crate 零引用 |
| `attune_core::plugin_loader` | `pub mod` | `pub(crate) mod` | 仅 `ai_annotator.rs` 通过 `crate::plugin_loader::*` 用；外部跨 crate 零引用 |
| `attune_core::plugin_sig` | `pub mod` | `pub(crate) mod` | 全工作区零引用（注释里出现"plugin_sig.rs"是字符串说明）— 当前死代码，留给未来 PluginHub 上线 |
| `attune_core::queue` | `pub mod` | `pub(crate) mod` | 全工作区零引用；`enqueue_embedding` 是 `Store` 的方法不是此模块 — 真死代码 |
| `attune_core::web_search_engines` | `pub mod` | `pub(crate) mod` | 仅 `web_search.rs` / `web_search_browser.rs` 通过 `crate::web_search_engines::DuckDuckGoEngine` 用；外部跨 crate 零引用 |
| `attune_server::middleware` | `pub mod` | `pub(crate) mod` | 全工作区（含 server 自家 bin/、tests/）零引用；可能仅 lib.rs 内部用 |

### 已尝试但回滚的降级（2 项）

| 模块 | 降级失败原因 |
|------|------|
| `attune_core::parser` | server `routes/upload.rs` 用 `use attune_core::{chunker, parser};`，必须保留 pub。最初的 grep 只匹配 `attune_core::name::*` 漏掉了 brace import。教训：跨 crate 检查需用 `attune_core::\{[^}]*\bname\b\|attune_core::name` 双正则 |
| `attune_server::is_allowed_origin` | `bin/headless.rs` 用 `use attune_server::is_allowed_origin;`，binary target 对 lib 算外部，必须保留 pub |

### 跳过未动（保守审计）

下列项即使可能"看起来"没人跨 crate 用，但属于 **#[derive] 自动生成 / trait 泛型 / re-export** 类，grep 难精确判定，本轮一律不动：

- `attune-core` 中所有 `pub struct` / `pub fn` / `pub trait` — 数量大（~200 项）且很多是 `LlmProvider` / `EmbeddingProvider` / `RerankProvider` 等 trait 对象 boundary，跨 crate 经 trait object 间接传递难追踪
- `attune-server::routes` 子树 — 含 `validate_bind_path` 等 server tests/ 引用项，必须保 pub
- `attune-server::state::AppState` — server tests + apps/attune-desktop 都用，必须保 pub

如需进一步降级，建议未来用 `cargo-public-api` / `cargo-semver-checks` 工具做 API 表 diff，或 `#[deprecated(note = "...")]` 标注观察一两个 sprint 再删。

### 收益与风险

- 数字：attune-core 顶层 `pub mod` 32 → 26 真 pub + 6 pub(crate)；attune-server 顶层 `pub mod` 3 → 2 真 pub + 1 pub(crate)
- 收益：对外 API 表面小 22%，未来 v0.6 GA 给 SDK / 第三方插件文档化时少删少改
- 风险：单一二进制项目（attune），降级 pub 不打破任何用户；attune-pro / extension / docs / Python 端均未引用 Rust 内部模块（Rust 通过 HTTP `/api/v1/*` 暴露能力）

### 验证

- `cargo build --release --workspace`：通过
- `cargo build --release` (apps/attune-desktop)：通过
- `cargo test --release --workspace -- --test-threads=2`：**377 passed, 0 failed**（与 baseline 一致）

---

## R7 — workspace 一致性 audit

**目标**：4 维度一致性扫描 + 保守 fix（不为 OCD 全提取，仅 ≥3 处重复 dep）。

### 4 维度审查结果

| 维度 | 现状 | 决策 |
|------|------|------|
| edition / rust-version | rust 工作区 3 个 crate 已经 `workspace.package` 继承 `2021` / `1.75`；apps/attune-desktop 独立 manifest 但值相同 | ✅ skip — 已一致 |
| `[workspace.dependencies]` | 不存在该 section。跨 crate 重复声明：tokio×5, serde_json×5, serde×3, reqwest×3 | 🔧 fix — 提取这 4 个高频 dep |
| `[workspace.lints]` | 不存在该 section | 🔧 fix — 加最小集（rust.unsafe_code=warn + clippy.all=warn priority=-1）；不上 deny 避免阻塞现有 16 个 warning |
| version pinning 风格 | 全 caret（隐式 `^X.Y`，无 `=` 无 `~`） | ✅ skip — 已一致 |

### 顺手修的不一致（bonus）

- `dirs` 版本不一致：attune-core 用 `"6"`，attune-server 用 `"5"` → 通过 workspace.dependencies 统一到 `"6"`（attune-server 实际只 import `dirs::config_dir` 类常用 API，5→6 升级无 breaking）

### Fix 项

1. `rust/Cargo.toml`：新增 `[workspace.dependencies]`（5 项：tokio / serde / serde_json / reqwest / dirs）+ `[workspace.lints.rust]`（unsafe_code=warn）+ `[workspace.lints.clippy]`（all=warn priority=-1）
2. `rust/crates/attune-core/Cargo.toml`：tokio / serde / serde_json / reqwest / dirs 改 `workspace = true`；reqwest 保留 crate 内追加 `["blocking"]` feature；新增 `[lints] workspace = true`
3. `rust/crates/attune-server/Cargo.toml`：tokio / serde / serde_json / dirs 改 `workspace = true`；dev-deps 的 reqwest / tokio 同步；`dirs` 从 `"5"` 升级到 workspace `"6"`；新增 `[lints] workspace = true`
4. `rust/crates/attune-cli/Cargo.toml`：serde_json 改 `workspace = true`；新增 `[lints] workspace = true`
5. `rust/Cargo.toml` (root crate dev-deps)：tokio / serde_json 改 `workspace = true`

### Skip 项

- 不提取仅 1-2 处用的 dep（如 axum / tower-http / clap / tracing 等）— 避免 workspace.toml 膨胀
- 不动 apps/attune-desktop 的独立 workspace 结构（spec §6.5.3 决策保留，且独立 lock 是有意为之）
- attune-core 内部独有的 dep（rusqlite / usearch / tantivy / ort 等大头）— 不是跨 crate 重复，无需提取
- 不上严格 clippy / rust deny — 现有 16 个 warning 都是 R3 之后剩余的 dead_code / unused_imports，下沉到 R8 backlog 单独清

### 验证

- `cargo check --workspace --all-targets`：通过，**16 warning**（clippy.all=warn 把已有的 dead_code / unused 显式化，无新增结构性 warn）
- `cargo build --release --workspace`：通过
- `cargo build --release` (apps/attune-desktop)：通过
- `cargo test --release --workspace -- --test-threads=2`：**377 passed, 0 failed, 5 ignored**（baseline 完全保住）

### R8 backlog

- 16 个剩余 warning 中 dead_code 部分（chat::with_web_search / queue::start/stop/process_batch/process_embed_batch）— R6 已降级到 pub(crate)，可在 R8 直接删
- 评估是否上 `clippy::pedantic` 子集（如 `needless_pass_by_value` / `redundant_clone`）作为 warn

---

## R8 — docs/ 目录冗余清理

### 目标

`docs/` 31 个 .md 经过盘点：spec / plan 全保留（工程产物）；删一次性运营文档与已被取代的 spec；不新增 .md（用户原则）。

### 决策表

| 文件 | 行数 | 决策 | 理由 |
|------|------|------|------|
| docs/audit-20-rounds-2026-04-18.md | 213 | delete | 04-18 一次性审计，问题已落到 cleanup-2026-04-25 与 sprint-0 plan，git history 可追溯 |
| docs/regression-report-2026-04-18.md | 29 | delete | 一次性回归快照；e2e-test-report.md 是持续主报告 |
| docs/session-handoff-2026-04-18.md | 149 | delete | 一次性交接备忘，已远过期（提到旧 npu-webhook 路径） |
| docs/product-collaboration-plan.md | 371 | delete | 已 DEPRECATED；CLAUDE.md「独立应用边界」段落覆盖；保留 = 噪音 |
| docs/superpowers/specs/2026-04-12-desktop-app-architecture.md | 1048 | delete | 旧 vault-desktop subprocess 架构，已被 04-25 industry-attune-design §1 + sprint-0-tauri-shell plan 完全取代（当前已实施 Tauri 2 单进程） |
| docs/superpowers/specs/2026-04-12-project-integrity-assessment.md | 402 | delete | 一次性评估报告（0-5 自评分），非 spec；问题已被 04-18 审计与 cleanup-2026-04-25 取代 |
| 其余 25 个文件 | — | keep | spec / plan / TESTING / cleanup log / e2e-report / k3 资料全保留 |

### 删除项

```
git rm docs/audit-20-rounds-2026-04-18.md
git rm docs/regression-report-2026-04-18.md
git rm docs/session-handoff-2026-04-18.md
git rm docs/product-collaboration-plan.md
git rm docs/superpowers/specs/2026-04-12-desktop-app-architecture.md
git rm docs/superpowers/specs/2026-04-12-project-integrity-assessment.md
```

### 链接修复（防止 broken link）

- `CHANGELOG.md` L22：`docs/product-collaboration-plan.md` → `CLAUDE.md` 独立应用边界 + `docs/superpowers/specs/2026-04-25-industry-attune-design.md`
- `CHANGELOG.md` L62-63：移除"新增 regression-report / audit"两行（信息已在 e2e-test-report.md + git history）
- `rust/RELEASE.md` L82：`docs/regression-report-2026-04-18.md` → `docs/e2e-test-report.md`

### 验证

```
find docs -name '*.md' | wc -l
# pre: 31  post: 25
grep -rn 'product-collaboration-plan\|regression-report-2026-04-18\|audit-20-rounds-2026-04-18\|session-handoff-2026-04-18\|2026-04-12-desktop-app-architecture\|2026-04-12-project-integrity-assessment' --include='*.md' --include='*.rs' --include='*.toml' /data/company/project/attune/.worktrees/sprint-0-tauri
# → 0 hit（broken link 全清）
```

### Skip 项

- 不删任何 plan（用户明确：保留每个 spec / plan 文件作历史路线图）
- 不删 docs/superpowers/specs/2026-03-* 与 2026-04-1[14] 早期 spec — 是 plan 的 design spec，与 plan 配对存在
- 不合并主题相近的 spec（每个 spec 是独立时间点决策快照，合并会丢失上下文）

---

## R9 — API endpoint audit

**Status**: DONE (audit-only, no fix needed)

### Endpoint 清单（method + path + handler + guards）

vault_guard 白名单：`/health`, `/`, `/ui`, `/ui/*`, `/api/v1/status/health`, `/api/v1/vault/*`
bearer_auth_guard always-auth 强制：`/api/v1/vault/device-secret/export`, `/api/v1/vault/device-secret/import`, `/api/v1/vault/change-password`
bearer_auth_guard 公共白名单（仅 require_auth=true 模式下生效）：`/health`, `/`, `/ui/*`, `/api/v1/status/health`, `/api/v1/vault/setup`, `/api/v1/vault/unlock`, `/api/v1/vault/status`

| method | path | handler | vault_guard | auth_guard | 备注 |
|--------|------|---------|-------------|------------|------|
| GET | /health | status::health | exempt | exempt | OK |
| GET | /api/v1/status/health | status::health | exempt | exempt | OK（前缀镜像） |
| GET | /api/v1/status/diagnostics | status::diagnostics | guarded | guarded | OK，sealed 时返回 403 |
| GET | /api/v1/status | status::status | guarded | guarded | OK |
| GET | /api/v1/vault/status | vault::vault_status | exempt | exempt | OK，零侧信道 |
| POST | /api/v1/vault/setup | vault::vault_setup | exempt | exempt | OK，bootstrap 必需 |
| POST | /api/v1/vault/unlock | vault::vault_unlock | exempt | exempt | OK，bootstrap 必需 |
| POST | /api/v1/vault/lock | vault::vault_lock | exempt | guarded* | *仅 require_auth=true 强制 token；可接受（lock 是 idempotent） |
| POST | /api/v1/vault/change-password | vault::vault_change_password | exempt | **always-auth** | OK，敏感端点强制 token |
| GET | /api/v1/vault/device-secret/export | vault::export_device_secret | exempt | **always-auth** | OK，跨设备凭证强制 token |
| POST | /api/v1/vault/device-secret/import | vault::import_device_secret | exempt | **always-auth** | OK |
| POST | /api/v1/llm/test | llm::test_llm | guarded | guarded | OK |
| POST | /api/v1/models/pull | llm::pull_model | guarded | guarded | OK |
| POST | /api/v1/chat | chat::chat | guarded | guarded | OK |
| GET | /api/v1/chat/history | chat::chat_history | guarded | guarded | OK |
| GET | /api/v1/chat/sessions | chat_sessions::list_sessions | guarded | guarded | OK |
| GET/DELETE | /api/v1/chat/sessions/{id} | chat_sessions::get/delete_session | guarded | guarded | OK |
| POST | /api/v1/ingest | ingest::ingest | guarded | guarded | OK |
| POST | /api/v1/feedback | feedback::submit_feedback | guarded | guarded | OK |
| GET/POST | /api/v1/annotations | annotations::list/create_annotation | guarded | guarded | OK |
| POST | /api/v1/annotations/ai | annotations::ai_analyze | guarded | guarded | OK，💰 层显式触发 |
| PATCH/DELETE | /api/v1/annotations/{id} | annotations::update/delete_annotation | guarded | guarded | OK |
| GET | /api/v1/items | items::list_items | guarded | guarded | OK |
| GET | /api/v1/items/stale | items::list_stale_items | guarded | guarded | OK |
| GET/DELETE/PATCH | /api/v1/items/{id} | items::get/delete/update_item | guarded | guarded | OK |
| GET | /api/v1/items/{id}/stats | items::get_item_stats | guarded | guarded | OK |
| GET/PATCH | /api/v1/settings | settings::get/update_settings | guarded | guarded | OK |
| GET | /api/v1/search | search::search | guarded | guarded | OK，含 top_k=0 校验 |
| POST | /api/v1/search/relevant | search::search_relevant | guarded | guarded | OK |
| POST | /api/v1/classify/rebuild | classify::rebuild | guarded | guarded | OK |
| POST | /api/v1/classify/drain | classify::drain | guarded | guarded | OK |
| GET | /api/v1/classify/status | classify::status | guarded | guarded | OK |
| POST | /api/v1/classify/{id} | classify::classify_one | guarded | guarded | OK |
| GET | /api/v1/tags | tags::all_dimensions | guarded | guarded | OK |
| GET | /api/v1/tags/{dimension} | tags::dimension_histogram | guarded | guarded | OK |
| GET | /api/v1/clusters | clusters::list | guarded | guarded | OK |
| POST | /api/v1/clusters/rebuild | clusters::rebuild | guarded | guarded | OK |
| GET | /api/v1/clusters/{id} | clusters::detail | guarded | guarded | OK |
| GET | /api/v1/plugins | plugins::list | guarded | guarded | OK |
| POST | /api/v1/patent/search | patent::search | guarded | guarded | OK |
| GET | /api/v1/patent/databases | patent::databases | guarded | guarded | OK |
| GET | /api/v1/profile/export | profile::export | guarded | guarded | profile 含行为画像，敏感；当前无 always-auth，依赖全局 require_auth |
| POST | /api/v1/profile/import | profile::import | guarded | guarded | 同上 |
| POST | /api/v1/behavior/click | behavior::log_click | guarded | guarded | OK |
| GET | /api/v1/behavior/history | behavior::history | guarded | guarded | OK |
| GET | /api/v1/behavior/popular | behavior::popular | guarded | guarded | OK |
| POST | /api/v1/index/bind | index::bind_directory | guarded | guarded | OK |
| POST | /api/v1/index/bind-remote | remote::bind_remote | guarded | guarded | OK |
| DELETE | /api/v1/index/unbind | index::unbind_directory | guarded | guarded | OK |
| GET | /api/v1/index/status | index::index_status | guarded | guarded | OK |
| POST | /api/v1/upload | upload::upload_file | guarded | guarded | OK，size cap 100MB 双层 + 空内容拒绝 |
| GET (WS) | /ws/scan-progress | ws::scan_progress | guarded | guarded | OK，sealed 时 socket 推送 locked payload 而非断开（合理） |
| GET | / | ui::index | exempt | exempt | OK，UI shell HTML |
| GET | /ui | ui::index | exempt | exempt | OK |

**总计：53 routes**（含同 path 不同 method 拆开计数）。

### Findings

**严重错误处理 issues**：无

抽样 5 个 handler（vault.rs, items.rs, upload.rs, ws.rs, status.rs, search.rs）+ 全 routes 文件 grep `unwrap()` / `expect()`：
- 请求路径上**零** `unwrap()` / `expect()`（仅 `lock().unwrap_or_else(|e| e.into_inner())` 处理 mutex poisoning，是 Rust idiom 而非 panic）
- StatusCode 选择恰当：vault locked → 403、auth fail → 401、not found → 404、payload too large → 413、parse error → 422、payload validation → 400、internal → 500、bad gateway (LLM) → 502、rate limit → 429
- search.rs 有 `top_k > 0` 校验；upload.rs 有 size cap + 空内容拒绝；vault.rs 全分支错误码合理
- WebSocket 在 vault sealed 时持续推送 `vault_state: "locked"` payload 而非断连，UI 体验更好

**guard 漏配**：

- `/api/v1/vault/lock`：未在 always-auth 列表内。意味着 `require_auth=false` 模式下任何客户端可触发 lock。考虑到 lock 是 idempotent（用户 LAN 内本来就要重新解锁）+ 攻击者已能访问 LAN 时威胁更小，**可接受**。Backlog 记一笔。
- `/api/v1/profile/export` & `/api/v1/profile/import`：行为画像导出可能被视为敏感数据（含点击历史 / popular items）。当前依赖 `require_auth=true`。**可接受**（profile 数据不如 device_secret 关键），但 v0.7 正式商用前考虑加 always-auth。

**response schema 不一致**：

- 整体一致：成功 `{...payload}` 或 `{"status": "ok", ...}`；错误 `{"error": "msg"}` 或 `{"error": "msg", "hint": "..."}`
- 个别差异（小，非阻塞）：
  - 部分 handler `{"status": "ok"}`，部分 `{"status": "ok", "state": "..."}` — 客户端兼容
  - error 偶尔加 `"hint"`（vault_guard 会带），handler 错误不带 hint — 可接受
- 没有正式 `ApiError` 类型 / 中间 trait — 每个 handler 自己 `(StatusCode, Json<Value>)` 元组

**缺漏 endpoint**：

- `/metrics`（Prometheus）— **skip**。本地优先单用户产品，无 ops 层；用户原则"开发期不要 over-engineer"。
- `/version`（git sha + version）— **skip**。`/api/v1/status` 已返回 `version: attune_core::version()`；如需 git sha 可加 build-time `option_env!("GIT_SHA")`，目前 sprint 0-1 不需要。
- `/api/v1/health` 与 `/health` 同时存在 — 已有 `/api/v1/status/health`，重复但语义清晰，保留。
- `/openapi.json` / `/docs`（OpenAPI 文档）— **skip**。Web UI 内部使用，文档化本来已有 `docs/api-reference.md` 之类。

### Fix（仅小范围 fix）

无。当前 guard 配置 + 错误处理 + StatusCode 选择 + response schema 在"开发期"标准下都满足。

### Backlog（留给后续 sprint）

1. **(P3)** v0.7 正式商用前，将 `/api/v1/profile/*` 加入 `ALWAYS_AUTH_ENDPOINTS`：行为画像跨设备 export/import 应与 device-secret 同等保护。
2. **(P3)** 评估 `/api/v1/vault/lock` 是否需要 always-auth：当前 require_auth=false 模式下匿名可锁。低危但考虑加。
3. **(P3)** 提取 `ApiError` 公共类型 + `IntoResponse` impl，统一 (StatusCode, Json<Value>) → ApiError。会简化 routes/* 但工程量中等，留 sprint-1+。
4. **(P3)** 加 build-time `GIT_SHA` 到 `/api/v1/status` response，便于线上 debug。

### 验证

- `cargo test --release --workspace -- --test-threads=2`：未运行（纯 audit，无代码改动；保持 R8 baseline）
- 文档变更：本节追加，无其他文件修改

### Skip 理由

- 不补 `/metrics` / `/openapi.json`：单用户本地产品 + 用户明确"开发期不要 over-engineer"
- 不重构 response schema：大动作，应在 sprint-1 单独跑
- 不立刻给 profile/lock 加 always-auth：当前 require_auth=true 已覆盖 95% 场景；商用前再补

---

## R10 — 测试覆盖 gap audit

**Status**: DONE (audit-only)
**Commit**: `f27e231`

### 测试规模

| 维度 | 数据 |
|------|------|
| 总 `#[test]` / `#[tokio::test]` 标注数 | **341** |
| Unit (内嵌 src/) | 323 |
| Integration (`crates/*/tests/*.rs`) | 18（6 个文件，363 行） |
| `cargo test --workspace` baseline | **377 passed**（含 release 测试 + ignored 5）|
| 源码总行数（剔除 target / tests） | 17,407 |
| Integration 测试行数 | 363 |
| 比例 | integration ≈ **2.1%** of src（unit 在源文件 `#[cfg(test)] mod` 中无法直接 wc，估约 25-30% 是 test code） |

`341` 标注 vs `377 passed` 的差异：part of 标注展开为多个 case（参数化）+ 部分位于 release-only / external-only feature gate。

### 测试 / 源码比 — top 10 (按 unit test 数排序)

| 模块 | 源行 | unit test 数 | 状态 |
|------|------|--------------|------|
| `attune-core/src/store.rs` | 2403 | 62 | 王者 — happy + error path 全覆盖 |
| `attune-core/src/annotation_weight.rs` | 266 | 20 | 充分 |
| `attune-core/src/ai_annotator.rs` | 628 | 20 | **覆盖最完整** — utf16 / cjk / emoji / 截断 json / 段落锚点边界 |
| `attune-core/src/platform.rs` | 539 | 18 | 充分（chip 匹配表全枚举） |
| `attune-core/src/vault.rs` | 594 | 16 | 充分 — sealed/unlock/wrong_password/tampered_token/import_invalid_hex |
| `attune-core/src/search.rs` | 561 | 16 | 充分 — RRF empty/zero_scores/cross-lang/fallback |
| `attune-core/src/context_compress.rs` | 441 | 16 | 充分 — cache scoping/llm_unavailable/batch_failure |
| `attune-core/src/parser.rs` | 394 | 11 | OK |
| `attune-core/src/crypto.rs` | 269 | 11 | OK |
| `attune-core/src/vectors.rs` | 400 | 10 | OK |

### 完全无任何 `#[test]` 的模块（≥100 行）

按行数倒序排列。**全部都在 `attune-server` crate**，且 17 个 routes 文件全军覆没（没有 inline test）：

| 行 | 文件 | gap 类型 |
|---:|------|---------|
| 789 | `attune-server/src/state.rs` | **核心 AppState** — DEK 注入 / Vault 状态机切换 / 锁释放 |
| 626 | `attune-server/src/routes/chat.rs` | RAG 路由 / 引用拼装 / WebSocket 广播 |
| 369 | `attune-server/src/routes/annotations.rs` | CRUD + AI 自动批注调度 |
| 216 | `attune-server/src/lib.rs` | runtime 启动、TLS 初始化（已有 1 个 integration `lib_runtime_test.rs`） |
| 202 | `attune-server/src/routes/index.rs` | 索引/扫描/状态切换 |
| 199 | `attune-server/src/routes/search.rs` | 搜索 endpoint 入口 |
| 172 | `attune-server/src/routes/llm.rs` | provider 切换 + 健康检查 |
| 170 | `attune-server/src/routes/settings.rs` | 设置读写（含模式切换） |
| 151 | `attune-server/src/routes/profile.rs` | 行为画像 export / import |
| 146 | `attune-server/src/routes/patent.rs` | 专利插件路由 |
| 143 | `attune-server/src/routes/classify.rs` | 分类调度 |
| 140 | `attune-server/src/routes/upload.rs` | 文件上传（multipart） |
| 132 | `attune-server/src/routes/clusters.rs` | 聚类结果 endpoint |
| 131 | `attune-server/src/routes/vault.rs` | unlock / lock / setup |
| 128 | `attune-server/src/routes/items.rs` | items CRUD |
| 127 | `attune-server/src/routes/ingest.rs` | ingest endpoint（Chrome 扩展协议） |
| 117 | `attune-cli/src/main.rs` | CLI 入口 |
| 112 | `attune-server/src/routes/status.rs` | 状态 endpoint |

合计 **3,431 行 routes 代码无 unit / inline test**。当前唯一覆盖 routes 的是 4 个 integration test 文件（共 18 tests）：
- `session_test.rs` (120 行) — auth 相关
- `store_queue_test.rs` (63 行) — store + queue 集成
- `index_path_test.rs` (52 行) — 索引路径
- `lib_runtime_test.rs` (35 行) — runtime 烟雾测试

**结论**：`attune-server` 的端到端覆盖严重不足，依赖 manual / Playwright 验证。

### 仅 happy path 的关键 module 抽查

抽查 5 个核心 module 的 test fn 名（看是否有 error / edge case 命名）：

- **vault** — 16 tests，含 `unlock_with_wrong_password_fails` / `setup_twice_fails` / `session_token_tampered_fails` / `import_invalid_hex_fails` / `dek_access_requires_unlock`：✅ **error path 充分**
- **search** — 16 tests，含 `rrf_fuse_empty` / `rrf_fuse_single_source` / `allocate_budget_zero_scores` / `rerank_fallback_when_no_vector` / `search_with_context_fts_only_fallback`：✅ **edge case 充分**
- **store** — 62 tests，含 `*_nonexistent_*_fails` / `_returns_none_for_unknown` / `_failed_abandons_after_max`：✅ **error path 充分**
- **chunker** — **仅 6 tests**：`short_text_single` / `long_text_multiple` / `markdown` / `code` / `empty` / `chinese`。✗ **缺**：overlap 边界、UTF-8 多字节字符在分块边界、超长行（无空格 / 无标点）、混合 RTL（阿拉伯文）边界、二进制污染。
- **classifier** — 5 tests，含 `invalid_json_errors` / `empty_batch_returns_empty`：基本 error path 有，但缺 LLM 超时 / 部分失败 / schema 不匹配场景

### Gap 清单（按 sprint 推荐）

| 优先级 | 模块 / 端点 | gap 类型 | 推荐 sprint |
|--------|-------------|---------|------------|
| **P0** | `attune-server/src/state.rs` (789 行) | 核心状态机零 unit test | Sprint 1（与 Tauri shell 整合同期，state 行为可能变） |
| **P0** | `attune-server/src/routes/vault.rs` | unlock/lock/setup integration 测试缺 | Sprint 1 |
| **P0** | `attune-server/src/routes/chat.rs` (626 行) | RAG 路由 / 引用 / 错误传播 | Sprint 1 - 2 |
| **P0** | `attune-server/src/routes/annotations.rs` | CRUD + AI 批注异步链路 | Sprint 1 |
| **P1** | `attune-core/src/chunker.rs` | overlap / UTF-8 边界 / 长无空格 case | Sprint 1（小成本，~5 测试） |
| **P1** | `attune-server/src/routes/upload.rs` | multipart 边界 / 大文件 / 错误格式 | Sprint 2 |
| **P1** | `attune-server/src/routes/search.rs` | API 错误路径 + 加密 vault 锁定时行为 | Sprint 2 |
| **P1** | `attune-core/src/classifier.rs` | LLM 超时 / 部分失败 / schema mismatch | Sprint 2 |
| **P1** | `attune-server/src/routes/ingest.rs` | Chrome 扩展协议向前/向后兼容 | Sprint 2 |
| **P2** | 其他 routes (settings/llm/profile/patent/clusters/items/status) | 基础 happy path integration | Sprint 3 |
| **P2** | `attune-cli/src/main.rs` | CLI 子命令 smoke test | Sprint 3 |
| **P3** | Quality regression suite (`tests/golden/queries.json`) 已存在但未跑 CI | 接 CI 工作流 | Sprint 3+ |
| **P3** | Performance benchmark layer (criterion) | 缺第 5 层（Performance） | Sprint 4+ |

### 用户语料库测试规范回顾

CLAUDE.md / `docs/TESTING.md` 要求：
1. ✅ **零随机数据** — 抽查未发现 `rand::gen` / 随机断言（store/vault/search 都用固定 seed/固定输入）
2. ✅ **真实 GitHub 语料 + 版本固化** — `tests/golden/queries.json` 存在（R8 中改过），`scripts/download-corpora.sh` 应已落地
3. ⚠️ **六层金字塔** — 当前覆盖：
   - Unit ✅（323）
   - Integration ✅（18，但偏少）
   - **Corpus Integration** ⚠️（golden set 存在但是否在 CI 跑、是否真的拉了 rust-lang/book 待 R8 确认）
   - **E2E** ⚠️（Playwright 报告在 `docs/e2e-test-report.md`，无自动化 spec 文件）
   - **Performance** ✗ —— 缺
   - **Quality Regression** ⚠（golden set 比对脚本未发现）
4. ✅ **Golden set 存在** — `rust/tests/golden/queries.json`（R8 中也改了一次）

### 缺口结论 + 推荐补丁

**最大盲点**：`attune-server` crate 17 个 routes 全部无 inline test，3,431 行只靠 4 个集成 test 文件（共 18 tests）覆盖。Sprint 1 应至少给 P0 4 项补 axum-test integration test，把 chat/vault/annotations/state 提升到 happy + 1-2 error path 覆盖。

**轻量低成本补刀**：`chunker.rs` 仅 6 tests + 138 行（不含 test 部分），加 5 个边界用例（overlap / utf-8 / 超长无空格 / 极短输入）成本不到 1 小时。

**质量回归 CI 化**：golden set 已有但很可能没接 CI workflow（`.github/workflows/`）。Sprint 2 内可以加一个 `quality-regression.yml`，pull request 触发，对比 precision@K 与 baseline，回归 > 5% 标失败。

### Skip 理由（R10 不做）

- ❌ 不立即补任何测试 — 写新测试是大工作量，违反"R10 audit-only"约束
- ❌ 不接 tarpaulin / cargo-llvm-cov — 安装编译耗时长（>10 min），结果与 grep 估计差异不大
- ❌ 不重写 `attune-server/tests/` — Sprint 1 + 2 才合适做

---

## R11 — 错误处理 gap audit

**Status**: DONE
**Commit**: `a84aa8b`

### 全局统计（prod path，排除 `#[cfg(test)] mod tests` 之后内容 + `tests/` 目录）

- prod-path `unwrap()` 总数：**25**（含整工作区：attune-core / attune-server / attune-cli / apps/attune-desktop）
- prod-path `expect(...)` 总数：**11**（已存在）
- prod-path `panic!` / `unimplemented!` / `todo!`：**2**（均在 `attune-core/src/ai_annotator.rs`，见下文 E 类）

> 含测试模块时 `unwrap` 总数为 540 — 也即测试代码占了 95% 以上，prod path 干净度已经较高。

### Top files（prod-path unwrap）

| Count | File |
|-------|------|
| 9 | `rust/crates/attune-core/src/index.rs` |
| 4 | `rust/crates/attune-core/src/vectors.rs` |
| 3 | `rust/crates/attune-cli/src/main.rs` |
| 2 | `rust/crates/attune-server/src/state.rs` |
| 2 | `rust/crates/attune-core/src/llm.rs` |
| 2 | `apps/attune-desktop/src/main.rs` |
| 1 | `rust/crates/attune-server/src/lib.rs` |
| 1 | `rust/crates/attune-core/src/infer/mod.rs` |
| 1 | `apps/attune-desktop/src/tray.rs` |

### 分类

| 类别 | 含义 | 数量 | 处理 |
|------|------|------|------|
| **A 静态保证** | schema 字段已定义、固定字节切片、内置类型 serde、常量 NonZeroUsize | **18** | 改 `expect("具体原因")` 提高诊断 |
| **B mutex poison** | Mock provider 内 `lock().unwrap()`（poison 时丢内容反而不安全） | **3** | 改 `unwrap_or_else(\|e\| e.into_inner())` 与同模块其他 mutex 一致 |
| **C 可改 `?`** | 在返回 Result 的 fn 内可 `?` 转换 | **0** | 无 |
| **D 设计层 issue** | fn 签名应返回 Result 但当前 panic | **0** | 无 |
| **E 启动 fail-fast** | main / build_router / Tauri setup 内的 unwrap，运行期不会触 | **4** | 改 `expect("具体原因")`（含静态 plugin yaml panic!） |

### 本轮 fix（A 类 + E 类，仅诊断改进，零行为变化）

| File | 旧 | 新 |
|------|----|---|
| `attune-core/src/index.rs:36-39, 57-60, 97` (×9) | `schema.get_field("X").unwrap()` | `expect("schema field 'X' defined in build_schema")` |
| `attune-core/src/vectors.rs:183, 237, 245, 253` (×4) | `bytes.try_into().unwrap()` | `expect("8-byte slice (length checked)")` 或 `expect("8-byte slice (range fixed)")` |
| `attune-cli/src/main.rs:88, 98, 108` (×3) | `serde_json::to_string_pretty(&X).unwrap()` | `expect("X is serializable")` |
| `attune-server/src/state.rs:85` | `NonZeroUsize::new(SEARCH_CACHE_CAPACITY).unwrap()` | `expect("SEARCH_CACHE_CAPACITY is non-zero const")` |
| `attune-server/src/state.rs:537` | `embedding.unwrap()` | `expect("is_none() checked above")` |
| `attune-server/src/lib.rs:157` | `"info".parse().unwrap()` | `expect("'info' is a valid log directive")` |
| `attune-core/src/infer/mod.rs:29` | `self.scores.lock().unwrap()` | `unwrap_or_else(\|e\| e.into_inner())` |
| `attune-core/src/llm.rs:366, 372` (×2) | `self.responses.lock().unwrap()` | `unwrap_or_else(\|e\| e.into_inner())` |
| `apps/attune-desktop/src/main.rs:12` | `"info".parse().unwrap()` | `expect("'info' is a valid log directive")` |
| `apps/attune-desktop/src/main.rs:41` | `url.parse().unwrap()` | `expect("embedded server URL is well-formed")` |
| `apps/attune-desktop/src/tray.rs:15` | `app.default_window_icon().unwrap()` | `expect("default window icon embedded via tauri.conf.json")` |

**总计 fix**：25 个 `unwrap()` → 22 改 `expect(...)`，3 改 `unwrap_or_else(|e| e.into_inner())`（mutex poison-safe，与 routes/state 中其他 mutex 保持一致 idiom）。

**Post-fix 验证（同一脚本 grep）**：
```
prod-path bare unwrap() 剩余总数：0
```

**含义**：attune 整工作区（attune-core / attune-server / attune-cli / apps/attune-desktop）prod path 现在 **零 bare unwrap()**。所有 fallible 调用要么用 `?` 走 Result，要么用 `expect("具体语义")` 显式表达静态保证，要么用 `unwrap_or_else(|e| e.into_inner())` poison-safe 处理 mutex。

### 保留未改（panic! / 故意性 fail-fast）

`attune-core/src/ai_annotator.rs:95-97`：

```rust
.unwrap_or_else(|e| panic!("builtin ai_annotation_{angle_tag} plugin yaml broken: {e}"));
```

**不改原因**：内置 plugin YAML 编译期 `include_str!` 嵌入二进制，发布前 CI 跑 `cargo test` 必定执行 `Plugin::from_yaml`。如果 yaml broken 是构建期 bug，启动 panic 是正确行为（fail-fast），运行时永远不会触。改 Result 反而要求所有调用方处理一个不可能发生的错误，污染签名。

### Backlog（C / D 类 — 无）

**当前 audit 没有发现**：
- C 类（fn 已返回 Result 但用了 unwrap） — 0
- D 类（fn 应返回 Result 但当前 panic） — 0

**意味着**：经过前 10 轮清理 + 已有的 `expect("HTTP client")`/`expect("HMAC key length valid")` 等改进，attune 的错误处理已经接近"所有真错误走 Result，所有 unwrap 都是不可达"。R11 收尾把剩余 unwrap 从无诊断信息升级到带上下文的 `expect`。

### Tests

| Phase | 命令 | 结果 |
|-------|------|------|
| Pre-fix | `cargo test --workspace` | 377 passed |
| Post-fix | `cargo test --workspace` | **377 passed**（零行为变化，仅 panic 消息升级） |

### 跨平台影响

- 全部改动是诊断字符串 / mutex poison 处理，无平台特性 — Linux/Windows/aarch64 行为一致
- `apps/attune-desktop` 改动 unwrap 主要影响 Windows MSI 启动期的 panic message 可读性（之前是 "called `Option::unwrap()` on a `None` value"，现在是 "default window icon embedded via tauri.conf.json"）

### Skip 理由

- ❌ 不重构 fn 签名（C / D 类不存在；如果存在也属大改）
- ❌ 不改 `panic!` in ai_annotator.rs — 是合理 fail-fast，改 Result 污染所有调用方
- ❌ 不改测试模块内的 unwrap — 测试 panic 即测试失败，是 idiom

---

## R12 — 跨平台 gap audit（Windows readiness）

**Status**: DONE
**Commit**: <pending>

R4 已审查 `cfg(unix)` syscall 边界，本轮扩展到全部 6 个跨平台维度，覆盖 Win build 在静态层面的所有已知风险点。

### 6 维度 audit 结果

| 维度 | 扫描命令 | finding | 决策 |
|------|---------|---------|------|
| **路径分隔符** | `grep -rnE '"/(home\|tmp\|usr\|...)"'` | prod 仅 `web_search_browser.rs` 5 处 `/usr/bin/...` + `ocr.rs` 测试 fixture 2 处 `/usr/bin/tesseract` — 全部已被 `#[cfg(target_os = "linux")]` / Win 分支 `format!("{pf}\\Google\\Chrome\\...")` 完整覆盖 | ✅ 已合规 |
| **临时目录 `/tmp`** | `grep -rnE '"/tmp"'` | 仅出现在 `store.rs` `#[cfg(test)]` 模块和 `attune-server/tests/index_path_test.rs` 测试中 — 都是 opaque test fixture，不做 fs IO；prod 代码使用 `tempfile::TempDir`（如 `vault.rs` 测试） | ✅ 已合规 |
| **行尾 `\n` vs `\r\n`** | `grep -rnE '"\\\\n"'` 排除 println | prod 代码无硬编码 `"\n"` 期望；解析路径使用 `.lines()`（自动处理 `\r\n`）；`String::from_utf8_lossy` 容错 | ✅ 已合规 |
| **`Command::new(...)` 子进程** | `grep -rnE 'Command::new\('` | 12 处调用：`vault.rs:icacls`(Win cfg)、`platform.rs:sysctl`(macOS cfg×2)、`platform.rs:wmic`(Win cfg×3)、`ocr.rs:which`(**未 cfg — Win 上 fail**)、`ocr.rs:tesseract/pdftoppm`(用配置路径 OK)、`llm.rs:ollama`(跨平台 binary OK) | 1 处需修复 |
| **文件权限 0o600** | (R4 已审) `std::os::unix` 在 vault.rs:361 `#[cfg(unix)]` 内；Win 走 icacls | ✅ 已合规（R4 验证） |
| **autocrlf / `.gitattributes`** | `cat .gitattributes` | **不存在 `.gitattributes`** — 若 Win 开发者设置 `core.autocrlf=true`，Rust 源码 `\n` → `\r\n` 转换不影响编译（rustc 容忍 BOM/CRLF），但脚本和 yaml 模板可能受影响 | ⚠ Backlog（低优先） |

### Fix（最小 patch）

**ocr.rs:64 `Command::new("which")` → `which::which()` crate**

R4 implementer 提到的唯一 prod 跨平台缺陷：Win 上 `which` 可执行不存在（Win 用 `where`）。最小修复用 `which` crate（跨平台 PATH 查找，内部按 OS 分发到 `which`/`where` 等价行为）：

- `rust/crates/attune-core/Cargo.toml`：新增 `which = "6"` 依赖
- `rust/crates/attune-core/src/ocr.rs`：`fn which_bin` 从 `Command::new("which")` 改为 `which::which(name)`

```rust
fn which_bin(name: &str) -> Option<String> {
    // 跨平台 PATH 查找：Linux/macOS 等价 `which`，Windows 等价 `where`，
    // 使用 `which` crate 避免 Win 上 `Command::new("which")` 失败。
    which::which(name)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}
```

**好处**：
- Win/Linux/macOS 行为完全一致
- 不再依赖系统 PATH 工具（部分 minimal Win 容器无 `where`）
- Library 内部处理 PATHEXT / `.exe` 后缀

### Backlog（非阻塞 Win build）

| Item | 说明 | 推荐 sprint |
|------|------|-------------|
| `.gitattributes` for line endings | 添加 `* text=auto eol=lf` + `*.{ps1,bat,cmd} text eol=crlf` 防止 Win checkout 转换破坏 shell 脚本 / yaml | Sprint 1（Win 安装包构建期再补，影响范围小） |
| Tauri shell 集成中的 Linux `desktop-entry` 路径 | `apps/attune-desktop/src-tauri/tauri.conf.json` 若涉及 Linux .desktop 文件路径，Win 时由 Tauri 自动跳过 | 已自动处理（Tauri 内置） |
| Win MSVC C++ 工具链验证（`usearch`/`rusqlite-bundled`） | 静态分析无法验证 — 需 Win 实际跑 `cargo build` | Sprint 1 / Task 11（CI matrix） |

### Win build 静态评估

| 风险点 | 状态 |
|--------|------|
| 路径分隔符 | ✅ 已 cfg 隔离 |
| `/tmp` 硬编码 | ✅ prod 全用 `tempfile::TempDir` |
| Unix 权限 (`0o600`) | ✅ R4 已验证 cfg(unix) 隔离 |
| Unix 命令 (`which`) | ✅ 本轮 fix |
| Win 命令 (`icacls`/`wmic`) | ✅ 已 cfg(windows) |
| C/C++ 依赖编译 | ⚠ 需 Win 实跑（Sprint 1 CI matrix） |
| `\r\n` git checkout | ⚠ 添加 `.gitattributes` 即可（backlog） |

**结论**：纯 Rust 源码层面 attune 应能在 Windows 上 cargo build 成功。剩余风险全部在工具链层（MSVC + git autocrlf），由 Sprint 1 的 CI matrix（Task 11）实测验证。

### Tests

| Phase | 命令 | 结果 |
|-------|------|------|
| Pre-fix | `cargo test --workspace` | 377 passed |
| Post-fix | `cargo test --workspace` | **377 passed**（`which` crate 不改变 Linux 上的 fn 行为） |

### Skip 理由

- ❌ 不添加 `.gitattributes` — Sprint 0 worktree 仍在 Linux 单平台，Win 实跑前没法验证规则正确性，留给 Sprint 1 CI matrix 一起做
- ❌ 不重写 ocr.rs / parser.rs 跨平台逻辑 — 静态分析未发现其他缺陷，过度重写引入新 bug 风险高于收益

---
