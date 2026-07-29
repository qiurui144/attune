//! Local resource admission for single-machine workloads.
//!
//! This module is intentionally pure and deterministic: callers feed it a resource
//! snapshot plus the workload estimate, and it returns whether the work may run
//! now, should queue, should degrade to CPU, or must be rejected. OS probing stays
//! outside this module so tests can cover low-RAM, low-disk, no-GPU and battery
//! scenarios without depending on the machine running the test.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalWorkKind {
    Llm,
    Vision,
    Embedding,
    Ocr,
    Asr,
    ModelDownload,
}

impl LocalWorkKind {
    fn is_heavy(self) -> bool {
        matches!(
            self,
            LocalWorkKind::Llm
                | LocalWorkKind::Vision
                | LocalWorkKind::Ocr
                | LocalWorkKind::Asr
                | LocalWorkKind::ModelDownload
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecutionMode {
    Cpu,
    Gpu,
    CpuFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAdmissionReason {
    ParallelLimit,
    InsufficientMemory,
    InsufficientVram,
    LowDisk,
    LowBattery,
    GpuUnavailable,
    NetworkOffline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAdmission {
    Run(LocalExecutionMode),
    Queue(LocalAdmissionReason),
    Reject(LocalAdmissionReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalResourceSnapshot {
    pub cpu_cores: u16,
    pub ram_available_bytes: u64,
    pub disk_available_bytes: u64,
    /// `None` means no GPU/VRAM was detected or the probe failed.
    pub gpu_vram_available_bytes: Option<u64>,
    pub on_battery: bool,
    pub battery_pct: Option<u8>,
    pub offline: bool,
}

impl LocalResourceSnapshot {
    pub fn unknown_conservative() -> Self {
        Self {
            cpu_cores: 0,
            ram_available_bytes: 0,
            disk_available_bytes: 0,
            gpu_vram_available_bytes: None,
            on_battery: false,
            battery_pct: None,
            offline: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalResourcePolicy {
    pub ram_reserve_bytes: u64,
    pub vram_reserve_bytes: u64,
    pub disk_reserve_bytes: u64,
    pub low_battery_pct: u8,
}

impl Default for LocalResourcePolicy {
    fn default() -> Self {
        Self {
            // Keep enough headroom for the desktop shell, SQLite, browser view and
            // OS cache on a personal machine.
            ram_reserve_bytes: 1024 * 1024 * 1024,
            vram_reserve_bytes: 512 * 1024 * 1024,
            disk_reserve_bytes: 2 * 1024 * 1024 * 1024,
            low_battery_pct: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorkload {
    pub kind: LocalWorkKind,
    pub model_id: String,
    pub requires_gpu: bool,
    pub allow_cpu_fallback: bool,
    pub requires_network: bool,
    pub estimated_ram_bytes: u64,
    pub estimated_vram_bytes: u64,
    pub scratch_bytes: u64,
    pub max_parallel: usize,
}

impl LocalWorkload {
    pub fn new(kind: LocalWorkKind, model_id: impl Into<String>) -> Self {
        Self {
            kind,
            model_id: model_id.into(),
            requires_gpu: false,
            allow_cpu_fallback: true,
            requires_network: false,
            estimated_ram_bytes: 0,
            estimated_vram_bytes: 0,
            scratch_bytes: 0,
            max_parallel: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActiveLocalWork {
    pub running_count: usize,
    pub reserved_ram_bytes: u64,
    pub reserved_vram_bytes: u64,
    pub reserved_scratch_bytes: u64,
}

pub fn admit_local_work(
    snapshot: &LocalResourceSnapshot,
    policy: &LocalResourcePolicy,
    workload: &LocalWorkload,
    active: &ActiveLocalWork,
) -> LocalAdmission {
    if workload.requires_network && snapshot.offline {
        return LocalAdmission::Queue(LocalAdmissionReason::NetworkOffline);
    }

    let disk_needed = policy
        .disk_reserve_bytes
        .saturating_add(active.reserved_scratch_bytes)
        .saturating_add(workload.scratch_bytes);
    if workload.scratch_bytes > 0 && snapshot.disk_available_bytes < disk_needed {
        return LocalAdmission::Reject(LocalAdmissionReason::LowDisk);
    }

    if snapshot.on_battery
        && workload.kind.is_heavy()
        && snapshot
            .battery_pct
            .is_some_and(|pct| pct <= policy.low_battery_pct)
    {
        return LocalAdmission::Queue(LocalAdmissionReason::LowBattery);
    }

    if active.running_count >= workload.max_parallel.max(1) {
        return LocalAdmission::Queue(LocalAdmissionReason::ParallelLimit);
    }

    let ram_needed = policy
        .ram_reserve_bytes
        .saturating_add(active.reserved_ram_bytes)
        .saturating_add(workload.estimated_ram_bytes);
    if snapshot.ram_available_bytes < ram_needed {
        return LocalAdmission::Queue(LocalAdmissionReason::InsufficientMemory);
    }

    if workload.requires_gpu {
        let Some(vram_available) = snapshot.gpu_vram_available_bytes else {
            return if workload.allow_cpu_fallback {
                LocalAdmission::Run(LocalExecutionMode::CpuFallback)
            } else {
                LocalAdmission::Reject(LocalAdmissionReason::GpuUnavailable)
            };
        };
        let vram_needed = policy
            .vram_reserve_bytes
            .saturating_add(active.reserved_vram_bytes)
            .saturating_add(workload.estimated_vram_bytes);
        if vram_available < vram_needed {
            return LocalAdmission::Queue(LocalAdmissionReason::InsufficientVram);
        }
        return LocalAdmission::Run(LocalExecutionMode::Gpu);
    }

    LocalAdmission::Run(LocalExecutionMode::Cpu)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn desktop() -> LocalResourceSnapshot {
        LocalResourceSnapshot {
            cpu_cores: 8,
            ram_available_bytes: 16 * GB,
            disk_available_bytes: 100 * GB,
            gpu_vram_available_bytes: Some(8 * GB),
            on_battery: false,
            battery_pct: None,
            offline: false,
        }
    }

    fn llm() -> LocalWorkload {
        LocalWorkload {
            kind: LocalWorkKind::Llm,
            model_id: "qwen2.5:7b-q4".into(),
            requires_gpu: true,
            allow_cpu_fallback: true,
            requires_network: false,
            estimated_ram_bytes: 6 * GB,
            estimated_vram_bytes: 5 * GB,
            scratch_bytes: 512 * 1024 * 1024,
            max_parallel: 1,
        }
    }

    #[test]
    fn admits_gpu_work_when_headroom_exists() {
        assert_eq!(
            admit_local_work(
                &desktop(),
                &LocalResourcePolicy::default(),
                &llm(),
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Run(LocalExecutionMode::Gpu)
        );
    }

    #[test]
    fn no_gpu_degrades_to_cpu_when_allowed() {
        let mut snap = desktop();
        snap.gpu_vram_available_bytes = None;
        assert_eq!(
            admit_local_work(
                &snap,
                &LocalResourcePolicy::default(),
                &llm(),
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Run(LocalExecutionMode::CpuFallback)
        );
    }

    #[test]
    fn no_gpu_rejects_when_cpu_fallback_is_not_allowed() {
        let mut snap = desktop();
        snap.gpu_vram_available_bytes = None;
        let mut work = llm();
        work.allow_cpu_fallback = false;
        assert_eq!(
            admit_local_work(
                &snap,
                &LocalResourcePolicy::default(),
                &work,
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Reject(LocalAdmissionReason::GpuUnavailable)
        );
    }

    #[test]
    fn queues_second_local_model_when_parallel_limit_reached() {
        assert_eq!(
            admit_local_work(
                &desktop(),
                &LocalResourcePolicy::default(),
                &llm(),
                &ActiveLocalWork {
                    running_count: 1,
                    reserved_ram_bytes: 6 * GB,
                    reserved_vram_bytes: 5 * GB,
                    reserved_scratch_bytes: 0,
                },
            ),
            LocalAdmission::Queue(LocalAdmissionReason::ParallelLimit)
        );
    }

    #[test]
    fn queues_when_ram_headroom_would_be_exhausted() {
        let mut snap = desktop();
        snap.ram_available_bytes = 6 * GB;
        assert_eq!(
            admit_local_work(
                &snap,
                &LocalResourcePolicy::default(),
                &llm(),
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Queue(LocalAdmissionReason::InsufficientMemory)
        );
    }

    #[test]
    fn queues_when_vram_headroom_would_be_exhausted() {
        let mut snap = desktop();
        snap.gpu_vram_available_bytes = Some(5 * GB);
        assert_eq!(
            admit_local_work(
                &snap,
                &LocalResourcePolicy::default(),
                &llm(),
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Queue(LocalAdmissionReason::InsufficientVram)
        );
    }

    #[test]
    fn rejects_model_download_when_disk_headroom_is_too_low() {
        let mut snap = desktop();
        snap.disk_available_bytes = 3 * GB;
        let mut work = LocalWorkload::new(LocalWorkKind::ModelDownload, "vision-model");
        work.requires_network = true;
        work.scratch_bytes = 2 * GB;
        assert_eq!(
            admit_local_work(
                &snap,
                &LocalResourcePolicy::default(),
                &work,
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Reject(LocalAdmissionReason::LowDisk)
        );
    }

    #[test]
    fn queues_heavy_work_on_low_battery() {
        let mut snap = desktop();
        snap.on_battery = true;
        snap.battery_pct = Some(12);
        assert_eq!(
            admit_local_work(
                &snap,
                &LocalResourcePolicy::default(),
                &llm(),
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Queue(LocalAdmissionReason::LowBattery)
        );
    }

    #[test]
    fn queues_network_work_while_offline() {
        let mut snap = desktop();
        snap.offline = true;
        let mut work = LocalWorkload::new(LocalWorkKind::ModelDownload, "embed-model");
        work.requires_network = true;
        work.scratch_bytes = GB;
        assert_eq!(
            admit_local_work(
                &snap,
                &LocalResourcePolicy::default(),
                &work,
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Queue(LocalAdmissionReason::NetworkOffline)
        );
    }

    #[test]
    fn unknown_probe_is_conservative_not_unbounded() {
        let mut work = LocalWorkload::new(LocalWorkKind::Embedding, "bge-small");
        work.estimated_ram_bytes = 256 * 1024 * 1024;
        assert_eq!(
            admit_local_work(
                &LocalResourceSnapshot::unknown_conservative(),
                &LocalResourcePolicy::default(),
                &work,
                &ActiveLocalWork::default()
            ),
            LocalAdmission::Queue(LocalAdmissionReason::InsufficientMemory)
        );
    }
}
