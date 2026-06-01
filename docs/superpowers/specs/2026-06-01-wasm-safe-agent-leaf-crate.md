# Spec — WASM-safe Agent Leaf Crate 抽取

> 状态:草案待评审 · 作者:架构设计 agent · 日期:2026-06-01
> 关联前置:`docs/superpowers/specs/2026-05-31-agent-cross-platform-distribution.md`(WASM runtime 接入,已实现于 develop,未 tag)
> + `docs/reports/2026-06-01_agent-cross-platform-impl.md`(实施报告)
> + `attune-pro/docs/reports/2026-06-01_attune-pro-perf-wasm-prep.md`(暴露本阻塞)
>
> **已调研真实代码(非臆断)**:
> - `rust/crates/attune-core/src/agents/mod.rs:73` `AgentOutput<T>`(6 字段,仅依赖 `serde`)
> - `rust/crates/attune-core/src/agents/mod.rs:103` `pub trait Agent { type Input; type Output; fn id/description/case_kinds/run }`,`run` 返回 `Result<AgentOutput<Self::Output>>`
> - `rust/crates/attune-core/src/error.rs:4` `VaultError`(含 `#[from] rusqlite::Error` / `serde_yaml::Error`),`error.rs:66` `pub type Result<T> = Result<T, VaultError>`
> - `attune-pro/plugins/law-pro/src/bin/agent_*.rs` 全部 `use attune_core::agents::{Agent, AgentOutput}` + `attune_core::error::{Result, VaultError}`(LLM bin 另 `use attune_core::llm`)
> - attune-core `version = "1.1.0"`;law-pro `version = "1.0.5"`

---

## 0. 调研结论(现状基线,设计前提)

实测 `cargo build --target wasm32-wasip1 -p attune-core --no-default-features` 仍失败,根因为依赖图而非 agent 逻辑:

1. **`Agent` trait + `AgentOutput<T>` 本身已 wasm-safe** —— 只依赖 `serde` 和 `crate::error::Result`。问题全在它们所在的 `attune-core` crate 把 `rusqlite`(bundled C SQLite)、`usearch`(C++)、`tantivy`、`hdbscan`、`tokio`、`reqwest`(→`socket2`/`mio`)整树拖入。
2. **`error::Result` 是隐蔽耦合点**:agent bin `use attune_core::error::{Result, VaultError}`,而 `VaultError` 通过 `#[from] rusqlite::Error` / `#[from] serde_yaml::Error` 直接引用 native-only 类型。即使只 import `Result`,链接 `attune-core` 即拖入这些。
3. **agent bin 契约已就绪**:law-pro 确定性 bin 已是 `stdin JSON → stdout AgentOutput JSON → exit 0/1/2(/3)`,逻辑层零改动需求(per perf-wasm-prep 报告)。
4. **结论**:必须抽一个零 native 依赖的 leaf crate,承载 `Agent` trait + `AgentOutput<T>` + 一个 wasm-safe 的轻量错误类型,agent bin 改链 leaf crate;`attune-core` 反过来依赖并 re-export leaf crate 以保现有调用不破。

---

## 1. 目标定位

**用户痛点**:attune-pro 的确定性 agent(利息计算 / 诉讼时效 / 证据链等)逻辑纯计算、契约已对齐 WASM,但因经 `attune-core` 间接拉入 native-only 依赖,**编不了 `wasm32-wasip1`** → 无法享受 v1.1.0 WASM runtime "一包通吃所有平台" 的分发能力(Windows/Linux/riscv 各出一份 native 二进制的维护负担依旧)。

**与产品定位对齐**:WASM 一包通吃直接服务 attune 跨平台分发北极星(P0 Windows + P1 Linux + P2 riscv K3 镜像)。本 spec 解除前置 spec(2026-05-31)落地后暴露的唯一真实阻塞,使 attune-pro 确定性 agent 真正可编 `.wasm`。

**不是什么**:不是重写 agent 逻辑、不是改 WASM runtime(那是前置 spec 的产物)、不是迁移 attune-pro(那是 attune-pro 仓后续工作)。本 spec 只在 OSS attune 仓做 **crate 抽取 + re-export**,产出可被链接成 wasm 的 leaf crate。

---

## 2. 范围边界

### 本 spec 做

1. 新建 wasm-safe leaf crate(命名见 §4,暂定 `attune-agent-sdk`),迁入:`Agent` trait、`AgentOutput<T>` + 其 `impl`、一个 wasm-safe 错误类型 `AgentError` + `AgentResult<T>` 别名。
2. leaf crate 依赖**仅** `serde`(+ 可选 `thiserror`,需确认 thiserror wasm-safe);**零** `rusqlite`/`tokio`/`reqwest`/`usearch`/`tantivy`/`hdbscan`/`socket2`。
3. `attune-core` 改为依赖 leaf crate,并 `pub use` re-export `Agent`/`AgentOutput`(保 `attune_core::agents::{Agent, AgentOutput}` 路径不变)。
4. 定义 `VaultError` ↔ `AgentError` 的 `From` 转换,使 attune-core 内部现有 `agents::*` 调用与 `error::Result` 链路无感。
5. 验证判据:leaf crate `cargo build --target wasm32-wasip1` 干净 + 一个 pilot agent bin(选 attune-pro `agent_limitation_check`,跨仓验证仅作判据描述,不在本仓提交)链 leaf 后能编 `.wasm`。
6. leaf crate 自带 6 类下限测试 + 现有 `agent_golden_gate`/`agent_quality` 不回归。

### 本 spec 不做(写死,禁止 silent scope creep)

- ❌ 不改任何 agent 业务逻辑 / 计算公式。
- ❌ 不迁移 attune-pro plugin(改 law-pro/patent-pro 的 `Cargo.toml` dep、产 `.wasm`、填 `plugin.yaml runtime: wasm` 全在 **attune-pro 仓**后续做;本 spec 仅在 §4/§10 描述其改动面作为契约)。
- ❌ 不动 `agents::{flow, flow_runner, registry, scheduler, document_classifier}`(这些是 attune-core 内编排器,依赖 DB/网络,**留在 attune-core**,不进 leaf)。
- ❌ 不把 `attune_core::llm`(LlmProvider 等)迁入 leaf(LLM agent 走 reqwest,native-only,保 `rust_binary`)。
- ❌ 不引入新 async runtime / 不改 WASM runtime 执行语义。

### 哪些 agent 适用

| 类别 | 适用 leaf crate(可编 wasm) |
|------|------|
| 纯确定性计算(law-pro:civil_loan/limitation/labor_dispute/evidence_chain/sale_contract/housing_rent/divorce/traffic_accident/inheritance;patent-pro 结构化抽取计算部分) | ✅ 改链 leaf |
| native-only(fact_extract/defamation/各 LLM extractor、PDF/OCR/Chrome 预处理) | ❌ 保留链 `attune-core` + `rust_binary` |

---

## 3. 架构数据流

### 3.1 crate 依赖方向(抽取前 vs 抽取后)

```
                          抽取前(现状,编不了 wasm)
   ┌────────────────────────────────────────────────────────────────┐
   │ law-pro agent bin  ── use attune_core::agents::{Agent,Output}    │
   │                       use attune_core::error::{Result,VaultError}│
   │                                │                                 │
   │                                ▼                                 │
   │                          attune-core (整树)                      │
   │   Agent/AgentOutput  +  rusqlite(C) + usearch(C++) + tantivy     │
   │                          + tokio + reqwest(socket2/mio) + ...     │
   │                                │                                 │
   │                                ▼ target wasm32-wasip1            │
   │              socket2 compile_error!  ✗ 编译失败                  │
   └────────────────────────────────────────────────────────────────┘

                          抽取后(本 spec,可编 wasm)
   ┌────────────────────────────────────────────────────────────────┐
   │  attune-agent-sdk  (leaf, 零 native)                             │
   │    pub trait Agent { type Input; type Output; fn run -> ... }    │
   │    pub struct AgentOutput<T> { computation, audit_trail, ... }   │
   │    pub enum AgentError;  pub type AgentResult<T>                 │
   │    deps: serde  [+ thiserror?]                                   │
   └───────▲───────────────────────────────▲────────────────────────┘
           │ depends + pub use(re-export)   │ depends(直链,不经 attune-core)
           │                                │
   ┌───────┴────────────┐        ┌──────────┴──────────────────────┐
   │   attune-core       │        │  law-pro / patent-pro 确定性     │
   │  pub use sdk::{      │        │  agent bin(attune-pro 仓后续)   │
   │    Agent, AgentOutput}│       │  use attune_agent_sdk::{Agent,  │
   │  From<AgentError>     │        │      AgentOutput, AgentError}   │
   │    for VaultError     │        │       │ target wasm32-wasip1    │
   │  + flow/registry/...  │        │       ▼                        │
   │  (DB/net 留这,native) │        │   ✓ <id>.wasm 产出             │
   └─────────▲────────────┘        └─────────────────────────────────┘
             │ native 调用方(server/CLI)无感:
             │ attune_core::agents::Agent 路径仍可用(re-export)
   ┌─────────┴────────────────────────────────────────────────────┐
   │ attune-server / CLI / capability_dispatch(WasmRunner / 直调)   │
   └────────────────────────────────────────────────────────────────┘
```

### 3.2 运行期数据流(不变)

leaf crate 抽取**不改运行期数据流**。确定性 agent 仍走前置 spec 定义的 `capability_dispatch::dispatch_capability`:`runtime: wasm` → `WasmRunner`(stdin JSON → stdout `AgentOutput` JSON → exit 0/1/2)。leaf crate 只决定 agent bin **能否编成 wasm**,不参与执行期宿主侧逻辑。

### 3.3 不涉及 DB / cache / API 协议变更

纯 crate 重构。无 DB schema 变更、无 cache 层变更、无对外 REST/WS 协议变更。`AgentOutput<T>` 的 JSON wire 形态逐字节不变(同 6 字段、同字段名、同 serde derive)。

---

## 4. 模块边界

| crate / file | 变更 | 说明 |
|--------------|------|------|
| `rust/crates/attune-agent-sdk/`(新 crate) | 新 | leaf crate。`src/lib.rs` = `Agent` trait + `AgentOutput<T>` + `AgentError`/`AgentResult`;`Cargo.toml` deps 仅 `serde`(+ 评审定 `thiserror`)。`version` 独立节奏(首版 `0.1.0`,per §1.1.8 插件/子 crate 版本独立)。 |
| `attune-core/src/agents/mod.rs` | 改 | 删除本地 `Agent` trait + `AgentOutput<T>` 定义体;改为 `pub use attune_agent_sdk::{Agent, AgentOutput};`。保留 `flow`/`flow_runner`/`registry`/`scheduler`/`document_classifier` 子模块 + `locate_workspace_file`/`load_workspace_flows`(这些留 attune-core)。 |
| `attune-core/src/error.rs` | 改 | 新增 `impl From<attune_agent_sdk::AgentError> for VaultError`(map 到 `InvalidInput`/`Classification` 等已有变体),使内部 agent 实现可继续返回 `crate::error::Result`。 |
| `attune-core/Cargo.toml` | 改 | 新增 `attune-agent-sdk = { path = "../attune-agent-sdk" }`(workspace member)。 |
| `rust/Cargo.toml`(workspace) | 改 | `members` 增 `crates/attune-agent-sdk`;`[workspace.dependencies]` 登记 `attune-agent-sdk`。 |
| `attune-core/src/agents/document_classifier.rs` 等内部 agent | 评估 | 若内部 agent 实现 `Agent` trait,确认 `run` 签名仍兼容 re-export 后的 trait(签名不变,应零改动)。 |
| **attune-pro/plugins/law-pro 等(跨仓,后续)** | 后续 | law-pro `Cargo.toml` 加 `attune-agent-sdk` dep;确定性 bin/lib 把 `use attune_core::agents::{Agent,AgentOutput}` → `use attune_agent_sdk::{Agent,AgentOutput}`,`use attune_core::error::{Result,VaultError}` → `use attune_agent_sdk::{AgentResult, AgentError}`。**不在本 spec 提交,仅定契约。** |

**跨仓边界**:本 spec 在 OSS attune 仓产出 leaf crate + re-export;attune-pro 各 vertical 按 leaf crate 契约改链 + 产 `.wasm`。OSS 仓不含任何 vertical 业务逻辑或 `.wasm` 产物(三产品矩阵边界保持)。

---

## 5. API 契约(leaf crate `attune-agent-sdk`)

### 5.1 `Agent` trait(签名与现状逐字一致,仅换 crate)

```rust
pub trait Agent {
    type Input;
    type Output;
    fn id(&self) -> &str;
    fn description(&self) -> &str;
    fn case_kinds(&self) -> &[&str];
    fn run(&self, input: Self::Input) -> AgentResult<AgentOutput<Self::Output>>;
}
```

> `run` 返回类型由 `attune_core::error::Result` 改为 leaf 本地 `AgentResult`(= `Result<_, AgentError>`)。attune-core 通过 `From<AgentError> for VaultError` 在 `?` 边界自动桥接,内部调用方不感知。

### 5.2 `AgentOutput<T>`(6 字段,JSON wire 不变)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutput<T> {
    pub computation: T,
    pub audit_trail: String,
    pub red_lines_violated: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub followups: Vec<String>,
    pub confidence: f64,
}
impl<T> AgentOutput<T> {
    pub fn has_red_lines(&self) -> bool;
    pub fn needs_attention(&self) -> bool;
}
```

### 5.3 wasm-safe 错误类型

```rust
#[derive(Debug)]               // thiserror 可选,需确认其 wasm-safe(纯 proc-macro,应安全)
pub enum AgentError {
    InvalidInput(String),      // 对应 native VaultError::InvalidInput
    Computation(String),       // 计算/业务内部错
    Serialization(String),     // serde_json 失败的字符串化(leaf 不依赖 serde_json 具体类型,存 String)
    RedLine(String),           // 业务红线触发(映射 exit code 2)
}
pub type AgentResult<T> = Result<T, AgentError>;
```

> 关键:leaf 的 `AgentError` **不含** `#[from] rusqlite::Error` / `serde_yaml::Error`(这是现 `VaultError` 不可 wasm 的根因)。序列化错误以 `String` 承载,leaf 不在签名里暴露 native crate 类型。

---

## 6. 扩展点

- **新 vertical 确定性 agent**:任何插件只需 `attune-agent-sdk = "0.1"` + impl `Agent`,即可编 wasm,无需碰 attune-core。
- **错误变体扩展**:`AgentError` 新增变体时同步在 attune-core 的 `From<AgentError> for VaultError` 补 arm(`#[non_exhaustive]` 标注 `AgentError` 以允许向后兼容增变体)。
- **exit-code 映射扩展点**:agent bin 的 `main` 把 `AgentResult` → process exit(0/1/2)的逻辑可后续抽一个 leaf 内的 `run_agent_main()` helper(本 spec 不做,标记为未来可选,避免 scope creep)。

---

## 7. 错误 + 边界 case

| case | 处理 |
|------|------|
| leaf crate 误引入 native dep | CI 加 `cargo build -p attune-agent-sdk --target wasm32-wasip1` 守卫,引入即红 |
| attune-core 内部 agent 实现 `run` 返回 `AgentError` | `From<AgentError> for VaultError` 在调用方 `?` 处自动转,无 panic |
| `serde` feature 漂移 | leaf `serde` 用 `default-features=false` + 显式 `features=["derive","alloc"]`(wasm 无 std 分配器假设时仍安全;wasip1 有 std,保守显式) |
| `thiserror` 若不 wasm-safe | 降级为手写 `impl std::fmt::Display + std::error::Error`(评审决策点) |
| re-export 后路径冲突 | `attune_core::agents::Agent` 与新 `attune_agent_sdk::Agent` 必须是**同一类型**(re-export 而非重定义),否则 trait 不兼容 → 抽取即重定义会破坏 attune-pro 现有 `impl Agent for ...` |
| 孤儿规则(orphan rule) | `From<AgentError> for VaultError` 在 attune-core 定义(VaultError 是本地类型),合法;反向 `From<VaultError> for AgentError` **不能**在 leaf 定义(VaultError 非 leaf 本地类型)→ 不需要该方向 |

---

## 8. 成本契约

| 维度 | 影响 |
|------|------|
| 编译(native) | leaf crate 极小(2 个类型 + serde),增量编译几乎无感;attune-core 少了本地 trait 定义,净中性 |
| 编译(wasm) | leaf `cargo build --target wasm32-wasip1` 应秒级;pilot agent bin wasm 产物预估 < 数百 KB(纯计算 + serde,无 native runtime) |
| 二进制体积 | native attune-core 二进制不变(re-export 零运行期开销);attune-pro 确定性 agent 改为单份 `.wasm` 替代 N 份平台 native 二进制 → 分发体积净降(一包通吃) |
| 运行期成本 | 🆓 零成本层(确定性计算,毫秒级);本 spec 不改成本层归属 |

---

## 9. 测试矩阵

| 类型 | 下限 | 内容 |
|------|------|------|
| leaf wasm 编译守卫 | 1 | `cargo build -p attune-agent-sdk --target wasm32-wasip1` 干净(CI 新增 job) |
| pilot agent wasm 编译 | 1 | attune-pro `agent_limitation_check` 链 leaf 后 `cargo build --target wasm32-wasip1` 出 `.wasm`(跨仓判据,attune-pro 侧验,本 spec 只描述判据) |
| Golden case | 复用现有 | `AgentOutput` serde roundtrip(已存 mod.rs tests)迁入 leaf;`agent_golden_gate.rs` / `agent_quality.rs` 不回归(1.00 deterministic) |
| 属性测试 | ≥3 | `AgentOutput<T>` serde roundtrip proptest(任意 T=Value / 任意字段长度);`needs_attention`/`has_red_lines` 逻辑不变量 |
| 边界 | ≥5 | 空 computation、超长 audit_trail、confidence 边界(0.0/1.0/NaN 拒绝)、空/满 red_lines、Unicode audit_trail |
| 异常 / 错误 | ≥3 | `AgentError` 各变体 Display 文案;`From<AgentError> for VaultError` 映射正确;序列化失败转 String |
| 集成 E2E | ≥1 | attune-core 链 leaf 后 `cargo test -p attune-core` 全过(re-export 路径可用);现有 211+ tests 不回归 |
| 回归 fixture | 每 bug +1 | 抽取过程若 break 任何现有 test,加 reproducer 入 leaf golden |

**JSON wire 不变验证(关键)**:同一 `AgentOutput` 值,抽取前后 `serde_json::to_string` 输出逐字节相等(防字段顺序/命名漂移破坏现有 wasm/subprocess 契约)。

---

## 10. 向后兼容

1. **`attune_core::agents::{Agent, AgentOutput}` 路径保持**:经 `pub use` re-export,现有 attune-core/attune-server/CLI/内部 agent 调用零改动。
2. **同一类型保证**:re-export(非重定义)确保 `attune_core::agents::Agent` 与 `attune_agent_sdk::Agent` 是同一 trait → attune-pro 现有 `impl attune_core::agents::Agent for CivilLoanAgent` 在过渡期仍编译(指向同一 trait)。
3. **attune-pro 渐进迁移**:attune-pro 可分两步 —— (a) 先随 attune 1.1.0 升级 dep,确定性 bin **可继续**用 `attune_core::agents::*`(re-export 仍可用,只是仍编不了 wasm);(b) 真要编 wasm 时再把 import 切到 `attune_agent_sdk::*` + 移除对 attune-core 的链接。两步互不阻塞,无 flag-day。
4. **`error::Result` 兼容**:attune-core 内部 agent 仍可返回 `crate::error::Result`(VaultError),靠 `From<AgentError>` 桥接;attune-pro 确定性 bin 迁移后改用 `AgentResult`。
5. **JSON schema 版本**:`AgentOutput` wire format 不变 → 不需 schema versioning / migration。老 client(已装的 subprocess agent)输出格式不变。
6. **crate 版本**:leaf `attune-agent-sdk 0.1.0` 独立 SemVer(per §1.1.8);attune-core 1.1.0 强配对主仓。leaf 首发随 attune 1.1.0 一起 tag。

---

## 11. 风险登记

| # | 风险 | 缓解 |
|---|------|------|
| R1 | **循环依赖**:leaf 反向依赖 attune-core | 严守 leaf = 叶子,只依赖 serde;`From<AgentError> for VaultError` 在 attune-core 侧定义,方向单一(core→leaf),无环 |
| R2 | **孤儿 trait / 重定义破坏 impl**:抽取时若重定义 trait 而非 re-export,attune-pro 现有 `impl Agent` 失效 | 强制 `pub use`(同一类型);CI 跑 attune-pro 编译(跨仓 smoke)确认 impl 仍 resolve |
| R3 | **serde 版本不一致**:leaf 与 attune-core 用不同 serde minor → `AgentOutput` 跨 crate 类型不兼容 | leaf serde 走 `[workspace.dependencies]` 统一版本,不独立 pin |
| R4 | **thiserror wasm 可疑** | 评审定:能则用(纯 proc-macro 应安全),否则手写 Display/Error;不阻塞抽取 |
| R5 | **与 attune WASM runtime release sequencing**:leaf 必须随含 WASM runtime 的 attune 1.1.0 一起发,否则 attune-pro 无 runner 可跑 wasm | leaf 0.1.0 + attune-core 1.1.0 同 tag 发布;RELEASE.md 标注 "确定性 agent wasm 化前置:需 attune ≥ 1.1.0 + attune-agent-sdk 0.1" |
| R6 | **隐性 native 依赖回流**:未来有人给 leaf 加 `use serde_yaml`/`chrono` 等 | CI wasm 编译守卫(§7)+ leaf `Cargo.toml` deny-list 注释;review SOP 检查 leaf deps 增项 |
| R7 | **跨仓漂移**:attune-pro 迟迟不迁,leaf 沦为死代码 | attune-pro 仓建迁移 task(blockedBy attune 1.1.0 tag);本 spec §10.3 两步迁移降低 attune-pro 改动压力 |
| R8 | **内部 agent(document_classifier 等)签名隐性依赖 VaultError** | 抽取前 grep 内部 `impl Agent` 的 `run` 返回类型;若依赖 VaultError,经 `From` 桥接,trait 签名用 `AgentResult` 不破坏(`?` 自动转) |
