// 全局 GovernorRegistry —— 中央登记每个 TaskKind 一个 governor，
// 暴露全局 pause / 全局切档 / 状态快照（H3 顶栏 Pause 按钮 / H5 attune --diag 的入口）。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use super::governor::{PowerPolicy, TaskGovernor, TaskStatus};
use super::monitor::{ResourceMonitor, SysinfoMonitor};
use super::profiles::{Profile, TaskKind};

/// 中央 governor 仓库。每个 TaskKind 在进程内只有一份 [`TaskGovernor`]。
///
/// 设计要点：
/// - `governors` 用 RwLock<HashMap>，读多写少（注册仅在 worker 启动时）
/// - 共享同一个 monitor，避免每个 governor 都开 sysinfo（开销 N 倍）
/// - 全局 profile 改变时同步广播到所有已注册 governor
pub struct GovernorRegistry {
    governors: RwLock<HashMap<TaskKind, Arc<TaskGovernor>>>,
    monitor: Arc<dyn ResourceMonitor>,
    profile: RwLock<Profile>,
    power_policy: RwLock<PowerPolicy>,
}

impl GovernorRegistry {
    /// 用真 sysinfo monitor 构造。
    pub fn new() -> Self {
        Self::with_monitor(Arc::new(SysinfoMonitor::new()))
    }

    /// 用自定义 monitor（测试时传 MockMonitor）。
    pub fn with_monitor(monitor: Arc<dyn ResourceMonitor>) -> Self {
        Self {
            governors: RwLock::new(HashMap::new()),
            monitor,
            profile: RwLock::new(Profile::default()),
            power_policy: RwLock::new(PowerPolicy::default()),
        }
    }

    /// 注册或获取该任务的 governor。同一 TaskKind 多次调用返回同一实例。
    pub fn register(&self, kind: TaskKind) -> Arc<TaskGovernor> {
        // 先 read 看是否已存在 — 避免高频 worker 重启时争 write 锁
        if let Ok(map) = self.governors.read() {
            if let Some(g) = map.get(&kind) {
                return Arc::clone(g);
            }
        }
        let mut map = match self.governors.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // double-check（防止 read 释放后另一个线程已插入）
        if let Some(g) = map.get(&kind) {
            return Arc::clone(g);
        }
        let profile = match self.profile.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        };
        let g = Arc::new(TaskGovernor::new(kind, profile, Arc::clone(&self.monitor)));
        // 应用当前全局电源策略（之后 set_power_policy 也会广播到此 governor）。
        let pp = match self.power_policy.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        };
        g.set_power_policy(pp);
        map.insert(kind, Arc::clone(&g));
        g
    }

    /// 克隆当前所有 governor 的 Arc — 用于在释放 map 锁后安全调用 governor 方法，
    /// 防止未来 governor 内部反向引用 registry 时死锁（防御性设计）。
    fn snapshot_governors(&self) -> Vec<Arc<TaskGovernor>> {
        let map = match self.governors.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        map.values().map(Arc::clone).collect()
    }

    /// H3：顶栏一键暂停所有后台任务。
    pub fn pause_all(&self) {
        for g in self.snapshot_governors() {
            g.pause();
        }
    }

    /// 顶栏恢复按钮。
    pub fn resume_all(&self) {
        for g in self.snapshot_governors() {
            g.resume();
        }
    }

    /// 全局切档 —— 同步广播到所有已注册 governor。
    /// 之后 register 的 governor 也会取到新档位。
    pub fn set_profile(&self, p: Profile) {
        if let Ok(mut g) = self.profile.write() {
            *g = p;
        }
        for g in self.snapshot_governors() {
            g.set_profile(p);
        }
    }

    pub fn current_profile(&self) -> Profile {
        match self.profile.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    /// 全局电源策略 —— settings 路由更新时调用，广播到所有已注册 governor。
    /// 之后 register 的 governor 也会取到新策略。
    pub fn set_power_policy(&self, pp: PowerPolicy) {
        if let Ok(mut g) = self.power_policy.write() {
            *g = pp;
        }
        for g in self.snapshot_governors() {
            g.set_power_policy(pp);
        }
    }

    pub fn current_power_policy(&self) -> PowerPolicy {
        match self.power_policy.read() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    /// 所有任务的状态快照 — 喂给 H5 `attune --diag` 与未来 H6 telemetry chart。
    /// 顺序按 TaskKind 字符串 id 字典序，便于 diff。
    pub fn snapshot(&self) -> Vec<TaskStatus> {
        let mut out: Vec<TaskStatus> = self
            .snapshot_governors()
            .iter()
            .map(|g| TaskStatus::from_governor(g))
            .collect();
        out.sort_by_key(|s| s.id);
        out
    }
}

impl Default for GovernorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 进程内单例。第一次调用时初始化为真 sysinfo 后端。
///
/// 测试需要注入 mock 时不要走单例 — 直接 `GovernorRegistry::with_monitor()`。
pub fn global_registry() -> &'static GovernorRegistry {
    static REG: OnceLock<GovernorRegistry> = OnceLock::new();
    REG.get_or_init(GovernorRegistry::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource_governor::monitor::{MockMonitor, Sample};

    fn registry_with_mock() -> GovernorRegistry {
        GovernorRegistry::with_monitor(Arc::new(MockMonitor::new(Sample {
            cpu_pct: 5.0,
            rss_bytes: 100 * 1024 * 1024,
            power: crate::platform::PowerState::default(),
            captured_secs: 0,
        })))
    }

    #[test]
    fn register_returns_same_instance_for_same_kind() {
        let r = registry_with_mock();
        let a = r.register(TaskKind::EmbeddingQueue);
        let b = r.register(TaskKind::EmbeddingQueue);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn register_different_kinds_returns_different_governors() {
        let r = registry_with_mock();
        let a = r.register(TaskKind::EmbeddingQueue);
        let b = r.register(TaskKind::SkillEvolution);
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn pause_all_pauses_every_governor() {
        let r = registry_with_mock();
        let a = r.register(TaskKind::EmbeddingQueue);
        let b = r.register(TaskKind::FileScanner);
        let c = r.register(TaskKind::AiAnnotator);
        assert!(a.should_run() && b.should_run() && c.should_run());
        r.pause_all();
        assert!(!a.should_run());
        assert!(!b.should_run());
        assert!(!c.should_run());
        r.resume_all();
        assert!(a.should_run() && b.should_run() && c.should_run());
    }

    #[test]
    fn set_profile_broadcasts_to_existing_governors() {
        let r = registry_with_mock();
        let a = r.register(TaskKind::EmbeddingQueue);
        assert_eq!(a.current_profile(), Profile::Balanced);
        r.set_profile(Profile::Aggressive);
        assert_eq!(a.current_profile(), Profile::Aggressive);
        // budget 也同步刷新
        assert_eq!(a.current_budget().cpu_pct_max, 60.0);
    }

    #[test]
    fn set_profile_applies_to_future_governors() {
        let r = registry_with_mock();
        r.set_profile(Profile::Conservative);
        let g = r.register(TaskKind::EmbeddingQueue);
        assert_eq!(g.current_profile(), Profile::Conservative);
        assert_eq!(g.current_budget().cpu_pct_max, 15.0);
    }

    #[test]
    fn snapshot_returns_all_registered_in_stable_order() {
        let r = registry_with_mock();
        r.register(TaskKind::FileScanner);
        r.register(TaskKind::EmbeddingQueue);
        r.register(TaskKind::AiAnnotator);
        let snap = r.snapshot();
        assert_eq!(snap.len(), 3);
        // 字典序：ai_annotator < embedding_queue < file_scanner
        assert_eq!(snap[0].id, "ai_annotator");
        assert_eq!(snap[1].id, "embedding_queue");
        assert_eq!(snap[2].id, "file_scanner");
    }

    #[test]
    fn concurrent_register_returns_same_arc() {
        use std::thread;
        let r = Arc::new(registry_with_mock());
        let mut handles = vec![];
        for _ in 0..10 {
            let r = Arc::clone(&r);
            handles.push(thread::spawn(move || {
                r.register(TaskKind::EmbeddingQueue)
            }));
        }
        let firsts: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let baseline = &firsts[0];
        for g in &firsts[1..] {
            assert!(Arc::ptr_eq(baseline, g), "all concurrent registers must return the same Arc");
        }
    }
}
