# 跨厂商本地推理加速 EP 选型矩阵（Cross-Vendor Local-Accel ONNX EP Selection Matrix）

> **状态:DRAFT — 供用户评审(§3.1 spec-first)。未实现。**
> 日期:2026-06-17 · 作者:spec-analyst(AI 起草)
> 扩展:#6 AMD NPU 检测(`attune-core/src/platform/npu.rs`,已 ship)
> 关联:`docs/superpowers/specs/2026-06-16-vision-understanding-enhancement.md`(底座 ONNX 推理消费方)
> 数据来源(§6.3):本仓实测 + `ort` crate 官方文档(context7 `/pykeio/ort` 已核);非本仓/非官方核实项一律标 `PENDING-VERIFY`。

---

## 0. 目录(TOC)

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误 + 边界 case](#7-错误--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [附录 A. ORT EP 打包 / 编译特性矩阵(重点章节)](#附录-a-ort-ep-打包--编译特性矩阵重点章节)

---

## 1. 目标定位

**用户痛点**:attune 的本地底座是 **ONNX 模型**(embedding `bge-*` / rerank / OCR PP-OCRv5),全部经 `ort` crate(2.0.0-rc.12,见 `attune-core/Cargo.toml:73`)运行。当前 `infer/provider.rs` 只对 **CUDA→CPU** 做了 EP 选择;`provider.rs:27-28` 留有 TODO 注释「IntelNpu / IntelIgpu → OpenVINO EP;AmdNpu → DirectML EP,待 ort features 添加后激活」。这意味着:

- **NVIDIA 用户**已享受 CUDA 加速(已 ship)。
- **Intel(CPU/iGPU Arc-Xe/NPU)、AMD GPU(RDNA)、AMD NPU(XDNA)用户全部退化到纯 CPU**,闲置算力(§ 成本感知「⚡ 本地算力层」)未被利用。建库阶段的 embedding 批量生成、OCR 区域识别因此慢数倍。

**目标**:把 #6 已建立的硬件检测能力(`platform::detect_npu()` + `platform::npu` AMD XDNA 细粒度检测)接到 ORT session 构建上,形成**跨厂商的底座 ONNX 推理 EP 自动选型**——检测到什么硬件 + 当前 artifact 编进了哪些 EP → 选最优 EP,链式 fallback 到 CPU,**永不因 EP 不可用而崩**。

**与产品定位对齐**:attune = 「降低 token + 数据安全 + 本地优先混合智能」。本地底座加速是「⚡ 本地算力层」的核心兑现——它让「建库阶段自动跑 embedding/classify」在用户已有的 GPU/NPU 上跑得起来,而不是只能等 CPU。**零云成本、零隐私外泄**,正是本地优先的护城河。

**非目标(写死,见 §2)**:本 spec **不**改 Ollama 后端选择(Ollama 自管 CPU/CUDA/ROCm/Vulkan/Metal),**不**改 ASR(whisper.cpp 走 subprocess,独立加速路径),**不**引入本地 LLM(LLM 默认云端 token,M2 决策)。

---

## 2. 范围边界

### 做(v1)

1. **底座 ONNX 推理的 EP 自动选型**:`infer::provider::build_session()` 从单一 CUDA/CPU 判断,升级为 `AccelCapabilities → 推荐 EP 链 → ORT session`。覆盖 embedding / rerank / OCR(`infer/embedding.rs`、`infer/reranker.rs`、`ocr/nontext/layout.rs` 三处 `Session::builder()` 调用点)。
2. **加速能力检测层** `AccelCapabilities`:综合 (a) `platform::detect_npu()` 硬件结果、(b) 当前 artifact **编译期可用的 EP**(`ort::ep::*::is_available()`)、(c) 环境变量覆盖(`ATTUNE_ORT_EP`),给出有序 EP 推荐链。
3. **运行时 EP 可用性 probe + graceful CPU fallback**:EP 未编译进 artifact / 运行时注册失败 → 自动降级 CPU,**不 panic、不 error**(§7)。
4. **Cargo features 矩阵 + 每平台 artifact 内置 EP 决策**(附录 A):明确 Linux deb / Win msi / macOS / K3 riscv64 各内置哪些 EP。
5. **EP 选型 telemetry**:每次 session 构建记录 `requested_ep / actual_ep / fallback_reason`,供 UI 显示「当前 embedding 跑在 CUDA / CPU」+ 诊断。

### 不做(本 spec 范围外,显式排除以防 scope creep)

- **Ollama 后端切换**:Ollama 自管硬件后端(CUDA/ROCm/Vulkan/Metal/OpenVINO),attune 只通过 HTTP 调用,不干预其 EP。Chat/部分 embedding 走 Ollama 路径不受本 spec 影响。
- **ASR 加速**:whisper.cpp 是独立 subprocess 二进制(可能各带 CPU/CUDA build),其加速由打包二进制决定,不经 `ort`,本 spec 不动。
- **K3 一体机 IME/RVV 路径**:K3 走自研推理服务(`docs/k3-ai-service/`),底座经 K3 :8080 HTTP,不经本机 `ort` EP。K3 riscv64 artifact 本身只内置 CPU EP(见附录 A)。
- **本地 LLM**:不在范围(M2 决策:LLM 默认云端 token)。
- **真正执行驱动安装**:#6 的 `npu.rs` 已确立「只产出 consent-gated 命令字符串,不执行」的安全边界。本 spec **沿用**该边界,不新增任何自动提权/换内核/装运行时的执行能力。OpenVINO runtime / Ryzen AI SW / ROCm 的安装仍是 consent-gated 引导(见 §7 + 附录 A)。

### 后续版本(v.next,不在 v1)

- VitisAI EP(AMD XDNA NPU,Win/Ryzen AI SW)真集成 + golden 验证 → **v2**(理由见附录 A:需 Ryzen AI SW 运行时,打包/许可复杂,先验证 detection 链)。
- ROCm EP(AMD GPU Linux)→ **v2**(需系统 ROCm 运行时,体积/兼容面大)。
- TensorRT EP(NVIDIA 进阶)→ **v2**(需 TensorRT 运行时,首次 engine build 慢)。
- Intel iGPU/NPU 经 OpenVINO 的细分调优(精度/device 选择)→ v.next。

---

## 3. 架构数据流

```
                          ┌─────────────────────────────────────────────┐
                          │  platform 层(#6 已 ship + 本 spec 复用)      │
                          │                                               │
  硬件/内核探测 ──────────▶│  detect_npu() -> NpuKind                       │
  (lspci / cpuinfo /      │    {IntelNpu,IntelIgpu,AmdNpu,Cuda,None}      │
   /dev/accel / amdxdna)  │  platform::npu::NpuStatus(AMD XDNA 细粒度)     │
                          └───────────────────┬───────────────────────────┘
                                              │ hardware signal
                                              ▼
        ┌───────────────────────────────────────────────────────────────────┐
        │  infer::accel(本 spec 新增)                                         │
        │                                                                     │
        │  AccelCapabilities::detect()                                        │
        │   ├─ a) hardware = detect_npu()                                     │
        │   ├─ b) compiled_eps = [ep.is_available() for ep in known]  ◀── ORT │
        │   │       (CPU 永远 true;CUDA/DirectML/CoreML 视 cargo feature)     │
        │   ├─ c) env override = ATTUNE_ORT_EP (force / disable)              │
        │   └─▶ recommend_ep_chain() -> Vec<EpChoice>  (有序,末位永远 CPU)    │
        └───────────────────────────┬───────────────────────────────────────┘
                                     │ ordered EP chain, e.g.
                                     │   [Cuda, Cpu]  /  [DirectML, Cpu]
                                     │   [OpenVINO{device}, Cpu]  /  [Cpu]
                                     ▼
        ┌───────────────────────────────────────────────────────────────────┐
        │  infer::provider::build_session(model_path)  (改造现有)             │
        │                                                                     │
        │  Session::builder()                                                 │
        │    .with_execution_providers(chain.map(EpChoice::build))            │
        │       └─ ORT 语义:按序注册;EP 注册失败 → 跳下一个;                 │
        │          op 不支持 → 算子级 fallback;全失败 → CPU(ORT 默认)        │
        │    .commit_from_file(model_path) -> Session                         │
        │                                                                     │
        │  records EpSelectionTelemetry{requested, actual, fallback_reason}   │
        └───────────────────────────┬───────────────────────────────────────┘
                                     │ Session
                                     ▼
         消费方(全部已存在,调用点不变,只是 session 更快):
           infer/embedding.rs   infer/reranker.rs   ocr/nontext/layout.rs
                                     │
                                     ▼  telemetry
         UI:「embedding 当前跑在 CUDA(本地·~0.4s/批)」+ Settings 诊断面板
```

**关键设计点**:`AccelCapabilities` 是「硬件信号 × 编译期 EP × env 覆盖」三者的交集决策。**硬件有 NPU 但 artifact 没编进对应 EP → 推荐链直接落 CPU**(这正是当前 Intel/AMD 用户的状态,本 spec 让它从「硬编码 CPU」变成「probe 后落 CPU 并 telemetry 说明原因」)。

**DB tables / cache**:本 spec **不新增持久化表**。`AccelCapabilities` 检测结果进程内缓存(`OnceLock`,首次 session 构建时探测一次,避免每次 reindex 重复探测);telemetry 走现有日志 + 内存计数器(暴露给 `/api/v1/status` 类只读端点,不落库)。

---

## 4. 模块边界

| crate / module | 角色 | 改动类型 |
|---|---|---|
| `attune-core/src/platform/mod.rs`(`detect_npu` / `NpuKind`) | 硬件检测 SSOT | **复用,不改** |
| `attune-core/src/platform/npu.rs`(#6 AMD XDNA) | AMD NPU 细粒度 | **复用,只读**(v1 不接 VitisAI,但 `NpuStatus::is_ready()` 作为 v2 VitisAI 推荐前置条件预留) |
| `attune-core/src/infer/accel.rs` | **新增** — `AccelCapabilities` + `EpChoice` + `recommend_ep_chain` + telemetry | **新建** |
| `attune-core/src/infer/provider.rs` | EP→session 构建 | **改造**(现有 CUDA/CPU 分支 → 调 `accel::recommend_ep_chain`) |
| `attune-core/src/infer/mod.rs` | 模块导出 | 加 `pub mod accel;` |
| `attune-core/Cargo.toml` `[features]` | EP feature gate | **扩展**(新增 `openvino`/`rocm`/`tensorrt`/`vitisai` feature passthrough,默认全 OFF;现有 `cuda`/`directml`/`coreml` 保留) |
| `attune-server` 只读 status 端点 | 暴露 EP telemetry | 小改(加字段) |
| `.github/workflows/*release*.yml` | 每平台 artifact 的 EP feature 组合 | **改造**(按附录 A 矩阵给不同 target 不同 `--features`) |

**跨仓边界**:无。本 spec 完全在 OSS attune 仓内(`attune-core` + `attune-server` + CI),不触 attune-pro / attune-enterprise / cloud。符合 OSS 边界规则:EP 加速「对任何领域的个人通用用户都有价值」,属 OSS。

---

## 5. API 契约

**Rust 内部 API(crate 内,非 REST)**:

```rust
// infer/accel.rs

/// 一个 EP 选择(待 build 成 ort ExecutionProviderDispatch)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpChoice {
    Cpu,
    Cuda,
    DirectMl,
    CoreMl,
    OpenVino { device: OpenVinoDevice }, // v1 检测+telemetry;真启用待 feature 编入
    Rocm,                                // v2
    TensorRt,                            // v2
    VitisAi,                             // v2(AMD XDNA)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenVinoDevice { Cpu, Gpu, Npu, Auto }

/// 加速能力快照:硬件 × 编译期 EP × env 覆盖。
#[derive(Debug, Clone)]
pub struct AccelCapabilities {
    pub hardware: crate::platform::NpuKind, // detect_npu()
    pub compiled_eps: Vec<EpChoice>,        // ep.is_available() == true 的集合(CPU 必在)
    pub env_override: Option<EpOverride>,   // ATTUNE_ORT_EP
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpOverride { Force(EpChoice), DisableAccel /* 强制 CPU */ }

impl AccelCapabilities {
    /// 探测一次(进程内 OnceLock 缓存)。永不 panic。
    pub fn detect() -> &'static AccelCapabilities;

    /// 推荐有序 EP 链;末位 **永远** 是 EpChoice::Cpu(兜底不变量)。
    /// 仅返回「硬件命中 ∧ 已编译可用 ∧ 未被 env disable」的 EP + CPU。
    pub fn recommend_ep_chain(&self) -> Vec<EpChoice>;
}

/// 单次 session 构建的 EP 结果(telemetry)。
#[derive(Debug, Clone)]
pub struct EpSelectionTelemetry {
    pub requested: Vec<EpChoice>,
    pub actual: EpChoice,            // 实际承载推理的 EP(best-effort 推断,见 §7 注)
    pub fallback_reason: Option<String>, // e.g. "directml not compiled in this artifact"
}
```

```rust
// infer/provider.rs(改造后签名不变,行为升级)
pub fn build_session(model_path: &Path) -> Result<Session>;
// 内部:let chain = AccelCapabilities::detect().recommend_ep_chain();
//       Session::builder().with_execution_providers(chain.iter().map(EpChoice::build))…
```

**REST(只读,server 侧)**:`GET /api/v1/status`(或现有等价端点)响应**新增**字段:

```json
{
  "accel": {
    "hardware": "intel-igpu",
    "compiled_eps": ["cpu"],
    "active_ep": "cpu",
    "fallback_reason": "openvino not compiled in linux-deb artifact (v1)",
    "hint": "kebab-error-code: accel-ep-not-compiled"
  }
}
```

`hint.kebab-error-code` 取值集合见 §7。

**env 契约**:`ATTUNE_ORT_EP`(可选):`cuda`/`directml`/`coreml`/`openvino`/`cpu`/`off`。值非法 → 忽略 + warn,落 CPU 链(不 error)。

---

## 6. 扩展点 / 插件接口

**加一个新 EP** 的步骤(把扩展成本写死,避免未来散弹式改动):

1. `EpChoice` 加一个枚举值。
2. `EpChoice::build()` 加该 EP 的 `ort::ep::<X>::default().build()` 分支(`#[cfg(feature = "<x>")]` 编译门;未编入时该分支 `is_available()==false`,自动不进链)。
3. `EpChoice::is_available()` 包 `ort::ep::<X>::default().is_available()`。
4. `recommend_ep_chain()` 的硬件→EP 映射表加一行(`NpuKind::X → [EpChoice::X, Cpu]`)。
5. `Cargo.toml [features]` 加 `<x> = ["ort/<x>"]`。
6. CI release workflow:决定哪个 target artifact 编入该 feature(附录 A 矩阵加一列)。
7. 测试:`tests/accel_matrix.rs` 加该 EP 的「编入/未编入」两态 case。

**不变量(扩展时必须守)**:`recommend_ep_chain()` 返回链**末位永远 CPU**;任何 EP 都是「增量」,移除某 EP feature 后默认行为 = 退回 CPU(§10)。

---

## 7. 错误 + 边界 case

**核心硬约束:EP 未编译进当前 artifact / 运行时不可用 → graceful 降级 CPU,绝不 panic、绝不让 session 构建 error。**

ORT 语义(已核 context7 `/pykeio/ort`):
- `with_execution_providers([...])` 默认行为 = **EP 注册失败则静默跳过,全失败则落 CPU**(ORT 内建)。
- `.error_on_failure()` 会让 EP 注册失败时报错退出 —— **本 spec 明确不对底座 EP 用 `.error_on_failure()`**(底座推理永远要能跑,加速是 best-effort)。
- `ep.is_available()` 检查**编译期**是否编进该 EP(注:返回 true 仍可能运行时注册失败,故必须叠加 ORT 的链式 fallback)。

| # | 边界 / 错误 case | 处理 | kebab error-code / telemetry |
|---|---|---|---|
| E1 | 硬件有 NPU/GPU,但 artifact **未编入**对应 EP(v1 Intel/AMD 常态) | `is_available()==false` → 该 EP 不进链 → CPU | `accel-ep-not-compiled` |
| E2 | EP 编入但**运行时注册失败**(缺驱动 / 缺 OpenVINO runtime / 缺 ROCm) | ORT 链式跳过 → 下一 EP / CPU | `accel-ep-runtime-unavailable` |
| E3 | EP 注册成功但**算子不支持** | ORT 算子级 fallback 到下一 EP / CPU(无需我方介入) | `accel-op-fallback`(best-effort,可能仅日志) |
| E4 | `ATTUNE_ORT_EP` 值非法 | 忽略 + warn → CPU 链 | `accel-env-invalid` |
| E5 | `ATTUNE_ORT_EP=off` | 强制 CPU-only 链(诊断/规避 bug 用) | `accel-disabled-by-env` |
| E6 | `detect_npu()` 本身 panic / 探测异常 | `detect()` 内 `catch`/`Result` 兜底 → 视作 `NpuKind::None` → CPU | `accel-detect-failed` |
| E7 | GPU 显存不足(CUDA/DirectML OOM) | ORT 报错冒泡为 `VaultError`;**v1 不自动 retry-on-CPU**(避免半初始化状态);telemetry 记录,UI 提示「切 CPU 或减小批」 | `accel-device-oom` |
| E8 | `active_ep` 推断不准 | `actual` 字段标 best-effort;ORT 不暴露精确「哪个 EP 跑了这张图」,以「注册成功的最高优先 EP」近似,telemetry 注明 `approx=true` | — |

**graceful degradation 总原则**:加速失败的最坏结果 = **退回到本 spec 之前的纯 CPU 行为**(即今天的行为)。用户永远有可用的底座推理。

---

## 8. 成本契约

EP 加速归属 **⚡ 本地算力层**(§ 成本感知与触发契约):

| 层级 | 本 spec 是否触及 | 说明 |
|---|---|---|
| 🆓 零成本(CPU 毫秒级) | CPU EP 是兜底 | OCR/BM25 等不变 |
| ⚡ **本地算力(GPU/NPU 秒级)** | **本 spec 核心** | embedding/rerank/OCR 用本机 GPU/NPU 加速;**零云成本、零隐私外泄**;建库阶段自动跑(顶栏「暂停后台任务」开关已存在,沿用) |
| 💰 时间/金钱(LLM 云端) | **不触及** | 本 spec 完全不涉及云 token / 付费 API |

**UI 成本显示契约**:
- Settings「硬件加速」诊断面板显示 `active_ep` + `hardware` + `fallback_reason`(让用户一眼看到「embedding 跑在 CUDA/iGPU/CPU」)。
- 加速是「⚡ 本地算力」→ **不显示金钱成本**,可显示「本地 · ~Ns」相对耗时(沿用现有 `~本地 · 2s` 范式)。
- **不**因加速可用而把建库阶段升级到「💰 层」——成本契约第 1 条不变(建库永不调云 LLM)。

---

## 9. 测试矩阵(§6.1 六类下限)

测试代码与本 spec 落地同 commit(实施阶段);`tests/accel_matrix.rs` + `infer/accel.rs` inline `#[cfg(test)]`。

| 类型 | 下限 | 用例(摘) |
|---|---|---|
| **Golden / happy** | ≥6 | (hw=Cuda, compiled=[cuda,cpu]) → chain=[Cuda,Cpu];(hw=IntelIgpu, compiled=[cpu]) → [Cpu];(hw=AmdNpu, compiled=[cpu]) → [Cpu];(hw=None) → [Cpu];(hw=Cuda, compiled=[cpu] 即 artifact 没编 cuda) → [Cpu] + E1 telemetry;(hw=IntelIgpu, compiled=[openvino,cpu]) → [OpenVino{Auto},Cpu] |
| **属性测试(proptest)** | ≥3 | (1) `recommend_ep_chain()` 返回链**末位恒为 Cpu**(任意 hardware×compiled×env);(2) 链中不含未 `is_available()` 的 EP;(3) `env=off` ⇒ 链==[Cpu] 恒成立 |
| **边界 case** | ≥5 | 空 compiled(理论不可能,CPU 必在)→ assert CPU 兜底;env 大小写/空白;hardware=Cuda 但 compiled 不含 cuda;重复 EP 去重;OnceLock 缓存幂等(detect 两次同址) |
| **异常 / 错误** | ≥3 | E4 非法 env → CPU + warn;E6 detect panic 注入 → None;E2 mock「编入但 register 失败」→ 链跳过(用 `is_available` stub) |
| **集成 E2E** | ≥1 | 真 ORT:`build_session()` 用真 ONNX 小模型,在 CI(无 GPU)上断言落 CPU 且推理产出 == 现有 baseline(回归保护:加速层不改数值结果) |
| **回归 fixture** | 每修 bug +1 | 每个发现的 EP 选择 bug → 加 golden case;**数值回归**:embedding/rerank/OCR 输出在 CPU EP 下与 #6 前 baseline bit-级/容差一致 |

**多平台编译矩阵(CI,附录 A)**:每个 target `cargo build --features <组合>` 必须过;native target 跑 `accel_matrix` 测试;交叉编译只验编译(per 现有 §测试隔离规范)。

**§4.5 LLM 兜底**:本 spec **无 LLM agent**(纯硬件/确定性逻辑),不适用 N=3 real-LLM gate。确定性 PASS rate 要求 = 1.00(§6.1)。

**测试隔离(§现有约定)**:EP 检测涉及系统硬件 → 用 `MockAccelCapabilities` / 显式注入 `NpuKind`,不依赖 CI runner 真有 GPU;`is_available()` 在测试中走可注入的 seam,不真探测。

---

## 10. 向后兼容

| 维度 | 保证 |
|---|---|
| **默认行为** | **CPU 行为完全不变**。所有新 EP 都是 cargo feature 增量,默认 OFF(除已有 `cuda`/`directml`/`coreml` 保持现状)。未编入任何加速 EP 的 artifact 行为 == 今天纯 CPU。 |
| **CUDA 路径** | 现有 `cuda` feature + CUDA→CPU 行为**保持**(只是改由 `recommend_ep_chain` 统一产出,结果等价)。CUDA 用户无感知。 |
| **数值结果** | EP 仅改**算子执行后端**,不改模型/精度配置(f16 量化等不变)→ embedding/rerank/OCR **输出在容差内一致**;§9 回归 fixture 守此。 |
| **API schema** | `/status` **新增** `accel` 字段(纯增量,老 client 忽略未知字段)。无字段删除/改名。 |
| **`build_session` 签名** | 不变(`fn(&Path) -> Result<Session>`),调用点零改动。 |
| **migration path** | 无 DB schema 变更 → **无 migration**。env `ATTUNE_ORT_EP` 是新增可选项,缺省 == 旧行为。 |
| **回滚** | 任一 EP feature 出问题 → release workflow 去掉该 `--features` 即退回 CPU,无需改代码。 |

---

## 11. 风险登记

| # | 风险 | 等级 | 缓解 |
|---|---|---|---|
| R1 | **二进制体积膨胀**:DirectML / CUDA / OpenVINO 各带数十~数百 MB 运行时 dylib,与「thin-deb 38M / msi 23M」决策冲突(§ 技术栈瘦包) | 高 | 附录 A 矩阵**保守**:Win msi 仅内置 DirectML(Win 最稳且 DML dylib 相对小);Linux deb **不**内置 GPU EP(走 `download-binaries` 或 runtime-fetch 增量,与现有「首次运行拉模型」一致心智);CUDA 走可选「GPU 增强包」而非默认 deb。**体积影响在附录 A 逐 target 量化(PENDING-VERIFY:需真 build 测体积)** |
| R2 | **EP 编入但运行时崩**(缺驱动/缺 runtime DLL,如 DML 缺 `DirectML.dll`、CUDA 缺 cuDNN) | 高 | 不用 `.error_on_failure()`;依赖 ORT 链式 fallback + 我方 `is_available()` 前置 probe;E2 telemetry 暴露原因 |
| R3 | **`active_ep` 不可精确观测**:ORT 不报「哪张图跑哪个 EP」 | 中 | telemetry `actual` 标 `approx=true`,以「注册成功的最高优先 EP」近似;UI 措辞「预计加速:CUDA」而非断言 |
| R4 | **VitisAI / ROCm / OpenVINO 集成现状不稳**(rc 期 `ort` EP 支持成熟度参差) | 中 | v1 **不**真启用 VitisAI/ROCm/TensorRT(只检测+telemetry);只把 OpenVINO/DirectML 列入「可选编入」,默认仍保守;**ort 各 EP 真实可用性需 PENDING-VERIFY 实测**(context7 仅确认 API 存在,未确认每个 EP 在 rc.12 的 build 可用性) |
| R5 | **跨平台 CI 体积/编译失败**:某 EP 在某 target 编不过 | 中 | CI 矩阵逐 target `cargo build --features`;失败 target 退到 CPU-only feature 组合(附录 A 已按 target 拆) |
| R6 | **OnceLock 缓存导致热插拔不生效**(用户中途插 GPU) | 低 | v1 接受:检测一次/进程;文档说明重启生效;v.next 可加显式 re-detect |
| R7 | **数值漂移**:某 EP 算子实现与 CPU 有数值差,污染向量库 | 中 | §9 回归 fixture 强制容差一致;若某 EP 漂移超容差 → 该 EP 不进默认链,标 Known Limitation |
| R8 | **与 #6 npu.rs 的耦合**:v2 接 VitisAI 时依赖 `NpuStatus::is_ready()` | 低 | v1 仅只读复用,不改 npu.rs;v2 spec 再定义 VitisAI ← `NpuStatus` 的契约 |

---

## 附录 A. ORT EP 打包 / 编译特性矩阵(重点章节)

> 这是本 spec **最需评审**的部分:每个 ORT EP 是 `ort` crate 的**编译期 feature**(+ 多数还需特定 ORT 二进制 / 系统运行时),**不能全塞进一个 binary**。

### A.1 `ort` crate 现状(本仓实测 + context7 核)

- 本仓 `attune-core/Cargo.toml:73`:`ort = "2.0.0-rc.12"`,features 含 `download-binaries`(自动拉预编译 ORT 二进制)、`copy-dylibs`。
- 已声明 feature passthrough:`cuda = ["ort/cuda"]`、`directml = ["ort/directml"]`、`coreml = ["ort/coreml"]`(`Cargo.toml:120-122`),但 `provider.rs` **仅实际用了 CUDA**;DirectML/CoreML 已编入能力但未接选型逻辑。
- EP 模型(已核 context7 `/pykeio/ort`):每个 EP = 一个 cargo feature;`with_execution_providers([...])` 按序注册;`is_available()` 查编译期支持;默认全失败静默落 CPU;`.error_on_failure()` 可改为报错(本 spec 不用)。

### A.2 厂商 × EP × 平台 选型矩阵(已确立选型)

| 厂商 / 硬件 | ONNX 底座推荐 EP | Linux | Windows | macOS | 备注 |
|---|---|---|---|---|---|
| **Intel**(CPU/iGPU Arc-Xe/NPU) | **OpenVINO EP**(统一 device=CPU/GPU/NPU/AUTO) | ✓(需 OpenVINO runtime) | ✓(需 OpenVINO runtime) | n/a | 需系统/捆绑 OpenVINO runtime;**v1:检测+telemetry,默认不编入**(R1 体积 + R4 成熟度) |
| **AMD GPU(RDNA)** | ONNX:**DirectML**(Win 最稳)/ **ROCm**(Linux);Ollama:ROCm(Linux)/Vulkan(Win) | ROCm(需 ROCm runtime) | **DirectML** | n/a | **v1 仅 Win DirectML 可选编入**;ROCm 推 v2(R1/R4) |
| **AMD NPU(XDNA)** | **VitisAI EP**(Win/Ryzen AI SW;Linux amdxdna 早期) | amdxdna 早期 | 需 Ryzen AI SW 运行时 | n/a | **v1 仅检测(#6 已 ship)+ telemetry,不启用 VitisAI**;真集成 v2。Ollama 不支持 NPU |
| **NVIDIA** | **CUDA**(已 ship)/ TensorRT(进阶);Ollama:CUDA | ✓(已 ship) | ✓ | n/a | CUDA 已实装;TensorRT v2 |
| **CPU**(所有平台) | **CPU EP**(ORT 默认) | ✓ | ✓ | ✓ | **始终兜底**,链末位不变量 |
| **K3 一体机(riscv64)** | CPU EP only(底座经 K3 :8080 HTTP,不经本机 ort) | ✓(CPU) | n/a | n/a | riscv64 artifact 只编 CPU;加速由 K3 服务自管 IME/RVV |
| **macOS** | (CoreML EP 已有 feature) | n/a | n/a | 暂不做 | macOS 平台「暂不做」(§平台优先级);CoreML feature 保留,不投入 v1 验证 |

### A.3 每平台 artifact「内置哪些 EP」决策(v1 务实范围)

| Artifact | v1 内置 EP | 不内置(推后) | 体积影响 | 理由 |
|---|---|---|---|---|
| **Linux deb**(P1) | **CPU only**(默认) | OpenVINO / ROCm / CUDA | 与现 38M thin-deb 一致(~0) | 保守:GPU/NPU runtime 体积大 + Linux 驱动碎片化;走「GPU 增强包」或 runtime-fetch(v.next) |
| **Linux 「GPU 增强」可选包**(新,v1 可选) | CPU + **CUDA** | — | +数百 MB(CUDA dylib,**PENDING-VERIFY 实测**) | 给 NVIDIA Linux 进阶用户,单独 asset,不进默认 deb |
| **Windows msi**(P0) | CPU + **DirectML** | OpenVINO / VitisAI / CUDA | +DML dylib(**PENDING-VERIFY**,预期 < CUDA) | DirectML 是 Win 上对 NV/AMD/Intel **统一**的最稳 GPU 路径,单一 EP 覆盖多厂商,体积/兼容最优 |
| **Windows 「Intel/AMD-NPU」可选包**(推后 v2) | CPU + OpenVINO(+ VitisAI) | — | PENDING | 需 OpenVINO / Ryzen AI SW runtime;v2 |
| **macOS**(暂不做) | (CoreML feature 存在但不验证) | — | — | 平台暂不做 |
| **K3 riscv64 镜像** | **CPU only** | 全部 GPU/NPU EP | ~0 | 底座经 K3 :8080,本机 ort 只需 CPU |

### A.4 运行时 EP 可用性 probe 策略

1. **编译期**:`ort::ep::<X>::is_available()` —— 该 artifact 是否编进 X。false → 不进链(E1)。
2. **运行时注册**:`with_execution_providers` 注册;ORT 内部失败则静默跳过(E2)。我方**不**用 `.error_on_failure()`。
3. **runtime 依赖缺失**(DML.dll / OpenVINO / ROCm / cuDNN):落在 (2) 的注册失败路径 → fallback CPU + telemetry。
4. **首次探测缓存**:`AccelCapabilities::detect()` `OnceLock`,进程内一次。

### A.5 EP 缺失时的 CPU fallback(总结)

```
recommend_ep_chain() 永远返回 [..accel_eps, Cpu]
   ↓
with_execution_providers(chain)  // 无 .error_on_failure()
   ↓
ORT: 注册失败/op 不支持 → 下一 EP → 最终 Cpu(ORT 默认静默落 CPU)
   ↓
最坏结果 == 本 spec 之前的纯 CPU 行为(零退化)
```

### A.6 v1 范围结论(供评审拍板)

- **v1 真启用**:CPU(全平台兜底)+ CUDA(NVIDIA,已 ship,Linux 走可选 GPU 包)+ **DirectML(Windows,新接选型,覆盖 NV/AMD/Intel GPU)**。
- **v1 仅检测 + telemetry(不启用 EP)**:Intel OpenVINO、AMD NPU VitisAI、AMD GPU ROCm —— 检测到硬件但 artifact 不编入 → telemetry 告知「检测到 X,当前版本未启用该加速」,为 v2 真集成铺路。
- **v2**:OpenVINO(Intel)/ VitisAI(AMD XDNA)/ ROCm(AMD GPU Linux)/ TensorRT(NVIDIA 进阶)真集成 + golden 数值回归 + 各自 runtime 的 consent-gated 安装引导(复用 #6 模式)。

**待评审决策点(D1–D4)**:
- **D1**:Win msi v1 是否内置 DirectML?(体积 +Δ vs 覆盖 NV/AMD/Intel 三厂商 GPU 的收益)——本 spec 建议 **是**。
- **D2**:NVIDIA Linux 用户走「独立 GPU 增强包」还是塞进默认 deb?——建议 **独立包**(护 thin-deb)。
- **D3**:OpenVINO/VitisAI/ROCm v1 就「检测+telemetry」是否够?还是 v1 至少上 OpenVINO(Intel iGPU 装机量大)?——建议 v1 仅检测,但**此点最值得用户拍板**。
- **D4**:`active_ep` best-effort 近似(R3)是否可接受 UI 措辞「预计加速」?——建议 **可接受**。
