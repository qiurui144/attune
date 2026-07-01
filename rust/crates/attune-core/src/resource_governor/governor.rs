// TaskGovernor — 单个后台任务的"协作式调度"决策点。
//
// Worker loop 在每次迭代头部调用 [`TaskGovernor::should_run`]，
// 在每次工作完成后调用 [`TaskGovernor::after_work`] 决定下次 sleep 时长。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::budget::Budget;
use super::monitor::{ResourceMonitor, Sample};
use super::profiles::{Profile, TaskKind};
use crate::platform::PowerSource;

/// LLM 调用滑动窗口大小（以小时为单位记数）。
const LLM_WINDOW_SECS: u64 = 3600;

/// 电池/能效约束态下，后台任务的有效 CPU 上限（覆盖 profile budget，取 min）。
/// 15% = Conservative 档，足够后台 embedding 慢速推进但不烧电池/不抢前台。
const BATTERY_CPU_CAP: f32 = 15.0;
/// 电池/能效约束态下，每批后强制退让时长（即使 CPU 未超 budget 也慢下来）。
const BATTERY_THROTTLE_MS: u64 = 2000;

/// 电池态下后台任务的处置策略（settings 可配，registry 下发到所有 governor）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BatteryPolicy {
    /// 电池时节流（Conservative budget），低电/热时暂停。默认。
    #[default]
    Throttle,
    /// 电池时直接暂停全部后台任务（接电恢复）。
    Pause,
    /// 不做电源感知（始终按 profile budget，台式/不在乎电池的用户）。
    Off,
}

/// 电源感知策略配置。`should_run`/`after_work` 读取它 + Sample.power 做决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PowerPolicy {
    pub mode: BatteryPolicy,
    /// 低于此电量(%)则后台任务暂停（仅 Throttle 模式；Pause 模式任何电池即停）。
    pub low_battery_pct: u8,
}

impl Default for PowerPolicy {
    fn default() -> Self {
        PowerPolicy {
            mode: BatteryPolicy::Throttle,
            low_battery_pct: 20,
        }
    }
}

/// 单任务调度器。Clone 友好：内部全是 `Arc<...>`。
pub struct TaskGovernor {
    pub kind: TaskKind,
    profile: Arc<RwLock<Profile>>,
    budget: Arc<RwLock<Budget>>,
    paused: Arc<AtomicBool>,
    monitor: Arc<dyn ResourceMonitor>,
    last_sample: Arc<Mutex<Sample>>,
    /// LLM 调用时间戳滑动窗口（单调时间，秒）。
    llm_calls: Arc<Mutex<VecDeque<u64>>>,
    /// 电源感知策略（registry 跨所有 governor 下发；默认 Throttle/20%）。
    power_policy: Arc<RwLock<PowerPolicy>>,
    /// 用于计算 `Sample::captured_secs` 的基准。
    start: Instant,
}

impl TaskGovernor {
    pub fn new(kind: TaskKind, profile: Profile, monitor: Arc<dyn ResourceMonitor>) -> Self {
        let budget = profile.budget_for(kind);
        Self {
            kind,
            profile: Arc::new(RwLock::new(profile)),
            budget: Arc::new(RwLock::new(budget)),
            paused: Arc::new(AtomicBool::new(false)),
            monitor,
            last_sample: Arc::new(Mutex::new(Sample::default())),
            llm_calls: Arc::new(Mutex::new(VecDeque::new())),
            power_policy: Arc::new(RwLock::new(PowerPolicy::default())),
            start: Instant::now(),
        }
    }

    /// 设置电源感知策略（registry 下发；settings 路由更新时调用）。
    pub fn set_power_policy(&self, p: PowerPolicy) {
        if let Ok(mut g) = self.power_policy.write() {
            *g = p;
        }
    }

    pub fn power_policy(&self) -> PowerPolicy {
        match self.power_policy.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    /// Worker loop 头部调用。返回 false → worker 应短 sleep 后重试。
    ///
    /// 决策顺序：
    /// 1. 全局/本任务被 pause → false
    /// 2. CPU 已超 budget.cpu_pct_max → false
    /// 3. 否则 → true
    pub fn should_run(&self) -> bool {
        if self.paused.load(Ordering::SeqCst) {
            return false;
        }
        let sample = self.monitor.sample_self();
        if let Ok(mut last) = self.last_sample.lock() {
            *last = sample;
        }
        let budget = match self.budget.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        };
        // 电源感知（两层杠杆中的运行时主杠杆）：电池/热 → 暂停或收紧 CPU 上限。
        let policy = self.power_policy();
        let mut cpu_max = budget.cpu_pct_max;
        if policy.mode != BatteryPolicy::Off {
            // 低电(<low_battery_pct)或热降频 → 硬暂停后台，等接电/降温。
            if sample.power.should_pause_background(policy.low_battery_pct) {
                return false;
            }
            match policy.mode {
                // Pause 模式：任何电池供电即停全部后台。
                BatteryPolicy::Pause if matches!(sample.power.source, PowerSource::Battery) => {
                    return false;
                }
                // Throttle 模式：能效约束态(电池/saver/热)收紧到 Conservative 档。
                BatteryPolicy::Throttle if sample.power.is_energy_constrained() => {
                    cpu_max = cpu_max.min(BATTERY_CPU_CAP);
                }
                _ => {}
            }
        }
        if sample.cpu_pct > cpu_max {
            return false;
        }
        true
    }

    /// 完成一批工作后调用。返回 worker 应当 sleep 的时长。
    ///
    /// - 若上次采样接近 budget（>80%），返回 throttle 上限
    /// - 否则返回最小退让（10ms，让出 CPU）
    pub fn after_work(&self) -> Duration {
        let sample = match self.last_sample.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        };
        let budget = match self.budget.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        };
        let policy = self.power_policy();
        let energy_constrained =
            policy.mode != BatteryPolicy::Off && sample.power.is_energy_constrained();
        if sample.cpu_pct > budget.cpu_pct_max * 0.8 {
            // 超 budget：能效约束态取更长退让（min throttle = 2s）。
            let ms = if energy_constrained {
                budget.throttle_on_exceed_ms.max(BATTERY_THROTTLE_MS)
            } else {
                budget.throttle_on_exceed_ms
            };
            Duration::from_millis(ms)
        } else if energy_constrained {
            // CPU 未超但在电池：仍强制慢速退让（省电，后台不抢功耗）。
            Duration::from_millis(BATTERY_THROTTLE_MS)
        } else {
            Duration::from_millis(10)
        }
    }

    /// LLM 调用类任务（SkillEvolution / MemoryConsolidation）每次调用 LLM 前 check。
    /// 返回 false → 已超过本小时配额，调用方应跳过本次。
    pub fn allow_llm_call(&self) -> bool {
        let budget = match self.budget.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        };
        let Some(limit) = budget.llm_calls_per_hour else {
            return true;
        };
        let now = self.start.elapsed().as_secs();
        let mut calls = match self.llm_calls.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // 清理过期窗口
        while let Some(&t) = calls.front() {
            if now.saturating_sub(t) > LLM_WINDOW_SECS {
                calls.pop_front();
            } else {
                break;
            }
        }
        if (calls.len() as u32) >= limit {
            return false;
        }
        calls.push_back(now);
        true
    }

    pub fn set_profile(&self, p: Profile) {
        if let Ok(mut g) = self.profile.write() {
            *g = p;
        }
        let new_budget = p.budget_for(self.kind);
        if let Ok(mut g) = self.budget.write() {
            *g = new_budget;
        }
    }

    pub fn current_profile(&self) -> Profile {
        match self.profile.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    pub fn current_budget(&self) -> Budget {
        match self.budget.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn last_sample(&self) -> Sample {
        match self.last_sample.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }
}

/// 单任务对外快照 — 用于 [`super::registry::GovernorRegistry::snapshot`] 与
/// `attune --diag` (H5)。
#[derive(Debug, Clone, Serialize)]
pub struct TaskStatus {
    pub id: &'static str,
    pub profile: Profile,
    pub paused: bool,
    pub last_cpu_pct: f32,
    pub last_rss_bytes: u64,
    pub budget_cpu_pct_max: f32,
    pub budget_ram_bytes_max: u64,
}

impl TaskStatus {
    pub fn from_governor(g: &TaskGovernor) -> Self {
        let s = g.last_sample();
        let b = g.current_budget();
        Self {
            id: g.kind.as_str(),
            profile: g.current_profile(),
            paused: g.is_paused(),
            last_cpu_pct: s.cpu_pct,
            last_rss_bytes: s.rss_bytes,
            budget_cpu_pct_max: b.cpu_pct_max,
            budget_ram_bytes_max: b.ram_bytes_max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{PowerProfile, PowerState};
    use crate::resource_governor::monitor::MockMonitor;

    fn governor_with_cpu(kind: TaskKind, profile: Profile, cpu_pct: f32) -> TaskGovernor {
        let m = Arc::new(MockMonitor::new(Sample {
            cpu_pct,
            rss_bytes: 100 * 1024 * 1024,
            power: PowerState::default(),
            captured_secs: 0,
        }));
        TaskGovernor::new(kind, profile, m)
    }

    fn governor_with_power(cpu_pct: f32, power: PowerState) -> TaskGovernor {
        let m = Arc::new(MockMonitor::new(Sample::with_power(
            cpu_pct,
            100 * 1024 * 1024,
            power,
        )));
        TaskGovernor::new(TaskKind::EmbeddingQueue, Profile::Balanced, m)
    }

    fn battery(pct: u8) -> PowerState {
        PowerState {
            source: PowerSource::Battery,
            battery_pct: Some(pct),
            profile: PowerProfile::Balanced,
            thermal_pressure: false,
        }
    }

    #[test]
    fn should_run_when_cpu_below_budget() {
        let g = governor_with_cpu(TaskKind::EmbeddingQueue, Profile::Balanced, 10.0);
        // Balanced EmbeddingQueue cap = 25%, sample = 10% → 应允许
        assert!(g.should_run());
    }

    #[test]
    fn should_not_run_when_cpu_exceeds_budget() {
        let g = governor_with_cpu(TaskKind::EmbeddingQueue, Profile::Balanced, 99.0);
        assert!(!g.should_run());
    }

    #[test]
    fn pause_stops_should_run_immediately() {
        let g = governor_with_cpu(TaskKind::FileScanner, Profile::Aggressive, 1.0);
        assert!(g.should_run());
        g.pause();
        assert!(!g.should_run());
        g.resume();
        assert!(g.should_run());
    }

    #[test]
    fn after_work_returns_throttle_when_near_budget() {
        // 设置 cpu_pct = 24（接近 Balanced EmbeddingQueue 的 25 上限的 96%）
        let g = governor_with_cpu(TaskKind::EmbeddingQueue, Profile::Balanced, 24.0);
        // 触发一次 sample 写入 last_sample
        let _ = g.should_run();
        let d = g.after_work();
        assert_eq!(d, Duration::from_millis(1000)); // Balanced throttle
    }

    #[test]
    fn after_work_minimal_when_far_from_budget() {
        let g = governor_with_cpu(TaskKind::EmbeddingQueue, Profile::Balanced, 5.0);
        let _ = g.should_run();
        assert_eq!(g.after_work(), Duration::from_millis(10));
    }

    #[test]
    fn set_profile_recomputes_budget() {
        let g = governor_with_cpu(TaskKind::EmbeddingQueue, Profile::Balanced, 0.0);
        assert_eq!(g.current_budget().cpu_pct_max, 25.0);
        g.set_profile(Profile::Aggressive);
        assert_eq!(g.current_budget().cpu_pct_max, 60.0);
        assert_eq!(g.current_profile(), Profile::Aggressive);
    }

    #[test]
    fn allow_llm_call_unlimited_when_no_cap() {
        let g = governor_with_cpu(TaskKind::EmbeddingQueue, Profile::Balanced, 0.0);
        // EmbeddingQueue 无 LLM cap
        for _ in 0..1000 {
            assert!(g.allow_llm_call());
        }
    }

    #[test]
    fn allow_llm_call_caps_at_limit() {
        // Conservative SkillEvolution = 5/h
        let g = governor_with_cpu(TaskKind::SkillEvolution, Profile::Conservative, 0.0);
        for _ in 0..5 {
            assert!(g.allow_llm_call(), "first 5 calls should succeed");
        }
        assert!(!g.allow_llm_call(), "6th call should be denied");
    }

    #[test]
    fn task_status_round_trips_state() {
        let g = governor_with_cpu(TaskKind::FileScanner, Profile::Conservative, 7.5);
        let _ = g.should_run(); // 触发 sample 写入
        let s = TaskStatus::from_governor(&g);
        assert_eq!(s.id, "file_scanner");
        assert_eq!(s.profile, Profile::Conservative);
        assert_eq!(s.last_cpu_pct, 7.5);
        assert_eq!(s.budget_cpu_pct_max, 10.0);
    }

    // ── 电源感知策略 ──

    #[test]
    fn battery_throttle_tightens_cpu_cap() {
        // Balanced EmbeddingQueue cap=25%. AC@20% 会跑；电池态收紧到 15% → 20%>15% 拒。
        let ac = governor_with_power(20.0, PowerState::default());
        assert!(ac.should_run(), "AC 20% < 25% budget → run");
        let bat = governor_with_power(20.0, battery(80));
        assert!(
            !bat.should_run(),
            "battery caps to 15% → 20% exceeds → throttle"
        );
        // 电池但 CPU 低于 15% → 仍跑（慢速推进）
        let bat_low_cpu = governor_with_power(10.0, battery(80));
        assert!(bat_low_cpu.should_run(), "battery 10% < 15% cap → run");
    }

    #[test]
    fn low_battery_pauses_background() {
        // 默认 low_battery_pct=20；电量 15% < 20 → 暂停，无关 CPU。
        let g = governor_with_power(1.0, battery(15));
        assert!(!g.should_run(), "15% < 20% floor → pause");
    }

    #[test]
    fn thermal_pressure_pauses_even_on_ac() {
        let hot = PowerState {
            source: PowerSource::Ac,
            battery_pct: None,
            profile: PowerProfile::Performance,
            thermal_pressure: true,
        };
        let g = governor_with_power(1.0, hot);
        assert!(!g.should_run(), "thermal pressure → pause even on AC");
    }

    #[test]
    fn pause_mode_stops_any_battery() {
        let g = governor_with_power(1.0, battery(95)); // 电量充足
        g.set_power_policy(PowerPolicy {
            mode: BatteryPolicy::Pause,
            low_battery_pct: 20,
        });
        assert!(!g.should_run(), "Pause mode: any battery → stop");
    }

    #[test]
    fn off_mode_ignores_power() {
        // Off 模式：电池 + 低电也按 profile budget(25%) 跑。
        let g = governor_with_power(20.0, battery(10));
        g.set_power_policy(PowerPolicy {
            mode: BatteryPolicy::Off,
            low_battery_pct: 20,
        });
        assert!(g.should_run(), "Off: 20% < 25% budget, power ignored");
    }

    #[test]
    fn after_work_battery_forces_longer_yield() {
        // 电池态 + CPU 低 → 仍强制 2s 退让（省电）。
        let bat = governor_with_power(5.0, battery(80));
        let _ = bat.should_run();
        assert_eq!(bat.after_work(), Duration::from_millis(BATTERY_THROTTLE_MS));
        // AC + CPU 低 → 最小 10ms。
        let ac = governor_with_power(5.0, PowerState::default());
        let _ = ac.should_run();
        assert_eq!(ac.after_work(), Duration::from_millis(10));
    }
}
