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
    /// Windows DirectML(AMD/NVIDIA Windows GPU;Intel 自动策略优先 OpenVINO)。
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

/// 推理任务类别 — 不同任务对同一硬件的 EP 选择不同(per-task EP 规则)。
///
/// 背景(bench 实测,vlm-llm-bench reports/2026-06-19-all-model-matrix-results.en.md):
/// **Intel 机器上 PP-OCR/rapidocr 走 DirectML → CER 202%(全废,完全不可用)**;同机
/// OpenVINO CER 7.04%。AMD 机器 OCR 走 DirectML 反而比 CPU 快 3.4×(matrix:32)。
/// 因此 Intel 通用底座自动选型也优先 OpenVINO;AMD Windows GPU 仍保留 DirectML。
///
/// `Generic` = 通用底座推理(embedding / rerank / 一般 ONNX),按平台实测矩阵选型。
/// `Ocr` = OCR 专用,触发 per-task 过滤(Intel 禁 DirectML,强制 OpenVINO 或 CPU 兜底)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InferTask {
    /// 通用底座推理(embedding / rerank / 其他 ONNX)— 无 per-task EP 限制。
    Generic,
    /// OCR(PP-OCR / rapidocr)— Intel 禁 DirectML(实测 CER 202%),AMD 用 DirectML。
    Ocr,
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
///
/// ⚠️ **ort-dynamic clean-install hang(155H 真机实证)**:openvino/rocm 变体走
/// `ort-dynamic`(不捆绑 onnxruntime),其 dylib 在首次运行下载的 EP 栈里。栈未就位时
/// 调 `ort::ep::*::is_available()` → ORT `LoadLibrary` 一个不存在的 dll → **Windows 上
/// 卡死**。该函数经 `cached_selection()` 在 vault setup handler **同步路径**调用,故
/// clean install 时 vault setup 阻塞 120s+,且卡点使 EP 栈永不下载(双症同源)。
/// 因此 ort-dynamic 下,dylib 尚不可加载时**跳过 live 探测、乐观纳入**该 EP(让
/// `spawn_stack_bootstrap` 去拉栈;真可用性留待实际 session build,缺则 graceful CPU)。
#[cfg(all(feature = "ort-dynamic", any(feature = "openvino", feature = "rocm", feature = "vitis")))]
fn ort_dylib_loadable() -> bool {
    if std::env::var_os("ORT_DYLIB_PATH").is_some() {
        return true;
    }
    ["openvino", "rocm", "vitisai"]
        .iter()
        .any(|s| crate::infer::stack_installer::probe_stack(s))
}

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
    // openvino/rocm/vitis 走 ort-dynamic:dylib 未就位时跳过会卡死的 live 探测,
    // 乐观纳入(详见 ort_dylib_loadable)。ort-bundled 下照常探测。
    #[cfg(feature = "openvino")]
    {
        #[cfg(feature = "ort-dynamic")]
        let available = if ort_dylib_loadable() {
            ort::ep::OpenVINO::default().is_available().unwrap_or(false)
        } else {
            true
        };
        #[cfg(not(feature = "ort-dynamic"))]
        let available = ort::ep::OpenVINO::default().is_available().unwrap_or(false);
        if available {
            eps.push(EpChoice::OpenVino(OpenVinoDevice::Auto));
        }
    }
    #[cfg(feature = "rocm")]
    {
        #[cfg(feature = "ort-dynamic")]
        let available = if ort_dylib_loadable() {
            ort::ep::ROCm::default().is_available().unwrap_or(false)
        } else {
            true
        };
        #[cfg(not(feature = "ort-dynamic"))]
        let available = ort::ep::ROCm::default().is_available().unwrap_or(false);
        if available {
            eps.push(EpChoice::Rocm);
        }
    }
    #[cfg(feature = "vitis")]
    {
        #[cfg(feature = "ort-dynamic")]
        let available = if ort_dylib_loadable() {
            ort::ep::Vitis::default().is_available().unwrap_or(false)
        } else {
            true
        };
        #[cfg(not(feature = "ort-dynamic"))]
        let available = ort::ep::Vitis::default().is_available().unwrap_or(false);
        if available {
            eps.push(EpChoice::VitisAi);
        }
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

    /// 推荐有序 EP 链(通用任务,= `recommend_ep_chain_for(InferTask::Generic)`)。
    ///
    /// **末位永远 `EpChoice::Cpu`**(兜底不变量)。规则:
    /// 1. `env=off` → `[Cpu]`(强制 CPU-only)。
    /// 2. `env=force(X)` 且 X 已编入 → `[X, Cpu]`;X 未编入 → 退回自动选型(并 warn)。
    /// 3. 自动:遍历硬件优先级,把「硬件命中 ∧ 已编入」的 EP 依次入链,末尾补 CPU。
    ///
    /// 链中**不含**未编入的 EP(`is_available()==false` 已在 `compiled` 里过滤)。
    pub fn recommend_ep_chain(&self) -> Vec<EpChoice> {
        self.recommend_ep_chain_for(InferTask::Generic)
    }

    /// 推荐有序 EP 链 — **任务感知**。在通用链基础上叠加 per-task EP 规则
    /// (`InferTask::Ocr` 在 Intel 硬件上禁 DirectML,实测 CER 202%)。末位仍恒为 CPU。
    pub fn recommend_ep_chain_for(&self, task: InferTask) -> Vec<EpChoice> {
        recommend_ep_chain_for_task(
            self.os,
            &self.hardware,
            &self.compiled,
            self.env_override.as_ref(),
            task,
        )
    }

    /// **电源感知** EP 链 — 能效约束态(电池/saver/热)把 NPU 重排到链首
    /// (VitisAi/OpenVino-NPU 是 bench 实测的能效最优路径:OCR 同精度、功耗远低于
    /// GPU)。AC/Unknown → 与 `recommend_ep_chain_for` 字节一致(性能档不变)。
    ///
    /// 注意(两层调度约束):此偏好只在 **session 构造时**生效;运行时 AC↔电池切换不
    /// 重建 session(切 embedding EP 会触发全库重嵌),由 resource_governor 节流/暂停
    /// 应对(运行时主杠杆)。无 NPU 机器在电池下保持 GPU 链但靠 governor 重度节流兜底。
    pub fn recommend_ep_chain_for_power(
        &self,
        task: InferTask,
        power: &crate::platform::PowerState,
    ) -> Vec<EpChoice> {
        if power.is_energy_constrained() {
            let hw = reorder_npu_first(&self.hardware);
            recommend_ep_chain_for_task(self.os, &hw, &self.compiled, self.env_override.as_ref(), task)
        } else {
            self.recommend_ep_chain_for(task)
        }
    }

    /// 链首(实际首选)EP — telemetry / UI 显示「预计加速:X」。
    pub fn primary_ep(&self) -> EpChoice {
        self.recommend_ep_chain().into_iter().next().unwrap_or(EpChoice::Cpu)
    }

    /// 任务感知的链首 EP。
    pub fn primary_ep_for(&self, task: InferTask) -> EpChoice {
        self.recommend_ep_chain_for(task).into_iter().next().unwrap_or(EpChoice::Cpu)
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
            // Intel iGPU → OpenVINO(device=GPU). vlm-llm-benchmark reports show
            // Intel Arc/iGPU should use OpenVINO for OCR/embedding/rerank/LLM paths;
            // DirectML OCR is invalid (CER 202%) and must not be the automatic hint.
            AccelKind::IntelIgpu => {
                push_first_compiled(
                    &mut chain,
                    &[EpChoice::OpenVino(OpenVinoDevice::Gpu)],
                    &is_compiled,
                );
                continue;
            }
            AccelKind::Cpu => continue, // CPU 由末尾兜底统一加
        };
        push_first_compiled(&mut chain, candidates, &is_compiled);
    }

    dedup_with_cpu_tail(chain)
}

/// 能效约束态(电池/saver/热)把 NPU 类硬件重排到序首,其余保持相对优先序。
/// NPU(AMD XDNA / Intel NPU)是 vlm-llm-bench 实测的能效最优路径,故电池下优先。
/// 纯函数,可测。
fn reorder_npu_first(hw: &[AccelKind]) -> Vec<AccelKind> {
    let is_npu = |h: &AccelKind| matches!(h, AccelKind::AmdNpu | AccelKind::IntelNpu);
    let mut out: Vec<AccelKind> = hw.iter().copied().filter(is_npu).collect();
    out.extend(hw.iter().copied().filter(|h| !is_npu(h)));
    out
}

/// 任务感知的 EP 链选型(在通用 `recommend_ep_chain_pure` 上叠加 per-task 规则)。
///
/// 当前唯一 per-task 规则:**OCR 在 Intel 硬件上禁 DirectML**(bench 实测 Intel+DirectML
/// OCR CER 202% 全废 → 必须 OpenVINO 或 CPU 兜底)。AMD OCR 仍用 DirectML(比 CPU 快 3.4×)。
///
/// 实现:`Generic` 任务直接走通用链;`Ocr` 任务先算通用链,然后:
/// 1. 若机器有 Intel 加速器(IntelIgpu / IntelNpu),从链中**剔除 DirectML**
///    (含 env `Force(DirectMl)` 的情形 — 已知全废组合,不让它产生垃圾输出);
/// 2. 若剔除后 Intel iGPU 有 OpenVINO 已编入且不在链中,补 OpenVINO(device=GPU)在 CPU 前;
/// 3. 末位 CPU 兜底不变量恒成立。
///
/// 纯函数,无副作用(可任意 os × hardware × compiled × env × task 组合测试)。
pub fn recommend_ep_chain_for_task(
    os: &str,
    hardware: &[AccelKind],
    compiled: &[EpChoice],
    over: Option<&EpOverride>,
    task: InferTask,
) -> Vec<EpChoice> {
    let base = recommend_ep_chain_pure(os, hardware, compiled, over);

    match task {
        InferTask::Generic => base,
        InferTask::Ocr => apply_ocr_ep_rules(base, hardware, compiled),
    }
}

/// 对一条已选 EP 链施加 OCR per-task 规则(见 `recommend_ep_chain_for_task`)。
fn apply_ocr_ep_rules(
    base: Vec<EpChoice>,
    hardware: &[AccelKind],
    compiled: &[EpChoice],
) -> Vec<EpChoice> {
    let has_intel = hardware
        .iter()
        .any(|k| matches!(k, AccelKind::IntelIgpu | AccelKind::IntelNpu));

    // 无 Intel 硬件 → OCR 规则不触发(AMD/NVIDIA 链按通用,DirectML on AMD 是实测最优)。
    if !has_intel {
        return base;
    }

    let is_compiled = |c: EpChoice| compiled.iter().any(|x| x.id() == c.id());

    // Intel + OCR:剔除 DirectML(实测全废),其余保序。
    let mut filtered: Vec<EpChoice> = base.into_iter().filter(|e| *e != EpChoice::DirectMl).collect();

    // 若有 Intel iGPU 且 OpenVINO 已编入但不在链中(例如 env 覆盖/旧策略造成),
    // 补 OpenVINO(device=GPU)到 CPU 前 —— 给 Intel OCR 一个实测可用的加速 EP。
    let has_intel_igpu = hardware.iter().any(|k| matches!(k, AccelKind::IntelIgpu));
    let ov_gpu = EpChoice::OpenVino(OpenVinoDevice::Gpu);
    let already_has_ov = filtered.iter().any(|e| e.id() == ov_gpu.id());
    if has_intel_igpu && is_compiled(ov_gpu) && !already_has_ov {
        // 插到末位 CPU 之前(若有 CPU);否则 push 后由 dedup_with_cpu_tail 收尾。
        filtered.push(ov_gpu);
    }

    // 重新整理(去重 + 末位 CPU 不变量)。
    dedup_with_cpu_tail(filtered)
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
    fn intel_igpu_windows_with_openvino_compiled_picks_openvino_gpu() {
        let chain = recommend_ep_chain_pure(
            "windows",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::DirectMl, EpChoice::OpenVino(OpenVinoDevice::Auto)],
            None,
        );
        assert_eq!(chain_ids(&chain), ["openvino", "cpu"]);
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
        // NVIDIA→cuda 首;AMD GPU 在 Win 映射 DirectML;Intel iGPU 只在 OpenVINO 编入时补 OpenVINO。
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

    // ── OCR per-task EP 规则(任务 1:GA-level latent bug 修复) ──────────
    //
    // bench 实测:Intel + DirectML OCR CER 202%(全废)→ 必须 OpenVINO 或 CPU。
    // AMD + DirectML OCR 比 CPU 快 3.4×(保留)。各 tier 末位 CPU 兜底。

    #[test]
    fn ocr_intel_igpu_windows_never_picks_directml() {
        // 通用链:Intel iGPU + Win + 只编 DirectML → 不再误走 DirectML,直接 CPU 兜底。
        let generic = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::DirectMl],
            None,
            InferTask::Generic,
        );
        assert_eq!(chain_ids(&generic), ["cpu"]);

        // OCR 链:DirectML 被剔除;无 OpenVINO 编入 → 落 [cpu](兜底,不全废)。
        let ocr = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::DirectMl],
            None,
            InferTask::Ocr,
        );
        assert_eq!(chain_ids(&ocr), ["cpu"], "Intel OCR must NOT use DirectML (CER 202%)");
        assert!(!ocr.contains(&EpChoice::DirectMl));
    }

    #[test]
    fn ocr_intel_igpu_windows_with_openvino_prefers_openvino() {
        // Intel iGPU + Win, DirectML + OpenVINO 都编入 → OCR 选 OpenVINO(GPU),不选 DirectML。
        let ocr = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::DirectMl, EpChoice::OpenVino(OpenVinoDevice::Auto)],
            None,
            InferTask::Ocr,
        );
        assert!(!ocr.contains(&EpChoice::DirectMl), "Intel OCR must drop DirectML");
        assert!(ocr.iter().any(|e| e.id() == "openvino"), "Intel OCR should use OpenVINO");
        assert_eq!(*ocr.last().unwrap(), EpChoice::Cpu);
    }

    #[test]
    fn ocr_intel_npu_windows_never_picks_directml() {
        // 同机 Intel NPU + iGPU(常见 Core Ultra)+ Win + DirectML → OCR 不得选 DirectML。
        let ocr = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::IntelNpu, AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::DirectMl],
            None,
            InferTask::Ocr,
        );
        assert!(!ocr.contains(&EpChoice::DirectMl));
        assert_eq!(*ocr.last().unwrap(), EpChoice::Cpu);
    }

    #[test]
    fn ocr_intel_force_directml_env_still_dropped() {
        // 即便用户 env 强制 DirectML,Intel OCR 仍剔除(已知全废组合,不让产生垃圾输出)。
        let ocr = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::DirectMl],
            Some(&EpOverride::Force(EpChoice::DirectMl)),
            InferTask::Ocr,
        );
        assert!(!ocr.contains(&EpChoice::DirectMl), "Intel OCR drops DirectML even on env force");
        assert_eq!(chain_ids(&ocr), ["cpu"]);
    }

    #[test]
    fn ocr_amd_gpu_windows_keeps_directml() {
        // AMD GPU + Win + DirectML → OCR 保留 DirectML(实测比 CPU 快 3.4×)。
        let ocr = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::AmdGpu],
            &[EpChoice::Cpu, EpChoice::DirectMl],
            None,
            InferTask::Ocr,
        );
        assert_eq!(chain_ids(&ocr), ["directml", "cpu"], "AMD OCR keeps DirectML (3.4x faster)");
    }

    #[test]
    fn ocr_amd_npu_windows_directml_present_keeps_it() {
        // AMD NPU 机(也有 AMD iGPU 走 DirectML) — OCR 不受 Intel 规则影响。
        let ocr = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::AmdGpu, AccelKind::AmdNpu],
            &[EpChoice::Cpu, EpChoice::DirectMl, EpChoice::VitisAi],
            None,
            InferTask::Ocr,
        );
        // AMD 机 OCR 链不剔除 DirectML(无 Intel 硬件 → OCR 规则不触发)。
        assert!(ocr.contains(&EpChoice::DirectMl) || ocr.contains(&EpChoice::VitisAi));
        assert_eq!(*ocr.last().unwrap(), EpChoice::Cpu);
    }

    #[test]
    fn ocr_cpu_only_machine_falls_back_to_cpu() {
        // 无加速硬件 → OCR 链 = [cpu](各 tier 末位 CPU 兜底)。
        let ocr = recommend_ep_chain_for_task("linux", &[], &[EpChoice::Cpu], None, InferTask::Ocr);
        assert_eq!(chain_ids(&ocr), ["cpu"]);
    }

    #[test]
    fn ocr_nvidia_cuda_unaffected_by_ocr_rule() {
        // NVIDIA + CUDA → OCR 规则只针对 Intel DirectML,不动 CUDA。
        let ocr = recommend_ep_chain_for_task(
            "linux",
            &[AccelKind::NvidiaGpu],
            &[EpChoice::Cpu, EpChoice::Cuda],
            None,
            InferTask::Ocr,
        );
        assert_eq!(chain_ids(&ocr), ["cuda", "cpu"]);
    }

    #[test]
    fn ocr_intel_igpu_linux_uses_openvino_gpu() {
        // Linux Intel iGPU: 通用链已是 OpenVINO(GPU)(非 DirectML)→ OCR 链相同(无 DirectML 可剔)。
        let ocr = recommend_ep_chain_for_task(
            "linux",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::OpenVino(OpenVinoDevice::Auto)],
            None,
            InferTask::Ocr,
        );
        assert_eq!(ocr[0], EpChoice::OpenVino(OpenVinoDevice::Gpu));
        assert_eq!(*ocr.last().unwrap(), EpChoice::Cpu);
    }

    #[test]
    fn prop_ocr_intel_never_yields_directml() {
        // 属性:任何 os × compiled × env 组合下,只要硬件含 Intel,OCR 链恒不含 DirectML。
        let intel_hw_sets: &[&[AccelKind]] = &[
            &[AccelKind::IntelIgpu],
            &[AccelKind::IntelNpu],
            &[AccelKind::IntelNpu, AccelKind::IntelIgpu],
        ];
        let all_eps = [
            EpChoice::Cpu, EpChoice::Cuda, EpChoice::DirectMl,
            EpChoice::OpenVino(OpenVinoDevice::Auto), EpChoice::Rocm, EpChoice::VitisAi,
        ];
        let overrides: &[Option<EpOverride>] = &[
            None,
            Some(EpOverride::Force(EpChoice::DirectMl)),
            Some(EpOverride::Force(EpChoice::OpenVino(OpenVinoDevice::Auto))),
        ];
        for os in ["windows", "linux", "macos"] {
            for hw in intel_hw_sets {
                for ep_mask in 0u32..(1 << all_eps.len()) {
                    let mut compiled: Vec<EpChoice> = all_eps.iter().enumerate()
                        .filter(|(i, _)| ep_mask & (1 << i) != 0).map(|(_, &e)| e).collect();
                    if !compiled.contains(&EpChoice::Cpu) {
                        compiled.push(EpChoice::Cpu);
                    }
                    for over in overrides {
                        let chain = recommend_ep_chain_for_task(os, hw, &compiled, over.as_ref(), InferTask::Ocr);
                        assert!(!chain.contains(&EpChoice::DirectMl),
                            "Intel OCR must never contain DirectML: os={os} hw={hw:?} compiled={compiled:?} over={over:?} chain={chain:?}");
                        assert_eq!(*chain.last().unwrap(), EpChoice::Cpu,
                            "OCR chain must end with CPU: {chain:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn prop_ocr_generic_identical_when_no_intel() {
        // 无 Intel 硬件时,OCR 链 == 通用链(OCR 规则不改 AMD/NVIDIA 行为)。
        let non_intel_hw: &[&[AccelKind]] = &[
            &[AccelKind::NvidiaGpu],
            &[AccelKind::AmdGpu],
            &[AccelKind::AmdGpu, AccelKind::AmdNpu],
            &[],
        ];
        let compiled = [EpChoice::Cpu, EpChoice::Cuda, EpChoice::DirectMl, EpChoice::Rocm, EpChoice::VitisAi];
        for os in ["windows", "linux"] {
            for hw in non_intel_hw {
                let generic = recommend_ep_chain_for_task(os, hw, &compiled, None, InferTask::Generic);
                let ocr = recommend_ep_chain_for_task(os, hw, &compiled, None, InferTask::Ocr);
                assert_eq!(generic, ocr, "non-Intel: OCR chain must equal generic: os={os} hw={hw:?}");
            }
        }
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

    // ── 电源感知 EP 偏好 (0.3) ──
    use crate::platform::{PowerProfile, PowerSource, PowerState};

    fn battery_state() -> PowerState {
        PowerState {
            source: PowerSource::Battery,
            battery_pct: Some(70),
            profile: PowerProfile::Balanced,
            thermal_pressure: false,
        }
    }

    #[test]
    fn reorder_npu_first_moves_npu_ahead() {
        let hw = vec![AccelKind::IntelIgpu, AccelKind::IntelNpu, AccelKind::Cpu];
        let out = reorder_npu_first(&hw);
        assert_eq!(out[0], AccelKind::IntelNpu, "NPU 重排到序首");
        assert_eq!(out[1], AccelKind::IntelIgpu);
    }

    #[test]
    fn reorder_npu_first_noop_when_no_npu() {
        let hw = vec![AccelKind::AmdGpu, AccelKind::Cpu];
        assert_eq!(reorder_npu_first(&hw), hw, "无 NPU → 顺序不变");
    }

    #[test]
    fn power_ac_keeps_gpu_first_battery_prefers_npu() {
        // Intel 笔电:iGPU(性能优先序在前) + NPU。compiled 含两者 EP。
        let sel = AccelSelection {
            os: "windows",
            hardware: vec![AccelKind::IntelIgpu, AccelKind::IntelNpu],
            compiled: vec![
                EpChoice::OpenVino(OpenVinoDevice::Gpu),
                EpChoice::OpenVino(OpenVinoDevice::Npu),
                EpChoice::Cpu,
            ],
            env_override: None,
        };
        // AC：iGPU 优先 → OpenVINO(GPU) 链首(Intel DirectML OCR/embedding/rerank 不作为自动首选)。
        let ac = sel.recommend_ep_chain_for_power(InferTask::Generic, &PowerState::default());
        assert_eq!(
            ac[0],
            EpChoice::OpenVino(OpenVinoDevice::Gpu),
            "AC 性能档:Intel OpenVINO GPU 链首"
        );
        // 电池：NPU 重排到首 → OpenVINO(NPU) 链首(能效路径)。
        let bat = sel.recommend_ep_chain_for_power(InferTask::Generic, &battery_state());
        assert_eq!(
            bat[0],
            EpChoice::OpenVino(OpenVinoDevice::Npu),
            "电池能效档:NPU 链首"
        );
        // 末位恒 CPU 兜底(两种电源态都成立)。
        assert_eq!(*ac.last().unwrap(), EpChoice::Cpu);
        assert_eq!(*bat.last().unwrap(), EpChoice::Cpu);
    }

    #[test]
    fn power_no_npu_battery_keeps_chain_governor_throttles() {
        // 无 NPU 机器:电池下链不变(GPU 仍在),靠 resource_governor 节流兜底。
        let sel = AccelSelection {
            os: "windows",
            hardware: vec![AccelKind::AmdGpu],
            compiled: vec![EpChoice::DirectMl, EpChoice::Cpu],
            env_override: None,
        };
        let ac = sel.recommend_ep_chain_for_power(InferTask::Generic, &PowerState::default());
        let bat = sel.recommend_ep_chain_for_power(InferTask::Generic, &battery_state());
        assert_eq!(ac, bat, "无 NPU → 电源态不改 EP 链(governor 负责节流)");
        assert_eq!(ac[0], EpChoice::DirectMl);
    }
}
