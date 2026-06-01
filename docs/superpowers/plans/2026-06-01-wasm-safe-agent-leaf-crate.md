# Plan — WASM-safe Agent Leaf Crate 抽取实施计划

> 状态:待评审 · 作者:实施规划 agent · 日期:2026-06-01
> 关联 spec(已批准):`docs/superpowers/specs/2026-06-01-wasm-safe-agent-leaf-crate.md`
> 跨仓:本计划主体在 **attune(OSS)** 仓;attune-pro 改链为下游 sequencing(见 §6)
> 目标版本:`attune-agent-sdk 0.1.0`(独立 SemVer)随 `attune-core 1.1.0` / attune `v1.1.0` 同 tag 发布

---

## 0. 真实代码复核结论(plan 前置,已逐项查证)

| 复核项 | 实测结论 | 影响 plan 的点 |
|--------|----------|----------------|
| `rust/Cargo.toml` members | `["crates/attune-core","crates/attune-cli","crates/attune-server","crates/attune-accounts"]` + 同名 `default-members` | 新 crate 必须同时加进 `members`(default-members 不必加 —— leaf 不是 build/run 目标) |
| `[workspace.dependencies]` | 已有 `serde = { version="1", features=["derive"] }` | leaf 走 `serde.workspace = true`,**不独立 pin**(R3 缓解) |
| `attune-core/Cargo.toml` | `thiserror = "2"` + `serde/serde_json = workspace` | leaf 若用 thiserror 走 `"2"` 同步;`From<AgentError>` 在 core 侧加,无新 dep |
| `agents/mod.rs:14` | `use crate::error::Result;`(trait `run` 返回此 `Result`) | 删本地 trait/struct 改 re-export 后,此 `use` 仅剩 `locate_workspace_file`/`load_workspace_flows` 还需吗?**否** —— 这两个 fn 不用 `Result`(用 `Option`/`std::result::Result<_,String>`),`use crate::error::Result` 在删除 trait 后变 unused,需一并删 |
| `agents/mod.rs:120-238` 现有 tests | `has_red_lines`/`needs_attention`/serde roundtrip/generic 共 8 个 + 3 个 workspace-file 测试 | 前 8 个(测 `AgentOutput`)迁入 leaf;后 3 个(测 `locate_workspace_file`/`load_workspace_flows`)**留 attune-core** |
| `error.rs:3` `VaultError` | `#[from] rusqlite::Error`/`serde_yaml::Error`/`std::io::Error`/`serde_json::Error` | 这是 native 耦合根因 —— leaf `AgentError` **零** `#[from]` native;`From<AgentError> for VaultError` 加在 error.rs |
| 内部 `impl Agent` | `agents/document_classifier.rs` + `agents/registry.rs` 各有 impl;`mod.rs:117` 是 trait 定义本体 | re-export 后这俩 impl 指向同一 trait,签名不变零改动(R8 验证项) |
| attune-pro `law-pro/Cargo.toml` | `attune-core = { workspace = true }` + `chrono`(interest_calculator 用)+ `proptest`(确定性 agent 测试) | 下游加 `attune-agent-sdk` dep;`agent_limitation_check.rs` pilot bin 真实存在 |
| golden gate | `attune-pro/plugins/law-pro/tests/agent_golden_gate.rs` + attune-core `src/agent_quality.rs` | 两者都不得回归(GA 验收项) |

> ⚠️ **复核新增发现(写进 plan)**:删除 `mod.rs` 本地 trait 后,`use crate::error::Result`(行 14)会成为 unused import → D2 必须一并删除,否则 `-D warnings`(若开)或 clippy 报 unused。

---

## 1. 阶段 / 日历

> 总跨度估 5 个工作单元(D1–D5),纯 crate 重构无外部依赖,可单会话连续推进;`cargo build --target wasm32-wasip1` 需先 `rustup target add wasm32-wasip1`(D1 前置)。

| 阶段 | 主题 | 关键交付 | 验证门 |
|------|------|----------|--------|
| **D1** | 建 `attune-agent-sdk` leaf crate | 新 crate 含 `Agent` trait + `AgentOutput<T>` + `AgentError`/`AgentResult`;workspace 注册;6 类下限测试落地 | `cargo build -p attune-agent-sdk`(native)+ `cargo build -p attune-agent-sdk --target wasm32-wasip1` 双绿;`cargo test -p attune-agent-sdk` 全过 |
| **D2** | attune-core re-export + From 桥接 + 测试迁移 | `agents/mod.rs` 删本地定义改 `pub use`;删 unused `use crate::error::Result`;`error.rs` 加 `From<AgentError> for VaultError`;8 个 `AgentOutput` 测试迁入 leaf,3 个 workspace-file 测试留 core | `cargo test -p attune-core` 211+ 不回归;`cargo clippy -p attune-core` 无新 warning;JSON wire 逐字节相等断言通过 |
| **D3** | pilot 确定性 agent 链 leaf + wasm 编译(跨仓判据描述) | 在 attune 仓**不提交** attune-pro 代码;本阶段产出 = leaf 的 wasm 编译守卫 CI job + 一份"pilot 改链 recipe"文档片段(law-pro `agent_limitation_check`,供 attune-pro 仓执行) | leaf CI wasm job 绿;recipe 经本地 dry 验证(在 attune-pro 仓侧,见 §6 sequencing,不在本仓 commit) |
| **D4** | re-export 稳定性 + 全量回归 | attune-core / attune-server / attune-cli / attune-accounts 全 workspace build + test;确认内部 `impl Agent`(document_classifier/registry)零改动仍编译 | `cargo test --workspace` 全绿;`cargo clippy --workspace --all-targets` 无新 warning;`agent_quality.rs` 不回归 |
| **D5** | 收尾:文档 + release 节奏对齐 | leaf README(crate 级)+ attune RELEASE.md `v1.1.0` 节补 "WASM-safe agent SDK" highlight + sequencing 注;DEVELOP.md crate 列表加 attune-agent-sdk | RC 四节门(§7.2)Gate1 文档无漂移;leaf 版本 0.1.0 与 attune-core 1.1.0 同 tag |

---

## 2. 文件清单(每阶段精确 path)

### D1 — 新建 leaf crate

| 操作 | path | 内容 |
|------|------|------|
| 新建 | `rust/crates/attune-agent-sdk/Cargo.toml` | `[package] name="attune-agent-sdk" version="0.1.0" edition.workspace=true license.workspace=true publish=false`;`[dependencies] serde={workspace=true} thiserror="2"`(评审若否决 thiserror 则手写 Display/Error,见 R4);`[dev-dependencies] serde_json={workspace=true} proptest="1"` |
| 新建 | `rust/crates/attune-agent-sdk/src/lib.rs` | `Agent` trait(§5.1 签名,`run` 返回 `AgentResult<AgentOutput<Self::Output>>`)+ `AgentOutput<T>`(6 字段 + `has_red_lines`/`needs_attention` impl)+ `#[non_exhaustive] AgentError`(InvalidInput/Computation/Serialization/RedLine)+ `pub type AgentResult<T>`;crate-level `//!` doc 含 "零 native 依赖,可编 wasm32-wasip1" 不变量 + deny-list 注释(禁 rusqlite/tokio/reqwest/serde_yaml/chrono 回流) |
| 改 | `rust/Cargo.toml` | `members` 数组追加 `"crates/attune-agent-sdk"`(**不**加进 default-members);`[workspace.dependencies]` 追加 `attune-agent-sdk = { path = "crates/attune-agent-sdk" }` |

> D1 测试(全部 inline `#[cfg(test)]` 或 `tests/`,满足 §9 矩阵):
> - 迁入的 8 个 `AgentOutput` 测试(has_red_lines×2 / needs_attention×4 / serde_roundtrip / generic)
> - 属性测试 ≥3:`AgentOutput<Value>` serde roundtrip proptest;`needs_attention` 不变量(red_lines 非空 ⟹ true);confidence 任意 f64 不 panic
> - 边界 ≥5:空 computation / 超长 audit_trail / confidence 0.0,1.0 / 空+满 red_lines / Unicode audit_trail
> - 异常 ≥3:`AgentError` 4 变体 Display 文案;非穷举性(`#[non_exhaustive]` 编译标记)

### D2 — attune-core re-export + 桥接

| 操作 | path | 内容 |
|------|------|------|
| 改 | `rust/crates/attune-core/Cargo.toml` | `[dependencies]` 追加 `attune-agent-sdk = { workspace = true }` |
| 改 | `rust/crates/attune-core/src/agents/mod.rs` | 删除行 71–118 的本地 `AgentOutput<T>` + impl + `Agent` trait 定义体;改为 `pub use attune_agent_sdk::{Agent, AgentOutput, AgentError, AgentResult};`;删除行 14 `use crate::error::Result;`(变 unused);保留 `locate_workspace_file`/`load_workspace_flows` + `pub mod` 子模块声明;删除行 120–197 已迁走的 `AgentOutput` 测试,保留行 199–237 三个 workspace-file 测试 |
| 改 | `rust/crates/attune-core/src/error.rs` | 在 `pub type Result` 之后新增 `impl From<attune_agent_sdk::AgentError> for VaultError`:`InvalidInput→InvalidInput`、`Computation→Classification`、`Serialization→Json`(注:不能直接构造 `serde_json::Error`,映射到 `VaultError::InvalidInput(format!(...))` 或新增 `Serialization(String)` 变体 —— **评审决策点**,见 R-impl-1)、`RedLine→InvalidInput`;并加该 From 的单测(每变体映射断言) |

> D2 关键断言(§9 JSON wire 不变):新增一个测试,构造固定 `AgentOutput` 值,`serde_json::to_string` 与抽取前期望字符串逐字节相等(同 6 字段顺序、同字段名)。

### D3 — pilot 改链 recipe + wasm CI 守卫

| 操作 | path | 内容 |
|------|------|------|
| 新建/改 | `.github/workflows/rust-release.yml` 或新增 `wasm-guard.yml` | 新 CI job:`rustup target add wasm32-wasip1` + `cargo build -p attune-agent-sdk --target wasm32-wasip1`,引入 native dep 即红(R6 守卫) |
| 文档 | plan 内 §6 recipe 段(不在 attune 仓产代码) | attune-pro `agent_limitation_check` 改链步骤(供 attune-pro 仓 sequencing 执行) |

### D4 — 全量回归(无新文件,验证为主)

- 验证 `cargo test --workspace` + `cargo clippy --workspace --all-targets`;若发现内部 `impl Agent`(`agents/document_classifier.rs` / `agents/registry.rs`)需改 `run` 返回类型 → 经 `From<AgentError>` 桥接应零改动,若 break 则加 reproducer。

### D5 — 文档收尾

| 操作 | path | 内容 |
|------|------|------|
| 新建 | `rust/crates/attune-agent-sdk/README.md`(可选,crate 级) | leaf 定位 + wasm-safe 不变量 + deny-list |
| 改 | `RELEASE.md` `v1.1.0` 节 | Highlights 加 "WASM-safe agent SDK (attune-agent-sdk 0.1.0) — 确定性 agent 可编 wasm";Known Limitations 标 sequencing(需 ≥ attune 1.1.0) |
| 改 | `DEVELOP.md` | crate 列表 / 架构图加 attune-agent-sdk(leaf) |

---

## 3. commit 分批(单一职责)

| # | 阶段 | commit message | 触及文件 |
|---|------|----------------|----------|
| C1 | D1 | `feat(agent-sdk): add wasm-safe attune-agent-sdk leaf crate (Agent + AgentOutput + AgentError)` | 新 crate 2 文件 + `rust/Cargo.toml` members/workspace.deps |
| C2 | D2 | `refactor(core): re-export Agent/AgentOutput from attune-agent-sdk + From<AgentError> bridge` | `attune-core/Cargo.toml` + `agents/mod.rs` + `error.rs`(含测试迁移 + JSON wire 断言) |
| C3 | D3 | `ci(wasm): add wasm32-wasip1 build guard for attune-agent-sdk` | workflow yml |
| C4 | D4 | `test(core): verify workspace build + internal impl Agent unchanged after re-export`(若无代码改动则并入 C2,仅当 D4 发现需 fix 才独立) | (按需) |
| C5 | D5 | `docs(release): v1.1.0 — wasm-safe agent SDK + sequencing note` | RELEASE.md / DEVELOP.md / leaf README |

> 每 commit 后走 §5.2 两轮 code review;C2 涉及 trait re-export(向后兼容关键),额外走 `engineering-skills:adversarial-reviewer`。
> commit 不直接进 main —— 走 develop;tag(`v1.1.0` + `desktop-v1.1.0`)在 develop→main `--no-ff` merge 后于 main 打。

---

## 4. 风险登记

### 继承 spec §11

| # | 风险 | 缓解(实施落点) |
|---|------|----------------|
| R1 | 循环依赖(leaf 反依赖 core) | leaf `Cargo.toml` 仅 serde+thiserror;`From<AgentError> for VaultError` 在 **core 侧**(C2),方向 core→leaf 单一,`cargo build` 无环报错即守卫 |
| R2 | re-export 写成重定义 → attune-pro `impl Agent` 失效 | C2 强制 `pub use attune_agent_sdk::{Agent,...}`(同一类型);D4 + §6 跨仓 smoke 验证 attune-pro `impl Agent` 仍 resolve |
| R3 | serde 版本不一致致跨 crate 类型不兼容 | leaf `serde.workspace = true`(复用 root `[workspace.dependencies] serde`),不独立 pin |
| R4 | thiserror wasm 可疑 | thiserror 2.x 纯 proc-macro(展开为 `impl Display/Error`,零运行期 dep),D1 wasm build 即验证;若红则手写 `impl Display + std::error::Error`(wasip1 有 std,安全) |
| R5 | 与 WASM runtime release sequencing | leaf 0.1.0 + attune-core 1.1.0 同 tag(D5);RELEASE.md 标注前置;attune-pro 改链 task blockedBy attune v1.1.0 tag(§6) |
| R6 | 隐性 native 依赖回流 | C3 CI wasm 守卫 + leaf Cargo.toml deny-list 注释 + review SOP 查 leaf deps 增项 |
| R7 | 跨仓漂移(attune-pro 不迁) | attune-pro 仓建迁移 task;spec §10.3 两步迁移(先升 dep 仍用 re-export,再切 import)降低压力 |
| R8 | 内部 agent 签名隐性依赖 VaultError | D4 验证 `agents/document_classifier.rs`/`registry.rs` 的 `impl Agent` —— trait 签名用 `AgentResult`,`?` 处自动 `From` 桥接,应零改动;若 break 加 reproducer |

### 实施新增

| # | 风险 | 缓解 |
|---|------|------|
| R-impl-1 | **`From<AgentError> for VaultError` 中 `Serialization` 无法映射回 `serde_json::Error`** —— `VaultError::Json(#[from] serde_json::Error)` 需真 `serde_json::Error` 实例,无法从 String 凭空构造 | **评审决策点**:推荐映射 `AgentError::Serialization(s) → VaultError::InvalidInput(format!("serialization: {s}"))`(不新增变体,最小改面);备选给 `VaultError` 加 `Serialization(String)` 变体(改面更大,影响前端 error code 映射 per error.rs 测试注释)。plan 默认取前者 |
| R-impl-2 | **workspace 编译顺序** —— cargo 自动按依赖拓扑排序;leaf 无依赖最先编,core 依赖 leaf 后编 | 无需手工干预;C1 单独 build leaf 验证可独立编译(无环前提) |
| R-impl-3 | **`mod.rs` 删本地定义后 `use crate::error::Result` 变 unused** | C2 一并删除该 use(复核已确认 `locate_workspace_file`/`load_workspace_flows` 不用它) |
| R-impl-4 | **leaf `version = "0.1.0"` 误被 bump 随 attune tag** | per §1.1.8 插件/子 crate 版本独立 —— leaf 只在真有 delta 时 bump;首发 0.1.0 随 1.1.0 tag 但版本号不绑定 |
| R-impl-5 | **`agent_quality.rs`(attune-core)依赖本地 `AgentOutput` 路径** | re-export 后 `attune_core::agents::AgentOutput` 路径不变,应零改动;D4 跑 `agent_quality` 验证 |

---

## 5. GA 验收清单(可勾选)

- [ ] `cargo build -p attune-agent-sdk`(native)绿
- [ ] `rustup target add wasm32-wasip1` 后 `cargo build -p attune-agent-sdk --target wasm32-wasip1` **干净**(无 native dep 拉入)
- [ ] `cargo test -p attune-agent-sdk` 全过(8 迁入 + ≥3 proptest + ≥5 边界 + ≥3 异常)
- [ ] `attune_core::agents::{Agent, AgentOutput}` 路径仍可用(re-export 同一类型)
- [ ] `cargo test -p attune-core` 211+ tests 不回归
- [ ] `cargo test --workspace` 全绿(core/server/cli/accounts)
- [ ] `cargo clippy --workspace --all-targets` 无新 warning(含删 unused `use crate::error::Result`)
- [ ] `From<AgentError> for VaultError` 每变体映射单测通过
- [ ] JSON wire 逐字节相等断言通过(`AgentOutput` 抽取前后 `to_string` 一致)
- [ ] 内部 `impl Agent`(document_classifier / registry)零改动仍编译
- [ ] `agent_quality.rs`(attune-core)不回归
- [ ] CI wasm 守卫 job 配置并绿
- [ ] **(跨仓判据,attune-pro 侧验)** law-pro `agent_limitation_check` 链 leaf 后 `cargo build --target wasm32-wasip1` 出 `.wasm`
- [ ] **(跨仓)** attune-pro `impl Agent` 编译通过(re-export 兼容)
- [ ] **(跨仓)** law-pro `agent_golden_gate.rs` 全绿(1.00 deterministic)
- [ ] RELEASE.md `v1.1.0` 节(Highlights/Known Limitations sequencing)无文档漂移(Gate 1)
- [ ] leaf 0.1.0 + attune-core 1.1.0 同 tag,DEVELOP.md crate 列表已更新

---

## 6. 跨仓分工 + sequencing

| 仓 | 职责 | 触发条件 |
|----|------|----------|
| **attune(OSS,本计划)** | 建 leaf crate `attune-agent-sdk` + attune-core re-export + From 桥接 + wasm CI 守卫 + 文档;发含 leaf 0.1.0 + attune-core 1.1.0 + WASM runtime(前置 spec 已落)的 **attune v1.1.0 release tag** | 本 plan 评审通过即开 |
| **attune-pro(私有,下游)** | law-pro/patent-pro 确定性 agent `Cargo.toml` 加 `attune-agent-sdk` dep;bin/lib import 由 `attune_core::agents::{Agent,AgentOutput}` → `attune_agent_sdk::{Agent,AgentOutput}`、`attune_core::error::{Result,VaultError}` → `attune_agent_sdk::{AgentResult,AgentError}`;`cargo build --target wasm32-wasip1` 产 `.wasm`;填 `plugin.yaml runtime: wasm`;golden gate 不回归 | **blockedBy attune v1.1.0 tag**(leaf 须先随 attune 发布,attune-pro 才能 dep 到稳定 leaf + 有 WasmRunner 可跑) |

### pilot 改链 recipe(供 attune-pro 仓 D3 执行,本仓不 commit)

1. `law-pro/Cargo.toml` `[dependencies]` 加 `attune-agent-sdk = { workspace = true }`(attune workspace 已登记)。
2. `src/bin/agent_limitation_check.rs`:`use attune_core::agents::{Agent, AgentOutput};` → `use attune_agent_sdk::{Agent, AgentOutput};`;`use attune_core::error::{Result, VaultError};` → `use attune_agent_sdk::{AgentResult as Result, AgentError};`(别名最小改 body)。
3. 确认该 bin 不 `use attune_core::llm`(确定性 agent 无 LLM)→ 移除对 attune-core 的链接(若 lib 部分仍需则保留,仅 bin crate 改)。
4. `rustup target add wasm32-wasip1` → `cargo build -p law-pro --bin agent_limitation_check --target wasm32-wasip1` → 产 `.wasm`。
5. 跑 `agent_golden_gate.rs` 确认 1.00 不回归。

### 两步迁移(spec §10.3,降低 attune-pro flag-day 压力)

- (a) attune-pro 先随 attune 1.1.0 升 dep,确定性 bin **可继续**用 `attune_core::agents::*`(re-export 仍可用,只是仍编不了 wasm)—— 不阻塞。
- (b) 真要编 wasm 的 vertical 再切 import 到 `attune_agent_sdk::*` + 去 attune-core 链接 —— 逐个 vertical 推进,无 flag-day。

---

## 7. 评审决策点(plan 评审需拍板)

1. **thiserror vs 手写**(R4):默认用 thiserror 2.x(与 attune-core 同版);若评审要求 leaf 零 proc-macro dep,降级手写 `impl Display + Error`。
2. **`Serialization` 映射**(R-impl-1):默认 `AgentError::Serialization(s) → VaultError::InvalidInput`;备选给 VaultError 加 `Serialization(String)` 变体(改面更大)。
3. **CI 守卫落点**(D3):新建 `wasm-guard.yml` 独立 workflow vs 并入现有 `rust-release.yml` 的 job —— 默认独立 workflow(职责清晰,PR 触发更快)。
