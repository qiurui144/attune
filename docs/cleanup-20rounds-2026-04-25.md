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
