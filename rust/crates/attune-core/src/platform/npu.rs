//! AMD Ryzen AI (XDNA) NPU 细粒度检测 + consent-gated 驱动安装引导。
//!
//! 背景:`HardwareProfile`(见 `platform/mod.rs`)只粗粒度探测 NPU 在不在
//! (`has_amd_xdna_npu`)。本模块补齐 Python `detector.py` 的等价能力:识别芯片代、
//! 判断 amdxdna 内核模块/firmware/device node/内核版本/IOMMU 是否就位,并产出**安装计划**
//! —— 缺失项对应的引导命令,每条标注危险等级。
//!
//! **安全边界(硬约束)**:本模块**绝不**真正执行换内核 / DKMS 编译 / 提权安装。最危险的
//! 步骤也只是产出一条 `consent_required=true` 的命令字符串,由 UI 经用户同意后执行。
//! 能安全自动的(如 firmware 包安装)标 `SafeAuto`,但执行权仍在上层调用方。
//!
//! 来源(已核实,§6.3):
//! - kernel.org amdnpu: 驱动模块 `amdxdna`、device node `/dev/accel/accel0`、firmware `/lib/firmware/amdnpu/`
//! - Gentoo wiki / xdna-driver: PCI `1022:1502`(Phoenix/Hawk Point rev 0x00)、`1022:17f0`(Strix rev 0x10/0x11)
//! - amd/RyzenAI-SW: Windows 侧经 Ryzen AI Software installer 装 NPU 驱动

/// 危险等级 —— 决定一条安装步骤能否被自动执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Danger {
    /// 可安全自动执行(幂等、无破坏性、不换内核)。例:装 linux-firmware 包。
    /// 注意:即便是 SafeAuto,本模块也只产出命令;执行权在上层 + 用户同意。
    SafeAuto,
    /// 需用户显式同意(可能提权 / 加载内核模块)。例:`sudo modprobe amdxdna`。
    NeedsConsent,
    /// 必须人工操作(换内核 / BIOS / 下载安装器),不提供自动命令。
    Manual,
}

impl Danger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Danger::SafeAuto => "safe-auto",
            Danger::NeedsConsent => "needs-consent",
            Danger::Manual => "manual",
        }
    }
}

/// 单条安装/修复步骤。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallStep {
    /// 人类可读描述(本地化由 UI 负责,这里给中文+技术标识)。
    pub description: String,
    /// 可执行命令;`None` 表示纯手工步骤(如「下载 RyzenAI 安装器」)。
    pub command: Option<String>,
    pub danger: Danger,
    /// 是否需要用户显式同意才能由上层执行。
    pub consent_required: bool,
}

/// 缺失项 + 修复步骤集合。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NpuInstallPlan {
    /// 缺失能力的人类可读列表。
    pub missing: Vec<String>,
    pub steps: Vec<InstallStep>,
}

impl NpuInstallPlan {
    /// 是否一切就位(无缺失)。
    pub fn is_ready(&self) -> bool {
        self.missing.is_empty()
    }
}

/// AMD NPU 芯片代静态条目。
#[derive(Debug, Clone, Copy)]
pub struct AmdNpuChip {
    pub chip_id: &'static str,
    pub name: &'static str,
    pub xdna_version: u8,
    pub tops: u16,
    /// 主线 amdxdna 驱动最低内核版本(out-of-tree DKMS 可更早,见 notes)。
    pub min_kernel: &'static str,
    /// CPU model name 中的识别关键字(小写匹配)。
    pub cpu_keywords: &'static [&'static str],
    /// PCI device id(vendor 固定 0x1022 AMD)。
    pub pci_device: u16,
    /// PCI revision 匹配(用于区分 Phoenix vs Strix vs Strix Halo 共享 device id 的情况)。
    /// `None` = 不靠 revision 区分。
    pub pci_revision: Option<u8>,
}

/// 已核实的 AMD Ryzen AI NPU 芯片表。
///
/// 与 Python `detector.py::AMD_NPU_CHIPS` 对齐;PCI id/revision 据 kernel.org + Gentoo wiki 核实。
/// 注:Phoenix/Hawk Point 共用 `1022:1502` rev 0x00,靠 CPU 名区分;Strix 系共用 `1022:17f0`,
/// 本表 rev 0x10=Strix Point、0x11=Krackan Point(均 XDNA2;Strix Halo 亦 17f0 系)。代差不影响
/// NPU 驱动选型,PCI 命中后再靠 CPU 名细分。
pub const AMD_NPU_CHIPS: &[AmdNpuChip] = &[
    AmdNpuChip {
        chip_id: "strix_point",
        name: "AMD Ryzen AI 3xx (Strix Point, XDNA2)",
        xdna_version: 2,
        tops: 50,
        min_kernel: "6.14",
        cpu_keywords: &["ryzen ai 3", "strix", "ryzen ai 9", "hx 370", "365"],
        pci_device: 0x17f0,
        pci_revision: Some(0x10),
    },
    AmdNpuChip {
        chip_id: "krackan_point",
        name: "AMD Ryzen AI 2xx (Krackan Point, XDNA2)",
        xdna_version: 2,
        tops: 50,
        min_kernel: "6.14",
        cpu_keywords: &["ryzen ai 2", "krackan"],
        pci_device: 0x17f0,
        pci_revision: Some(0x11),
    },
    AmdNpuChip {
        chip_id: "hawk_point",
        name: "AMD Ryzen 8x40 (Hawk Point, XDNA1)",
        xdna_version: 1,
        tops: 16,
        min_kernel: "6.14",
        cpu_keywords: &["hawk point", "ryzen 8", "8840", "8845", "8945"],
        pci_device: 0x1502,
        pci_revision: Some(0x00),
    },
    AmdNpuChip {
        chip_id: "phoenix",
        name: "AMD Ryzen 7x40 (Phoenix, XDNA1)",
        xdna_version: 1,
        tops: 10,
        min_kernel: "6.14",
        cpu_keywords: &["phoenix", "ryzen 7040", "7840", "7940", "7640"],
        pci_device: 0x1502,
        pci_revision: Some(0x00),
    },
];

/// 通过 CPU model name 关键字识别芯片代。
pub fn identify_chip_by_cpu(cpu_model: &str) -> Option<&'static AmdNpuChip> {
    let m = cpu_model.to_ascii_lowercase();
    AMD_NPU_CHIPS
        .iter()
        .find(|c| c.cpu_keywords.iter().any(|kw| m.contains(kw)))
}

/// 通过 PCI device id + revision 识别芯片代(优先于 CPU 名,更精确)。
/// `vendor` 必须是 0x1022(AMD);否则 None。
pub fn identify_chip_by_pci(vendor: u16, device: u16, revision: u8) -> Option<&'static AmdNpuChip> {
    if vendor != 0x1022 {
        return None;
    }
    // 先按 device + revision 精确匹配,再退化到 device-only。
    AMD_NPU_CHIPS
        .iter()
        .find(|c| c.pci_device == device && c.pci_revision == Some(revision))
        .or_else(|| AMD_NPU_CHIPS.iter().find(|c| c.pci_device == device))
}

/// 内核版本比较:`current >= min` ?
/// 解析形如 "6.14.0-27-generic" / "6.8" 的前两段(major.minor)+ 可选 patch。
pub fn kernel_ge(current: &str, min: &str) -> bool {
    fn parse(v: &str) -> (u32, u32, u32) {
        // 取版本主体(到第一个 '-' 之前),再按 '.' 拆
        let body = v.trim().split('-').next().unwrap_or("");
        let mut it = body.split('.');
        let a = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let b = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let c = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        (a, b, c)
    }
    parse(current) >= parse(min)
}

/// AMD NPU 细粒度状态报告(只读探测结果)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpuStatus {
    pub chip_id: String,
    pub chip_name: String,
    pub xdna_version: u8,
    pub tops: u16,
    /// amdxdna 内核模块已加载(/proc/modules)。
    pub driver_loaded: bool,
    /// /lib/firmware/amdnpu/ 存在且非空。
    pub firmware_present: bool,
    /// /dev/accel/accel0 存在。
    pub device_node_present: bool,
    pub kernel_ok: bool,
    pub current_kernel: String,
    pub min_kernel: String,
    /// IOMMU 已启用(XDNA SVA 需要);Windows 不适用 → true。
    pub iommu_ok: bool,
    /// 目标 OS:"linux" / "windows" / "macos"。
    pub os: String,
}

impl NpuStatus {
    /// 全部信号就位 = NPU 可用。
    pub fn is_ready(&self) -> bool {
        self.driver_loaded
            && self.firmware_present
            && self.device_node_present
            && self.kernel_ok
            && self.iommu_ok
    }

    /// 纯函数核心:由已采集的原始信号构造 status。便于测试(不碰真机 /proc)。
    #[allow(clippy::too_many_arguments)]
    pub fn from_signals(
        chip: &AmdNpuChip,
        driver_loaded: bool,
        firmware_present: bool,
        device_node_present: bool,
        current_kernel: &str,
        iommu_ok: bool,
        os: &str,
    ) -> Self {
        NpuStatus {
            chip_id: chip.chip_id.to_string(),
            chip_name: chip.name.to_string(),
            xdna_version: chip.xdna_version,
            tops: chip.tops,
            driver_loaded,
            firmware_present,
            device_node_present,
            kernel_ok: kernel_ge(current_kernel, chip.min_kernel),
            current_kernel: current_kernel.to_string(),
            min_kernel: chip.min_kernel.to_string(),
            // Windows/macOS 上 IOMMU 概念不适用,不阻塞。
            iommu_ok: if os == "linux" { iommu_ok } else { true },
            os: os.to_string(),
        }
    }

    /// 根据缺失项生成 consent-gated 安装计划。**不执行任何命令**。
    pub fn install_plan(&self) -> NpuInstallPlan {
        let mut plan = NpuInstallPlan::default();

        if self.os == "windows" {
            // Windows:NPU 驱动经 AMD Ryzen AI Software installer。无法可靠探测 → 一律 Manual 引导。
            plan.missing.push("Windows NPU 驱动状态需人工确认".to_string());
            plan.steps.push(InstallStep {
                description: "在设备管理器查看 \"AMD IPU Device\" / \"Neural processors\";\
                              缺失则从 AMD Ryzen AI Software 安装 NPU 驱动 (RyzenAI-SW)"
                    .to_string(),
                command: None,
                danger: Danger::Manual,
                consent_required: true,
            });
            return plan;
        }

        if self.os == "macos" {
            // 不适用。
            return plan;
        }

        // Linux 分支
        if !self.kernel_ok {
            plan.missing
                .push(format!("内核 >= {}（当前 {}）", self.min_kernel, self.current_kernel));
            plan.steps.push(InstallStep {
                description: format!(
                    "amdxdna 主线驱动需要内核 >= {}。升级内核(如 Ubuntu HWE 内核或 25.04+)后重试",
                    self.min_kernel
                ),
                command: None,
                danger: Danger::Manual,
                consent_required: true,
            });
        }

        if !self.driver_loaded {
            plan.missing.push("amdxdna 内核模块未加载".to_string());
            if self.kernel_ok {
                plan.steps.push(InstallStep {
                    description: "加载 amdxdna 内核模块".to_string(),
                    command: Some("sudo modprobe amdxdna".to_string()),
                    danger: Danger::NeedsConsent,
                    consent_required: true,
                });
            } else {
                plan.steps.push(InstallStep {
                    description: "内核 < 6.14:从 AMD 官方源安装 amdxdna DKMS(需提权,可能编译失败)"
                        .to_string(),
                    command: None,
                    danger: Danger::Manual,
                    consent_required: true,
                });
            }
        }

        if !self.firmware_present {
            plan.missing.push("NPU firmware (/lib/firmware/amdnpu/) 缺失".to_string());
            plan.steps.push(InstallStep {
                description: "安装/更新 linux-firmware 以获得 amdnpu 固件".to_string(),
                command: Some("sudo apt-get install -y linux-firmware".to_string()),
                danger: Danger::SafeAuto,
                consent_required: true,
            });
        }

        if !self.device_node_present && self.driver_loaded {
            // 模块在但没 device node:多半是 firmware 没加载或 IOMMU 未开。
            plan.missing
                .push("/dev/accel/accel0 不存在(NPU 未就绪)".to_string());
        }

        if !self.iommu_ok {
            plan.missing.push("IOMMU 未启用(XDNA SVA 需要)".to_string());
            plan.steps.push(InstallStep {
                description: "在 BIOS/UEFI 启用 IOMMU,并确保内核参数含 amd_iommu=on iommu=pt"
                    .to_string(),
                command: None,
                danger: Danger::Manual,
                consent_required: true,
            });
        }

        plan
    }

    /// Linux 真机探测:综合 PCI / CPU / module / dev node / firmware / kernel / IOMMU。
    /// 非 AMD Ryzen AI 硬件 → None。所有 fs 读失败保守降级,不 panic。
    #[cfg(target_os = "linux")]
    pub fn detect_amd() -> Option<NpuStatus> {
        let cpu_model = read_cpu_model().unwrap_or_default();

        // 1. 先尝试 PCI 精确识别(最可靠);失败回退 CPU 名。
        let chip = scan_pci_for_npu().or_else(|| identify_chip_by_cpu(&cpu_model));
        let chip = chip?;

        let driver_loaded = std::fs::read_to_string("/proc/modules")
            .map(|m| m.contains("amdxdna"))
            .unwrap_or(false);
        let device_node_present = std::path::Path::new("/dev/accel/accel0").exists();
        let firmware_present = dir_non_empty("/lib/firmware/amdnpu");
        let current_kernel = read_kernel_release().unwrap_or_default();
        let iommu_ok = dir_non_empty("/sys/class/iommu");

        Some(NpuStatus::from_signals(
            chip,
            driver_loaded,
            firmware_present,
            device_node_present,
            &current_kernel,
            iommu_ok,
            "linux",
        ))
    }

    /// Windows:经 CPU 名识别 Ryzen AI 芯片;驱动状态无法可靠探测,交 install_plan 引导。
    #[cfg(target_os = "windows")]
    pub fn detect_amd() -> Option<NpuStatus> {
        let cpu_model = read_cpu_model().unwrap_or_default();
        let chip = identify_chip_by_cpu(&cpu_model)?;
        // Windows 上驱动/firmware 状态保守标 false → install_plan 给 Manual 引导。
        Some(NpuStatus::from_signals(
            chip, false, false, false, "0", true, "windows",
        ))
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    pub fn detect_amd() -> Option<NpuStatus> {
        None
    }

    /// 人类可读一行诊断。
    pub fn summary(&self) -> String {
        if self.is_ready() {
            format!("{} — NPU 就绪 ({} TOPS, XDNA{})", self.chip_name, self.tops, self.xdna_version)
        } else {
            let plan = self.install_plan();
            format!(
                "{} — NPU 未就绪;缺失: {}",
                self.chip_name,
                plan.missing.join("; ")
            )
        }
    }
}

// ── VitisAI 运行时探测 + 下载推荐(零再分发:仅检测 + 指向 AMD 官方)──────────
//
// 法律边界(2026-06-25 用户拍板):attune **不托管 / 不再分发** AMD Ryzen AI SDK
// (闭源 `vai_rt`/`voe`/`xclbin`,EULA 经官方门户点击接受)。本节只做三件事:
//   (1) 检测用户**是否已自行安装** Ryzen AI 运行时(VitisAI EP);
//   (2) NPU 在但运行时缺 → 产出**指向 AMD 官方下载页**的推荐 + benchmark 数据;
//   (3) EP 选型链已是「有 vitis(编入+可用)就用,否则 DirectML→CPU」(见 accel.rs)。
// 数据源:vlm-llm-bench(2026-06-19)rapidocr-amd-npu(VitisAI) CER 7.04% / p50 2031ms
// vs DirectML CER ~7% / p50 468ms —— **同精度,NPU 更省电(电池场景优),DirectML 更快**。

/// AMD 官方 Ryzen AI Software 安装指引(下载 + EULA 接受在 AMD 站点完成)。
pub const RYZEN_AI_INSTALL_URL: &str = "https://ryzenai.docs.amd.com/en/latest/inst.html";

/// VitisAI 运行时是否就位 —— 纯函数,便于测试。
///
/// `env_install_path`：`RYZEN_AI_INSTALLATION_PATH`(AMD installer 设置,权威信号)。
/// `ep_lib_present`：已知 VitisAI EP 运行时库(Win `onnxruntime_vitisai_ep.dll` /
/// Linux `libvitisai_ep` 等)是否存在于磁盘。任一为真即视为已安装。
pub fn vitisai_runtime_from_signals(env_install_path: Option<&str>, ep_lib_present: bool) -> bool {
    env_install_path.map(|p| !p.trim().is_empty()).unwrap_or(false) || ep_lib_present
}

/// 真机探测 VitisAI 运行时是否已由用户安装(不安装、不下载、只读探测)。
pub fn vitisai_runtime_present() -> bool {
    let env_path = std::env::var("RYZEN_AI_INSTALLATION_PATH").ok();
    // 若 env 指向安装根,顺带查 deployment 下的 EP 库是否真在(env 可能 stale)。
    let ep_lib_present = env_path
        .as_deref()
        .map(|root| {
            let p = std::path::Path::new(root);
            p.join("deployment").join("onnxruntime_vitisai_ep.dll").exists()
                || p.join("onnxruntime_vitisai_ep.dll").exists()
        })
        .unwrap_or(false);
    vitisai_runtime_from_signals(env_path.as_deref(), ep_lib_present)
}

/// VitisAI 引导状态。
///
/// 诚实边界(2026-06-25 ort 可行性调研结论):attune 当前**无法**自动调用 VitisAI NPU
/// —— `ort/vitis` 是空 cfg 开关,pykeio download-binaries 预编译**不含** VitisAI EP
/// (AMD 定制 build),且 attune host ORT(~1.20)与 Ryzen AI 1.7.1 的 ORT(~1.23)ABI
/// 不兼容。⇒ 即便用户自装 Ryzen AI,本版也不会用它。故**不做**「装了就能用」误导性推荐;
/// AMD NPU 在场时如实告知「已用 DirectML+CPU 同精度,NPU 加速在路线图上」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VitisAiStatus {
    /// 运行时就位 + 二进制真编入可用的 vitis EP → NPU 生效中。
    /// **前向兼容**:当前 shipped 构建恒不满足(见上),保留以备 ORT 升级 sprint 解锁。
    Active,
    /// AMD NPU 在场,但本版经 DirectML(GPU)+ CPU 推理(与 NPU 同精度,开箱即用);
    /// NPU(VitisAI)硬件加速在路线图上(待 ORT 升级),当前版本不调用。
    DirectmlActiveNpuRoadmap,
}

/// benchmark 对照(vlm-llm-bench 实测,§6.3 有源)。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct VitisAiBenchmark {
    pub npu_ocr_cer_pct: f32,
    pub directml_ocr_cer_pct: f32,
    pub npu_p50_ms: u32,
    pub directml_p50_ms: u32,
    pub note: &'static str,
}

impl VitisAiBenchmark {
    /// vlm-llm-bench 2026-06-19:rapidocr-amd-npu(VitisAI) vs rapidocr-amd-directml。
    pub const BENCH: VitisAiBenchmark = VitisAiBenchmark {
        npu_ocr_cer_pct: 7.04,
        directml_ocr_cer_pct: 7.04,
        npu_p50_ms: 2031,
        directml_p50_ms: 468,
        note: "同精度;NPU 更省电(电池/后台场景优),DirectML 更快(交互场景优)",
    };
}

/// AMD NPU 加速引导(给 wizard / Settings 的「监测 + 数据」一站式 payload)。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VitisAiAdvice {
    pub npu_present: bool,
    pub runtime_present: bool,
    pub vitis_compiled: bool,
    pub status: VitisAiStatus,
    /// AMD NPU 加速是否为「路线图未解锁」状态(当前 shipped 构建恒 true 当 NPU 在场)。
    /// 不做「装了就能用」CTA:attune 当前无法调用 VitisAI(见 VitisAiStatus 文档)。
    pub npu_roadmap: bool,
    /// AMD 官方 Ryzen AI 了解链接(信息性,非「装了就能用」承诺)。
    pub learn_more_url: &'static str,
    pub rationale: &'static str,
    pub benchmark: VitisAiBenchmark,
}

/// 构建引导建议 —— 纯函数。`None` = 无 AMD XDNA NPU,不展示任何引导。
///
/// 诚实:当前 shipped 构建 `vitis_compiled` 恒 false(见 VitisAiStatus),故 NPU 在场时
/// 一律 `DirectmlActiveNpuRoadmap` —— 如实告知「已用 DirectML+CPU 同精度,NPU 在路线图上」,
/// **不**诱导用户去装 Ryzen AI(装了本版也用不上)。`Active` 仅前向兼容保留。
pub fn vitisai_advice(npu_present: bool, runtime_present: bool, vitis_compiled: bool) -> Option<VitisAiAdvice> {
    if !npu_present {
        return None;
    }
    let active = runtime_present && vitis_compiled; // 当前 shipped 恒 false
    let (status, rationale) = if active {
        (
            VitisAiStatus::Active,
            "AMD NPU(VitisAI)路径生效中:与 DirectML 同精度,后台/电池场景更省电。",
        )
    } else {
        (
            VitisAiStatus::DirectmlActiveNpuRoadmap,
            "已用 AMD GPU(DirectML)+ CPU 推理,与 NPU 同精度,开箱即用无需操作。NPU(VitisAI)硬件加速在路线图上(待 ORT 升级解锁),当前版本暂不调用。",
        )
    };
    Some(VitisAiAdvice {
        npu_present,
        runtime_present,
        vitis_compiled,
        status,
        npu_roadmap: !active,
        learn_more_url: RYZEN_AI_INSTALL_URL,
        rationale,
        benchmark: VitisAiBenchmark::BENCH,
    })
}

// ── 平台辅助(真机读取) ────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn read_cpu_model() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in info.lines().take(40) {
        if let Some(v) = line.strip_prefix("model name\t: ") {
            return Some(v.trim().to_string());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn read_cpu_model() -> Option<String> {
    // `wmic.exe` 在 Win11 24H2+ 已默认移除 —— 旧实现子进程失败 → CPU 型号取不到 →
    // `identify_chip_by_cpu` 认不出 Ryzen AI 代际 → NPU 安装计划退化。改走 sysinfo 原生
    // API(与 `platform::mod.rs::HardwareProfile::detect` 同源,不依赖 wmic.exe)。
    let sys = sysinfo::System::new_all();
    let model = sys.cpus().first()?.brand().trim().to_string();
    if model.is_empty() {
        None
    } else {
        Some(model)
    }
}

#[cfg(target_os = "linux")]
fn read_kernel_release() -> Option<String> {
    // /proc/sys/kernel/osrelease,如 "6.14.0-27-generic"
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(target_os = "linux")]
fn dir_non_empty(path: &str) -> bool {
    std::fs::read_dir(path)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// 扫描 PCI 设备目录,找 AMD NPU(vendor 0x1022 + 已知 device id)。
/// 读 `/sys/bus/pci/devices/*/{vendor,device,revision}`(16 进制 "0x...")。
#[cfg(target_os = "linux")]
fn scan_pci_for_npu() -> Option<&'static AmdNpuChip> {
    let entries = std::fs::read_dir("/sys/bus/pci/devices").ok()?;
    for e in entries.flatten() {
        let base = e.path();
        let parse_hex = |name: &str| -> Option<u32> {
            let raw = std::fs::read_to_string(base.join(name)).ok()?;
            let t = raw.trim();
            let t = t.strip_prefix("0x").unwrap_or(t);
            u32::from_str_radix(t, 16).ok()
        };
        let (Some(vendor), Some(device)) = (parse_hex("vendor"), parse_hex("device")) else {
            continue;
        };
        let revision = parse_hex("revision").unwrap_or(0) as u8;
        if let Some(chip) =
            identify_chip_by_pci(vendor as u16, device as u16, revision)
        {
            return Some(chip);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(id: &str) -> &'static AmdNpuChip {
        AMD_NPU_CHIPS.iter().find(|c| c.chip_id == id).unwrap()
    }

    // ── 芯片识别 ──────────────────────────────────────────────

    #[test]
    fn identify_strix_by_cpu() {
        let c = identify_chip_by_cpu("AMD Ryzen AI 9 HX 370 w/ Radeon 890M").unwrap();
        assert_eq!(c.chip_id, "strix_point");
        assert_eq!(c.xdna_version, 2);
    }

    #[test]
    fn identify_phoenix_by_cpu() {
        let c = identify_chip_by_cpu("AMD Ryzen 7 7840U w/ Radeon 780M Graphics").unwrap();
        assert_eq!(c.chip_id, "phoenix");
    }

    #[test]
    fn identify_hawk_point_by_cpu() {
        let c = identify_chip_by_cpu("AMD Ryzen 7 8845HS w/ Radeon 780M").unwrap();
        assert_eq!(c.chip_id, "hawk_point");
        assert_eq!(c.tops, 16);
    }

    #[test]
    fn non_ryzen_ai_cpu_returns_none() {
        assert!(identify_chip_by_cpu("AMD Ryzen 5 5600X 6-Core Processor").is_none());
        assert!(identify_chip_by_cpu("Intel(R) Core(TM) i7-12700H").is_none());
        assert!(identify_chip_by_cpu("").is_none());
    }

    #[test]
    fn identify_by_pci_phoenix_and_strix() {
        // Phoenix/Hawk Point: 1022:1502 rev 0x00
        let c = identify_chip_by_pci(0x1022, 0x1502, 0x00).unwrap();
        assert_eq!(c.pci_device, 0x1502);
        // Strix Point: 1022:17f0 rev 0x10
        let c = identify_chip_by_pci(0x1022, 0x17f0, 0x10).unwrap();
        assert_eq!(c.chip_id, "strix_point");
        // Strix Halo: 1022:17f0 rev 0x11 → krackan entry (same device, rev 0x11)
        let c = identify_chip_by_pci(0x1022, 0x17f0, 0x11).unwrap();
        assert_eq!(c.pci_device, 0x17f0);
    }

    #[test]
    fn pci_wrong_vendor_returns_none() {
        // Intel vendor → None even if device id coincides
        assert!(identify_chip_by_pci(0x8086, 0x1502, 0x00).is_none());
    }

    #[test]
    fn pci_unknown_device_returns_none() {
        assert!(identify_chip_by_pci(0x1022, 0xdead, 0x00).is_none());
    }

    #[test]
    fn pci_revision_fallback_to_device_only() {
        // 未知 revision 但 device 命中 → 退化到 device-only 匹配,不返回 None
        let c = identify_chip_by_pci(0x1022, 0x17f0, 0xff);
        assert!(c.is_some(), "unknown revision should fall back to device-only match");
    }

    // ── 内核版本比较 ──────────────────────────────────────────

    #[test]
    fn kernel_ge_basic() {
        assert!(kernel_ge("6.14.0-27-generic", "6.14"));
        assert!(kernel_ge("6.15.2", "6.14"));
        assert!(!kernel_ge("6.8.0-50-generic", "6.14"));
        assert!(kernel_ge("6.14", "6.14"));
    }

    #[test]
    fn kernel_ge_malformed_is_conservative() {
        // 解析失败 → (0,0,0),不会误判为满足
        assert!(!kernel_ge("garbage", "6.14"));
        assert!(!kernel_ge("", "6.14"));
    }

    // ── status 构造 + 就绪判定 ────────────────────────────────

    #[test]
    fn status_all_signals_ready() {
        let s = NpuStatus::from_signals(chip("strix_point"), true, true, true, "6.14.0-27", true, "linux");
        assert!(s.is_ready());
        assert!(s.install_plan().is_ready());
        assert!(s.summary().contains("就绪"));
    }

    #[test]
    fn status_driver_not_loaded_yields_consent_modprobe() {
        let s = NpuStatus::from_signals(chip("phoenix"), false, true, false, "6.14.0", true, "linux");
        assert!(!s.is_ready());
        let plan = s.install_plan();
        assert!(plan.missing.iter().any(|m| m.contains("amdxdna")));
        let modprobe = plan
            .steps
            .iter()
            .find(|st| st.command.as_deref() == Some("sudo modprobe amdxdna"))
            .expect("expected modprobe step");
        assert_eq!(modprobe.danger, Danger::NeedsConsent);
        assert!(modprobe.consent_required);
    }

    #[test]
    fn status_firmware_missing_yields_safe_auto() {
        let s = NpuStatus::from_signals(chip("strix_point"), true, false, true, "6.14.0", true, "linux");
        let plan = s.install_plan();
        assert!(plan.missing.iter().any(|m| m.contains("firmware")));
        let fw = plan
            .steps
            .iter()
            .find(|st| st.command.as_deref() == Some("sudo apt-get install -y linux-firmware"))
            .expect("expected firmware step");
        assert_eq!(fw.danger, Danger::SafeAuto);
    }

    #[test]
    fn status_old_kernel_yields_manual_upgrade() {
        let s = NpuStatus::from_signals(chip("strix_point"), false, false, false, "6.8.0-50", true, "linux");
        assert!(!s.kernel_ok);
        let plan = s.install_plan();
        assert!(plan.missing.iter().any(|m| m.contains("内核")));
        // 老内核下 amdxdna 步骤应为 Manual(不给裸 modprobe)
        assert!(plan.steps.iter().any(|st| st.danger == Danger::Manual));
        assert!(
            !plan.steps.iter().any(|st| st.command.as_deref() == Some("sudo modprobe amdxdna")),
            "must not suggest modprobe when kernel too old"
        );
    }

    #[test]
    fn status_iommu_off_flagged_manual() {
        let s = NpuStatus::from_signals(chip("phoenix"), true, true, true, "6.14.0", false, "linux");
        assert!(!s.is_ready());
        let plan = s.install_plan();
        assert!(plan.missing.iter().any(|m| m.contains("IOMMU")));
        assert!(plan
            .steps
            .iter()
            .any(|st| st.description.contains("IOMMU") && st.danger == Danger::Manual));
    }

    // ── 安全不变量:计划绝不含破坏性自动执行 ──────────────────

    #[test]
    fn install_plan_never_auto_executes_dangerous_command() {
        // 对所有缺失组合,断言:任何带 command 的 step 都 consent_required;
        // 且没有 SafeAuto 步骤含提权换内核 / rm / dkms 等破坏性动作。
        for &all_present in &[true, false] {
            let s = NpuStatus::from_signals(
                chip("strix_point"),
                all_present, all_present, all_present, "6.8.0", all_present, "linux",
            );
            for st in s.install_plan().steps {
                if let Some(cmd) = &st.command {
                    assert!(st.consent_required, "command step must require consent: {cmd}");
                    assert!(!cmd.contains("rm "), "no rm in any auto command: {cmd}");
                    assert!(!cmd.contains("dkms"), "dkms is manual-only, never a command: {cmd}");
                    assert!(
                        !cmd.contains("reboot") && !cmd.contains("mkinitcpio"),
                        "no kernel-altering auto command: {cmd}"
                    );
                }
                // 换内核 / BIOS 类必须 Manual
                if st.description.contains("内核") && st.command.is_none() {
                    assert_eq!(st.danger, Danger::Manual);
                }
            }
        }
    }

    #[test]
    fn windows_plan_is_manual_installer_guidance() {
        let s = NpuStatus::from_signals(chip("strix_point"), false, false, false, "0", true, "windows");
        let plan = s.install_plan();
        assert!(!plan.steps.is_empty());
        let step = &plan.steps[0];
        assert_eq!(step.danger, Danger::Manual);
        assert!(step.command.is_none(), "windows install has no auto command");
        assert!(step.description.contains("RyzenAI-SW"));
    }

    #[test]
    fn windows_iommu_not_blocking() {
        // Windows 上 iommu_ok 入参 false 也应被规整为 true(不适用)
        let s = NpuStatus::from_signals(chip("phoenix"), true, true, true, "0", false, "windows");
        assert!(s.iommu_ok, "IOMMU concept N/A on Windows");
    }

    #[test]
    fn danger_as_str_stable() {
        assert_eq!(Danger::SafeAuto.as_str(), "safe-auto");
        assert_eq!(Danger::NeedsConsent.as_str(), "needs-consent");
        assert_eq!(Danger::Manual.as_str(), "manual");
    }

    #[test]
    fn detect_amd_does_not_panic_on_current_host() {
        // 只断言不 panic;CI 机器通常无 NPU → None。
        let _ = NpuStatus::detect_amd();
    }

    // ── VitisAI 运行时探测 + 引导 ─────────────────────────────────

    #[test]
    fn runtime_signals_env_or_lib_means_present() {
        assert!(vitisai_runtime_from_signals(Some(r"C:\Program Files\RyzenAI\1.7.1"), false));
        assert!(vitisai_runtime_from_signals(None, true));
        assert!(vitisai_runtime_from_signals(Some("/opt/ryzen-ai"), true));
    }

    #[test]
    fn runtime_signals_absent_or_blank_means_not_present() {
        assert!(!vitisai_runtime_from_signals(None, false));
        assert!(!vitisai_runtime_from_signals(Some("   "), false)); // env 设了但空白
        assert!(!vitisai_runtime_from_signals(Some(""), false));
    }

    #[test]
    fn runtime_present_does_not_panic_on_current_host() {
        let _ = vitisai_runtime_present();
    }

    #[test]
    fn advice_none_without_amd_npu() {
        assert!(vitisai_advice(false, false, false).is_none());
        assert!(vitisai_advice(false, true, true).is_none());
    }

    #[test]
    fn advice_directml_active_npu_roadmap_when_no_usable_vitis() {
        // 当前 shipped 现实:vitis 未编入 → NPU 在场也走 DirectML+CPU,NPU 列路线图。
        for &(rt, compiled) in &[(false, false), (true, false), (false, true)] {
            let a = vitisai_advice(true, rt, compiled).unwrap();
            assert_eq!(a.status, VitisAiStatus::DirectmlActiveNpuRoadmap, "rt={rt} compiled={compiled}");
            assert!(a.npu_roadmap);
            // 数据一定带上(同精度 CER + 双路 p50)。
            assert_eq!(a.benchmark.npu_ocr_cer_pct, a.benchmark.directml_ocr_cer_pct);
            assert!(a.benchmark.npu_p50_ms > a.benchmark.directml_p50_ms); // NPU 慢但省电
        }
    }

    #[test]
    fn advice_active_only_when_runtime_present_and_vitis_compiled() {
        // 前向兼容:仅当 ORT 升级后真编入 vitis + 运行时在场,才置 Active。
        let a = vitisai_advice(true, true, true).unwrap();
        assert_eq!(a.status, VitisAiStatus::Active);
        assert!(!a.npu_roadmap);
    }

    #[test]
    fn advice_serializes_with_kebab_status() {
        let a = vitisai_advice(true, false, false).unwrap();
        let j = serde_json::to_string(&a).unwrap();
        assert!(j.contains("\"directml-active-npu-roadmap\""), "status kebab-case: {j}");
        assert!(j.contains("\"benchmark\""));
        assert!(j.contains("ryzenai.docs.amd.com"));
        assert!(!j.contains("recommend"), "no misleading install CTA: {j}");
    }
}
