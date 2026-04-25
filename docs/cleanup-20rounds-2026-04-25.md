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
