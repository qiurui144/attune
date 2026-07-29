use std::path::PathBuf;

pub mod accel;
pub mod cpu_db;
pub mod npu;
pub mod power;
pub mod region;
pub mod tier;

pub use accel::{AccelCapabilities, AccelKind, Accelerator};
pub use npu::{Danger, NpuInstallPlan, NpuStatus};
pub use power::{PowerProfile, PowerSource, PowerState};
pub use region::{detect_region, Region};
pub use tier::{classify_hardware, ModelRecommendation, Tier};

const APP_DIR: &str = "attune";
const LEGACY_APP_DIR: &str = "npu-vault";

std::thread_local! {
    /// Per-thread override for the resolved app data/config dir.
    ///
    /// When `Some`, [`data_dir`] / [`config_dir`] return this path verbatim
    /// (already the final app dir — no `attune/` suffix is appended), bypassing
    /// the `dirs::`/HOME fallback chain entirely. When `None` (the production
    /// default), resolution is byte-for-byte identical to having no override.
    ///
    /// This is a genuine data-dir-injection seam: it lets tests pin a temp dir
    /// without relying on `dirs::data_local_dir()` honoring `HOME`/`XDG_*` —
    /// which it does NOT on Windows (it reads `%LOCALAPPDATA%` via the
    /// Known-Folder API). A thread-local is correct because cargo runs each test
    /// on its own thread, so there is no process-global env clobber and the
    /// behavior is identical on Windows and Linux. See `set_dir_override_for_test`.
    static DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Read the current thread-local dir override, if any.
fn dir_override() -> Option<PathBuf> {
    DIR_OVERRIDE.with(|c| c.borrow().clone())
}

/// Set (or clear with `None`) the per-thread app-dir override. Test-only seam.
///
/// The override path is treated as the FINAL app dir — `data_dir()`/`config_dir()`
/// return it directly (no `attune/` suffix). Callers should `create_dir_all` it
/// themselves before use and restore the previous value on exit.
#[doc(hidden)]
pub fn set_dir_override_for_test(path: Option<PathBuf>) -> Option<PathBuf> {
    DIR_OVERRIDE.with(|c| c.replace(path))
}

pub fn data_dir() -> PathBuf {
    // Test-injection seam (thread-local): when set, return verbatim and skip the
    // dirs::/HOME fallback chain. None → behavior identical to production.
    if let Some(p) = dir_override() {
        return p;
    }
    // 容器/headless 环境中 dirs::data_local_dir() 可能返回 None（无 HOME 变量）；
    // 回退到 $HOME/.local/share 或当前目录，确保不 panic。
    //
    // 迁移规则：老目录 npu-vault/ 若存在且新目录 attune/ 不存在，返回老路径（就地复用，
    // 避免升级丢数据）。新建用户使用 attune/。
    let base = dirs::data_local_dir()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_app_dir(base)
}

pub fn config_dir() -> PathBuf {
    if let Some(p) = dir_override() {
        return p;
    }
    // 同上，回退到 $HOME/.config 或当前目录
    let base = dirs::config_dir()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_app_dir(base)
}

/// 迁移兼容：新老目录名都认。老安装返回老路径、新安装用新名字。
fn resolve_app_dir(base: PathBuf) -> PathBuf {
    let new_path = base.join(APP_DIR);
    let legacy_path = base.join(LEGACY_APP_DIR);
    if !new_path.exists() && legacy_path.exists() {
        legacy_path
    } else {
        new_path
    }
}

pub fn db_path() -> PathBuf {
    data_dir().join("vault.db")
}

pub fn device_secret_path() -> PathBuf {
    config_dir().join("device.key")
}

/// 模型缓存目录：~/.local/share/attune/models/（老路径 npu-vault/ 自动兼容）
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// 设备形态 — 决定默认 LLM 路径与若干默认配置。
///
/// **背景**：attune 在两类形态上有不同的"默认体验"：
/// - **Laptop**（默认）：本地不预装 LLM，default `llm.provider = "openai_compat"`，wizard 引导用户填远端 endpoint + API key
/// - **LocalSchedulerAppliance**：本地高性能设备经 **local-scheduler :8090 统一收口**
///   （OpenAI/Ollama-compat），default `llm.provider = "openai_compat"` + endpoint
///   `http://127.0.0.1:8090/v1`。**attune 不直连 Ollama :11434** —— Ollama/llama.cpp
///   是 scheduler 内部 worker，attune 经 :8090 路由，禁旁路直连。
/// - **Server**：headless 服务器，行为同 Laptop（远端 token 默认）
/// - **Unknown**：检测失败，按 Laptop 处理
///
/// 检测顺序（优先级递减）：
/// 1. 环境变量 `ATTUNE_FORM_FACTOR` (local_scheduler / laptop / server) — 显式 override
/// 2. DMI / `/sys/class/dmi/id/product_name` 包含已知本地调度器设备关键字
/// 3. 默认 Laptop
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormFactor {
    #[default]
    Laptop,
    LocalSchedulerAppliance,
    Server,
    Unknown,
}

impl FormFactor {
    /// 是否默认走**本地推理收口**。
    ///
    /// 语义 = "默认 LLM 在设备本地解决"。本地调度器形态经 **local-scheduler :8090 统一收口**
    /// （OpenAI/Ollama-compat），**不是**直连 Ollama :11434 —— 命名保留 `local_llm` 是相对
    /// "云端 token"而言（本地 vs 远端），不表示"直连 Ollama"。
    ///
    /// 调用点：`build_llm_from_settings` 在 `settings.llm.endpoint` 为空时的**末位降级兜底**
    /// 才用本函数走 Ollama auto-detect；本地调度器默认 settings 已带 :8090 endpoint（优先级 1），
    /// 正常路径下不会落到该兜底（兜底仅在用户清空 endpoint 的异常态下生效）。
    pub fn prefers_local_llm(&self) -> bool {
        matches!(self, FormFactor::LocalSchedulerAppliance)
    }
}

/// 可用的硬件加速后端
#[derive(Debug, Clone, PartialEq)]
pub enum NpuKind {
    IntelNpu,
    IntelIgpu,
    AmdNpu,
    Cuda,
    None,
}

/// 探测本机最优 Execution Provider
///
/// 优先级：NPU_VAULT_EP 环境变量 > CUDA > CPU fallback
pub fn detect_npu() -> NpuKind {
    match std::env::var("NPU_VAULT_EP").as_deref() {
        Ok("openvino") => NpuKind::IntelNpu,
        Ok("directml") => NpuKind::AmdNpu,
        Ok("cuda") => NpuKind::Cuda,
        Ok("cpu") | Ok("none") => NpuKind::None,
        _ => {
            if std::path::Path::new("/dev/nvidia0").exists() {
                NpuKind::Cuda
            } else {
                NpuKind::None
            }
        }
    }
}

// ── 硬件画像（细粒度检测） ────────────────────────────────────────────────────

/// 具体的硬件能力报告，用于启动时选择最优配置与打印诊断
#[derive(Debug, Clone, Default)]
pub struct HardwareProfile {
    pub cpu_vendor: String,             // e.g. "AuthenticAMD" / "GenuineIntel"
    pub cpu_model: String,              // e.g. "AMD Ryzen 7 8845H..."
    pub has_nvidia_gpu: bool,           // /dev/nvidia0
    pub has_amd_gpu: bool,              // /dev/kfd + /dev/dri/renderD*（AMD 集显或独显）
    pub has_intel_igpu: bool,           // Intel iGPU（/dev/dri/renderD* vendor=0x8086）
    pub amd_gfx_target: Option<String>, // e.g. "gfx1103" (Radeon 780M)，用于 ROCm 匹配
    pub has_amd_xdna_npu: bool,         // /dev/accel/accel0 + amdxdna 模块（Ryzen AI）
    pub has_intel_npu: bool,            // /dev/accel/accel0 + intel_vpu 模块
    pub total_ram_bytes: u64,           // 总内存字节；硬件档位匹配用
    pub os: &'static str,               // "linux" | "macos" | "windows"
    pub has_riscv_npu: bool,
    pub form_factor: FormFactor, // Laptop / LocalSchedulerAppliance / Server / Unknown — 决定 LLM 默认路径
    pub gpu_label: Option<String>, // 统一给 UI 的 GPU 描述
}

impl HardwareProfile {
    /// 检测当前宿主的硬件画像（只读、幂等、无副作用）
    pub fn detect() -> Self {
        let mut p = Self {
            os: if cfg!(target_os = "linux") {
                "linux"
            } else if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(target_os = "windows") {
                "windows"
            } else {
                "unknown"
            },
            ..Default::default()
        };

        // CPU vendor/model（Linux 读 /proc/cpuinfo）
        #[cfg(target_os = "linux")]
        if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in info.lines().take(40) {
                if let Some(v) = line.strip_prefix("vendor_id\t: ") {
                    p.cpu_vendor = v.trim().to_string();
                }
                if let Some(v) = line.strip_prefix("model name\t: ") {
                    p.cpu_model = v.trim().to_string();
                }
                if !p.cpu_vendor.is_empty() && !p.cpu_model.is_empty() {
                    break;
                }
            }
        }

        // NVIDIA GPU
        p.has_nvidia_gpu = std::path::Path::new("/dev/nvidia0").exists();

        #[cfg(target_os = "linux")]
        {
            let (has_amd_render, has_intel_render, gpu_label) = detect_linux_render_gpu_vendors();
            p.has_amd_gpu = has_amd_render
                || (std::path::Path::new("/dev/kfd").exists()
                    && std::path::Path::new("/dev/dri").exists());
            p.has_intel_igpu = has_intel_render;
            p.gpu_label = if p.has_nvidia_gpu {
                Some("NVIDIA GPU".to_string())
            } else {
                gpu_label
            };
        }

        #[cfg(not(target_os = "linux"))]
        {
            p.has_amd_gpu = false;
            p.has_intel_igpu = false;
        }

        // AMD gfx target（识别 Radeon 780M / 780M = gfx1103 等；用于 ROCm HSA 覆盖）
        if p.has_amd_gpu {
            p.amd_gfx_target = detect_amd_gfx_target();
        }

        // RISC-V NPU (Spacemit X100 A100): CPU名含 spacemit/x100 或 k3-scheduler 存在
        p.has_riscv_npu = cfg!(target_os = "linux")
            && !cfg!(target_arch = "x86_64")
            && !cfg!(target_arch = "aarch64")
            && (p.cpu_model.to_ascii_lowercase().contains("spacemit")
                || p.cpu_model.to_ascii_lowercase().contains("x100"));

        // NPU：区分 AMD XDNA vs Intel VPU
        if std::path::Path::new("/dev/accel/accel0").exists() {
            if let Ok(mods) = std::fs::read_to_string("/proc/modules") {
                if mods.contains("amdxdna") {
                    p.has_amd_xdna_npu = true;
                }
                if mods.contains("intel_vpu") {
                    p.has_intel_npu = true;
                }
            }
        }

        // 设备形态（env var override 优先，否则 DMI 关键字匹配，否则默认 Laptop）
        p.form_factor = detect_form_factor();

        // 总内存 + CPU（平台相关）
        #[cfg(target_os = "linux")]
        {
            if let Ok(info) = std::fs::read_to_string("/proc/meminfo") {
                for line in info.lines().take(5) {
                    if let Some(rest) = line.strip_prefix("MemTotal:") {
                        if let Some(kb_str) = rest.split_whitespace().next() {
                            if let Ok(kb) = kb_str.parse::<u64>() {
                                p.total_ram_bytes = kb * 1024;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // macOS：sysctl hw.memsize（总内存）+ machdep.cpu.brand_string（CPU 型号）
        #[cfg(target_os = "macos")]
        {
            if let Some(ram) = sysctl_u64("hw.memsize") {
                p.total_ram_bytes = ram;
            }
            if let Some(model) = sysctl_string("machdep.cpu.brand_string") {
                p.cpu_model = model;
            }
            // Apple Silicon 的 vendor 统一为 "Apple"，Intel Mac 可通过 sysctl 得到
            p.cpu_vendor =
                sysctl_string("machdep.cpu.vendor").unwrap_or_else(|| "Apple".to_string());
        }

        // Windows：sysinfo 读总内存 + CPU（原 wmic.exe 在 Win11 24H2+ 已默认移除，
        // 子进程失败 → RAM=0/cpu="" → tier=unsupported → EP 栈不自动配置；sysinfo 走
        // 原生 API 不依赖 wmic.exe）。GPU 走 PowerShell CIM（sysinfo 不覆盖 GPU）。
        #[cfg(target_os = "windows")]
        {
            {
                let sys = sysinfo::System::new_all();
                let ram = sys.total_memory(); // sysinfo 0.32：字节
                if ram > 0 {
                    p.total_ram_bytes = ram;
                }
                if let Some(cpu) = sys.cpus().first() {
                    let model = cpu.brand().trim().to_string();
                    if !model.is_empty() {
                        p.cpu_model = model;
                    }
                    let vendor = cpu.vendor_id().trim().to_string();
                    if !vendor.is_empty() {
                        p.cpu_vendor = vendor;
                    }
                }
            }
            let (has_nv, has_amd, has_intel, label) = detect_windows_gpu_vendors();
            p.has_nvidia_gpu = p.has_nvidia_gpu || has_nv;
            p.has_amd_gpu = p.has_amd_gpu || has_amd;
            p.has_intel_igpu = p.has_intel_igpu || has_intel;
            if p.gpu_label.is_none() {
                p.gpu_label = label;
            }
            // NPU 探测（Linux 走 /dev/accel + /proc/modules；Windows 走 CIM PnP
            // ComputeAccelerator）。VEN_8086→Intel NPU(OpenVINO)；VEN_1022→AMD XDNA(VitisAI)。
            let (intel_npu, amd_npu) = detect_windows_npu();
            p.has_intel_npu = p.has_intel_npu || intel_npu;
            p.has_amd_xdna_npu = p.has_amd_xdna_npu || amd_npu;
        }

        // 通用 fallback label（如果上面还没给）
        if p.gpu_label.is_none() {
            p.gpu_label = if p.has_nvidia_gpu {
                Some("NVIDIA GPU".to_string())
            } else if p.has_amd_gpu {
                Some("AMD GPU".to_string())
            } else if p.has_intel_igpu {
                Some("Intel iGPU".to_string())
            } else {
                None
            };
        }

        p
    }

    /// 是否有任何硬件加速（GPU/NPU）— 决定是否能跑稍大的模型
    pub fn has_accelerator(&self) -> bool {
        self.has_nvidia_gpu
            || self.has_amd_gpu
            || self.has_intel_igpu
            || self.has_amd_xdna_npu
            || self.has_intel_npu
    }

    /// 根据 RAM + 加速器档位，推荐默认本地摘要模型（仅"用户主动想用本地时"的建议）。
    ///
    /// **v0.6.0-rc.3 行为变化**（per CLAUDE.md "M2 决策" + 用户 2026-04-27 反馈）：
    /// - LLM 默认走**远端 token**（不在本地预装），settings.rs::default_settings.llm.provider 默认引导用户填远端 endpoint
    /// - 本函数仅在用户**显式选本地** Ollama 后给"硬件推荐"用，不再被 default_settings 用作 hardcode 默认
    /// - 本地调度器设备可选装本地 LLM；普通桌面用户应避免本地 chat（避免 OOM / 3B 效果差）
    ///
    /// 推荐档位（用户显式选本地时）：
    /// | RAM    | 加速器   | 模型            |
    /// |--------|---------|-----------------|
    /// | ≥32 GB | 独显/NPU | qwen2.5:7b      |
    /// | 16-32  | 有     | qwen2.5:3b      |
    /// | 8-16   | 有/无    | qwen2.5:1.5b    |
    /// | <8 GB  | -       | llama3.2:1b     |
    ///
    /// RAM 为 0（检测失败）→ 保守退到 qwen2.5:1.5b
    pub fn recommended_summary_model(&self) -> &'static str {
        const GB: u64 = 1024 * 1024 * 1024;
        let gb = self.total_ram_bytes / GB;
        let accel = self.has_accelerator();

        if self.total_ram_bytes == 0 {
            // 检测失败：保守默认
            return "qwen2.5:1.5b";
        }
        match (gb, accel) {
            (32.., true) => "qwen2.5:7b",
            (32.., false) => "qwen2.5:3b", // 大内存但纯 CPU，3b 还是能跑
            (16..=31, _) => "qwen2.5:3b",
            (8..=15, _) => "qwen2.5:1.5b",
            _ => "llama3.2:1b",
        }
    }

    /// 人类可读的诊断报告（一行一特性）
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("OS={}", self.os)];
        if !self.cpu_model.is_empty() {
            parts.push(format!("CPU={} ({})", self.cpu_model, self.cpu_vendor));
        }
        if self.total_ram_bytes > 0 {
            const GB: u64 = 1024 * 1024 * 1024;
            parts.push(format!("RAM={} GB", self.total_ram_bytes / GB));
        }
        if self.has_nvidia_gpu {
            parts.push("NVIDIA GPU (/dev/nvidia0)".into());
        }
        if self.has_amd_gpu {
            let gfx = self.amd_gfx_target.as_deref().unwrap_or("unknown");
            parts.push(format!("AMD GPU (gfx={})", gfx));
        }
        if self.has_intel_igpu {
            parts.push("Intel iGPU (/dev/dri/renderD*)".into());
        }
        if self.has_amd_xdna_npu {
            parts.push("AMD XDNA NPU (Ryzen AI)".into());
        }
        if self.has_intel_npu {
            parts.push("Intel NPU (VPU)".into());
        }
        parts.join(" | ")
    }

    /// 基于检测到的硬件，把推荐的环境变量设到当前进程里（子进程继承）。
    /// 已有的环境变量不被覆盖（用户显式设置优先）。
    ///
    /// 返回 (key, reason) 列表，供启动日志打印。
    pub fn apply_recommended_env(&self) -> Vec<(String, String)> {
        let mut applied = Vec::new();

        // AMD iGPU / dGPU：HSA_OVERRIDE_GFX_VERSION
        // gfx1103 (Radeon 780M 等 RDNA3 APU) 不在 ROCm 官方白名单里，需要 override 为
        // 11.0.0 (gfx1100) 才能让 ROCm runtime 接受。
        if self.has_amd_gpu && std::env::var("HSA_OVERRIDE_GFX_VERSION").is_err() {
            let override_ver = match self.amd_gfx_target.as_deref() {
                Some("gfx1103") | Some("gfx1102") | Some("gfx1150") | Some("gfx1151") => {
                    Some("11.0.0")
                }
                Some("gfx1036") | Some("gfx1035") | Some("gfx1034") | Some("gfx1033")
                | Some("gfx1032") | Some("gfx1031") | Some("gfx1030") => Some("10.3.0"),
                _ => None,
            };
            if let Some(ver) = override_ver {
                std::env::set_var("HSA_OVERRIDE_GFX_VERSION", ver);
                applied.push((
                    "HSA_OVERRIDE_GFX_VERSION".into(),
                    format!(
                        "AMD {} → ROCm runtime 兼容 {}",
                        self.amd_gfx_target.as_deref().unwrap_or("?"),
                        ver
                    ),
                ));
            }
        }

        // NVIDIA：若 CUDA_VISIBLE_DEVICES 未设，默认用第一块卡
        if self.has_nvidia_gpu && std::env::var("CUDA_VISIBLE_DEVICES").is_err() {
            std::env::set_var("CUDA_VISIBLE_DEVICES", "0");
            applied.push((
                "CUDA_VISIBLE_DEVICES".into(),
                "NVIDIA 检测 → 默认启用 GPU 0".into(),
            ));
        }

        applied
    }
}

/// 检测设备形态：env var > DMI 关键字 > Laptop 默认
///
/// 1. `ATTUNE_FORM_FACTOR=local_scheduler|laptop|server` env var
/// 2. Linux DMI: `/sys/class/dmi/id/product_name` 包含本地调度器设备关键字
/// 3. Default: `Laptop`
///
/// 不依赖 GPU/NPU 检测 — 形态决定的是"用户预期"，不是"硬件能力"。一台带 NVIDIA 的桌面
/// 仍是 Laptop 形态（用户主动配置本地调度器路径需显式 env var）。
fn detect_form_factor() -> FormFactor {
    // 1. env var override（最高优先）
    if let Ok(v) = std::env::var("ATTUNE_FORM_FACTOR") {
        match v.trim().to_ascii_lowercase().as_str() {
            "local_scheduler" | "local-scheduler" | "localscheduler" | "appliance" => {
                return FormFactor::LocalSchedulerAppliance;
            }
            "laptop" | "desktop" => return FormFactor::Laptop,
            "server" | "headless" => return FormFactor::Server,
            _ => {} // 未识别值，继续 fallback
        }
    }

    // 2. Linux DMI 关键字（本地调度器设备）
    #[cfg(target_os = "linux")]
    {
        if let Ok(name) = std::fs::read_to_string("/sys/class/dmi/id/product_name") {
            let n = name.trim().to_ascii_lowercase();
            if n.contains("local-scheduler") || n.contains("attune-appliance") {
                return FormFactor::LocalSchedulerAppliance;
            }
        }
    }

    // 3. 默认 Laptop（远端 token 路径）
    FormFactor::Laptop
}

/// Linux 下通过 KFD topology 获取 AMD GPU 的 gfx target（形如 "gfx1103"）
///
/// 路径：`/sys/class/kfd/kfd/topology/nodes/*/properties`
/// properties 是多行 key/value，形如 `gfx_target_version 110003` → gfx1103。
/// 节点 0 通常是 CPU（gfx_target_version=0），节点 1+ 才是 GPU；扫全部，
/// 返回首个非零值。
#[cfg(target_os = "linux")]
fn detect_amd_gfx_target() -> Option<String> {
    let nodes_dir = "/sys/class/kfd/kfd/topology/nodes";
    let entries = std::fs::read_dir(nodes_dir).ok()?;
    for entry in entries.flatten() {
        let props_path = entry.path().join("properties");
        let Ok(content) = std::fs::read_to_string(&props_path) else {
            continue;
        };
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("gfx_target_version ") {
                if let Ok(n) = val.trim().parse::<u32>() {
                    if n == 0 {
                        continue;
                    } // CPU 行
                    let major = n / 10000;
                    let minor = (n / 100) % 100;
                    let step = n % 100;
                    return Some(format!("gfx{}{:x}{:x}", major, minor, step));
                }
            }
        }
    }
    None
}

/// Linux 渲染设备 vendor 扫描：识别 AMD / Intel iGPU。
#[cfg(target_os = "linux")]
fn detect_linux_render_gpu_vendors() -> (bool, bool, Option<String>) {
    let mut has_amd = false;
    let mut has_intel = false;

    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return (false, false, None);
    };

    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("renderD") {
            continue;
        }
        let vendor_path = e.path().join("device").join("vendor");
        let Ok(vendor) = std::fs::read_to_string(vendor_path) else {
            continue;
        };
        let v = vendor.trim().to_ascii_lowercase();
        if v == "0x1002" {
            has_amd = true;
        } else if v == "0x8086" {
            has_intel = true;
        }
    }

    let label = if has_amd && has_intel {
        Some("AMD GPU + Intel iGPU".to_string())
    } else if has_amd {
        Some("AMD GPU".to_string())
    } else if has_intel {
        Some("Intel iGPU".to_string())
    } else {
        None
    };

    (has_amd, has_intel, label)
}

#[cfg(not(target_os = "linux"))]
fn detect_amd_gfx_target() -> Option<String> {
    None
}

/// macOS sysctl 辅助：读取 u64 类型的系统参数（hw.memsize 等）
#[cfg(target_os = "macos")]
fn sysctl_u64(key: &str) -> Option<u64> {
    use std::process::Command;
    let out = Command::new("sysctl").args(["-n", key]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// macOS sysctl 辅助：读取字符串参数（machdep.cpu.brand_string 等）
#[cfg(target_os = "macos")]
fn sysctl_string(key: &str) -> Option<String> {
    use std::process::Command;
    let out = Command::new("sysctl").args(["-n", key]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Windows 物理内存：wmic computersystem 的 TotalPhysicalMemory 字段（bytes）
/// Windows GPU 厂商探测：PowerShell CIM `Win32_VideoController`（替代已移除的 wmic.exe）。
/// 扫描显卡名含 NVIDIA / AMD(Radeon) / Intel(Arc) 关键字。
#[cfg(target_os = "windows")]
fn detect_windows_gpu_vendors() -> (bool, bool, bool, Option<String>) {
    let out = match crate::process::command_no_window("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-CimInstance Win32_VideoController).Name -join ';'",
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return (false, false, false, None),
    };
    if !out.status.success() {
        return (false, false, false, None);
    }
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    let has_nv = text.contains("nvidia");
    let has_amd = text.contains("amd") || text.contains("radeon");
    let has_intel = text.contains("intel") || text.contains("arc");
    let label = if has_nv {
        Some("NVIDIA GPU".to_string())
    } else if has_amd {
        Some("AMD GPU".to_string())
    } else if has_intel {
        Some("Intel iGPU".to_string())
    } else {
        None
    };
    (has_nv, has_amd, has_intel, label)
}

/// 解析 Windows NPU PnP 探测输出 → `(has_intel_npu, has_amd_xdna_npu)`。
///
/// 输入是 `detect_windows_npu` 收集的逐行 `"<DeviceID>|<Name>"`（CIM
/// `Win32_PnPEntity` where `PNPClass='ComputeAccelerator'`）。跨厂商干净信号：
/// - DeviceID 含 `VEN_8086` → Intel NPU（Core Ultra "AI Boost"）
/// - DeviceID 含 `VEN_1022` → AMD XDNA NPU（Ryzen AI "NPU Compute Accelerator Device"）
///
/// DeviceID 缺可识别 `VEN_` 时回退到 Name 关键字（`ai boost`→Intel；
/// `npu compute accelerator`→AMD），保证厂商不带 PCI vendor 时仍能分流。
///
/// 纯函数、大小写无关、跨平台编译（Linux 上可单测，无需真 Windows 硬件）。空输入 /
/// 噪声 → `(false, false)`，永不 panic。
//
// 生产中仅 Windows 的 `detect_windows_npu` 调用；非 Windows 编译时只有 `#[cfg(test)]`
// 引用，故非 windows + 非 test build 标 dead_code allow（解析逻辑仍跨平台单测覆盖）。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn parse_windows_npu_pnp(out: &str) -> (bool, bool) {
    let mut has_intel = false;
    let mut has_amd = false;
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lc = line.to_ascii_lowercase();
        // 优先用 PCI vendor id（最干净跨厂商信号）。
        if lc.contains("ven_8086") {
            has_intel = true;
        } else if lc.contains("ven_1022") {
            has_amd = true;
        } else if lc.contains("ai boost") {
            // Name 回退：Intel Core Ultra NPU 暴露为 "Intel(R) AI Boost"。
            has_intel = true;
        } else if lc.contains("npu compute accelerator") {
            // Name 回退：AMD XDNA NPU 暴露为 "NPU Compute Accelerator Device"。
            has_amd = true;
        }
    }
    (has_intel, has_amd)
}

/// Windows NPU 探测：PowerShell CIM `Win32_PnPEntity` where PNPClass='ComputeAccelerator'。
///
/// 仿 [`detect_windows_gpu_vendors`] 的 CIM 模式（替代已移除的 wmic.exe）。每行输出
/// `"<DeviceID>|<Name>"`，交给纯解析器 [`parse_windows_npu_pnp`] 按 VEN_8086(Intel)/
/// VEN_1022(AMD) 分流。子进程失败 / 无 NPU → `(false, false)`，graceful 不 panic。
#[cfg(target_os = "windows")]
fn detect_windows_npu() -> (bool, bool) {
    // ComputeAccelerator class 是 Win11 24H2+ 上 Intel/AMD NPU 的统一 PNPClass；
    // 同时按 Name 兜底匹配，覆盖个别驱动版本不归类到该 class 的情形。
    let out = match crate::process::command_no_window("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_PnPEntity | \
             Where-Object { $_.PNPClass -eq 'ComputeAccelerator' -or \
             $_.Name -match 'AI Boost|NPU Compute Accelerator' } | \
             ForEach-Object { \"$($_.DeviceID)|$($_.Name)\" }",
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return (false, false),
    };
    if !out.status.success() {
        return (false, false);
    }
    parse_windows_npu_pnp(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_end_with_attune_or_legacy() {
        // 迁移期：新安装使用 attune/，老安装保持 npu-vault/。两者都认。
        let dd = data_dir();
        let cd = config_dir();
        let ends_ok = |p: &PathBuf| p.ends_with(APP_DIR) || p.ends_with(LEGACY_APP_DIR);
        assert!(
            ends_ok(&dd),
            "data_dir should end with attune or npu-vault: {:?}",
            dd
        );
        assert!(
            ends_ok(&cd),
            "config_dir should end with attune or npu-vault: {:?}",
            cd
        );
    }

    #[test]
    fn db_path_inside_data_dir() {
        let db = db_path();
        assert!(db.starts_with(data_dir()));
        assert_eq!(db.file_name().unwrap(), "vault.db");
    }

    #[test]
    fn device_secret_inside_config_dir() {
        let ds = device_secret_path();
        assert!(ds.starts_with(config_dir()));
        assert_eq!(ds.file_name().unwrap(), "device.key");
    }

    #[test]
    fn models_dir_inside_data_dir() {
        let md = models_dir();
        assert!(md.starts_with(data_dir()));
        assert!(md.to_str().unwrap().ends_with("models"));
    }

    #[test]
    fn dir_override_pins_data_and_config_then_restores() {
        // When the thread-local override is set, data_dir()/config_dir() return it
        // verbatim (no attune/ suffix). When cleared, behavior must be byte-identical
        // to production — this is the critical invariant for the test-injection seam.
        let prod_data = data_dir();
        let prod_config = config_dir();
        assert!(
            dir_override().is_none(),
            "no override leaked into this test"
        );

        let td = tempfile::tempdir().expect("tempdir");
        let pinned = td.path().to_path_buf();
        let prev = set_dir_override_for_test(Some(pinned.clone()));
        assert_eq!(prev, None, "fresh thread has no prior override");
        assert_eq!(data_dir(), pinned, "data_dir returns override verbatim");
        assert_eq!(config_dir(), pinned, "config_dir returns override verbatim");

        // Restoring None must reproduce production resolution exactly.
        let restored = set_dir_override_for_test(None);
        assert_eq!(restored, Some(pinned));
        assert_eq!(
            data_dir(),
            prod_data,
            "data_dir restored byte-identical to production"
        );
        assert_eq!(
            config_dir(),
            prod_config,
            "config_dir restored byte-identical to production"
        );
    }

    // ── Windows NPU PnP 解析（跨厂商 ComputeAccelerator） ───────────────────
    //
    // 真机探测（CIM Win32_PnPEntity，PNPClass='ComputeAccelerator'）：
    // - Intel Core Ultra 7 155H：Name="Intel(R) AI Boost"，DeviceID 含 VEN_8086 → Intel NPU
    // - AMD Ryzen 7 8845H：Name="NPU Compute Accelerator Device"，DeviceID 含 VEN_1022 → AMD XDNA NPU
    // 跨厂商干净信号：PNPClass='ComputeAccelerator'；VEN_8086=Intel / VEN_1022=AMD 区分。
    //
    // 解析器吃 PowerShell 每行 "<DeviceID>|<Name>" 的输出，吐 (has_intel_npu, has_amd_xdna_npu)。
    // 纯函数（跨平台编译，可在 Linux 上单测，无需真 Windows 硬件）。

    #[test]
    fn parse_npu_pnp_intel_ai_boost_sets_intel_only() {
        let out =
            "PCI\\VEN_8086&DEV_7D1D&SUBSYS_00000000&REV_04\\3&11583659&0&88|Intel(R) AI Boost";
        let (intel, amd) = parse_windows_npu_pnp(out);
        assert!(intel, "VEN_8086 ComputeAccelerator → Intel NPU");
        assert!(!amd, "no AMD NPU on Intel-only machine");
    }

    #[test]
    fn parse_npu_pnp_amd_xdna_sets_amd_only() {
        let out = "PCI\\VEN_1022&DEV_1502&SUBSYS_00000000&REV_00\\3&2411e6fe&0&41|NPU Compute Accelerator Device";
        let (intel, amd) = parse_windows_npu_pnp(out);
        assert!(amd, "VEN_1022 ComputeAccelerator → AMD XDNA NPU");
        assert!(!intel, "no Intel NPU on AMD-only machine");
    }

    #[test]
    fn parse_npu_pnp_empty_or_garbage_is_false() {
        // 无 NPU 机器（空输出 / 噪声）→ 两者皆 false，不 panic。
        assert_eq!(parse_windows_npu_pnp(""), (false, false));
        assert_eq!(parse_windows_npu_pnp("   \n  \n"), (false, false));
        assert_eq!(
            parse_windows_npu_pnp("some unrelated device|Foo Bar"),
            (false, false)
        );
    }

    #[test]
    fn parse_npu_pnp_name_match_fallback_without_vendor_id() {
        // DeviceID 没有可识别 VEN_ 时，回退到 Name 关键字匹配（AI Boost → Intel；
        // NPU Compute Accelerator → AMD），保证厂商缺 DeviceID 时仍能分流。
        let intel_by_name = parse_windows_npu_pnp("ACPI\\SOMETHING\\0|Intel(R) AI Boost");
        assert_eq!(intel_by_name, (true, false));
        let amd_by_name =
            parse_windows_npu_pnp("ACPI\\SOMETHING\\0|NPU Compute Accelerator Device");
        assert_eq!(amd_by_name, (false, true));
    }

    #[test]
    fn parse_npu_pnp_both_vendors_present() {
        // 防御性：两行不同厂商（理论上单机不会，但解析器须能同时置位）。
        let out = "PCI\\VEN_8086&DEV_7D1D\\x|Intel(R) AI Boost\nPCI\\VEN_1022&DEV_1502\\y|NPU Compute Accelerator Device";
        assert_eq!(parse_windows_npu_pnp(out), (true, true));
    }

    #[test]
    fn parse_npu_pnp_case_insensitive() {
        // PowerShell 大小写 / 厂商 ID 大小写无关。
        let out = "pci\\ven_8086&dev_7d1d\\x|intel(r) ai boost";
        assert_eq!(parse_windows_npu_pnp(out), (true, false));
    }

    // ── 端到端接线：Windows NPU profile → AccelCapabilities → EP 链 → wanted 栈 ──
    //
    // 证明 Windows NPU 探测一旦置位 has_intel_npu / has_amd_xdna_npu，下游 EP 选型自动
    // 把 openvino(Intel)/vitisai(AMD) 入链 → runtime_stack 进 wanted 集（spawn_stack_
    // bootstrap 据此装栈）。这是本任务的核心交付：Intel→openvino、AMD→vitisai wanted。

    use crate::infer::accel::{recommend_ep_chain_pure, EpChoice, OpenVinoDevice};
    use crate::platform::accel::AccelCapabilities;

    /// 从 profile 推导 driver-ready 硬件类别（复用生产 from_profile 路径）。
    fn ready_hw(p: &HardwareProfile) -> Vec<crate::platform::AccelKind> {
        AccelCapabilities::from_profile(p)
            .accelerators
            .iter()
            .filter(|a| a.driver_ready)
            .map(|a| a.kind)
            .collect()
    }

    /// EP 链 → 需 userspace 栈的 wanted 集（去重），= state.rs 的 wanted 计算逻辑。
    fn wanted_stacks(chain: &[EpChoice]) -> Vec<&'static str> {
        let mut w = Vec::new();
        for ep in chain {
            if let Some(s) = ep.runtime_stack() {
                if !w.contains(&s) {
                    w.push(s);
                }
            }
        }
        w
    }

    #[test]
    fn windows_intel_npu_profile_wants_openvino() {
        // 模拟 Intel Core Ultra 7 155H：has_intel_npu=true（CIM 探测置位）。
        let p = HardwareProfile {
            os: "windows",
            has_intel_npu: true,
            ..Default::default()
        };
        let hw = ready_hw(&p);
        assert!(
            hw.contains(&crate::platform::AccelKind::IntelNpu),
            "Intel NPU classified"
        );

        // artifact 编入 openvino + cpu → EP 链含 openvino(NPU)。
        let compiled = [EpChoice::Cpu, EpChoice::OpenVino(OpenVinoDevice::Auto)];
        let chain = recommend_ep_chain_pure("windows", &hw, &compiled, None);
        assert!(
            chain.iter().any(|e| e.id() == "openvino"),
            "Intel NPU → openvino in chain: {chain:?}"
        );
        assert_eq!(*chain.last().unwrap(), EpChoice::Cpu, "CPU 兜底末位");

        // wanted 栈含 openvino（Intel NPU 自动配置 OpenVINO 栈）。
        assert!(
            wanted_stacks(&chain).contains(&"openvino"),
            "Intel NPU machine must want openvino stack"
        );
    }

    #[test]
    fn windows_amd_xdna_npu_profile_wants_vitisai() {
        // 模拟 AMD Ryzen 7 8845H：has_amd_xdna_npu=true（CIM 探测置位）。
        let p = HardwareProfile {
            os: "windows",
            has_amd_xdna_npu: true,
            ..Default::default()
        };
        let hw = ready_hw(&p);
        assert!(
            hw.contains(&crate::platform::AccelKind::AmdNpu),
            "AMD XDNA NPU classified"
        );

        let compiled = [EpChoice::Cpu, EpChoice::VitisAi];
        let chain = recommend_ep_chain_pure("windows", &hw, &compiled, None);
        assert!(
            chain.iter().any(|e| e.id() == "vitisai"),
            "AMD XDNA → vitisai in chain: {chain:?}"
        );
        assert_eq!(*chain.last().unwrap(), EpChoice::Cpu);

        assert!(
            wanted_stacks(&chain).contains(&"vitisai"),
            "AMD XDNA NPU machine must want vitisai stack"
        );
    }

    #[test]
    fn windows_no_npu_profile_wants_neither_openvino_nor_vitisai() {
        // 无 NPU 的纯 CPU Windows 机：openvino/vitisai 都不入 wanted。
        let p = HardwareProfile {
            os: "windows",
            ..Default::default()
        };
        let hw = ready_hw(&p);
        let compiled = [
            EpChoice::Cpu,
            EpChoice::OpenVino(OpenVinoDevice::Auto),
            EpChoice::VitisAi,
        ];
        let chain = recommend_ep_chain_pure("windows", &hw, &compiled, None);
        let w = wanted_stacks(&chain);
        assert!(
            !w.contains(&"openvino"),
            "no NPU → no openvino wanted: {w:?}"
        );
        assert!(!w.contains(&"vitisai"), "no NPU → no vitisai wanted: {w:?}");
        assert_eq!(chain, vec![EpChoice::Cpu], "bare Windows → CPU-only chain");
    }

    #[test]
    fn detect_npu_returns_valid_variant() {
        let npu = detect_npu();
        let _ = format!("{:?}", npu);
    }

    #[test]
    fn detect_npu_respects_env_var() {
        std::env::set_var("NPU_VAULT_EP", "cuda");
        assert_eq!(detect_npu(), NpuKind::Cuda);
        std::env::set_var("NPU_VAULT_EP", "cpu");
        assert_eq!(detect_npu(), NpuKind::None);
        std::env::remove_var("NPU_VAULT_EP");
    }

    #[test]
    fn hardware_profile_detects_os() {
        let p = HardwareProfile::detect();
        assert!(
            !p.os.is_empty() && p.os != "unknown",
            "os should be one of linux/macos/windows on current target"
        );
    }

    #[test]
    fn hardware_profile_summary_non_empty() {
        let p = HardwareProfile::detect();
        let s = p.summary();
        assert!(s.contains("OS="), "summary must include OS");
    }

    #[test]
    fn apply_env_noop_on_bare_system() {
        // 在无 AMD/NVIDIA 的 CI 或普通工作站，不应设置任何变量
        let mut p = HardwareProfile::detect();
        p.has_nvidia_gpu = false;
        p.has_amd_gpu = false;
        std::env::remove_var("HSA_OVERRIDE_GFX_VERSION");
        std::env::remove_var("CUDA_VISIBLE_DEVICES");
        let applied = p.apply_recommended_env();
        assert!(
            applied.is_empty(),
            "bare system should apply no env vars: {applied:?}"
        );
    }

    #[test]
    fn summary_model_picks_7b_on_32gb_with_accel() {
        let p = HardwareProfile {
            total_ram_bytes: 32 * 1024 * 1024 * 1024,
            has_amd_xdna_npu: true,
            ..Default::default()
        };
        assert_eq!(p.recommended_summary_model(), "qwen2.5:7b");
    }

    #[test]
    fn summary_model_picks_3b_on_16_31gb() {
        let mut p = HardwareProfile {
            total_ram_bytes: 16 * 1024 * 1024 * 1024,
            has_amd_gpu: true,
            ..Default::default()
        };
        assert_eq!(p.recommended_summary_model(), "qwen2.5:3b");

        p.total_ram_bytes = 31 * 1024 * 1024 * 1024;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:3b");
    }

    #[test]
    fn summary_model_picks_1_5b_on_8_15gb() {
        let mut p = HardwareProfile {
            total_ram_bytes: 8 * 1024 * 1024 * 1024,
            ..Default::default()
        };
        assert_eq!(p.recommended_summary_model(), "qwen2.5:1.5b");

        p.total_ram_bytes = 15 * 1024 * 1024 * 1024;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:1.5b");
    }

    #[test]
    fn summary_model_8gb_with_or_without_accel_returns_same_tier() {
        // 规格：8-16 GB 档位下有/无加速器行为一致（均为 1.5b） — 回归测试
        let p = HardwareProfile {
            total_ram_bytes: 8 * 1024 * 1024 * 1024,
            has_nvidia_gpu: true,
            ..Default::default()
        };
        assert_eq!(
            p.recommended_summary_model(),
            "qwen2.5:1.5b",
            "8GB + accel should still pick 1.5b (RAM-bound)"
        );
    }

    #[test]
    fn summary_model_picks_tiny_on_lowend() {
        let p = HardwareProfile {
            total_ram_bytes: 4 * 1024 * 1024 * 1024,
            ..Default::default()
        };
        assert_eq!(p.recommended_summary_model(), "llama3.2:1b");
    }

    #[test]
    fn summary_model_conservative_on_unknown_ram() {
        // 检测失败 (total_ram_bytes = 0) → 保守 1.5b，避免跑爆小机器
        let p = HardwareProfile::default();
        assert_eq!(p.total_ram_bytes, 0);
        assert_eq!(p.recommended_summary_model(), "qwen2.5:1.5b");
    }

    #[test]
    fn summary_model_big_ram_no_accel_drops_one_tier() {
        // 32GB+ 纯 CPU → 3b (不是 7b)，避免 CPU 推理龟速
        let p = HardwareProfile {
            total_ram_bytes: 64 * 1024 * 1024 * 1024,
            ..Default::default()
        };
        assert_eq!(p.recommended_summary_model(), "qwen2.5:3b");
    }

    #[test]
    fn has_accelerator_checks_all_kinds() {
        let mut p = HardwareProfile::default();
        assert!(!p.has_accelerator());
        p.has_nvidia_gpu = true;
        assert!(p.has_accelerator());
        p.has_nvidia_gpu = false;
        p.has_amd_xdna_npu = true;
        assert!(p.has_accelerator());
    }

    #[test]
    fn ram_reflected_in_summary() {
        let p = HardwareProfile {
            total_ram_bytes: 16 * 1024 * 1024 * 1024,
            ..Default::default()
        };
        assert!(
            p.summary().contains("RAM=16 GB"),
            "summary should include RAM: {}",
            p.summary()
        );
    }

    // ── FormFactor 测试（v0.6.1 新增） ────────────────────────────────────────
    //
    // 注意：这些测试必须在同一个 #[test] 函数里串行操作 ATTUNE_FORM_FACTOR，
    // 因为 cargo test 默认并行；多个测试同时 set/unset 同一 env var 会冲突。
    // 参考 detect_npu_respects_env_var 的同样模式。

    #[test]
    fn form_factor_default_is_laptop() {
        // Default impl 必须返回 Laptop（笔电形态：远端 token 默认）
        // 这条不变量是 settings.rs::default_settings() form_factor 分支的前提。
        assert_eq!(FormFactor::default(), FormFactor::Laptop);
    }

    #[test]
    fn prefers_local_llm_only_for_local_scheduler_appliance() {
        // 本地调度器设备经 :8090 scheduler 收口(本地解决,非直连 Ollama);其他形态走远端 token
        assert!(FormFactor::LocalSchedulerAppliance.prefers_local_llm());
        assert!(!FormFactor::Laptop.prefers_local_llm());
        assert!(!FormFactor::Server.prefers_local_llm());
        assert!(!FormFactor::Unknown.prefers_local_llm());
    }

    #[test]
    fn detect_form_factor_respects_env_override() {
        // ATTUNE_FORM_FACTOR env var 是最高优先级 override
        std::env::set_var("ATTUNE_FORM_FACTOR", "local_scheduler");
        assert_eq!(detect_form_factor(), FormFactor::LocalSchedulerAppliance);

        std::env::set_var("ATTUNE_FORM_FACTOR", "local-scheduler");
        assert_eq!(detect_form_factor(), FormFactor::LocalSchedulerAppliance);

        std::env::set_var("ATTUNE_FORM_FACTOR", "appliance");
        assert_eq!(detect_form_factor(), FormFactor::LocalSchedulerAppliance);

        std::env::set_var("ATTUNE_FORM_FACTOR", "laptop");
        assert_eq!(detect_form_factor(), FormFactor::Laptop);

        std::env::set_var("ATTUNE_FORM_FACTOR", "desktop");
        assert_eq!(detect_form_factor(), FormFactor::Laptop);

        std::env::set_var("ATTUNE_FORM_FACTOR", "server");
        assert_eq!(detect_form_factor(), FormFactor::Server);

        std::env::set_var("ATTUNE_FORM_FACTOR", "headless");
        assert_eq!(detect_form_factor(), FormFactor::Server);

        // 大小写无关
        std::env::set_var("ATTUNE_FORM_FACTOR", "LOCAL_SCHEDULER");
        assert_eq!(detect_form_factor(), FormFactor::LocalSchedulerAppliance);

        std::env::set_var("ATTUNE_FORM_FACTOR", "  local_scheduler  ");
        assert_eq!(
            detect_form_factor(),
            FormFactor::LocalSchedulerAppliance,
            "trim whitespace"
        );

        // 未识别 → fallback 到 Laptop（不 panic）
        std::env::set_var("ATTUNE_FORM_FACTOR", "garbage_value");
        assert_eq!(
            detect_form_factor(),
            FormFactor::Laptop,
            "unknown value falls back to Laptop"
        );

        // 测试结束清理 env var，避免污染其他测试
        std::env::remove_var("ATTUNE_FORM_FACTOR");
    }

    #[test]
    fn form_factor_in_hardware_profile_detect() {
        // detect() 调用链路验证：HardwareProfile 应正确填充 form_factor 字段
        std::env::set_var("ATTUNE_FORM_FACTOR", "local_scheduler");
        let p = HardwareProfile::detect();
        assert_eq!(p.form_factor, FormFactor::LocalSchedulerAppliance);

        std::env::remove_var("ATTUNE_FORM_FACTOR");
        let p = HardwareProfile::detect();
        // 默认 Laptop（除非系统 DMI 显示本地调度器设备，CI 环境正常不会）
        assert!(
            matches!(
                p.form_factor,
                FormFactor::Laptop | FormFactor::LocalSchedulerAppliance
            ),
            "default form_factor should be Laptop or detected local scheduler appliance, got {:?}",
            p.form_factor
        );
    }
}
