//! 统一硬件加速器检测层（unified accelerator capabilities）。
//!
//! 把 `HardwareProfile`（#6，mod.rs 内的原始探测：PCI vendor / /dev/nvidia* /
//! /dev/accel / /proc/modules / CPU 名）汇成一个**全加速器统一视图**，给底座
//! ONNX Runtime 选择 Execution Provider 提供依据。
//!
//! **范围**：本模块只做**检测 / 枚举 + EP 提示字符串**。真正的 ORT EP 接线
//! （session builder 注册 provider）是后续任务，这里只给 `recommended_ep_hint()`
//! 建议。
//!
//! **设计**：原始 OS 探测全部委托给 `HardwareProfile`（不重写、不重复 fs/命令读）。
//! 本模块的分类逻辑 (`classify_*`) 是**纯函数**——吃 bool / &str，吐 `Accelerator`
//! ——因此可被单元测试无副作用地 mock（各厂商组合 → 正确分类），不需要真硬件。
//!
//! 所有探测只读、保守降级、不 panic（沿用 npu/HardwareProfile 风格）。

use super::HardwareProfile;

/// 加速器类别。`recommended_ep_hint()` 据此 + 当前 OS 选 ORT EP。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelKind {
    /// 始终存在的 CPU 后备。
    Cpu,
    /// NVIDIA dGPU（CUDA / TensorRT）。
    NvidiaGpu,
    /// AMD RDNA GPU（dGPU 或 RDNA APU 集显，走 ROCm / DirectML）。
    AmdGpu,
    /// AMD XDNA NPU（Ryzen AI，走 VitisAI EP）。
    AmdNpu,
    /// Intel iGPU（Arc / Iris Xe，走 OpenVINO）。
    IntelIgpu,
    /// Intel NPU（Core Ultra AI Boost，走 OpenVINO）。
    IntelNpu,
}

impl AccelKind {
    /// 稳定的机读标识（API / 诊断输出用 — 不本地化）。
    pub fn id(&self) -> &'static str {
        match self {
            AccelKind::Cpu => "cpu",
            AccelKind::NvidiaGpu => "nvidia_gpu",
            AccelKind::AmdGpu => "amd_gpu",
            AccelKind::AmdNpu => "amd_npu",
            AccelKind::IntelIgpu => "intel_igpu",
            AccelKind::IntelNpu => "intel_npu",
        }
    }
}

/// 单个加速器的检测结果。
///
/// - `present`：硬件存在（检测到设备 / vendor id）。
/// - `driver_ready`：内核驱动 / runtime 已就绪（NPU 需 accel char dev + 内核模块；
///   GPU 需 render node / nvidia dev）。`present && !driver_ready` 表示"有硬件但
///   驱动没装好"，UI 可据此提示用户装驱动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accelerator {
    pub kind: AccelKind,
    /// 厂商标识："nvidia" / "amd" / "intel" / "generic"（CPU）。
    pub vendor: &'static str,
    pub present: bool,
    pub driver_ready: bool,
    /// 人类可读注记（驱动状态 / EP 路径提示），非本地化（诊断用）。
    pub notes: String,
}

/// 本机加速器统一视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccelCapabilities {
    /// 当前 OS："linux" / "macos" / "windows" / "unknown"。
    pub os: &'static str,
    /// 检测到的加速器（含 present=false 的"有驱动无硬件"占位？不——只收 present 的 +
    /// 始终在的 CPU）。顺序 = 优先级递减（NPU/GPU 在前，CPU 兜底在后）。
    pub accelerators: Vec<Accelerator>,
}

impl AccelCapabilities {
    /// 检测本机加速器（只读、幂等、无副作用）。复用 `HardwareProfile::detect()`。
    pub fn detect() -> Self {
        Self::from_profile(&HardwareProfile::detect())
    }

    /// 从已有的 `HardwareProfile` 构建（避免二次探测；测试也走这条纯路径）。
    pub fn from_profile(p: &HardwareProfile) -> Self {
        let mut accelerators = Vec::new();

        // NPU / GPU 优先级递减；CPU 永远最后兜底。
        // NVIDIA：present 即 driver_ready（/dev/nvidia0 存在意味着内核驱动已加载）。
        if p.has_nvidia_gpu {
            accelerators.push(classify_nvidia(true));
        }
        // AMD XDNA NPU（Ryzen AI）：复用 HardwareProfile 的 amdxdna 探测。
        if p.has_amd_xdna_npu {
            accelerators.push(classify_amd_npu(true));
        }
        // Intel NPU（Core Ultra AI Boost）：复用 intel_vpu 探测。
        if p.has_intel_npu {
            accelerators.push(classify_intel_npu(true));
        }
        // AMD RDNA GPU：present 时 driver_ready（render node / kfd 已被探测到）。
        if p.has_amd_gpu {
            accelerators.push(classify_amd_gpu(true, p.amd_gfx_target.as_deref()));
        }
        // Intel iGPU（Arc / Iris Xe）。
        if p.has_intel_igpu {
            accelerators.push(classify_intel_igpu(true));
        }

        // CPU 始终存在（ORT CPU EP 兜底）。
        accelerators.push(classify_cpu(&p.cpu_model));

        Self { os: p.os, accelerators }
    }

    /// 是否有任何非 CPU 加速器。
    pub fn has_hw_accelerator(&self) -> bool {
        self.accelerators
            .iter()
            .any(|a| a.kind != AccelKind::Cpu && a.driver_ready)
    }

    /// 推荐的 ORT Execution Provider 提示（**仅建议字符串**，不接线）。
    ///
    /// 按"当前 OS + 检测到的就绪加速器"选最优 EP。优先级反映各 EP 在对应硬件上的
    /// 实际可用性 / 成熟度：
    ///
    /// | 加速器 (driver_ready)     | Linux        | Windows      | macOS  |
    /// |---------------------------|--------------|--------------|--------|
    /// | NVIDIA GPU                | `cuda`       | `cuda`       | —      |
    /// | AMD XDNA NPU              | `vitisai`    | `vitisai`    | —      |
    /// | Intel NPU                 | `openvino`   | `openvino`   | —      |
    /// | AMD RDNA GPU              | `rocm`       | `directml`   | —      |
    /// | Intel iGPU                | `openvino`   | `openvino`   | —      |
    /// | (none) / macOS            | `cpu`        | `cpu`        | `cpu`  |
    ///
    /// macOS 上 ORT 的 GPU EP（CoreML）此项目暂不投入（per CLAUDE.md macOS 暂不做），
    /// 一律 `cpu`。返回的字符串与 ORT EP 命名约定一致，后续接线任务直接映射。
    pub fn recommended_ep_hint(&self) -> &'static str {
        recommended_ep_for(self.os, &self.accelerators)
    }

    /// 人类可读诊断（一行）。
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("os={}", self.os)];
        for a in &self.accelerators {
            parts.push(format!(
                "{}({}{})",
                a.kind.id(),
                a.vendor,
                if a.present && !a.driver_ready { ",no-driver" } else { "" }
            ));
        }
        parts.push(format!("ep_hint={}", self.recommended_ep_hint()));
        parts.join(" | ")
    }
}

// ── 纯分类函数（可单元测试，无副作用） ──────────────────────────────────────

fn classify_cpu(cpu_model: &str) -> Accelerator {
    let notes = if cpu_model.is_empty() {
        "CPU fallback (ORT CPU EP)".to_string()
    } else {
        format!("CPU fallback (ORT CPU EP) — {cpu_model}")
    };
    Accelerator { kind: AccelKind::Cpu, vendor: "generic", present: true, driver_ready: true, notes }
}

fn classify_nvidia(present: bool) -> Accelerator {
    Accelerator {
        kind: AccelKind::NvidiaGpu,
        vendor: "nvidia",
        present,
        // /dev/nvidia0 存在 ⇒ 内核驱动已加载，CUDA EP 可用。
        driver_ready: present,
        notes: "NVIDIA GPU (CUDA EP)".to_string(),
    }
}

fn classify_amd_gpu(present: bool, gfx_target: Option<&str>) -> Accelerator {
    let notes = match gfx_target {
        Some(g) => format!("AMD RDNA GPU gfx={g} (ROCm/DirectML EP)"),
        None => "AMD RDNA GPU (ROCm/DirectML EP)".to_string(),
    };
    Accelerator { kind: AccelKind::AmdGpu, vendor: "amd", present, driver_ready: present, notes }
}

fn classify_amd_npu(present: bool) -> Accelerator {
    Accelerator {
        kind: AccelKind::AmdNpu,
        vendor: "amd",
        present,
        // has_amd_xdna_npu 已要求 /dev/accel/accel0 + amdxdna 模块 ⇒ 驱动就绪。
        driver_ready: present,
        notes: "AMD XDNA NPU / Ryzen AI (VitisAI EP)".to_string(),
    }
}

fn classify_intel_igpu(present: bool) -> Accelerator {
    Accelerator {
        kind: AccelKind::IntelIgpu,
        vendor: "intel",
        present,
        driver_ready: present,
        notes: "Intel iGPU Arc/Iris Xe (OpenVINO EP)".to_string(),
    }
}

fn classify_intel_npu(present: bool) -> Accelerator {
    Accelerator {
        kind: AccelKind::IntelNpu,
        vendor: "intel",
        present,
        // has_intel_npu 已要求 /dev/accel/accel0 + intel_vpu 模块 ⇒ 驱动就绪。
        driver_ready: present,
        notes: "Intel NPU / Core Ultra AI Boost (OpenVINO EP)".to_string(),
    }
}

/// EP 选择纯函数（与 `recommended_ep_hint` 同逻辑，单独抽出以便测试任意组合）。
///
/// macOS 一律 cpu。其他 OS 按就绪加速器优先级选 EP。
fn recommended_ep_for(os: &str, accels: &[Accelerator]) -> &'static str {
    if os == "macos" {
        return "cpu";
    }
    let ready = |k: AccelKind| accels.iter().any(|a| a.kind == k && a.driver_ready);
    let windows = os == "windows";

    // 优先级：NVIDIA > AMD NPU > Intel NPU > AMD GPU > Intel iGPU > CPU。
    if ready(AccelKind::NvidiaGpu) {
        "cuda"
    } else if ready(AccelKind::AmdNpu) {
        "vitisai"
    } else if ready(AccelKind::IntelNpu) {
        "openvino"
    } else if ready(AccelKind::AmdGpu) {
        if windows { "directml" } else { "rocm" }
    } else if ready(AccelKind::IntelIgpu) {
        "openvino"
    } else {
        "cpu"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个只设了指定加速器 flag 的 HardwareProfile（其余为 Default）。
    fn profile(os: &'static str) -> HardwareProfile {
        HardwareProfile { os, ..Default::default() }
    }

    #[test]
    fn detect_does_not_panic_and_always_has_cpu() {
        // 真机检测：不 panic，且 CPU 永远在列（兜底）。
        let caps = AccelCapabilities::detect();
        assert!(caps.accelerators.iter().any(|a| a.kind == AccelKind::Cpu));
        // CPU 必须是最后一个（优先级最低）。
        assert_eq!(caps.accelerators.last().unwrap().kind, AccelKind::Cpu);
        let _ = caps.summary(); // summary 也不 panic
    }

    #[test]
    fn bare_system_only_cpu_and_hints_cpu() {
        // 无任何加速器 → 只 CPU → ep_hint = cpu。
        let p = profile("linux");
        let caps = AccelCapabilities::from_profile(&p);
        assert_eq!(caps.accelerators.len(), 1);
        assert_eq!(caps.accelerators[0].kind, AccelKind::Cpu);
        assert!(!caps.has_hw_accelerator());
        assert_eq!(caps.recommended_ep_hint(), "cpu");
    }

    #[test]
    fn nvidia_classified_and_hints_cuda() {
        let mut p = profile("linux");
        p.has_nvidia_gpu = true;
        let caps = AccelCapabilities::from_profile(&p);
        let nv = caps.accelerators.iter().find(|a| a.kind == AccelKind::NvidiaGpu).unwrap();
        assert_eq!(nv.vendor, "nvidia");
        assert!(nv.present && nv.driver_ready);
        assert!(caps.has_hw_accelerator());
        assert_eq!(caps.recommended_ep_hint(), "cuda"); // cuda on both linux & windows
    }

    #[test]
    fn nvidia_hint_is_cuda_on_windows_too() {
        let mut p = profile("windows");
        p.has_nvidia_gpu = true;
        assert_eq!(AccelCapabilities::from_profile(&p).recommended_ep_hint(), "cuda");
    }

    #[test]
    fn amd_xdna_npu_hints_vitisai() {
        let mut p = profile("linux");
        p.has_amd_xdna_npu = true;
        let caps = AccelCapabilities::from_profile(&p);
        let npu = caps.accelerators.iter().find(|a| a.kind == AccelKind::AmdNpu).unwrap();
        assert_eq!(npu.vendor, "amd");
        assert_eq!(caps.recommended_ep_hint(), "vitisai");
    }

    #[test]
    fn intel_npu_hints_openvino() {
        let mut p = profile("linux");
        p.has_intel_npu = true;
        let caps = AccelCapabilities::from_profile(&p);
        assert!(caps.accelerators.iter().any(|a| a.kind == AccelKind::IntelNpu));
        assert_eq!(caps.recommended_ep_hint(), "openvino");
    }

    #[test]
    fn amd_gpu_hint_os_dependent() {
        let mut p = profile("linux");
        p.has_amd_gpu = true;
        p.amd_gfx_target = Some("gfx1103".to_string());
        let caps = AccelCapabilities::from_profile(&p);
        let gpu = caps.accelerators.iter().find(|a| a.kind == AccelKind::AmdGpu).unwrap();
        assert!(gpu.notes.contains("gfx1103"), "notes should carry gfx target: {}", gpu.notes);
        assert_eq!(caps.recommended_ep_hint(), "rocm"); // linux → rocm

        p.os = "windows";
        assert_eq!(AccelCapabilities::from_profile(&p).recommended_ep_hint(), "directml");
    }

    #[test]
    fn intel_igpu_hint_os_dependent() {
        let mut p = profile("linux");
        p.has_intel_igpu = true;
        assert_eq!(AccelCapabilities::from_profile(&p).recommended_ep_hint(), "openvino"); // linux

        p.os = "windows";
        assert_eq!(AccelCapabilities::from_profile(&p).recommended_ep_hint(), "openvino"); // windows
    }

    #[test]
    fn macos_always_cpu_even_with_accel() {
        // macOS 暂不投入 GPU EP → 即使探测到加速器也只 cpu。
        let mut p = profile("macos");
        p.has_nvidia_gpu = true;
        p.has_amd_gpu = true;
        assert_eq!(AccelCapabilities::from_profile(&p).recommended_ep_hint(), "cpu");
    }

    #[test]
    fn priority_npu_over_gpu_when_both_present() {
        // 同机有 NVIDIA GPU + AMD NPU + AMD GPU：NVIDIA 优先级最高 → cuda。
        let mut p = profile("linux");
        p.has_nvidia_gpu = true;
        p.has_amd_xdna_npu = true;
        p.has_amd_gpu = true;
        assert_eq!(AccelCapabilities::from_profile(&p).recommended_ep_hint(), "cuda");
    }

    #[test]
    fn priority_amd_npu_over_amd_gpu() {
        // 无 NVIDIA，AMD NPU + AMD GPU 共存：NPU 优先 → vitisai。
        let mut p = profile("linux");
        p.has_amd_xdna_npu = true;
        p.has_amd_gpu = true;
        assert_eq!(AccelCapabilities::from_profile(&p).recommended_ep_hint(), "vitisai");
    }

    #[test]
    fn driver_not_ready_excluded_from_hint() {
        // 纯分类函数：present 但 driver 未就绪 → 不参与 EP 选择。
        let not_ready = Accelerator {
            kind: AccelKind::NvidiaGpu,
            vendor: "nvidia",
            present: true,
            driver_ready: false,
            notes: "hw present, no driver".into(),
        };
        assert_eq!(recommended_ep_for("linux", &[not_ready]), "cpu");
    }

    #[test]
    fn accel_kind_ids_stable_and_unique() {
        use std::collections::HashSet;
        let kinds = [
            AccelKind::Cpu, AccelKind::NvidiaGpu, AccelKind::AmdGpu,
            AccelKind::AmdNpu, AccelKind::IntelIgpu, AccelKind::IntelNpu,
        ];
        let ids: HashSet<_> = kinds.iter().map(|k| k.id()).collect();
        assert_eq!(ids.len(), kinds.len(), "ids must be unique");
        assert_eq!(AccelKind::NvidiaGpu.id(), "nvidia_gpu");
    }

    #[test]
    fn cpu_notes_include_model_when_known() {
        let p = HardwareProfile { os: "linux", cpu_model: "AMD Ryzen 7 8845H".into(), ..Default::default() };
        let caps = AccelCapabilities::from_profile(&p);
        let cpu = caps.accelerators.iter().find(|a| a.kind == AccelKind::Cpu).unwrap();
        assert!(cpu.notes.contains("8845H"), "cpu notes should carry model: {}", cpu.notes);
    }

    #[test]
    fn summary_contains_os_and_ep_hint() {
        let mut p = profile("linux");
        p.has_nvidia_gpu = true;
        let s = AccelCapabilities::from_profile(&p).summary();
        assert!(s.contains("os=linux"), "summary: {s}");
        assert!(s.contains("ep_hint=cuda"), "summary: {s}");
    }
}
