# Implementation Plan A — Agent 跨平台分发(WASM runtime 接入)

> 状态:待评审 · 日期:2026-05-31 · 关联 spec:`docs/superpowers/specs/2026-05-31-agent-cross-platform-distribution.md`(已批准)
> 仓库:`/data/company/project/attune`,Rust 在 `rust/`。完成后本 plan 按 §3.2 立即删。
> **代码复核已做**(2026-05-31):plan 中每个"改哪个文件"均经 grep/cat 核对真实存在(见下「代码现状基线」)。

---

## 0. 代码现状基线(复核结论,非臆断)

| 项 | 现状(实测) | plan 依赖 |
|----|-----------|----------|
| `capability_dispatch.rs::dispatch(&CapabilityInvocation) -> Result<CapabilityResult>` | 存在,subprocess + try_wait 轮询 + timeout kill,exit_code/-1 timeout | 新增 wasm lane 复用 `CapabilityResult`/exit 语义 |
| `CapabilityResult{exit_code,stdout,stderr,timed_out}` + `is_success()`/`is_red_line()` | 存在,字段与 spec §5.2 完全一致 | wasm lane 产同结构,不改 |
| `CapabilityInvocation{binary,args,stdin,env,timeout}` | 存在 | wasm 复用(`binary` 改语义为 wasm 路径,或新增 entry 入口) |
| `resolve_binary(plugin_dir, capability_id) -> Option<PathBuf>` | 存在(`bin/run_<id>` / `target/release/` / PATH) | 新增并列 `resolve_wasm(plugin_dir, id)` |
| `plugin_loader.rs:49 PluginManifest{version:String,...}` | 存在,**无 `min_attune_version`** | 新增字段 |
| `plugin_loader.rs:153 SkillSpec{runtime:String, binary:Option<String>}` / `:188 AgentSpec{同}` | 存在,runtime doc 列三值但仅 rust_binary 实现 | 新增 `wasm:Option<String>` + `wasi_caps:Vec<String>` |
| `plugin_registry.rs:255 scan() -> Result<(Self, Vec<String>)>` / `:259 scan_impl` / `:261 errors:Vec` | 存在,第二 Vec 当前是 errors/warnings | 扩展语义,追加 incompatible 提示 |
| `plugin_sync.rs:176 install_plugin_package` / `:262 extract_tarball`(纯 Rust tar+flate2,防穿越) / `:318 copy_dir_recursive` | 存在 | 不改逻辑;wasm 文件随 tar 落盘,天然带过 |
| `routes/marketplace.rs:59 install_plugin`(`(StatusCode, Json)` tuple-style) | 存在 | 安装后加 scan + incompatible 结构化错误 |
| `version.rs` | **不存在**(需新建);`lib.rs:178` 已有 `env!("CARGO_PKG_VERSION")` | 新建 `version.rs` |
| `attune-core/Cargo.toml` | 有 tar/flate2/which/ort 等;**无 wasmtime/semver** | 新增 `wasmtime`(feature gated)+ `semver` |
| 现有 golden harness(tests/) | `oss_agent_real_llm_gate` / `*_golden_gate` / `*_proptests` / `*_integration`(document_classifier / memory_consolidation / self_evolving_skill / chat_reliability / linker / parse) | 新建 `wasm_capability_gate.rs` 对齐风格 |

**关键决策(影响实现,评审需确认)**:
- D-a:`CapabilityInvocation.binary: PathBuf` 复用为"执行体路径"(wasm 时即 `.wasm` 路径),还是新增 `entry` 抽象?**plan 推荐复用 `binary` 字段**(改 doc 注释,不改类型),减少 churn;`dispatch_capability` 按 `runtime` 决定如何解释该路径。
- D-b:`scan` 第二 Vec 现语义是 error/warning string。plan **不改签名**,incompatible 以 `"[incompatible] <id>: requires attune >= X.Y.Z"` 前缀字符串追加进同一 Vec(spec §10 已认可"扩展语义")。若评审要求强类型(`incompatible: Vec<IncompatiblePlugin>`)→ 需改 scan 签名 + 所有调用方,升级为 D2 子任务。

---

## 1. 日历 / 阶段(每阶段可独立测试 + 独立 commit 批)

> 个人/私有仓 → 按 §5.1 中型 feature 走 develop 直 commit;但本 feature 跨多 day + 跨仓 + 数据(manifest schema)变更 → 用 worktree 隔离(per §5.1 大 feature 分支)。建议 worktree:`feature/agent-wasm-runtime`。

| 阶段 | 主题 | 关键交付 | 可独立测试 | 预估 | 跨仓 |
|------|------|---------|-----------|------|------|
| **D1** | 版本 gate 基础设施(无 wasm 依赖,先落地低风险) | `version.rs` + `PluginManifest.min_attune_version` + `scan` version gate + marketplace 拒载提示 | `version` 单测 + `scan` incompatible 单测 + marketplace 集成 | 1d | 否 |
| **D2** | manifest runtime 字段扩展 + dispatch 分流骨架(wasm 暂 stub) | `SkillSpec/AgentSpec` 增 `wasm`/`wasi_caps`;`CapabilityRuntime` enum;`dispatch_capability` 分流(RustBinary=现 dispatch,Wasm=未启用 feature 时返回 `unsupported-runtime`) | 字段解析单测 + 分流单测(无 wasmtime 也编译) | 1d | 否 |
| **D3** | wasm runtime 真实落地(wasmtime + WASI preview1) | `Cargo.toml` 加 `wasmtime`(feature `wasm-runtime`)+ `semver`;新建 `wasm_runtime.rs`(`WasmRunner`:Engine 复用 + per-call Store + epoch deadline + StoreLimits + wasi_caps→WasiCtx);`dispatch_capability` Wasm 分支接 WasmRunner | wasm fixture(echo/red-line/trap/死循环)单测 + 边界 | 2d | 否 |
| **D4** | pilot agent 转 wasm + golden diff=0 + 跨平台 CI | 选 1 个确定性 reference skill(OSS 仓造 fixture wasm,非 vertical 业务)→ `wasm_capability_gate.rs` golden 对齐 native;CI matrix Linux+Windows 跑同一 `.wasm` 输出一致 | golden diff=0 + proptest + E2E(真 .attunepkg 含 wasm) | 2d | 否(OSS 用 reference fixture) |
| **D5** | 迁移协调 + 文档 + RELEASE | attune-pro 迁移契约文档双仓引用;RELEASE.md 标 wasmtime 版本 + min_attune_version 语义;`docs/<feature>.md` 单主题(若需) | 文档审计(Gate 1) | 1d | **是**(attune-pro 配合,见 §5) |

D1→D2→D3→D4 串行(D2 dispatch 分流依赖 D1 无关但 D3 依赖 D2 字段;D4 依赖 D3 runtime)。D5 与 D4 部分并行(文档可早起草)。

---

## 2. 文件清单(精确到 path)

### D1 — 版本 gate(无 wasm 依赖)
- **新建** `rust/crates/attune-core/src/version.rs`
  - `pub const ATTUNE_VERSION: &str = env!("CARGO_PKG_VERSION");`
  - `pub fn is_compatible(min_attune_version: &str) -> Result<bool>`(用 `semver::Version`/`VersionReq` 比较;非法 semver → `VaultError::InvalidInput`)
- **改** `rust/crates/attune-core/src/lib.rs` — `pub mod version;`(导出)
- **改** `rust/crates/attune-core/src/plugin_loader.rs:49 PluginManifest` — 增 `#[serde(default)] pub min_attune_version: Option<String>`
- **改** `rust/crates/attune-core/src/plugin_registry.rs:259 scan_impl` — 解析 manifest 后调 `version::is_compatible`;不兼容 → `continue`(skip)+ `errors.push(format!("[incompatible] {id}: requires attune >= {min}"))`;非法 semver → push `[invalid-min-version]`
- **改** `rust/crates/attune-core/src/plugin_sync.rs` — `install_plugin_package` 落盘后**不**自行 gate(gate 在 scan;但 install 可选 early-check,plan 放 scan 单点,避免双重逻辑)
- **改** `rust/crates/attune-server/src/routes/marketplace.rs:59 install_plugin` — 安装成功后调用 `PluginRegistry::scan`,若返回 Vec 含 `[incompatible]` 项匹配本 plugin_id → 返回 `(StatusCode::CONFLICT, Json{error:"plugin-incompatible-version", detail})`
- **改** `rust/crates/attune-core/Cargo.toml` — 加 `semver = "1"`(纯 Rust,零跨平台风险)

### D2 — runtime 字段 + dispatch 分流骨架
- **改** `plugin_loader.rs:153 SkillSpec` + `:188 AgentSpec` — 各增:
  - `#[serde(default)] pub wasm: Option<String>`
  - `#[serde(default)] pub wasi_caps: Vec<String>`
  - 加载期校验:`runtime=="wasm" && wasm.is_none()` → Err `wasm-entry-missing`;`wasi_caps` 白名单(`stdio`/`clock`/`read:*`/`env:*`)未知 → Err
- **改** `capability_dispatch.rs` — 新增:
  - `pub enum CapabilityRuntime { RustBinary, Wasm, DataOnly }`(`python_subprocess` 不入 enum;parse 时遇到 → `unsupported-runtime` Err)
  - `pub fn parse_runtime(s: &str) -> Result<CapabilityRuntime>`
  - `pub fn dispatch_capability(plugin_dir, runtime, entry, invocation) -> Result<CapabilityResult>`:RustBinary 分支调现有 `dispatch`(entry→resolve_binary);Wasm 分支 `#[cfg(feature="wasm-runtime")]` 调 WasmRunner,否则返回 `unsupported-runtime` Err;DataOnly 返回明确 Err(无执行体,宿主侧处理)
  - `pub fn resolve_wasm(plugin_dir: &Path, rel: &str) -> Option<PathBuf>`(plugin_dir.join(rel))
- **改** `routes/agents.rs` — 调用点从直接 `dispatch` 改走 `dispatch_capability`(传 spec.runtime + entry);**契约不变**,调用方无感知(spec §3.2)

### D3 — wasm runtime 真实落地
- **新建** `rust/crates/attune-core/src/wasm_runtime.rs`(`#[cfg(feature="wasm-runtime")]`)
  - `pub struct WasmRunner { engine: wasmtime::Engine }`(Engine 进程级复用,可选磁盘 cache config 摊销 JIT)
  - `pub fn run(&self, wasm_path, invocation) -> Result<CapabilityResult>`:per-call `Store`;`WasiCtxBuilder` 按 `wasi_caps` 翻译(默认无 fs/net,stdin pipe=invocation.stdin,stdout/stderr 捕获);`store.set_epoch_deadline` + 后台 epoch ticker(或 `Config::epoch_interruption`)实现 timeout→`timed_out=true,exit_code=-1`;`StoreLimits` 内存上限 256MB;`_start` 退出码 / `proc_exit` → exit_code;trap → exit_code=1 + stderr 记 trap
- **改** `capability_dispatch.rs` — Wasm 分支接 `WasmRunner::run`
- **改** `Cargo.toml` — `wasmtime = { version = "...", optional = true }`;`[features] wasm-runtime = ["dep:wasmtime"]`,默认 features 含 `wasm-runtime`(per spec §8 可关);wasi 用 wasmtime 内置 `wasmtime-wasi`(preview1)
- **改** `lib.rs` — `#[cfg(feature="wasm-runtime")] pub mod wasm_runtime;`

### D4 — pilot + 测试 + CI
- **新建** `rust/crates/attune-core/tests/wasm_capability_gate.rs`(golden + 边界 + 错误 + E2E,见 §6)
- **新建** `rust/crates/attune-core/tests/fixtures/wasm/`(reference `.wasm` fixture + 对应 native 基线产物;OSS 中性 echo/calc,非 vertical 业务)
  - 注:fixture 用预编译 `.wasm` 入库(CI 不强制装 wasm32 工具链),附 build 说明
- **改** `.github/workflows/rust-release.yml`(或 CI test workflow)— matrix 增"wasm golden on Windows runner"步骤:同一 `.wasm` 在 ubuntu + windows 跑 `cargo test --test wasm_capability_gate`,断言输出一致(证明"一包通吃")

### D5 — 文档 + 跨仓
- **改** `rust/crates/attune-server/RELEASE.md`(或仓 RELEASE.md)— v1.1.0 节标 wasmtime 版本(R7)+ min_attune_version 语义 + runtime 取值表
- **新建/改** `docs/<feature>.md` 单主题(若 §1.1.2 白名单需要;否则并入 DEVELOP.md「runtime 契约」节,避免新文件)— runtime 契约 + wasi_caps 白名单 + 迁移分类(供 attune-pro 引用)
- **跨仓**:attune-pro 仓在其 roadmap 登记"按本契约把确定性 agent 编译 wasm"(见 §5)

---

## 3. commit 分批(单一职责 + 建议 message)

| # | 阶段 | 文件 | message(中文,符合 §1.1) |
|---|------|------|--------------------------|
| 1 | D1 | version.rs + lib.rs + Cargo(semver) | `feat(version): add ATTUNE_VERSION + is_compatible semver gate` |
| 2 | D1 | plugin_loader(min_attune_version) | `feat(plugin): add PluginManifest.min_attune_version field (Option, backward-compat)` |
| 3 | D1 | plugin_registry(scan gate)+ 单测 | `feat(registry): scan rejects incompatible plugins via min_attune_version gate` |
| 4 | D1 | marketplace route + 集成测试 | `feat(marketplace): return plugin-incompatible-version on install version mismatch` |
| 5 | D2 | plugin_loader(wasm/wasi_caps 字段 + 校验) | `feat(plugin): add wasm/wasi_caps fields + load-time validation` |
| 6 | D2 | capability_dispatch(enum + dispatch_capability 分流骨架) | `feat(dispatch): add CapabilityRuntime enum + dispatch_capability runtime router` |
| 7 | D2 | agents.rs 改走统一入口 | `refactor(agents): route through dispatch_capability (contract unchanged)` |
| 8 | D3 | Cargo(wasmtime feature) | `build(deps): add wasmtime behind wasm-runtime feature (default on)` |
| 9 | D3 | wasm_runtime.rs(WasmRunner) | `feat(wasm): WasmRunner via wasmtime+WASI p1 (epoch deadline + StoreLimits)` |
| 10 | D3 | capability_dispatch wasm 分支接线 | `feat(dispatch): wire Wasm lane to WasmRunner (test-fix-verify)` |
| 11 | D4 | wasm fixture + golden gate 测试 | `test(wasm): golden diff=0 vs native baseline + boundary/error/E2E (test-fix-verify)` |
| 12 | D4 | CI matrix | `ci(wasm): cross-platform golden (linux+windows same .wasm)` |
| 13 | D5 | RELEASE + 契约文档 | `docs: wasm runtime contract + min_attune_version + migration classes` |

涉及 agent 的 commit(11)按 §Agent 验证铁律含 `test-fix-verify` + golden gate 1.00。

---

## 4. 风险登记(spec §11 继承 + 实施期新增)

继承 spec R1–R10(wasmtime 体积 / 浮点确定性 / 死循环 / WASI 受限 / version gate 误伤 / JIT 冷启 / 工具链漂移 / 误标 runtime / 跨仓空转 / python 死字段)。**实施期新增**:

| # | 风险 | 等级 | 缓解 |
|---|------|------|------|
| IR1 | wasmtime 版本选型:版本大、含 cranelift,可能拉高编译时间 + 与现有 deps(ort/tokio)的 wasm-encoder/cranelift 传递依赖冲突 | 中 | D3 先单独 `cargo tree` 验依赖图;锁定一个稳定 minor;`wasm-runtime` feature 默认开但可关(K3 极小镜像 D-c 确认是否默认关) |
| IR2 | WASI preview1 timeout:`epoch_interruption` 需后台线程定时 `engine.increment_epoch()`,lib 层不想引 tokio(现 dispatch 用 std::thread) | 中 | 用 std::thread spawn epoch ticker(与现 dispatch 的 std::thread timeout 模式一致),Store drop 时停 ticker;不引 tokio |
| IR3 | 确定性浮点:reference fixture 若含 f64 → wasm/native 可能 ULP 级不同,golden diff≠0 | 高 | fixture 用整数/定点计算(spec R2);CI 双平台 diff 守卫;一旦 diff≠0 block(per spec)。pilot 选纯整数 calc |
| IR4 | `CapabilityInvocation.binary` 复用为 wasm 路径语义模糊(D-a) | 低 | 改 doc 注释明确;或评审拍 D-a 后定 |
| IR5 | 跨仓节奏:attune-pro 迁移晚于 OSS ship → 契约无真实 consumer 验证 | 中 | OSS 自带 reference wasm fixture 作 consumer(D4),不依赖 attune-pro 即可证明链路;attune-pro 真迁移列其 roadmap(§5) |
| IR6 | Windows CI runner 跑 wasmtime + golden 可能首次配置踩坑(per §7.3 KVM embed 教训:CI 过≠真跑) | 中 | D4 CI 必须 Windows runner 真跑 `.wasm`(不是 mock);失败即修,不 skip |

---

## 5. 跨仓配合(attune-pro)标注

| 步骤 | OSS attune(本仓) | attune-pro 配合 |
|------|-------------------|------------------|
| 契约定义 | D2/D3 定 runtime 字段 + dispatch_capability + WasmRunner + wasi_caps 白名单 | 读契约,无代码 |
| reference 验证 | D4 OSS 自造中性 wasm fixture 验链路(不含 vertical 业务) | 无 |
| 真实迁移 | 无(OSS 无 vertical agent) | **attune-pro 把确定性 agent(loan_terms/case_no/patent_claims/本息利率计算)编译 `wasm32-wasip1`,按本契约填 manifest `runtime:wasm`+`wasm:`+`wasi_caps`,各 vertical 跑 golden diff=0(其 agent_golden_gate)才合入** |
| 文档双仓引用 | D5 OSS 文档为 SSOT | attune-pro roadmap 登记迁移任务,引用 OSS 契约文档 |
| min_attune_version | OSS 定 gate 语义 | attune-pro 各 vertical plugin.yaml 填 `min_attune_version: "1.1.0"`(用了 wasm 的包) |

**跨仓硬约束**(per CLAUDE.md):OSS 仓**不含任何 vertical wasm 产物**;OSS 不 link attune-pro;契约文档 OSS 为 SSOT。attune-pro 迁移**不阻塞** OSS v1.1.0 ship(OSS reference fixture 已自证链路)。

---

## 6. 测试策略(每阶段对应 + §6.1 六类下限 + Agent 验证铁律)

| 阶段 | 测试 | 六类覆盖 |
|------|------|---------|
| D1 | `version` 单测(合法/非法 semver/边界 `=`)≥5;`scan` incompatible 单测(min<curr 兼容 / min>curr skip / 缺字段=兼容 / 非法 semver 拒)≥4;marketplace 集成(装 min=99.0.0 包 → CONFLICT) | happy/edge/error |
| D2 | runtime 字段解析单测(wasm 缺 wasm 字段→Err / 未知 wasi_cap→Err / python→unsupported);dispatch 分流单测(无 wasmtime feature 时 wasm→unsupported,rust_binary 仍工作) | edge/error |
| D3 | wasm fixture 单测:echo(stdin→stdout)/ red-line(proc_exit 2)/ trap(unreachable→exit 1)/ 死循环(epoch timeout→timed_out)/ 内存炸(StoreLimits trap)/ 未授权 fs(WASI denied) | edge/error/资源耗尽/adversarial |
| D4 | **golden diff=0**:同 golden set wasm 输出**逐字节**=native ≥10 case;proptest ≥3(随机合法 facts→wasm==native,不 panic,exit∈{0,1,2});E2E ≥1(真 .attunepkg 含 wasm→install→scan→dispatch_capability→验 result + version gate);跨平台 CI(linux+windows 同 .wasm 一致) | happy/edge/回归/跨平台 |
| 全程 | 回归:每修 bug + 1 fixture 进 golden,ratchet 只升不降;`wasm_capability_gate.rs` deterministic pass rate=1.00(对齐现有 `*_golden_gate` harness 风格) | 回归 |

**LLM tier 矩阵**:本 feature wasm agent 是确定性纯计算,**不涉 LLM**(spec §9),无需 3-tier;现有 LLM agent 仍走宿主侧 lane,不受影响(回归验证 `oss_agent_real_llm_gate` 仍绿)。

---

## 7. GA 验收清单(可勾选)

- [ ] **双平台 golden diff=0**:同一 `.wasm` 在 Linux + Windows CI runner 跑同 golden set,输出逐字节一致(真跑,非 mock — per §7.3)
- [ ] **wasm agent exit-code 契约一致**:0/1/2/-1(timeout) 语义与 subprocess 完全对齐,`CapabilityResult` 同结构
- [ ] **min_version gate 拒老包**:装一个 `min_attune_version: "99.0.0"` 的包 → scan skip + marketplace 返回 `plugin-incompatible-version`(不 panic,清晰提示升级)
- [ ] **老包向后兼容**:无 `min_attune_version` 字段的存量包正常加载;现有 `runtime: rust_binary` 插件链路完全不变(回归绿)
- [ ] **.attunepkg 一包多平台真跑**:同一含 `.wasm` 的包,install_plugin_package→scan→dispatch_capability 在两平台均产正确 result(E2E)
- [ ] **wasm 边界守卫**:死循环 wasm→epoch timeout 杀掉(timed_out=true);内存超 256MB→StoreLimits trap;未授权 fs/net→WASI denied(均不拖垮宿主)
- [ ] **attune-pro 6 类下限仍绿**:跨仓迁移后,attune-pro 各 vertical `agent_golden_gate` deterministic pass rate=1.00(wasm 与 rust_binary agent 共存无回归)
- [ ] **feature 可关**:`cargo build --no-default-features`(去 `wasm-runtime`)编译通过,native cap 仍工作(K3 极小镜像路径)
- [ ] **Gate 1–4(§7.2)**:RELEASE.md 标 wasmtime 版本 + Known Limitations(python_subprocess 未实现 / component model 推后);clippy 干净;`cargo test --workspace` 全过;无新 `#[ignore]` 突增
- [ ] **文档无漂移**:runtime 取值表 / wasi_caps 白名单 / 迁移分类 与代码一致;契约文档 OSS SSOT,attune-pro 引用不漂移

---

## 8. 评审需拍板的开放项

1. **D-a**:wasm 执行体路径复用 `CapabilityInvocation.binary` 还是新增 `entry` 抽象?(plan 推荐复用 + 改注释)
2. **D-b**:`scan` incompatible 用字符串前缀(不改签名)还是强类型 `Vec<IncompatiblePlugin>`(改签名 + 全调用方)?(plan 推荐字符串,spec §10 已认可)
3. **D-c**:`wasm-runtime` feature 默认开 vs 默认关?(spec §8 说默认开、极小镜像可关;K3 镜像若纯 native 是否默认关需确认)
4. **wasmtime 版本**:D3 选型前 `cargo tree` 验依赖兼容,锁定 minor 写 RELEASE.md(R7/IR1)
