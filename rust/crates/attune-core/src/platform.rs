use std::path::PathBuf;

const APP_DIR: &str = "attune";
const LEGACY_APP_DIR: &str = "npu-vault";

pub fn data_dir() -> PathBuf {
    // 容器/headless 环境中 dirs::data_local_dir() 可能返回 None（无 HOME 变量）；
    // 回退到 $HOME/.local/share 或当前目录，确保不 panic。
    //
    // 迁移规则：老目录 npu-vault/ 若存在且新目录 attune/ 不存在，返回老路径（就地复用，
    // 避免升级丢数据）。新建用户使用 attune/。
    let base = dirs::data_local_dir()
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_app_dir(base)
}

pub fn config_dir() -> PathBuf {
    // 同上，回退到 $HOME/.config 或当前目录
    let base = dirs::config_dir()
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
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
    pub cpu_vendor: String,          // e.g. "AuthenticAMD" / "GenuineIntel"
    pub cpu_model: String,           // e.g. "AMD Ryzen 7 8845H..."
    pub has_nvidia_gpu: bool,        // /dev/nvidia0
    pub has_amd_gpu: bool,           // /dev/kfd + /dev/dri/renderD*（AMD 集显或独显）
    pub amd_gfx_target: Option<String>,  // e.g. "gfx1103" (Radeon 780M)，用于 ROCm 匹配
    pub has_amd_xdna_npu: bool,      // /dev/accel/accel0 + amdxdna 模块（Ryzen AI）
    pub has_intel_npu: bool,         // /dev/accel/accel0 + intel_vpu 模块
    pub total_ram_bytes: u64,        // 总内存字节；硬件档位匹配用
    pub os: &'static str,            // "linux" | "macos" | "windows"
}

impl HardwareProfile {
    /// 检测当前宿主的硬件画像（只读、幂等、无副作用）
    pub fn detect() -> Self {
        let mut p = Self {
            os: if cfg!(target_os = "linux") { "linux" }
                else if cfg!(target_os = "macos") { "macos" }
                else if cfg!(target_os = "windows") { "windows" }
                else { "unknown" },
            ..Default::default()
        };

        // CPU vendor/model（Linux 读 /proc/cpuinfo）
        #[cfg(target_os = "linux")]
        if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in info.lines().take(40) {
                if let Some(v) = line.strip_prefix("vendor_id\t: ") { p.cpu_vendor = v.trim().to_string(); }
                if let Some(v) = line.strip_prefix("model name\t: ") { p.cpu_model = v.trim().to_string(); }
                if !p.cpu_vendor.is_empty() && !p.cpu_model.is_empty() { break; }
            }
        }

        // NVIDIA GPU
        p.has_nvidia_gpu = std::path::Path::new("/dev/nvidia0").exists();

        // AMD GPU（集显或独显），通过 /dev/kfd + /dev/dri/renderD128 判定
        p.has_amd_gpu = std::path::Path::new("/dev/kfd").exists()
            && std::path::Path::new("/dev/dri/renderD128").exists();

        // AMD gfx target（识别 Radeon 780M / 780M = gfx1103 等；用于 ROCm HSA 覆盖）
        if p.has_amd_gpu {
            p.amd_gfx_target = detect_amd_gfx_target();
        }

        // NPU：区分 AMD XDNA vs Intel VPU
        if std::path::Path::new("/dev/accel/accel0").exists() {
            if let Ok(mods) = std::fs::read_to_string("/proc/modules") {
                if mods.contains("amdxdna") { p.has_amd_xdna_npu = true; }
                if mods.contains("intel_vpu") { p.has_intel_npu = true; }
            }
        }

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
            p.cpu_vendor = sysctl_string("machdep.cpu.vendor")
                .unwrap_or_else(|| "Apple".to_string());
        }

        // Windows：wmic memorychip + cpu name（两个命令，失败保持 0/empty）
        #[cfg(target_os = "windows")]
        {
            if let Some(ram) = wmic_total_physical_memory() {
                p.total_ram_bytes = ram;
            }
            if let Some((vendor, model)) = wmic_cpu_info() {
                p.cpu_vendor = vendor;
                p.cpu_model = model;
            }
            // Windows 下 NVIDIA 探测：通过 PowerShell 查 Win32_VideoController（非 /dev/nvidia0）
            if has_nvidia_on_windows() {
                p.has_nvidia_gpu = true;
            }
        }

        p
    }

    /// 是否有任何硬件加速（GPU/NPU）— 决定是否能跑稍大的模型
    pub fn has_accelerator(&self) -> bool {
        self.has_nvidia_gpu || self.has_amd_gpu || self.has_amd_xdna_npu || self.has_intel_npu
    }

    /// 根据 RAM + 加速器档位，推荐默认本地摘要模型（仅"用户主动想用本地时"的建议）。
    ///
    /// **v0.6.0-rc.3 行为变化**（per CLAUDE.md "M2 决策" + 用户 2026-04-27 反馈）：
    /// - LLM 默认走**远端 token**（不在本地预装），settings.rs::default_settings.llm.provider 默认引导用户填远端 endpoint
    /// - 本函数仅在用户**显式选本地** Ollama 后给"硬件推荐"用，不再被 default_settings 用作 hardcode 默认
    /// - K3 一体机形态可选装本地 LLM；普通桌面用户应避免本地 chat（避免 OOM / 3B 效果差）
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
            (32.., false) => "qwen2.5:3b",  // 大内存但纯 CPU，3b 还是能跑
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
        if self.has_nvidia_gpu { parts.push("NVIDIA GPU (/dev/nvidia0)".into()); }
        if self.has_amd_gpu {
            let gfx = self.amd_gfx_target.as_deref().unwrap_or("unknown");
            parts.push(format!("AMD GPU (gfx={})", gfx));
        }
        if self.has_amd_xdna_npu { parts.push("AMD XDNA NPU (Ryzen AI)".into()); }
        if self.has_intel_npu { parts.push("Intel NPU (VPU)".into()); }
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
                Some("gfx1103") | Some("gfx1102") | Some("gfx1150") | Some("gfx1151")
                    => Some("11.0.0"),
                Some("gfx1036") | Some("gfx1035") | Some("gfx1034") | Some("gfx1033")
                    | Some("gfx1032") | Some("gfx1031") | Some("gfx1030")
                    => Some("10.3.0"),
                _ => None,
            };
            if let Some(ver) = override_ver {
                std::env::set_var("HSA_OVERRIDE_GFX_VERSION", ver);
                applied.push((
                    "HSA_OVERRIDE_GFX_VERSION".into(),
                    format!("AMD {} → ROCm runtime 兼容 {}",
                        self.amd_gfx_target.as_deref().unwrap_or("?"), ver),
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
        let Ok(content) = std::fs::read_to_string(&props_path) else { continue };
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("gfx_target_version ") {
                if let Ok(n) = val.trim().parse::<u32>() {
                    if n == 0 { continue; }  // CPU 行
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

#[cfg(not(target_os = "linux"))]
fn detect_amd_gfx_target() -> Option<String> { None }

/// macOS sysctl 辅助：读取 u64 类型的系统参数（hw.memsize 等）
#[cfg(target_os = "macos")]
fn sysctl_u64(key: &str) -> Option<u64> {
    use std::process::Command;
    let out = Command::new("sysctl").args(["-n", key]).output().ok()?;
    if !out.status.success() { return None; }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// macOS sysctl 辅助：读取字符串参数（machdep.cpu.brand_string 等）
#[cfg(target_os = "macos")]
fn sysctl_string(key: &str) -> Option<String> {
    use std::process::Command;
    let out = Command::new("sysctl").args(["-n", key]).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Windows 物理内存：wmic computersystem 的 TotalPhysicalMemory 字段（bytes）
#[cfg(target_os = "windows")]
fn wmic_total_physical_memory() -> Option<u64> {
    use std::process::Command;
    let out = Command::new("wmic")
        .args(["computersystem", "get", "TotalPhysicalMemory", "/value"])
        .output().ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("TotalPhysicalMemory=") {
            if let Ok(n) = v.trim().parse::<u64>() {
                return Some(n);
            }
        }
    }
    None
}

/// Windows CPU 厂商+型号：wmic cpu get Manufacturer,Name
#[cfg(target_os = "windows")]
fn wmic_cpu_info() -> Option<(String, String)> {
    use std::process::Command;
    let out = Command::new("wmic")
        .args(["cpu", "get", "Manufacturer,Name", "/value"])
        .output().ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut vendor = String::new();
    let mut model = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Manufacturer=") { vendor = v.trim().to_string(); }
        if let Some(v) = line.strip_prefix("Name=") { model = v.trim().to_string(); }
    }
    if vendor.is_empty() && model.is_empty() { None } else { Some((vendor, model)) }
}

/// Windows NVIDIA 探测：扫描 Win32_VideoController 是否含 "NVIDIA"
#[cfg(target_os = "windows")]
fn has_nvidia_on_windows() -> bool {
    use std::process::Command;
    let out = match Command::new("wmic")
        .args(["path", "win32_VideoController", "get", "Name", "/value"])
        .output() {
        Ok(o) => o,
        Err(_) => return false,
    };
    if !out.status.success() { return false; }
    let text = String::from_utf8_lossy(&out.stdout);
    text.to_lowercase().contains("nvidia")
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
        assert!(ends_ok(&dd), "data_dir should end with attune or npu-vault: {:?}", dd);
        assert!(ends_ok(&cd), "config_dir should end with attune or npu-vault: {:?}", cd);
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
        assert!(!p.os.is_empty() && p.os != "unknown",
            "os should be one of linux/macos/windows on current target");
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
        assert!(applied.is_empty(), "bare system should apply no env vars: {applied:?}");
    }

    #[test]
    fn summary_model_picks_7b_on_32gb_with_accel() {
        let mut p = HardwareProfile::default();
        p.total_ram_bytes = 32 * 1024 * 1024 * 1024;
        p.has_amd_xdna_npu = true;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:7b");
    }

    #[test]
    fn summary_model_picks_3b_on_16_31gb() {
        let mut p = HardwareProfile::default();
        p.total_ram_bytes = 16 * 1024 * 1024 * 1024;
        p.has_amd_gpu = true;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:3b");

        p.total_ram_bytes = 31 * 1024 * 1024 * 1024;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:3b");
    }

    #[test]
    fn summary_model_picks_1_5b_on_8_15gb() {
        let mut p = HardwareProfile::default();
        p.total_ram_bytes = 8 * 1024 * 1024 * 1024;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:1.5b");

        p.total_ram_bytes = 15 * 1024 * 1024 * 1024;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:1.5b");
    }

    #[test]
    fn summary_model_8gb_with_or_without_accel_returns_same_tier() {
        // 规格：8-16 GB 档位下有/无加速器行为一致（均为 1.5b） — 回归测试
        let mut p = HardwareProfile::default();
        p.total_ram_bytes = 8 * 1024 * 1024 * 1024;
        p.has_nvidia_gpu = true;
        assert_eq!(p.recommended_summary_model(), "qwen2.5:1.5b",
            "8GB + accel should still pick 1.5b (RAM-bound)");
    }

    #[test]
    fn summary_model_picks_tiny_on_lowend() {
        let mut p = HardwareProfile::default();
        p.total_ram_bytes = 4 * 1024 * 1024 * 1024;
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
        let mut p = HardwareProfile::default();
        p.total_ram_bytes = 64 * 1024 * 1024 * 1024;
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
        let mut p = HardwareProfile::default();
        p.total_ram_bytes = 16 * 1024 * 1024 * 1024;
        assert!(p.summary().contains("RAM=16 GB"), "summary should include RAM: {}", p.summary());
    }
}
