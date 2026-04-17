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

        p
    }

    /// 人类可读的诊断报告（一行一特性）
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("OS={}", self.os)];
        if !self.cpu_model.is_empty() {
            parts.push(format!("CPU={} ({})", self.cpu_model, self.cpu_vendor));
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

/// Linux 下通过读 /sys 获取 AMD GPU 的 gfx target（形如 "gfx1103"）
///
/// 通常 card1 是集显（APU），card0 为独显；两个都扫一遍，返回首个有效值。
#[cfg(target_os = "linux")]
fn detect_amd_gfx_target() -> Option<String> {
    for card in ["card0", "card1"] {
        let p = format!("/sys/class/drm/{card}/device/gfx_target_version");
        if let Ok(s) = std::fs::read_to_string(&p) {
            // 值是十进制版本号，如 "110300" → gfx1103
            let s = s.trim();
            if let Ok(n) = s.parse::<u32>() {
                let major = n / 10000;
                let minor = (n / 100) % 100;
                let step = n % 100;
                return Some(format!("gfx{}{:x}{:x}", major, minor, step));
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn detect_amd_gfx_target() -> Option<String> { None }

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
}
