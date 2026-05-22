// 资源采样：CPU% / RSS / 时间戳。封装 sysinfo 跨平台细节。
//
// **CPU 语义（重要）**：`Sample::cpu_pct` 是**系统全局 CPU 占用百分比 (0–100)**，
// 不是单进程占用。这与"系统友好"哲学一致 — 关键不是"我用了多少"，而是"系统现在
// 忙不忙，我该不该让出 CPU"。多个 governor 共享同一全局指标，自动避免每任务
// budget 累加 > 100% 的失真。
//
// **设计取舍**：sysinfo 0.32 的 `Process::cpu_usage()` 在 self-process 下有
// 已知 quirk（部分平台返回 0），且 cpu_usage() 与 nice/cgroup 限制后的真实占用
// 不一致。改用 `sys.global_cpu_usage()` 更稳定且更符合"做系统好公民"的语义。
//
// **采样节流**：sysinfo 文档要求两次 CPU refresh 间隔 ≥ MINIMUM_CPU_UPDATE_INTERVAL
// (Linux 200ms)；过快刷新会让结果退化。这里用 250ms 内部缓存。
// **RSS**：`sys.process(pid).memory()` 返回当前进程驻留集，行为正常，可继续使用。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// CPU sample 缓存窗口 — 必须 ≥ sysinfo MINIMUM_CPU_UPDATE_INTERVAL。
/// 250ms 在所有平台都安全且对 worker 节流粒度足够。
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// 单次资源采样。`cpu_pct` 是**系统全局**占用，0–100。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Sample {
    pub cpu_pct: f32,
    pub rss_bytes: u64,
    /// Sample 生成时的 monotonic 秒数。仅内部时间差用，对外不序列化。
    #[serde(skip)]
    pub(crate) captured_secs: u64,
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            cpu_pct: 0.0,
            rss_bytes: 0,
            captured_secs: 0,
        }
    }
}

impl Sample {
    /// 构造一个用于注入 [`MockMonitor`] 的 sample。`captured_secs` 是 crate-private
    /// 的内部时间字段，外部 crate 无法用 struct-literal / struct-update 构造 `Sample`，
    /// 故提供此构造函数让集成测试能注入指定 `cpu_pct` / `rss_bytes`。
    pub fn new(cpu_pct: f32, rss_bytes: u64) -> Self {
        Self {
            cpu_pct,
            rss_bytes,
            captured_secs: 0,
        }
    }
}

/// 资源监控 trait。生产用 [`SysinfoMonitor`]，测试用 [`MockMonitor`] 注入固定 sample。
pub trait ResourceMonitor: Send + Sync {
    /// 返回系统全局 CPU% + 当前进程 RSS。
    fn sample_self(&self) -> Sample;
}

/// 真 sysinfo 后端 — 跨 Linux/Win/macOS。
pub struct SysinfoMonitor {
    inner: Mutex<SysinfoState>,
    pid: Pid,
    start: Instant,
}

struct SysinfoState {
    sys: System,
    last_refresh: Instant,
    last_sample: Sample,
}

impl SysinfoMonitor {
    pub fn new() -> Self {
        let pid = Pid::from_u32(std::process::id());
        let mut sys = System::new();
        // baseline refresh：CPU 全局 + process RSS。两次 refresh 间隔后 cpu_usage 才有意义。
        sys.refresh_cpu_specifics(CpuRefreshKind::new().with_cpu_usage());
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::new().with_memory(),
        );
        let initial_sample = sys
            .process(pid)
            .map(|p| Sample {
                cpu_pct: 0.0,
                rss_bytes: p.memory(),
                captured_secs: 0,
            })
            .unwrap_or_default();
        Self {
            inner: Mutex::new(SysinfoState {
                sys,
                // 让首次 sample_self() 必然 refresh
                last_refresh: Instant::now() - REFRESH_INTERVAL * 2,
                last_sample: initial_sample,
            }),
            pid,
            start: Instant::now(),
        }
    }

    fn elapsed_secs(&self) -> u64 {
        self.start.elapsed().as_secs()
    }
}

impl Default for SysinfoMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceMonitor for SysinfoMonitor {
    fn sample_self(&self) -> Sample {
        let captured = self.elapsed_secs();
        let mut state = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // 缓存窗口内直接返回上次结果 — 避免 sysinfo 高频刷新退化
        if state.last_refresh.elapsed() < REFRESH_INTERVAL {
            let mut s = state.last_sample;
            s.captured_secs = captured;
            return s;
        }
        // 全局 CPU + process RSS 配对刷新
        state.sys.refresh_cpu_specifics(CpuRefreshKind::new().with_cpu_usage());
        state.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::new().with_memory(),
        );
        // 全局 CPU% = 各核 cpu_usage 的均值（每核 0–100）
        let cpus = state.sys.cpus();
        let global_cpu = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
        };
        let rss = state
            .sys
            .process(self.pid)
            .map(|p| p.memory())
            .unwrap_or(0);
        let sample = Sample {
            cpu_pct: global_cpu,
            rss_bytes: rss,
            captured_secs: captured,
        };
        state.last_refresh = Instant::now();
        state.last_sample = sample;
        sample
    }
}

/// 测试专用 monitor。注入指定 sample，单元测试无需消耗真 CPU。
pub struct MockMonitor {
    sample: Mutex<Sample>,
}

impl MockMonitor {
    pub fn new(sample: Sample) -> Self {
        Self {
            sample: Mutex::new(sample),
        }
    }

    pub fn set(&self, sample: Sample) {
        let mut g = match self.sample.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *g = sample;
    }
}

impl ResourceMonitor for MockMonitor {
    fn sample_self(&self) -> Sample {
        let g = match self.sample.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn mock_monitor_returns_injected_sample() {
        let m = MockMonitor::new(Sample {
            cpu_pct: 42.0,
            rss_bytes: 1024,
            captured_secs: 0,
        });
        let s = m.sample_self();
        assert_eq!(s.cpu_pct, 42.0);
        assert_eq!(s.rss_bytes, 1024);
    }

    #[test]
    fn mock_monitor_set_updates() {
        let m = MockMonitor::new(Sample::default());
        m.set(Sample {
            cpu_pct: 99.0,
            rss_bytes: 0,
            captured_secs: 5,
        });
        assert_eq!(m.sample_self().cpu_pct, 99.0);
        assert_eq!(m.sample_self().captured_secs, 5);
    }

    #[test]
    fn sysinfo_monitor_returns_nonzero_rss() {
        // 真 monitor 至少能拿到当前进程的 RSS（>0），CPU% 首次可能为 0 是预期。
        let m = SysinfoMonitor::new();
        let s = m.sample_self();
        assert!(s.rss_bytes > 0, "RSS should be positive for a running test process");
    }

    #[test]
    fn sysinfo_monitor_cpu_pct_is_in_valid_range() {
        // 全局 CPU% 必须在 0–100 范围（每核 0–100，均值仍 0–100）
        let m = SysinfoMonitor::new();
        let _ = m.sample_self();
        thread::sleep(REFRESH_INTERVAL + Duration::from_millis(50));
        let s = m.sample_self();
        assert!(
            s.cpu_pct >= 0.0 && s.cpu_pct <= 100.0,
            "global cpu_pct should be in 0-100, got {}",
            s.cpu_pct
        );
    }

    #[test]
    fn sysinfo_monitor_caches_within_refresh_interval() {
        // 同一刷新窗口内多次 sample 应返回同一缓存值
        let m = SysinfoMonitor::new();
        let s1 = m.sample_self();
        let s2 = m.sample_self();
        assert_eq!(s1.cpu_pct, s2.cpu_pct);
        assert_eq!(s1.rss_bytes, s2.rss_bytes);
    }
}
