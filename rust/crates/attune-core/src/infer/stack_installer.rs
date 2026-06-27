//! EP 运行时**软件栈**按需安装器(on-demand EP-runtime-stack installer)。
//!
//! 用户决策(2026-06-17):EP 运行时 userspace 栈(OpenVINO runtime / ROCm userspace /
//! CUDA runtime libs / VitisAI 运行时等)**像底座模型一样**,在部署 / 首次运行时**按需
//! 安装**(系统没有才装),**不全量打包**(护 thin-deb)。**内核驱动除外** —— driver 仍由
//! #6 `platform::npu` 的 consent-gated 命令路径处理,本模块**绝不**装驱动 / 提权 / 换内核。
//!
//! ## 设计(仿 `model_store` / `model_source` S8)
//!
//! - **检测**:给定一个 EP 的 `runtime_stack` 标识(`cuda`/`openvino`/`rocm`/`directml`/
//!   `vitisai`),`probe_stack()` 检查该 userspace 栈是否就位(查 marker 文件 / 已知库路径)。
//! - **按需 fetch**:缺失 → 从 failover 源(company-mirror → CN mirror → 上游)流式下载
//!   栈压缩包到 `models_dir()/ep-stacks/<stack>/`,带 connect+total 超时(复用
//!   `model_store::download_hf_file_from` 的有界守卫),解压,落 `.ready` marker。
//! - **离线尊重**:`HF_HUB_OFFLINE` 置位 → 缓存未命中直接返错(不联网),与底座模型一致。
//! - **后台非阻塞 + 重试**:`spawn_stack_bootstrap` 后台线程顺序处理,最多 3 次重试 + 退避;
//!   进度 / 失败原因落 `StackInstallStatus`,经 status 端点暴露(平行 `model_bootstrap`)。
//! - **装不上 → graceful**:栈装不上 = 该 EP 不可用 → EP 选型链跳过它 → 落 CPU。绝不 panic。
//!
//! ## 安全边界(硬)
//!
//! 只装 **userspace** runtime libs(动态库 / Python wheel 风格的纯文件包),解压到用户
//! data 目录,**不**触碰系统目录、**不** sudo、**不**装内核模块 / 驱动 / 换内核。需要内核
//! 驱动的场景(NPU accel char dev / GPU kernel driver)由 #6 consent-gated 引导,本模块
//! 只负责"驱动已就绪后,补齐 userspace 运行时"。

use crate::error::{Result, VaultError};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 已知 EP 运行时栈标识(与 `accel::EpChoice::runtime_stack()` 对齐)。
///
/// `cuda` 同时覆盖 CUDA EP 与 TensorRT EP(都依赖 CUDA userspace runtime)。
pub const KNOWN_STACKS: [&str; 5] = ["cuda", "openvino", "rocm", "directml", "vitisai"];

/// 单个栈的 userspace 包大小估算(MB,UI 显示「将下载 ~N MB」;PENDING-VERIFY 真实包大小)。
///
/// 这些是 userspace runtime 体积量级(不含驱动)。值用于成本提示,非精确校验。
pub fn stack_size_mb(stack: &str) -> Option<u32> {
    match stack {
        "cuda" => Some(550),     // cudart + cuDNN userspace libs (PENDING-VERIFY)
        "openvino" => Some(280), // OpenVINO runtime libs (PENDING-VERIFY)
        "rocm" => Some(900),     // ROCm userspace runtime (PENDING-VERIFY)
        "directml" => Some(20),  // DirectML.dll redistributable (PENDING-VERIFY)
        "vitisai" => Some(150),  // Ryzen AI / VitisAI userspace (PENDING-VERIFY)
        _ => None,
    }
}

/// EP 栈缓存根目录:`~/.local/share/attune/ep-stacks/`。
pub fn stacks_root() -> PathBuf {
    crate::platform::models_dir().join("ep-stacks")
}

/// 某栈的缓存目录:`ep-stacks/<stack>/`。
pub fn stack_dir(stack: &str) -> PathBuf {
    stacks_root().join(stack)
}

/// 某栈的就绪 marker 文件:`ep-stacks/<stack>/.ready`。存在即视为已安装。
fn stack_ready_marker(stack: &str) -> PathBuf {
    stack_dir(stack).join(".ready")
}

/// 检测某栈的 userspace runtime 是否就位。
///
/// 顺序:
/// 1. `ATTUNE_EP_STACK_<STACK>` env 指向一个已存在目录 → 视作系统已装(用户/打包提供)。
/// 2. 我们自己装的缓存 marker `.ready` 存在 → 已装。
/// 3. 否则 → 未装(`probe` = false)。
///
/// **不**深扫系统库路径(避免误判 / 跨平台分歧);系统已装由打包脚本 / 用户经 env 显式声明。
pub fn probe_stack(stack: &str) -> bool {
    // 1. 显式 env 指向(系统/打包已提供该栈)。
    let env_key = format!("ATTUNE_EP_STACK_{}", stack.to_ascii_uppercase());
    if let Ok(p) = std::env::var(&env_key) {
        let p = p.trim();
        if !p.is_empty() && std::path::Path::new(p).exists() {
            return true;
        }
    }
    // 2. 我们装的缓存 marker。
    stack_ready_marker(stack).exists()
}

/// 离线开关(复用 `model_store::hf_offline` 语义)。
fn offline() -> bool {
    crate::infer::model_store::hf_offline()
}

/// 一个 EP 运行时栈的 failover 下载候选(endpoint 是 HF-resolve 兼容根)。
///
/// **EP-stack ≠ 普通模型**:`attune-ep-stacks/<stack>` artifact 是 **company-mirror-only**
/// (HF/HF-mirror 上 401/404,实测 rc.6 真机)。因此 EP-stack **不能**走 `model_source` 的
/// region-aware `resolve_sources_for`(International region 会把 HF region-boost ×1.5 提到
/// company-mirror 前 → 对 mirror-only artifact 必 401 → 浪费 3 次 attempt 后 directml 装不上)。
///
/// 这里**强制 region-agnostic**:company-mirror 始终在失败链首,后接 Full-coverage 的上游源
/// (hf-official / hf-mirror)作"万一将来 artifact 上了 HF"的兜底。**ModelScope 排除**——
/// 它 `OnlyXenovaOnnx` 不覆盖 `attune-ep-stacks/*`,放进去只会必 404 空转。
///
/// 区别于普通模型(走 HF + 多镜像 + region 路由,见 `model_source::resolve_sources_for`):
/// 那条路径**不变**;只有 mirror-only 的 EP-stack 在此强制 company-mirror 首位。
fn stack_sources(_stack_repo: &str) -> Vec<crate::infer::model_source::ModelSource> {
    use crate::infer::model_source::company_mirror_source;
    // company-mirror 永远首位(region-agnostic);其余 Full 源作 region-agnostic 兜底。
    let mut chain = vec![company_mirror_source()];
    for s in crate::infer::model_source::builtin_sources() {
        // 已含 company-mirror;ModelScope 不覆盖 EP-stack 命名空间 → 排除(必 404)。
        if s.id == "company-mirror" || s.coverage == crate::infer::model_source::SourceCoverage::OnlyXenovaOnnx {
            continue;
        }
        chain.push(s);
    }
    chain
}

/// 某栈在镜像上的 "repo" 命名空间 + 平台包文件名。
///
/// 平台差异:CUDA/OpenVINO/ROCm/VitisAI 的 userspace 包是 OS×arch 相关的;directml 仅 Win。
/// 返回 `(repo_id, filename)`;不支持的 OS/栈组合返回 `None`(调用方记为 unsupported)。
fn stack_artifact(stack: &str, os: &str) -> Option<(String, String)> {
    let repo = format!("attune-ep-stacks/{stack}");
    let file = match (stack, os) {
        ("cuda", "linux") => "cuda-runtime-linux-x86_64.tar.gz",
        ("cuda", "windows") => "cuda-runtime-windows-x86_64.tar.gz",
        ("openvino", "linux") => "openvino-runtime-linux-x86_64.tar.gz",
        ("openvino", "windows") => "openvino-runtime-windows-x86_64.tar.gz",
        ("rocm", "linux") => "rocm-runtime-linux-x86_64.tar.gz",
        ("directml", "windows") => "directml-runtime-windows-x86_64.tar.gz",
        ("vitisai", "windows") => "vitisai-runtime-windows-x86_64.tar.gz",
        ("vitisai", "linux") => "vitisai-runtime-linux-x86_64.tar.gz",
        _ => return None, // 不支持的组合(如 directml on linux / 任意栈 on macos)
    };
    Some((repo, file.to_string()))
}

/// 确保某 EP 运行时栈就位:已装(probe 命中)→ 秒返;缺失 → failover 下载 + 解压 + 落
/// `.ready` marker。**只装 userspace,绝不装驱动**(见模块文档安全边界)。
///
/// `HF_HUB_OFFLINE` 时缓存未命中直接返错(graceful,调用方降级 CPU)。不支持的
/// OS/栈组合返 `Err`(unsupported)。
pub fn ensure_stack(stack: &str) -> Result<PathBuf> {
    if !KNOWN_STACKS.contains(&stack) {
        return Err(VaultError::ModelLoad(format!("unknown EP runtime stack: {stack}")));
    }
    let dir = stack_dir(stack);
    let marker = stack_ready_marker(stack);

    // 已装(env 声明 / 缓存 marker)→ 秒返。
    if probe_stack(stack) {
        return Ok(dir);
    }

    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    };

    let Some((repo, filename)) = stack_artifact(stack, os) else {
        return Err(VaultError::ModelLoad(format!(
            "EP runtime stack {stack} not available for {os}; degrading to CPU. error-code=accel-stack-unsupported-platform"
        )));
    };

    // 离线:缓存未命中 + 不联网 → 返错(逃生开关)。
    if offline() {
        return Err(VaultError::ModelLoad(format!(
            "EP stack {stack} not installed and HF_HUB_OFFLINE is set; refusing network download. error-code=accel-stack-offline-missing"
        )));
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| VaultError::ModelLoad(format!("create EP stack dir {}: {e}", dir.display())))?;

    let archive = dir.join(&filename);
    let sources = stack_sources(&repo);
    // EP-stack 是 company-mirror-only:必须走 forced 变体绕过 HF_ENDPOINT 逃生门,否则
    // init_search_engines 设的 region 默认 HF_ENDPOINT 会把下载劫持到 huggingface.co → 401
    // (155H rc.2 真机实证)。company-mirror 始终在 stack_sources 链首。
    crate::infer::model_source::download_with_failover_forced(&sources, &repo, &filename, &archive)?;

    // 解压 tar.gz 到 stack_dir(纯 Rust flate2+tar,跨平台,不依赖系统 tar)。
    extract_tar_gz(&archive, &dir)?;
    // 解压后删归档省盘。
    let _ = std::fs::remove_file(&archive);

    // 落 .ready marker(probe 据此判已装)。
    std::fs::write(&marker, b"ready")
        .map_err(|e| VaultError::ModelLoad(format!("write EP stack ready marker: {e}")))?;

    Ok(dir)
}

/// 解压 tar.gz 到目标目录(防 zip-slip 路径穿越:拒绝逃出 dest 的条目)。
fn extract_tar_gz(archive: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    let f = std::fs::File::open(archive)
        .map_err(|e| VaultError::ModelLoad(format!("open EP stack archive: {e}")))?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut tar = tar::Archive::new(gz);
    let entries = tar
        .entries()
        .map_err(|e| VaultError::ModelLoad(format!("read EP stack tar entries: {e}")))?;
    let dest_canon = dest.to_path_buf();
    for entry in entries {
        let mut entry =
            entry.map_err(|e| VaultError::ModelLoad(format!("EP stack tar entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| VaultError::ModelLoad(format!("EP stack tar path: {e}")))?
            .into_owned();
        // 路径穿越守卫:拒绝绝对路径 / `..` 逃逸。
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(VaultError::ModelLoad(format!(
                "EP stack archive contains unsafe path {}; aborting. error-code=accel-stack-unsafe-archive",
                path.display()
            )));
        }
        let out = dest_canon.join(&path);
        // 文件条目可能没有先于它的目录条目 → 先建父目录,unpack 才不会因缺目录失败。
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| VaultError::ModelLoad(format!("create EP stack subdir {}: {e}", parent.display())))?;
        }
        entry
            .unpack(&out)
            .map_err(|e| VaultError::ModelLoad(format!("unpack EP stack entry {}: {e}", path.display())))?;
    }
    Ok(())
}

// ── 状态快照(平行 bootstrap_status::ModelBootstrapStatus) ───────────────────

/// 单个栈的安装阶段。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StackPhase {
    /// 已就位(系统已装 / 缓存命中)——无需下载。
    Present,
    /// 未安装(且未触发安装)。
    NotInstalled,
    /// 正在下载 + 解压。
    Installing,
    /// 安装完成(本次会话拉取并解压成功)。
    Installed,
    /// 失败(下载 / 解压 / 离线缺失 / 平台不支持)。
    Failed { error: String },
}

impl StackPhase {
    pub fn is_usable(&self) -> bool {
        matches!(self, StackPhase::Present | StackPhase::Installed)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StackEntry {
    #[serde(flatten)]
    pub phase: StackPhase,
    pub attempts: u32,
    /// userspace 包体积估算(MB),UI 成本提示。
    pub size_mb: Option<u32>,
}

impl Default for StackEntry {
    fn default() -> Self {
        Self { phase: StackPhase::NotInstalled, attempts: 0, size_mb: None }
    }
}

/// 各 EP 运行时栈安装状态快照。`Arc<Mutex<..>>` 内部共享,clone 廉价。
#[derive(Clone, Default)]
pub struct StackInstallStatus {
    inner: Arc<Mutex<BTreeMap<String, StackEntry>>>,
}

impl StackInstallStatus {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(BTreeMap::new())) }
    }

    fn with<R>(&self, f: impl FnOnce(&mut BTreeMap<String, StackEntry>) -> R) -> R {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }

    /// 标记某栈已就位(系统已装 / 缓存命中,无需下载)。
    pub fn mark_present(&self, stack: &str) {
        self.with(|m| {
            let e = m.entry(stack.to_string()).or_default();
            e.phase = StackPhase::Present;
            e.size_mb = stack_size_mb(stack);
        });
    }

    /// 标记进入安装中,attempts += 1。
    pub fn mark_installing(&self, stack: &str) {
        self.with(|m| {
            let e = m.entry(stack.to_string()).or_default();
            e.phase = StackPhase::Installing;
            e.attempts += 1;
            e.size_mb = stack_size_mb(stack);
        });
    }

    /// 标记安装完成。
    pub fn mark_installed(&self, stack: &str) {
        self.with(|m| {
            let e = m.entry(stack.to_string()).or_default();
            e.phase = StackPhase::Installed;
            e.size_mb = stack_size_mb(stack);
        });
    }

    /// 标记失败(带原因,不 panic)。
    pub fn mark_failed(&self, stack: &str, error: impl Into<String>) {
        self.with(|m| {
            let e = m.entry(stack.to_string()).or_default();
            e.phase = StackPhase::Failed { error: error.into() };
            e.size_mb = stack_size_mb(stack);
        });
    }

    /// 读取某栈阶段。
    pub fn phase(&self, stack: &str) -> Option<StackPhase> {
        self.with(|m| m.get(stack).map(|e| e.phase.clone()))
    }

    /// 读取某栈完整条目快照(phase + attempts + size),供再入守卫判断尝试预算。
    pub fn entry_snapshot(&self, stack: &str) -> Option<StackEntry> {
        self.with(|m| m.get(stack).cloned())
    }

    /// JSON 快照(供 status 端点嵌入,平行 model_bootstrap)。
    pub fn snapshot(&self) -> serde_json::Value {
        self.with(|m| {
            let mut stacks = serde_json::Map::new();
            for (k, v) in m.iter() {
                stacks.insert(k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
            }
            serde_json::json!({ "stacks": serde_json::Value::Object(stacks) })
        })
    }
}

/// 单个栈累计安装尝试上限(跨 bootstrap 再调用累计,**不**每次 unlock 清零)。
///
/// rc.5 真机回归:`spawn_stack_bootstrap` 每次 vault unlock 都被 `state::spawn_stack_bootstrap`
/// 重新调用且**无终态守卫**,每次重跑完整 3-attempt 循环 + 重新 `mark_installing`(attempts +=1)
/// → 真机抓到 `directml attempts=4 state=installing`(不是源缺;源实在 —— company-mirror 上
/// `directml-runtime-windows-x86_64.tar.gz` 实测 9.4MB gzip 在线)。守卫 = 已终态(Installed/
/// Present)或已耗尽尝试预算的栈,后续 unlock 不再重跑,attempts 不再无限涨。
pub const STACK_MAX_ATTEMPTS: u32 = 3;

/// 后台按需安装一组 EP 运行时栈(非阻塞,顺序,每栈累计最多 `STACK_MAX_ATTEMPTS` 次)。
///
/// `wanted` = EP 选型链里需要 userspace 栈的那些(`EpChoice::runtime_stack()` 去重)。
/// 已就位(probe 命中)→ 标 Present 跳过;缺失 → ensure_stack(下载+解压)。失败标 Failed
/// 不 panic;离线 / 平台不支持 → 一次失败即停(重试无意义)。
///
/// **再入守卫(rc.6)**:每栈已是终态(Present/Installed)→ 跳过;已累计达
/// `STACK_MAX_ATTEMPTS` 次尝试(无论当前 Failed 还是被快照抓到 Installing)→ 不再重试
/// (避免每次 unlock 重跑令 attempts 无限涨)。要重置须重启进程(冷启动清 status)。
///
/// 进度落 `status`,经 status 端点暴露。装不上的栈 → 对应 EP 在选型链运行时注册失败
/// → ORT 降级 CPU(已被 provider.rs 的 no-error_on_failure 兜住)。
pub fn spawn_stack_bootstrap(status: StackInstallStatus, wanted: Vec<String>) {
    std::thread::spawn(move || {
        const MAX_ATTEMPTS: u32 = STACK_MAX_ATTEMPTS;
        for stack in wanted {
            if !KNOWN_STACKS.contains(&stack.as_str()) {
                continue;
            }
            // 再入守卫:已终态(Present/Installed)→ 跳过;已耗尽尝试预算 → 不再重跑。
            // 这让重复 unlock 不会令 attempts 无限累加(rc.5 真机 directml attempts=4 根因)。
            if let Some(entry) = status.entry_snapshot(&stack) {
                if entry.phase.is_usable() {
                    log::debug!("EP stack {stack} already terminal ({:?}); skipping re-bootstrap", entry.phase);
                    continue;
                }
                if entry.attempts >= MAX_ATTEMPTS {
                    // 已用满预算:若仍非 Failed(被快照抓在 Installing),固化为 Failed,
                    // 不再重跑。后续 unlock 直接命中本守卫跳过。
                    if !matches!(entry.phase, StackPhase::Failed { .. }) {
                        status.mark_failed(
                            &stack,
                            format!(
                                "exhausted {MAX_ATTEMPTS} install attempts across sessions; not retrying until restart"
                            ),
                        );
                    }
                    log::warn!(
                        "EP stack {stack} reached attempt budget ({}/{MAX_ATTEMPTS}); not re-bootstrapping",
                        entry.attempts
                    );
                    continue;
                }
            }
            // 已就位:标 Present,跳过下载。
            if probe_stack(&stack) {
                status.mark_present(&stack);
                log::info!("EP stack {stack} already present (system/cache); skipping install");
                continue;
            }
            let offline = offline();
            let mut last_err = String::new();
            let mut done = false;
            // Budget-aware: only consume the remaining attempts so the cross-session total
            // never exceeds MAX_ATTEMPTS (each mark_installing increments the persisted count).
            let already = status.entry_snapshot(&stack).map(|e| e.attempts).unwrap_or(0);
            let remaining = MAX_ATTEMPTS.saturating_sub(already);
            for n in 1..=remaining {
                status.mark_installing(&stack);
                match ensure_stack(&stack) {
                    Ok(_) => {
                        status.mark_installed(&stack);
                        log::info!("EP stack {stack} installed (attempt {n})");
                        done = true;
                        break;
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        log::warn!("EP stack {stack} install attempt {n}/{MAX_ATTEMPTS} failed: {last_err}");
                        // 离线 / 平台不支持:重试无意义,直接停。
                        if offline || last_err.contains("unsupported-platform") {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(500 * n as u64));
                    }
                }
            }
            if !done {
                status.mark_failed(&stack, last_err.clone());
                log::warn!("EP stack {stack} giving up: {last_err}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_stacks_match_ep_choice_runtime_stacks() {
        use crate::infer::accel::{EpChoice, OpenVinoDevice};
        // 每个 EpChoice 的 runtime_stack(如有)都必须在 KNOWN_STACKS 里。
        let eps = [
            EpChoice::Cpu, EpChoice::Cuda, EpChoice::TensorRt, EpChoice::DirectMl,
            EpChoice::CoreMl, EpChoice::OpenVino(OpenVinoDevice::Auto), EpChoice::Rocm, EpChoice::VitisAi,
        ];
        for ep in eps {
            if let Some(s) = ep.runtime_stack() {
                assert!(KNOWN_STACKS.contains(&s), "{} stack {s} must be known", ep.id());
            }
        }
    }

    #[test]
    fn stack_size_known_for_all() {
        for s in KNOWN_STACKS {
            assert!(stack_size_mb(s).is_some(), "{s} should have a size estimate");
        }
        assert!(stack_size_mb("bogus").is_none());
    }

    #[test]
    fn stack_dir_under_models_dir() {
        let d = stack_dir("cuda");
        assert!(d.starts_with(crate::platform::models_dir()));
        assert!(d.to_str().unwrap().contains("ep-stacks"));
        assert!(d.to_str().unwrap().ends_with("cuda"));
    }

    #[test]
    fn ensure_unknown_stack_errors() {
        let r = ensure_stack("not-a-real-stack");
        assert!(r.is_err());
    }

    #[test]
    fn probe_respects_env_pointer() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ATTUNE_EP_STACK_OPENVINO", dir.path());
        assert!(probe_stack("openvino"), "env pointing to existing dir → present");
        std::env::set_var("ATTUNE_EP_STACK_OPENVINO", "/nonexistent/path/xyz");
        assert!(!probe_stack("openvino"), "env pointing to missing dir → not present (no marker)");
        std::env::remove_var("ATTUNE_EP_STACK_OPENVINO");
    }

    #[test]
    fn stack_sources_company_mirror_first_region_agnostic() {
        // EP-stack artifact 是 company-mirror-only(HF 401)。无论 region 探测结果如何,
        // EP-stack 的 failover 源链**首位必须是 company-mirror**(rc.6 真机:International
        // region 把 HF 提前 → 对 mirror-only artifact 必 401 → directml 装不上)。
        let sources = stack_sources("attune-ep-stacks/directml");
        assert!(!sources.is_empty(), "EP-stack must have at least company-mirror");
        assert_eq!(
            sources[0].id, "company-mirror",
            "EP-stack source chain head must be company-mirror (region-agnostic), got {}",
            sources[0].id
        );
        // 首位 company-mirror 必须 region-neutral(不因 region 被降权/跳过)。
        assert_eq!(sources[0].region_hint, None);
        // 不得包含 ModelScope(OnlyXenovaOnnx 不覆盖 attune-ep-stacks/* → 必 404,无谓重试)。
        assert!(
            !sources.iter().any(|s| s.id == "modelscope"),
            "ModelScope does not cover EP-stack namespace; must not be in chain"
        );
        // 所有源都必须 Full 覆盖(EP-stack repo 只有 Full 源能命中)。
        for s in &sources {
            assert!(
                s.coverage.covers("attune-ep-stacks/directml"),
                "source {} does not cover EP-stack repo",
                s.id
            );
        }
    }

    #[test]
    fn unsupported_platform_artifact_is_none() {
        // directml only on windows.
        assert!(stack_artifact("directml", "linux").is_none());
        assert!(stack_artifact("directml", "windows").is_some());
        // anything on macos unsupported.
        assert!(stack_artifact("cuda", "macos").is_none());
        assert!(stack_artifact("cuda", "linux").is_some());
    }

    #[test]
    fn offline_missing_stack_errors_not_hang() {
        // 离线 + 未装 + 支持平台 → 立即 Err,不联网。
        std::env::set_var("HF_HUB_OFFLINE", "1");
        // 用一个不会被 env/marker 命中的临时 HOME 防误判;这里 directml on linux 直接
        // unsupported,改用 cuda(linux 支持)但确保未装。
        std::env::remove_var("ATTUNE_EP_STACK_CUDA");
        let r = ensure_stack("cuda");
        std::env::remove_var("HF_HUB_OFFLINE");
        // 可能因为 marker 残留而 Ok;但默认无 marker → 应 Err(offline-missing 或 platform)。
        if let Err(e) = r {
            let msg = e.to_string();
            assert!(
                msg.contains("offline") || msg.contains("unsupported") || msg.contains("HF_HUB_OFFLINE"),
                "offline missing stack should be a bounded Err: {msg}"
            );
        }
    }

    #[test]
    fn extract_rejects_path_traversal() {
        // 构造一个含 `../evil.txt` 条目的 tar.gz。`tar::Builder` 自身会拒绝 `..` 路径,
        // 故手工写 512B tar header(name 字段直接放 `../evil.txt`)+ 1 个数据块,模拟
        // 恶意归档,断言我方 extract guard 在 `tar` crate 兜底之外仍主动拒绝(path-
        // traversal 守卫 = 防 zip-slip,office/EP-stack 解压对抗面 P0)。
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.tar.gz");
        {
            let mut block = [0u8; 512];
            let name = b"../evil.txt";
            block[..name.len()].copy_from_slice(name);
            // mode "0000644 "
            block[100..108].copy_from_slice(b"0000644\0");
            // size octal "0000000005 " (5 bytes) at offset 124, 12-byte field
            block[124..135].copy_from_slice(b"00000000005");
            block[135] = b' ';
            // typeflag '0' = regular file
            block[156] = b'0';
            // checksum: fill with spaces, compute, write octal
            for b in &mut block[148..156] { *b = b' '; }
            let sum: u32 = block.iter().map(|&b| b as u32).sum();
            let chk = format!("{sum:06o}\0 ");
            block[148..148 + chk.len()].copy_from_slice(chk.as_bytes());

            let mut data_block = [0u8; 512];
            data_block[..5].copy_from_slice(b"pwned");

            let f = std::fs::File::create(&archive).unwrap();
            let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            use std::io::Write;
            gz.write_all(&block).unwrap();
            gz.write_all(&data_block).unwrap();
            gz.write_all(&[0u8; 1024]).unwrap(); // two zero blocks = end of archive
            gz.finish().unwrap();
        }
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        let r = extract_tar_gz(&archive, &dest);
        assert!(r.is_err(), "path-traversal archive must be rejected");
        assert!(
            r.unwrap_err().to_string().contains("accel-stack-unsafe-archive"),
            "must reject via our explicit guard"
        );
        assert!(!dir.path().join("evil.txt").exists(), "traversal file must not be written");
    }

    #[test]
    fn extract_roundtrip_ok() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("good.tar.gz");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            let mut builder = tar::Builder::new(gz);
            let data = b"libfoo";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, "lib/foo.so", &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_tar_gz(&archive, &dest).unwrap();
        assert!(dest.join("lib/foo.so").exists());
        assert_eq!(std::fs::read(dest.join("lib/foo.so")).unwrap(), b"libfoo");
    }

    // ── 状态快照 ────────────────────────────────────────────────────────

    #[test]
    fn status_lifecycle() {
        let s = StackInstallStatus::new();
        assert!(s.phase("cuda").is_none());
        s.mark_installing("cuda");
        assert_eq!(s.phase("cuda"), Some(StackPhase::Installing));
        assert_eq!(s.snapshot()["stacks"]["cuda"]["state"], "installing");
        assert_eq!(s.snapshot()["stacks"]["cuda"]["attempts"], 1);
        s.mark_installed("cuda");
        assert!(s.phase("cuda").unwrap().is_usable());
        assert_eq!(s.snapshot()["stacks"]["cuda"]["size_mb"], 550);
    }

    #[test]
    fn status_present_is_usable() {
        let s = StackInstallStatus::new();
        s.mark_present("openvino");
        assert_eq!(s.phase("openvino"), Some(StackPhase::Present));
        assert!(s.phase("openvino").unwrap().is_usable());
    }

    #[test]
    fn status_failed_records_error() {
        let s = StackInstallStatus::new();
        s.mark_failed("rocm", "network down");
        match s.phase("rocm").unwrap() {
            StackPhase::Failed { error } => assert_eq!(error, "network down"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(!s.phase("rocm").unwrap().is_usable());
        assert_eq!(s.snapshot()["stacks"]["rocm"]["state"], "failed");
    }

    #[test]
    fn installing_increments_attempts() {
        let s = StackInstallStatus::new();
        s.mark_installing("vitisai");
        s.mark_installing("vitisai");
        s.mark_installing("vitisai");
        assert_eq!(s.snapshot()["stacks"]["vitisai"]["attempts"], 3);
    }

    #[test]
    fn entry_snapshot_reports_phase_and_attempts() {
        let s = StackInstallStatus::new();
        assert!(s.entry_snapshot("cuda").is_none());
        s.mark_installing("cuda");
        s.mark_installing("cuda");
        let e = s.entry_snapshot("cuda").unwrap();
        assert_eq!(e.attempts, 2);
        assert_eq!(e.phase, StackPhase::Installing);
    }

    // ── rc.6: stack bootstrap re-entry guard (bounded attempts across unlocks) ──
    //
    // rc.5 真机：spawn_stack_bootstrap 每次 vault unlock 重跑无守卫 → directml attempts
    // 累加到 4 还 state=installing。守卫断言：跨多次 bootstrap 调用，单栈 attempts 不超
    // STACK_MAX_ATTEMPTS，且达上限后固化为 Failed（不再 Installing 无限重试）。

    /// 用一个不支持平台的栈触发即时失败（不联网），多次调用 bootstrap，断言 attempts 封顶。
    #[test]
    fn stack_bootstrap_caps_attempts_across_invocations() {
        // directml on linux is unsupported → ensure_stack errors instantly (no network),
        // so each bootstrap attempt is deterministic + offline-safe in CI.
        #[cfg(target_os = "linux")]
        {
            let s = StackInstallStatus::new();
            // Invoke the bootstrap several times (simulating repeated vault unlocks).
            for _ in 0..5 {
                spawn_stack_bootstrap(s.clone(), vec!["directml".to_string()]);
                // join: the spawned thread is short (instant unsupported-platform error).
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
            // Wait for terminal state.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                if let Some(e) = s.entry_snapshot("directml") {
                    if matches!(e.phase, StackPhase::Failed { .. }) {
                        // The critical invariant: attempts never exceeds the budget,
                        // even though we invoked bootstrap 5 times.
                        assert!(
                            e.attempts <= STACK_MAX_ATTEMPTS,
                            "attempts {} must be capped at {STACK_MAX_ATTEMPTS} across re-invocations",
                            e.attempts
                        );
                        break;
                    }
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "stack should reach Failed terminal state; snapshot={}",
                    s.snapshot()
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    /// 已终态(Installed)的栈，再次 bootstrap 直接跳过（不重置 / 不重试）。
    #[test]
    fn stack_bootstrap_skips_terminal_installed() {
        let s = StackInstallStatus::new();
        s.mark_installed("cuda");
        let before = s.entry_snapshot("cuda").unwrap().attempts;
        spawn_stack_bootstrap(s.clone(), vec!["cuda".to_string()]);
        std::thread::sleep(std::time::Duration::from_millis(150));
        let after = s.entry_snapshot("cuda").unwrap();
        assert_eq!(after.phase, StackPhase::Installed, "must stay Installed");
        assert_eq!(after.attempts, before, "terminal stack must not be re-attempted");
    }

    /// 达到 attempts 预算但被抓在 Installing（真机 directml=4 场景），再入应固化为 Failed。
    #[test]
    fn stack_bootstrap_seals_stuck_installing_at_budget() {
        let s = StackInstallStatus::new();
        // Simulate a snapshot caught mid-installing at the attempt budget.
        for _ in 0..STACK_MAX_ATTEMPTS {
            s.mark_installing("openvino");
        }
        assert_eq!(s.entry_snapshot("openvino").unwrap().phase, StackPhase::Installing);
        // Re-invoke bootstrap: the guard must seal it to Failed (no further retry).
        spawn_stack_bootstrap(s.clone(), vec!["openvino".to_string()]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let e = s.entry_snapshot("openvino").unwrap();
            if matches!(e.phase, StackPhase::Failed { .. }) {
                assert_eq!(e.attempts, STACK_MAX_ATTEMPTS, "attempts must not exceed budget");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "stuck-installing at budget must be sealed to Failed; got {:?}",
                e.phase
            );
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
}
