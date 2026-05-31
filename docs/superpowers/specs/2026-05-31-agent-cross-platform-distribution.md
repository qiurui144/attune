# Spec A — Agent 跨平台分发(WASM runtime 接入)

> 状态:草案待评审 · 作者:架构设计 agent · 日期:2026-05-31
> 关联代码(已调研真实代码,非臆断):
> - `rust/crates/attune-core/src/capability_dispatch.rs` — agent/skill 真正执行入口(`dispatch()` subprocess + stdin/stdout JSON)
> - `rust/crates/attune-core/src/plugin_loader.rs:49` `PluginManifest` / `SkillSpec` / `AgentSpec`(`runtime` 字段已声明 `rust_binary|wasm|python_subprocess`,但仅 `rust_binary` 有实现)
> - `rust/crates/attune-core/src/plugin_registry.rs:255` `PluginRegistry::scan`(无版本兼容 gate)
> - `rust/crates/attune-core/src/plugin_sync.rs` `install_plugin_package`(解 tar → `copy_dir_recursive`,平台无关二进制直接落盘)
> - `rust/crates/attune-server/src/routes/marketplace.rs:131` 安装链路
> - `rust/crates/attune-core/tests/oss_agent_real_llm_gate.rs` / `agent_quality.rs`(golden gate 现状)

---

## 0. 调研结论(现状基线,设计前提)

| 维度 | 现状(实测代码) | 跨平台影响 |
|------|----------------|-----------|
| 执行模型 | `capability_dispatch::dispatch()`:`Command::new(binary).spawn()`,stdin 喂 JSON,stdout 收 JSON,exit code 0/1/2 | 仅能跑当前 OS+ISA 的 native ELF/PE |
| binary 解析 | `resolve_binary(plugin_dir, id)` → `bin/run_<id>` / `target/release/run_<id>` / `which` | 包里 `bin/` 是单平台编译产物 |
| manifest.runtime | `SkillSpec.runtime` / `AgentSpec.runtime` = `String`,doc 列三值;代码**只 dispatch rust_binary**,`wasm`/`python_subprocess` 无分支 | wasm 是"声明了没实现"的死字段 |
| 版本字段 | `PluginManifest.version`(plugin 自身版本);**无 `min_attune_version`** | 无法拒载不兼容包 |
| scan gate | `scan` 仅解析 + 验签(`plugin_sig.rs`),**无版本/平台兼容校验** | 装了跑不了直到调用才 NotFound |
| 安装 | `install_plugin_package` 解 tar(纯 Rust gzip 防穿越)→ `copy_dir_recursive` 保留 Unix 权限 | 单包内只有一平台 binary |
| wasm runtime dep | `grep wasmtime/wasmer/wasm32` → **0 命中**,workspace 无 wasm 依赖 | 需新引入 wasmtime |

**核心结论**:当前 `.attunepkg` **不能**保证所有人兼容。`runtime: rust_binary` + `bin/<编译产物>` 是平台锁定;x86_64-linux 二进制无法在 Windows(P0)/ macOS / riscv64 K3(P2)运行。用户问"attunepkg 能确保所有人兼容吗"——当前答案是不能。本 spec 设计 WASM runtime 接入,使**一个包通吃所有平台**。

---

## 1. 目标定位

**用户痛点**:law-pro 等 vertical 插件的确定性 agent(本金/利率计算、案号结构化、专利权利要求抽取等纯计算)以 `runtime: rust_binary` 编译进 `.attunepkg`,产物是平台相关二进制。一个包只能在编译它的那个 OS+ISA 上跑。结果:

- attune 主推 Windows(P0),但插件 CI 若在 Linux 编译 → Windows 用户装上去 `dispatch` 直接 `NotFound`。
- K3 一体机(riscv64 RVA23)与笔电(x86_64)需要各自不同的包。
- 第三方/社区插件作者要为每个平台分别交叉编译 + 分别签名 + 分别上架,门槛极高。

**目标**:确定性 agent/skill 编译到 `wasm32-wasip1`,由 attune-core 内嵌 wasmtime 执行。**一个 `.attunepkg` 含一份 `.wasm` 即在所有目标平台运行**,与现有 subprocess JSON 契约对齐(stdin JSON → stdout JSON → exit code)。同时补齐 `min_attune_version` 强制校验,让版本不兼容在**加载期**被清晰拒绝而非运行期崩。

**与产品 positioning 对齐**:
- 服务北极星"插件自主流转能力"(ACP-5):跨平台是流转的物理前提。
- 符合成本契约:wasm 执行属 🆓零成本/⚡本地算力层(纯 CPU 计算,无 LLM),不破坏"建库不升级到第三层"。
- 符合 OSS↔pro 边界:wasm sandbox 比 subprocess 更强隔离,attune-core 仍不 link 插件代码。

---

## 2. 范围边界

### 本 spec 做(v1.1.0 目标切片)

1. `wasm` runtime 在 `capability_dispatch` 落地:`runtime: wasm` 的 skill/agent 经 wasmtime 执行,契约与 subprocess 对齐。
2. 定义 wasm agent 的 stdin/stdout JSON 契约 + WASI 能力声明(`wasi_caps`)。
3. `PluginManifest` 增 `min_attune_version`,`scan`/install 期强制校验,不兼容→拒载+清晰提示。
4. manifest 增 `runtime: data_only`(纯 prompt+JSON schema,无二进制)作为第三种跨平台方案的形式化。
5. 兼容矩阵分析 + 现有 rust_binary agent 迁移路径(哪些先转 wasm)。
6. wasm 执行的测试矩阵(确定性 golden 对齐 native 基线)。

### 本 spec 不做(写死,禁止 silent scope creep)

- **不删除 `rust_binary` runtime**。它对需要原生性能/系统调用的 capability(如 OCR 预处理调 poppler、调系统 GPU)继续保留,作为"平台分包"路径。
- **不实现 `python_subprocess`**(仍是声明未实现;K3/笔电不预装 python runtime,本 spec 不碰)。
- **不做 wasm 内调用 LLM**。wasm agent 是确定性纯计算;LLM agent 仍走 attune-core 宿主侧(现有 `agents/` LLM lane),不进 wasm。
- **不做 wasm component model / WIT 接口**(v1.1 用 WASI preview1 + stdin/stdout 字节流,最小可用;component model 推 v1.x)。
- **不做 wasm 多线程/SIMD 加速**(单线程足够确定性计算;perf 优化推后)。
- **不做插件市场跨平台 UI 重构**(marketplace 安装链路只加版本 gate,不改 UI 形态;UI 推 v1.0.10)。
- **不强制全部存量 agent 立即迁 wasm**(迁移按 §10 分批,rust_binary 与 wasm 长期共存)。

---

## 3. 架构数据流

### 3.1 安装 → 加载 → 执行(ASCII)

```
                       ┌─────────────────────────── .attunepkg (一包通吃) ───────────────────────────┐
                       │  plugin.yaml(min_attune_version: "1.1.0")                                  │
                       │  wasm/<id>.wasm        ← wasm32-wasip1,平台无关,一份                         │
                       │  bin/run_<id>          ← (可选) rust_binary 平台产物,仅 native-only cap       │
                       │  prompt.md / schema    ← data_only agent 用                                  │
                       └──────────────────────────────┬───────────────────────────────────────────┘
                                                       │ marketplace install / plugin-install CLI
                                                       ▼
        plugin_sync::install_plugin_package  ── 解 tar(纯 Rust gzip,防穿越)→ copy_dir_recursive
                                                       │
                                                       ▼  ~/.local/share/attune/plugins/<id>/
        PluginRegistry::scan ──┬─ 验签 (plugin_sig.rs)
                               ├─ ★新: version gate  min_attune_version ≤ ATTUNE_VERSION ?
                               │      否 → skip + 收集到 incompatible[],返回给 UI 提示(不 panic)
                               └─ 解析 manifest(runtime 字段 per skill/agent)
                                                       │ 调用时(chat handler / agents route)
                                                       ▼
        capability_dispatch::dispatch_capability(spec, input_json)
                               │
              ┌────────────────┼──────────────────────────────┐
              ▼ rust_binary    ▼ wasm                          ▼ data_only
   现有 subprocess        ★新 wasm lane                  无执行体,宿主侧组合
   Command::new(bin)     WasmRunner::run:               prompt + schema 交 LLM lane
   stdin JSON→stdout     wasmtime Engine/Store          (本 spec 仅形式化声明)
   exit 0/1/2            stdin pipe = input_json
                         stdout pipe → output_json
                         WASI: 默认无 fs/net,按 wasi_caps 显式授
                         exit code 经 Proc exit / trap 映射
              └──────────────┬───────────────────────────────┘
                             ▼
              CapabilityResult { exit_code, stdout(JSON), stderr(audit), timed_out }
                             ▼  调用方按 plugin schema 解析(同今天)
```

### 3.2 契约统一原则

wasm lane 与 subprocess lane **共用** `CapabilityResult` 输出结构与 exit code 语义(0 成功 / 1 错误 / 2 业务红线 / -1 超时)。调用方代码无需感知 runtime 差异——`dispatch_capability` 内部按 `spec.runtime` 分流。这保证现有 `agents.rs` route / golden gate harness 不因 runtime 切换而改写。

### 3.3 不涉及 DB / cache 变更

wasm 执行无状态(确定性输入→输出),可复用现有 skill 输出 cache(`SkillSpec.cacheable` + chunk_hash,见成本契约 CLAUDE.md)。无新 DB table。

---

## 4. 模块边界

| crate / file | 变更 | 说明 |
|--------------|------|------|
| `attune-core/src/capability_dispatch.rs` | 改 | 新增 `runtime` 分流入口 `dispatch_capability`;新增 `wasm` 子模块 `WasmRunner`(wasmtime Engine 复用 + Store per-call + WASI ctx) |
| `attune-core/src/plugin_loader.rs` | 改 | `PluginManifest` 增 `min_attune_version: Option<String>`;`SkillSpec`/`AgentSpec` 增 `wasm: Option<String>`(wasm 相对路径)+ `wasi_caps: Vec<String>` |
| `attune-core/src/plugin_registry.rs` | 改 | `scan` 增 version gate,不兼容 plugin skip 并入 `incompatible: Vec<String>`(scan 返回的第二个 Vec 现已是 warnings,扩展语义) |
| `attune-core/src/version.rs`(新) | 新 | `ATTUNE_VERSION` 常量(从 Cargo.toml env)+ `is_compatible(min: &str) -> bool`(semver 比较) |
| `attune-core/Cargo.toml` | 改 | 新增 `wasmtime`(默认 feature `wasm-runtime`,可关) + `semver` |
| `attune-server/src/routes/marketplace.rs` | 改 | 安装后若 scan 报 incompatible → 返回结构化错误(min_attune_version 不满足) |
| `attune-server/src/routes/agents.rs` | 不变 | 走 `dispatch_capability` 统一入口,无感知 runtime |
| `attune-core/tests/wasm_capability_gate.rs`(新) | 新 | wasm lane golden + 边界 + 错误测试 |
| `attune-pro/plugins/<vertical>/` | 后续 | 实际把确定性 agent 编译 wasm(本 spec 不在 OSS 仓做,仅定契约) |

**跨仓边界**:本 spec 在 OSS attune 定义 runtime 契约 + WasmRunner;attune-pro 各 vertical 按契约产 `.wasm`。OSS 仓不含任何 vertical wasm 产物(边界保持)。

---

## 5. API 契约

### 5.1 manifest runtime 字段(plugin.yaml)

```yaml
# skill / agent 条目
- id: extract_loan_terms
  runtime: wasm                 # rust_binary | wasm | python_subprocess | data_only
  wasm: wasm/extract_loan_terms.wasm   # runtime=wasm 时必填,相对 plugin dir
  binary: bin/run_extract_loan_terms   # runtime=rust_binary 时必填(现状)
  wasi_caps:                    # 默认空 = 纯计算无 fs/net;按需显式声明
    - "stdio"                   # 默认隐含(stdin/stdout)
    # - "read:/tmp/attune-scratch"   # 需读临时目录(罕见)
    # - "clock"                       # 需 wall-clock
  cacheable: true

# plugin 级
min_attune_version: "1.1.0"     # ★新:低于此 attune 版本拒载
```

校验规则:
- `runtime: wasm` 而 `wasm` 字段缺失 → 加载期 `InvalidInput`。
- `wasi_caps` 含未知能力字符串 → 加载期拒绝(白名单:`stdio` / `clock` / `read:<path>` / `env:<KEY>`;**默认无 net、无任意 fs 写**)。
- `min_attune_version` 非合法 semver → 加载期拒绝。

### 5.2 wasm agent stdin/stdout JSON 契约(与 subprocess 对齐)

**输入(stdin,UTF-8 JSON)**——与现有 subprocess agent 输入同 schema:
```json
{
  "facts": { "...": "agent 输入事实,plugin schema 定义" },
  "context": { "locale": "zh-CN", "case_kind": "civil-loan" }
}
```

**输出(stdout,UTF-8 JSON)**:
```json
{
  "ok": true,
  "result": { "...": "plugin schema 定义的业务输出" },
  "audit_trail": ["step1: ...", "step2: ..."],
  "red_lines_violated": []
}
```

**exit code 映射**(wasm `_start` 退出码 / proc_exit):
| code | 语义 | 等同 subprocess |
|------|------|-----------------|
| 0 | 成功,stdout 为合法 result JSON | `is_success()` |
| 1 | 一般错误(输入非法 / 内部错),stderr 人类可读 | 错误 |
| 2 | 业务红线触发(hard_red_lines),stdout 含 `red_lines_violated` | `is_red_line()` |
| trap/panic | wasm 陷阱 → 宿主映射 exit_code = 1 + stderr 记 trap 原因 | NotFound/Io 类 |
| 超时(宿主 epoch interrupt) | exit_code = -1,timed_out = true | 同 subprocess timeout |

### 5.3 attune-core 内部 API

```rust
// capability_dispatch.rs
pub enum CapabilityRuntime { RustBinary, Wasm, DataOnly }   // python_subprocess 暂 unsupported→Err
pub fn dispatch_capability(
    plugin_dir: &Path, runtime: CapabilityRuntime,
    entry: &str,            // binary 或 wasm 相对路径
    invocation: &CapabilityInvocation,
) -> Result<CapabilityResult>;

// version.rs
pub const ATTUNE_VERSION: &str;          // env!("CARGO_PKG_VERSION") 来源
pub fn is_compatible(min_attune_version: &str) -> Result<bool>;
```

`CapabilityInvocation`(现有 stdin/env/timeout 复用)对 wasm 的语义:`stdin`→wasm stdin pipe,`env`→按 `wasi_caps` 中 `env:<KEY>` 白名单过滤后注入,`timeout`→wasmtime epoch deadline。

---

## 6. 扩展点

1. **新 runtime**:`CapabilityRuntime` enum 加分支即可(未来 `python_subprocess` 真实现 / wasm component model 走此扩展)。
2. **WASI 能力**:`wasi_caps` 白名单可扩(如 `read:<path>` 之外加 `random`),宿主 `WasiCtxBuilder` 集中翻译,plugin 声明驱动。
3. **新平台**:wasm 路径对新 ISA(如 LoongArch)零改动——wasmtime 支持的宿主即支持;native-only cap 才需补平台分包。
4. **签名链**:wasm 产物纳入现有 `plugin_sig.rs` 签名范围(整包签),无需新签名机制。
5. **版本兼容策略**:`is_compatible` 现仅 `min ≤ current`;未来可扩 `max_attune_version` / feature-flag 协商。

---

## 7. 错误 + 边界 case

| case | 处理 | 错误码(kebab) |
|------|------|----------------|
| `runtime: wasm` 但 `wasm` 字段缺 | 加载期拒载 | `wasm-entry-missing` |
| `.wasm` 文件不存在 / 非法 module | dispatch 返回 Err | `wasm-module-invalid` |
| wasm 执行 trap(越界 / unreachable / OOM) | 捕获 → exit_code=1,stderr 记 trap | `wasm-trap` |
| wasm 超时(死循环) | epoch interrupt → kill,exit_code=-1,timed_out | `wasm-timeout` |
| wasm 试图打开未授权 fs/net | WASI 默认拒,wasm 侧得到 errno → 业务自处理或 trap | `wasi-cap-denied` |
| stdout 非合法 JSON | dispatch 仍返回 raw stdout(同 subprocess,调用方解析失败自报) | 调用方 `output-parse-error` |
| `min_attune_version` > 当前 attune | scan skip,收集到 incompatible,UI 提示"需升级 attune 至 ≥ X.Y.Z" | `plugin-incompatible-version` |
| `min_attune_version` 非法 semver | 加载期拒载 | `invalid-min-version` |
| wasm 内存超 256MB(默认 limit) | StoreLimits 拒绝增长 → trap | `wasm-memory-limit` |
| 空 stdin / 空 facts | wasm 侧按业务返回 ok=false 或 red_line(同 subprocess) | 业务定义 |
| 老包无 `min_attune_version` 字段 | `Option=None` → 视为兼容(向后兼容,见 §10) | — |

边界硬约束:wasm Store 必须设 `epoch_deadline`(超时)+ `StoreLimits`(内存上限),防恶意/失控插件拖垮宿主。每次调用 fresh Store(无跨调用状态泄漏)。

---

## 8. 成本契约

| 维度 | wasm runtime | rust_binary(对照) |
|------|--------------|---------------------|
| 编译期(插件作者) | `cargo build --target wasm32-wasip1`,一次产一份 | 每平台各编译一次(N 份) |
| 包体积 | 单 `.wasm` 典型 0.5–3 MB(确定性计算,无大依赖) | 每平台 native binary 1–5 MB × N |
| 加载开销 | wasmtime `Module::new` 编译 ~10–100ms(可 `Engine` 级缓存 cranelift 产物到磁盘,二次秒级) | 进程 spawn ~1–5ms |
| 单次执行 | 接近 native(cranelift JIT),纯计算差距 < 2×;无进程 fork 开销 | native 全速 + fork 开销 |
| 内存 | Store linear memory 上限 256MB(可配),engine 常驻 ~数 MB | 子进程独立地址空间 |
| 算力归属 | 🆓零成本 / ⚡本地算力(纯 CPU,无 LLM,毫秒–秒级) | 同 |

**成本结论**:wasm 把插件作者的 N 平台编译成本降到 1 份;运行期开销对确定性计算可忽略(< 2× native,绝对值毫秒级)。属成本契约第 1–2 层,不触发"必须用户显式触发"的第 3 层。首次加载 JIT 编译开销用 wasmtime 磁盘缓存(`Engine` cache config)摊销。`wasm-runtime` 设为可关 feature——K3/极小镜像若只用 native cap 可编译期剔除 wasmtime 减体积。

---

## 9. 测试矩阵(per §6.1 六类下限 + Agent 验证铁律)

| 类型 | 下限 | 用例 |
|------|------|------|
| **golden(确定性对齐)** | wasm 产物对同一 golden set 输出**逐字节等于** native rust_binary 基线 ≥10 case | 取一个已迁移确定性 skill(如 loan_terms 计算),native 与 wasm 双跑 diff = 0 |
| **属性测试** | ≥3 proptest | 随机合法 facts → wasm 与 native 输出一致;不 panic;exit code ∈ {0,1,2} |
| **边界** | ≥5 `#[test]` | 空 stdin / 超大 stdin(10MB)/ 内存 limit 触发 / 超时(死循环 wasm fixture)/ 非法 .wasm 文件 |
| **错误/异常** | ≥3 | trap fixture / proc_exit(2) 红线 / WASI cap denied(试开未授权文件) |
| **集成 E2E** | ≥1 | 真 `.attunepkg`(含 wasm)→ install_plugin_package → scan → dispatch_capability → 验 result;含 min_attune_version gate(装一个声明 min=99.0.0 的包验证被拒) |
| **回归 fixture** | 每修一 bug +1 | 进 golden set,ratchet 只升不降 |
| **跨平台 CI** | wasm lane 在 Linux + Windows runner 都跑 golden(证明"一包通吃") | CI matrix:同一 `.wasm` 在两 OS 输出一致 |
| **兼容矩阵(LLM 不涉及)** | wasm agent 是确定性,无需 3-tier LLM;但若 agent 内部经宿主回调 LLM(本 spec 不做)则适用 §4.5 | n/a |

**Agent 验证铁律对齐**:wasm agent 进 `agent_golden_gate` 等价 harness,deterministic lane pass rate = 1.00。golden ground truth **独立计算**(不调 agent 自身),per CLAUDE.md 反模式禁令。

---

## 10. 向后兼容

| 场景 | 行为 |
|------|------|
| 现有 `runtime: rust_binary` 插件 | **完全不变**。`dispatch_capability` 的 RustBinary 分支即现 `dispatch()`,路径/契约不动 |
| 老包无 `min_attune_version` | `Option=None` → 视为兼容,正常加载(不强制存量包改写) |
| 老 `scan` 调用方 | `scan` 签名不变(已返回 `(Self, Vec<String>)`,第二 Vec 扩展为含 incompatible 提示),调用方收到更多 warning string,无破坏 |
| 新 attune 装老插件 | 老插件 min_version 缺或更低 → 兼容 |
| 老 attune 装新插件(声明 min > 老) | scan 期被拒 + 清晰提示升级;**不会**装上去运行期崩 |
| `python_subprocess` runtime | 仍未实现;dispatch 遇到返回明确 `unsupported-runtime` Err(而非 silent NotFound) |

**迁移路径(rust_binary → wasm,分批)**:

1. **首批迁 wasm(纯计算确定性 agent)**:loan_terms 计算 / case_no 结构化 / patent_claims 抽取 / 各 vertical 的本息·利率·期限计算类。判据:无系统调用、无大 native 依赖、纯 JSON→JSON。
2. **保留 rust_binary(native-only)**:依赖 poppler/OCR 预处理、调系统 Chrome(chromiumoxide)、需原生性能的重计算。这些走"平台分包"——若需跨平台再单独评估。
3. **data_only 候选**:纯 prompt + JSON schema、无任何计算逻辑的 agent(逻辑全在 LLM lane)→ 标 `runtime: data_only`,天然跨平台,无任何二进制。
4. **迁移不阻塞发版**:一个 vertical 内 wasm 与 rust_binary agent 可共存;按 agent 粒度逐个迁,每迁一个跑 §9 golden diff=0 才合入。

**三方案对比(回答"如何确保所有人兼容")**:

| 方案 | 跨平台 | 包体积 | 作者成本 | 性能 | 适用 | 推荐 |
|------|--------|--------|----------|------|------|------|
| **WASM(一包通吃)** | ✅ 全平台 | 单份小 | 编译 1 次 | 接近 native | 确定性纯计算 agent | ★ 主推 |
| 平台分包(rust_binary × N) | △ 需 N 包 | N 份 | 交叉编译 N 次 + N 签名 | 全速 native | native-only(OCR/系统调用) | 仅保留 native-only |
| data_only(无二进制) | ✅ 全平台 | 最小 | 零编译 | 取决 LLM | 逻辑全在 prompt/LLM | 纯 prompt agent |

---

## 11. 风险登记

| # | 风险 | 等级 | 缓解 |
|---|------|------|------|
| R1 | wasmtime 引入增大主二进制体积(~数 MB)+ 编译时间 | 中 | 设 `wasm-runtime` feature(默认开,极小镜像可关);wasmtime cranelift 仅宿主侧,不进 wasm |
| R2 | 确定性不一致:wasm 浮点/取整与 native 结果不逐字节相同 | 高 | §9 golden diff=0 强制;金额计算用定点/整数分(避免 f64);CI 双平台 diff 守卫;**不一致即 block 迁移** |
| R3 | 恶意插件 wasm 死循环 / 内存炸 | 中 | epoch deadline 超时 + StoreLimits 内存上限 + 默认无 fs/net;每调用 fresh Store |
| R4 | WASI preview1 能力受限(无网络、有限 fs),某些 agent 迁不动 | 中 | 这类 agent **不迁**,保留 rust_binary(§10 分类);WASI 能力按需 `wasi_caps` 显式授,不默认开 |
| R5 | min_attune_version gate 误伤:存量包无字段被当兼容,实际不兼容 | 低 | None=兼容是向后兼容必需;真不兼容靠 plugin 自身 runtime 报错兜底;未来 strict 模式可要求字段必填 |
| R6 | 首次 JIT 编译延迟(冷启 ~100ms)影响交互 | 低 | Engine 级磁盘缓存 cranelift 产物,二次加载秒级;属本地算力层,用户有 loading 反馈 |
| R7 | wasm32-wasip1 工具链 / wasmtime 版本漂移导致包失效 | 中 | manifest `min_attune_version` 锁兼容窗口;wasmtime 版本在 RELEASE.md 声明;wasm ABI 用稳定的 wasip1(非 unstable component model) |
| R8 | 包内同时含 wasm + bin 但平台错配,作者误标 runtime | 低 | 加载期校验 `runtime` 与字段一致性;CI lint 插件包 |
| R9 | 跨仓:attune-pro vertical 未按契约产 wasm,OSS 契约空转 | 中 | OSS 仓提供 reference wasm fixture + golden harness;attune-pro 迁移列入其 roadmap;契约文档双仓引用 |
| R10 | python_subprocess 仍是死字段,用户误用 | 低 | dispatch 遇到返回明确 `unsupported-runtime`,文档标"未实现" |
