//! 底座 ONNX 推理的 Execution Provider 自动选型层。
//!
//! 把 `platform::AccelCapabilities`(硬件检测,#6 + accel.rs)接到 `ort` session
//! 构建上:检测到什么硬件 + 当前 artifact 编进了哪些 EP → 选最优 EP 链,**末位永远
//! CPU**。EP 未编入 / 运行时不可用 → graceful 降级下一个 / CPU,**绝不 panic、绝不
//! 让 session error**(沿用 ORT 默认静默 fallback,**不**用 `.error_on_failure()`)。
//!
//! 兑现 `provider.rs` 历史 TODO(「IntelNpu/IntelIgpu → OpenVINO;AmdNpu → DirectML/
//! VitisAI,待 ort features 添加后激活」)。
//!
//! **设计**:`EpChoice` 是纯枚举;`recommend_ep_chain()` 是纯函数(吃 hardware hint +
//! 已编译 EP 集 + env override,吐有序 EP 链),可被单元测试无副作用地 mock(各厂商 ×
//! 编入/未编入 × env 组合 → 正确链),不需要真硬件、不需要真编入对应 EP。
//!
//! 真正的"哪些 EP 编进了 artifact"由 `compiled_eps()` 查 `ort` 编译期 feature
//! (`is_available()`)给出;选型逻辑只信这个集合,所以**硬件有 NPU 但 artifact 没编
//! 对应 EP → 链直接落 CPU**(telemetry 说明原因),与产品 thin-deb 决策一致。

use crate::platform::AccelKind;
use std::sync::OnceLock;

/// 一个 EP 选择(待 build 成 `ort` `ExecutionProviderDispatch`)。
///
/// 顺序无关;优先级由 `recommend_ep_chain()` 决定。`Cpu` 永远是兜底。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EpChoice {
    /// ORT 默认 CPU EP — 全平台兜底,链末位不变量。
    Cpu,
    /// NVIDIA CUDA(已 ship)。
    Cuda,
    /// NVIDIA TensorRT(进阶,需 TensorRT runtime)。
    TensorRt,
    /// Windows DirectML(对 NV/AMD/Intel GPU 统一,Win 最稳)。
    DirectMl,
    /// Apple CoreML(macOS;本项目 macOS 暂不投入,保留 feature)。
    CoreMl,
    /// Intel OpenVINO(CPU/iGPU/NPU 统一,device 由 `OpenVinoDevice` 选)。
    OpenVino(OpenVinoDevice),
    /// AMD ROCm(Linux AMD GPU)。
    Rocm,
    /// AMD XDNA NPU(Ryzen AI)经 VitisAI EP。
    VitisAi,
}

impl EpChoice {
    /// 稳定机读标识(telemetry / API / 诊断,不本地化)。
    pub fn id(&self) -> &'static str {
        match self {
            EpChoice::Cpu => "cpu",
            EpChoice::Cuda => "cuda",
            EpChoice::TensorRt => "tensorrt",
            EpChoice::DirectMl => "directml",
            EpChoice::CoreMl => "coreml",
            EpChoice::OpenVino(_) => "openvino",
            EpChoice::Rocm => "rocm",
            EpChoice::VitisAi => "vitisai",
        }
    }

    /// 该 EP 对应的「运行时软件栈」标识(stack installer 用;`None` = 无需额外 userspace 栈)。
    ///
    /// CPU / CoreML 不需要外置 userspace 栈(CPU 内建;CoreML 系统自带)。其余 EP 各需
    /// 一套 userspace runtime libs(driver 除外,driver 走 #6 consent-gated 路径)。
    pub fn runtime_stack(&self) -> Option<&'static str> {
        match self {
            EpChoice::Cpu | EpChoice::CoreMl => None,
            EpChoice::Cuda | EpChoice::TensorRt => Some("cuda"),
            EpChoice::DirectMl => Some("directml"),
            EpChoice::OpenVino(_) => Some("openvino"),
            EpChoice::Rocm => Some("rocm"),
            EpChoice::VitisAi => Some("vitisai"),
        }
    }
}

/// OpenVINO 目标设备。`Auto` 让 OpenVINO runtime 自选(NPU > GPU > CPU)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenVinoDevice {
    Cpu,
    Gpu,
    Npu,
    Auto,
}

impl OpenVinoDevice {
    /// OpenVINO `device_type` 字符串(`ort` `with_device_type` 用)。
    pub fn device_type(&self) -> &'static str {
        match self {
            OpenVinoDevice::Cpu => "CPU",
            OpenVinoDevice::Gpu => "GPU",
            OpenVinoDevice::Npu => "NPU",
            OpenVinoDevice::Auto => "AUTO",
        }
    }
}

/// `ATTUNE_ORT_EP` 环境覆盖。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpOverride {
    /// 强制某 EP 在链首(仍叠加 CPU 兜底);未编入则被 `recommend_ep_chain` 过滤回 CPU。
    Force(EpChoice),
    /// 强制 CPU-only(诊断 / 规避加速 bug)。
    DisableAccel,
}

/// 解析 `ATTUNE_ORT_EP`:`cuda`/`directml`/`coreml`/`openvino`/`rocm`/`tensorrt`/
/// `vitisai`/`cpu`/`off`。大小写 + 首尾空白无关。
///
/// 返回 `Err(原值)` 表示无法识别 — 调用方应 warn 并落 CPU 链(E4,不报错)。
pub fn parse_ep_override(raw: &str) -> std::result::Result<EpOverride, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "disable" | "cpu-only" => Ok(EpOverride::DisableAccel),
        "cpu" => Ok(EpOverride::Force(EpChoice::Cpu)),
        "cuda" => Ok(EpOverride::Force(EpChoice::Cuda)),
        "tensorrt" | "trt" => Ok(EpOverride::Force(EpChoice::TensorRt)),
        "directml" | "dml" => Ok(EpOverride::Force(EpChoice::DirectMl)),
        "coreml" => Ok(EpOverride::Force(EpChoice::CoreMl)),
        "openvino" | "ov" => Ok(EpOverride::Force(EpChoice::OpenVino(OpenVinoDevice::Auto))),
        "rocm" => Ok(EpOverride::Force(EpChoice::Rocm)),
        "vitisai" | "vitis" => Ok(EpOverride::Force(EpChoice::VitisAi)),
        other => Err(other.to_string()),
    }
}

/// 读取 `ATTUNE_ORT_EP` env 并解析。未设置 → `None`;非法 → warn + `None`(落默认链)。
fn env_override() -> Option<EpOverride> {
    let raw = std::env::var("ATTUNE_ORT_EP").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    match parse_ep_override(&raw) {
        Ok(o) => Some(o),
        Err(bad) => {
            log::warn!("ATTUNE_ORT_EP={bad:?} not recognized; ignoring (falling back to auto EP selection). error-code=accel-env-invalid");
            None
        }
    }
}

/// 当前 artifact **编译期可用**的 EP 集合(`ort` `is_available()` 查 cargo feature)。
///
/// CPU 永远在。其余按 `#[cfg(feature=...)]` 决定是否探测 —— 未编入的 feature 连
/// `ort::ep::<X>` 类型都不会引用(避免 default build 触碰未编 EP 的符号)。
///
/// `is_available()` 返回 `Ok(true)` ⟺ ORT 编进了该 EP。**注意**:返回 true 仍可能
/// 运行时注册失败(缺 driver / 缺 runtime DLL),那一层交给 ORT 链式 fallback(E2)。
pub fn compiled_eps() -> Vec<EpChoice> {
    // `is_available()` 来自 `ort::ep::ExecutionProvider` trait —— 仅在某 EP feature 开启
    // 时才需要在 scope(默认 build 全 EP off 时此 import 未用,故 cfg 门 + allow)。
    #[cfg(any(
        feature = "cuda",
        feature = "tensorrt",
        feature = "directml",
        feature = "coreml",
        feature = "openvino",
        feature = "rocm",
        feature = "vitis"
    ))]
    use ort::ep::ExecutionProvider;

    // `mut` 仅在某 EP feature 开启时被用到;默认 build(全 EP off)下 CPU 是唯一项。
    #[allow(unused_mut)]
    let mut eps = vec![EpChoice::Cpu]; // 永远在

    // 每个 EP 用 `is_available()` 双保险:即便 feature 开了但 ORT 二进制没编进(理论
    // 上 download-binaries 会拉对应变体),也会被 is_available()==false 过滤。
    #[cfg(feature = "cuda")]
    if ort::ep::CUDA::default().is_available().unwrap_or(false) {
        eps.push(EpChoice::Cuda);
    }
    #[cfg(feature = "tensorrt")]
    if ort::ep::TensorRT::default().is_available().unwrap_or(false) {
        eps.push(EpChoice::TensorRt);
    }
    #[cfg(feature = "directml")]
    if ort::ep::DirectML::default().is_available().unwrap_or(false) {
        eps.push(EpChoice::DirectMl);
    }
    #[cfg(feature = "coreml")]
    if ort::ep::CoreML::default().is_available().unwrap_or(false) {
        eps.push(EpChoice::CoreMl);
    }
    #[cfg(feature = "openvino")]
    if ort::ep::OpenVINO::default().is_available().unwrap_or(false) {
        eps.push(EpChoice::OpenVino(OpenVinoDevice::Auto));
    }
    #[cfg(feature = "rocm")]
    if ort::ep::ROCm::default().is_available().unwrap_or(false) {
        eps.push(EpChoice::Rocm);
    }
    #[cfg(feature = "vitis")]
    if ort::ep::Vitis::default().is_available().unwrap_or(false) {
        eps.push(EpChoice::VitisAi);
    }

    eps
}

/// 加速能力快照:硬件命中 × 编译期 EP × env 覆盖。
#[derive(Debug, Clone)]
pub struct AccelSelection {
    /// 当前 OS:`"linux"`/`"macos"`/`"windows"`/`"unknown"`。
    pub os: &'static str,
    /// 检测到的(driver-ready)加速器类别,优先级降序(NPU/GPU 在前,CPU 兜底)。
    pub hardware: Vec<AccelKind>,
    /// 当前 artifact 编入且 `is_available()` 的 EP(CPU 必在)。
    pub compiled: Vec<EpChoice>,
    /// `ATTUNE_ORT_EP` 覆盖。
    pub env_override: Option<EpOverride>,
}

impl AccelSelection {
    /// 从 `platform::AccelCapabilities` + 编译期 EP + env 构建。永不 panic。
    pub fn detect() -> Self {
        let caps = crate::platform::AccelCapabilities::detect();
        let hardware = caps
            .accelerators
            .iter()
            .filter(|a| a.driver_ready)
            .map(|a| a.kind)
            .collect();
        Self {
            os: caps.os,
            hardware,
            compiled: compiled_eps(),
            env_override: env_override(),
        }
    }

    /// 推荐有序 EP 链。**末位永远 `EpChoice::Cpu`**(兜底不变量)。
    ///
    /// 规则:
    /// 1. `env=off` → `[Cpu]`(强制 CPU-only)。
    /// 2. `env=force(X)` 且 X 已编入 → `[X, Cpu]`;X 未编入 → 退回自动选型(并 warn)。
    /// 3. 自动:遍历硬件优先级,把「硬件命中 ∧ 已编入」的 EP 依次入链,末尾补 CPU。
    ///
    /// 链中**不含**未编入的 EP(`is_available()==false` 已在 `compiled` 里过滤)。
    pub fn recommend_ep_chain(&self) -> Vec<EpChoice> {
        recommend_ep_chain_pure(self.os, &self.hardware, &self.compiled, self.env_override.as_ref())
    }

    /// 链首(实际首选)EP — telemetry / UI 显示「预计加速:X」。
    pub fn primary_ep(&self) -> EpChoice {
        self.recommend_ep_chain().into_iter().next().unwrap_or(EpChoice::Cpu)
    }
}

/// 进程内单次探测缓存(避免每次 reindex / 每个 session 重复探测)。
static CACHED: OnceLock<AccelSelection> = OnceLock::new();

/// 缓存版 `detect`(首次探测,后续返回同一快照)。EP 选型热点路径用这个。
pub fn cached_selection() -> &'static AccelSelection {
    CACHED.get_or_init(AccelSelection::detect)
}

/// 纯选型函数(无副作用,可任意组合测试)。
///
/// `hardware` 是 driver-ready 的加速器类别(优先级降序);`compiled` 是已编入 EP;
/// `over` 是 env 覆盖。返回有序 EP 链,**末位恒为 Cpu**。
pub fn recommend_ep_chain_pure(
    os: &str,
    hardware: &[AccelKind],
    compiled: &[EpChoice],
    over: Option<&EpOverride>,
) -> Vec<EpChoice> {
    // env=off → 强制 CPU-only。
    if matches!(over, Some(EpOverride::DisableAccel)) {
        return vec![EpChoice::Cpu];
    }

    let is_compiled = |c: EpChoice| compiled.iter().any(|x| x.id() == c.id());

    // env=force(X):X 已编入 → [X, Cpu];否则忽略并 warn,继续自动。
    if let Some(EpOverride::Force(forced)) = over {
        if *forced == EpChoice::Cpu {
            return vec![EpChoice::Cpu];
        }
        if is_compiled(*forced) {
            return dedup_with_cpu_tail(vec![*forced]);
        }
        log::warn!(
            "ATTUNE_ORT_EP forces {} but it is not compiled into this artifact; using auto selection. error-code=accel-ep-not-compiled",
            forced.id()
        );
    }

    // 自动:硬件优先级 → 候选 EP(每硬件给一组按 OS 选的候选,取首个已编入的)。
    let windows = os == "windows";
    let mut chain: Vec<EpChoice> = Vec::new();

    for &hw in hardware {
        // 每个硬件给「按 OS 排序的候选 EP」;取首个已编入的入链。
        let candidates: &[EpChoice] = match hw {
            AccelKind::NvidiaGpu => &[EpChoice::Cuda, EpChoice::TensorRt],
            // AMD XDNA NPU → VitisAI(Win/Linux amdxdna);未编入退回(无 GPU 候选,NPU
            // 与 GPU 是不同硬件,AMD GPU 单独由 AmdGpu 分支覆盖)。
            AccelKind::AmdNpu => &[EpChoice::VitisAi],
            // Intel NPU → OpenVINO(device=NPU)。
            AccelKind::IntelNpu => {
                push_first_compiled(&mut chain, &[EpChoice::OpenVino(OpenVinoDevice::Npu)], &is_compiled);
                continue;
            }
            // AMD RDNA GPU → Win:DirectML;Linux:ROCm。
            AccelKind::AmdGpu => {
                if windows {
                    &[EpChoice::DirectMl]
                } else {
                    &[EpChoice::Rocm]
                }
            }
            // Intel iGPU → Win:DirectML;Linux:OpenVINO(device=GPU)。
            AccelKind::IntelIgpu => {
                if windows {
                    push_first_compiled(&mut chain, &[EpChoice::DirectMl], &is_compiled);
                } else {
                    push_first_compiled(
                        &mut chain,
                        &[EpChoice::OpenVino(OpenVinoDevice::Gpu)],
                        &is_compiled,
                    );
                }
                continue;
            }
            AccelKind::Cpu => continue, // CPU 由末尾兜底统一加
        };
        push_first_compiled(&mut chain, candidates, &is_compiled);
    }

    dedup_with_cpu_tail(chain)
}

/// 把 `candidates` 中首个已编入的 EP 压进链(若都没编入则不压,留给 CPU 兜底)。
fn push_first_compiled(
    chain: &mut Vec<EpChoice>,
    candidates: &[EpChoice],
    is_compiled: &impl Fn(EpChoice) -> bool,
) {
    if let Some(&ep) = candidates.iter().find(|&&c| is_compiled(c)) {
        if !chain.iter().any(|e| e.id() == ep.id()) {
            chain.push(ep);
        }
    }
}

/// 去重(按 id) + 末位补 CPU(若不在) → 兜底不变量。
fn dedup_with_cpu_tail(mut chain: Vec<EpChoice>) -> Vec<EpChoice> {
    // 去掉链中的 CPU(末尾统一加),再按 id 去重保序。
    let mut seen: Vec<&'static str> = Vec::new();
    let mut out: Vec<EpChoice> = Vec::new();
    for ep in chain.drain(..) {
        if ep == EpChoice::Cpu {
            continue;
        }
        if !seen.contains(&ep.id()) {
            seen.push(ep.id());
            out.push(ep);
        }
    }
    out.push(EpChoice::Cpu); // 末位不变量
    out
}

/// 单次 session 构建的 EP 选型结果(telemetry,只读,不落库)。
#[derive(Debug, Clone, serde::Serialize)]
pub struct EpSelectionTelemetry {
    /// 请求的 EP 链(id 列表)。
    pub requested: Vec<String>,
    /// best-effort 推断的实际承载 EP(链首已编入 EP;ORT 不精确暴露,见 R3)。
    pub active: String,
    /// 实际 EP 是否为 best-effort 近似(ORT 不报「哪张图跑哪个 EP」)。
    pub approx: bool,
    /// 落 CPU / 降级原因(若有),含 kebab error-code。
    pub fallback_reason: Option<String>,
}

impl EpSelectionTelemetry {
    /// 从一条 EP 链构造 telemetry。`active` 取链首(best-effort)。
    pub fn from_chain(chain: &[EpChoice]) -> Self {
        let requested: Vec<String> = chain.iter().map(|e| e.id().to_string()).collect();
        let active = chain.first().map(|e| e.id().to_string()).unwrap_or_else(|| "cpu".into());
        let fallback_reason = if chain.len() == 1 && chain[0] == EpChoice::Cpu {
            Some("no hardware accelerator EP compiled/available; using CPU. error-code=accel-ep-not-compiled".to_string())
        } else {
            None
        };
        Self { requested, active, approx: true, fallback_reason }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::AccelKind;

    fn chain_ids(c: &[EpChoice]) -> Vec<&'static str> {
        c.iter().map(|e| e.id()).collect()
    }

    // ── Golden / happy(≥6) ──────────────────────────────────────────────

    #[test]
    fn cuda_compiled_picks_cuda_then_cpu() {
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::NvidiaGpu],
            &[EpChoice::Cpu, EpChoice::Cuda],
            None,
        );
        assert_eq!(chain_ids(&chain), ["cuda", "cpu"]);
    }

    #[test]
    fn intel_igpu_not_compiled_falls_to_cpu() {
        // 硬件有 Intel iGPU 但 artifact 只编了 CPU(v1 Linux deb 常态)→ [Cpu]。
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu],
            None,
        );
        assert_eq!(chain_ids(&chain), ["cpu"]);
    }

    #[test]
    fn amd_npu_not_compiled_falls_to_cpu() {
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::AmdNpu],
            &[EpChoice::Cpu],
            None,
        );
        assert_eq!(chain_ids(&chain), ["cpu"]);
    }

    #[test]
    fn no_hardware_is_cpu_only() {
        let chain = recommend_ep_chain_pure("linux", &[], &[EpChoice::Cpu], None);
        assert_eq!(chain_ids(&chain), ["cpu"]);
    }

    #[test]
    fn cuda_hardware_but_artifact_lacks_cuda_falls_to_cpu() {
        // 有 NVIDIA 卡,但当前 artifact 没编 cuda → [Cpu](E1)。
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::NvidiaGpu],
            &[EpChoice::Cpu],
            None,
        );
        assert_eq!(chain_ids(&chain), ["cpu"]);
    }

    #[test]
    fn intel_igpu_with_openvino_compiled_picks_openvino_gpu() {
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::OpenVino(OpenVinoDevice::Auto)],
            None,
        );
        assert_eq!(chain_ids(&chain), ["openvino", "cpu"]);
        // device 应是 GPU(Intel iGPU 命中 → OpenVINO device=GPU)
        assert_eq!(chain[0], EpChoice::OpenVino(OpenVinoDevice::Gpu));
    }

    #[test]
    fn amd_gpu_windows_directml_linux_rocm() {
        let win = recommend_ep_chain_pure(
            "windows",
            &[AccelKind::AmdGpu],
            &[EpChoice::Cpu, EpChoice::DirectMl],
            None,
        );
        assert_eq!(chain_ids(&win), ["directml", "cpu"]);

        let lin = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::AmdGpu],
            &[EpChoice::Cpu, EpChoice::Rocm],
            None,
        );
        assert_eq!(chain_ids(&lin), ["rocm", "cpu"]);
    }

    #[test]
    fn intel_npu_picks_openvino_npu() {
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::IntelNpu],
            &[EpChoice::Cpu, EpChoice::OpenVino(OpenVinoDevice::Auto)],
            None,
        );
        assert_eq!(chain[0], EpChoice::OpenVino(OpenVinoDevice::Npu));
    }

    #[test]
    fn amd_npu_with_vitis_compiled_picks_vitisai() {
        let chain = recommend_ep_chain_pure(
            "windows",
            &[AccelKind::AmdNpu],
            &[EpChoice::Cpu, EpChoice::VitisAi],
            None,
        );
        assert_eq!(chain_ids(&chain), ["vitisai", "cpu"]);
    }

    // ── 属性测试(≥3,手写覆盖任意组合) ─────────────────────────────────

    #[test]
    fn prop_chain_always_ends_with_cpu() {
        // 任意 hardware × compiled × env → 末位恒 CPU。
        let all_hw = [
            AccelKind::NvidiaGpu, AccelKind::AmdGpu, AccelKind::AmdNpu,
            AccelKind::IntelIgpu, AccelKind::IntelNpu, AccelKind::Cpu,
        ];
        let all_eps = [
            EpChoice::Cpu, EpChoice::Cuda, EpChoice::DirectMl,
            EpChoice::OpenVino(OpenVinoDevice::Auto), EpChoice::Rocm, EpChoice::VitisAi,
        ];
        for os in ["linux", "windows", "macos", "unknown"] {
            for hw_mask in 0u32..(1 << all_hw.len()) {
                let hw: Vec<AccelKind> = all_hw.iter().enumerate()
                    .filter(|(i, _)| hw_mask & (1 << i) != 0).map(|(_, &k)| k).collect();
                for ep_mask in 0u32..(1 << all_eps.len()) {
                    let mut compiled: Vec<EpChoice> = all_eps.iter().enumerate()
                        .filter(|(i, _)| ep_mask & (1 << i) != 0).map(|(_, &e)| e).collect();
                    if !compiled.contains(&EpChoice::Cpu) {
                        compiled.push(EpChoice::Cpu); // CPU 必在(invariant)
                    }
                    let chain = recommend_ep_chain_pure(os, &hw, &compiled, None);
                    assert_eq!(*chain.last().unwrap(), EpChoice::Cpu,
                        "os={os} hw={hw:?} compiled={compiled:?} chain must end with CPU: {chain:?}");
                }
            }
        }
    }

    #[test]
    fn prop_chain_only_contains_compiled_eps() {
        let all_hw = [AccelKind::NvidiaGpu, AccelKind::AmdGpu, AccelKind::IntelIgpu, AccelKind::IntelNpu, AccelKind::AmdNpu];
        // compiled 故意只给 CPU:任何硬件都不该让非编入 EP 入链。
        for os in ["linux", "windows"] {
            for hw_mask in 0u32..(1 << all_hw.len()) {
                let hw: Vec<AccelKind> = all_hw.iter().enumerate()
                    .filter(|(i, _)| hw_mask & (1 << i) != 0).map(|(_, &k)| k).collect();
                let chain = recommend_ep_chain_pure(os, &hw, &[EpChoice::Cpu], None);
                assert_eq!(chain, vec![EpChoice::Cpu],
                    "with only CPU compiled, chain must be [Cpu] regardless of hw={hw:?}");
            }
        }
    }

    #[test]
    fn prop_env_off_always_cpu_only() {
        let all_hw = [AccelKind::NvidiaGpu, AccelKind::AmdGpu, AccelKind::IntelIgpu];
        let compiled = [EpChoice::Cpu, EpChoice::Cuda, EpChoice::DirectMl, EpChoice::Rocm];
        for os in ["linux", "windows", "macos"] {
            for hw_mask in 0u32..(1 << all_hw.len()) {
                let hw: Vec<AccelKind> = all_hw.iter().enumerate()
                    .filter(|(i, _)| hw_mask & (1 << i) != 0).map(|(_, &k)| k).collect();
                let chain = recommend_ep_chain_pure(os, &hw, &compiled, Some(&EpOverride::DisableAccel));
                assert_eq!(chain, vec![EpChoice::Cpu],
                    "env=off must force CPU-only regardless of hw={hw:?}");
            }
        }
    }

    // ── 边界 case(≥5) ──────────────────────────────────────────────────

    #[test]
    fn boundary_empty_compiled_still_cpu_tail() {
        // compiled 为空(理论不该发生,CPU 必在)→ 仍补 CPU 兜底,不 panic。
        let chain = recommend_ep_chain_pure("linux", &[AccelKind::NvidiaGpu], &[], None);
        assert_eq!(chain, vec![EpChoice::Cpu]);
    }

    #[test]
    fn boundary_duplicate_hardware_dedups() {
        // 同硬件出现两次(理论上 detect 不会,但防御)→ EP 不重复。
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::NvidiaGpu, AccelKind::NvidiaGpu],
            &[EpChoice::Cpu, EpChoice::Cuda],
            None,
        );
        assert_eq!(chain_ids(&chain), ["cuda", "cpu"]);
    }

    #[test]
    fn boundary_env_parse_case_and_whitespace() {
        assert_eq!(parse_ep_override("  CUDA  ").unwrap(), EpOverride::Force(EpChoice::Cuda));
        assert_eq!(parse_ep_override("Off").unwrap(), EpOverride::DisableAccel);
        assert_eq!(parse_ep_override("DML").unwrap(), EpOverride::Force(EpChoice::DirectMl));
        assert!(parse_ep_override("nonsense").is_err());
    }

    #[test]
    fn boundary_force_uncompiled_falls_back_to_auto() {
        // force(cuda) 但 artifact 没编 cuda → 退回自动(此处自动也只有 CPU)。
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::NvidiaGpu],
            &[EpChoice::Cpu], // cuda 未编入
            Some(&EpOverride::Force(EpChoice::Cuda)),
        );
        assert_eq!(chain_ids(&chain), ["cpu"]);
    }

    #[test]
    fn boundary_multi_accel_priority_nvidia_first() {
        // 同机 NVIDIA + AMD GPU + Intel iGPU,全部编入 → NVIDIA 链首(硬件顺序优先级)。
        let chain = recommend_ep_chain_pure(
            "windows",
            &[AccelKind::NvidiaGpu, AccelKind::AmdGpu, AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::Cuda, EpChoice::DirectMl],
            None,
        );
        // NVIDIA→cuda 首;AMD GPU + Intel iGPU 在 Win 都映射 DirectML(去重一次)→ cuda, directml, cpu
        assert_eq!(chain_ids(&chain), ["cuda", "directml", "cpu"]);
    }

    // ── 异常 / 错误(≥3) ────────────────────────────────────────────────

    #[test]
    fn err_force_cpu_is_cpu_only() {
        let chain = recommend_ep_chain_pure(
            "linux",
            &[AccelKind::NvidiaGpu],
            &[EpChoice::Cpu, EpChoice::Cuda],
            Some(&EpOverride::Force(EpChoice::Cpu)),
        );
        assert_eq!(chain_ids(&chain), ["cpu"]);
    }

    #[test]
    fn err_invalid_env_returns_err_not_panic() {
        for bad in ["", "   ", "gpu", "metal", "vulkan", "123"] {
            // 空白本由 env_override() 提前拦;parse 对非空非法值返回 Err。
            if bad.trim().is_empty() {
                continue;
            }
            assert!(parse_ep_override(bad).is_err(), "{bad:?} should be unrecognized");
        }
    }

    #[test]
    fn err_macos_force_accel_still_listed_but_uncompiled_gives_cpu() {
        // macOS 上 force(cuda) — 不会编入(macOS 无 cuda)→ 退回自动 → CPU。
        let chain = recommend_ep_chain_pure(
            "macos",
            &[],
            &[EpChoice::Cpu],
            Some(&EpOverride::Force(EpChoice::Cuda)),
        );
        assert_eq!(chain_ids(&chain), ["cpu"]);
    }

    // ── OnceLock 缓存幂等 ──────────────────────────────────────────────

    #[test]
    fn cache_is_idempotent() {
        let a = cached_selection() as *const AccelSelection;
        let b = cached_selection() as *const AccelSelection;
        assert_eq!(a, b, "cached_selection must return the same instance");
    }

    #[test]
    fn detect_does_not_panic_and_chain_valid() {
        let sel = AccelSelection::detect();
        let chain = sel.recommend_ep_chain();
        assert!(!chain.is_empty());
        assert_eq!(*chain.last().unwrap(), EpChoice::Cpu);
        // compiled_eps 必含 CPU。
        assert!(sel.compiled.contains(&EpChoice::Cpu));
    }

    // ── telemetry ──────────────────────────────────────────────────────

    #[test]
    fn telemetry_marks_cpu_only_fallback_reason() {
        let t = EpSelectionTelemetry::from_chain(&[EpChoice::Cpu]);
        assert_eq!(t.active, "cpu");
        assert!(t.approx);
        assert!(t.fallback_reason.as_deref().unwrap().contains("accel-ep-not-compiled"));
    }

    #[test]
    fn telemetry_accel_chain_has_no_fallback_reason() {
        let t = EpSelectionTelemetry::from_chain(&[EpChoice::Cuda, EpChoice::Cpu]);
        assert_eq!(t.active, "cuda");
        assert!(t.fallback_reason.is_none());
        assert_eq!(t.requested, vec!["cuda", "cpu"]);
    }

    #[test]
    fn runtime_stack_mapping() {
        assert_eq!(EpChoice::Cpu.runtime_stack(), None);
        assert_eq!(EpChoice::CoreMl.runtime_stack(), None);
        assert_eq!(EpChoice::Cuda.runtime_stack(), Some("cuda"));
        assert_eq!(EpChoice::DirectMl.runtime_stack(), Some("directml"));
        assert_eq!(EpChoice::OpenVino(OpenVinoDevice::Auto).runtime_stack(), Some("openvino"));
        assert_eq!(EpChoice::Rocm.runtime_stack(), Some("rocm"));
        assert_eq!(EpChoice::VitisAi.runtime_stack(), Some("vitisai"));
    }
}
