// H1 multi-thread worker x governor 集成测试. **"integration" 这里指"多线程
// worker 与 governor 的真实交互", 不是"真 SysinfoMonitor 系统采样"** — 后者由
// monitor.rs 单测覆盖. 验证目标:
// (1) 全局 pause 后所有 worker 在 ≤ 1s 内停止处理
// (2) 切档后 budget 立即生效（影响 should_run 决策）
// (3) 多 worker 并发注册不丢失任何一个
//
// monitor 用 MockMonitor (cpu=0) 而非 SysinfoMonitor —— 这些测试验证的是
// pause/profile/registry 语义, 不是 sysinfo 采样行为. 用 SysinfoMonitor
// 在 GHA 高负载 runner 上会因为系统 CPU% > budget 导致 should_run() 永远 false,
// counter 全 0, 测试失败 (历史 fail: rust-test-windows-latest run 25850300599).
// SysinfoMonitor 本身的单元测试见 src/resource_governor/monitor.rs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use attune_core::resource_governor::{
    global_registry, GovernorRegistry, MockMonitor, Profile, Sample, TaskKind,
};

/// 私有 registry, 用 MockMonitor 注入 cpu=0 sample → should_run 只受 pause/budget 影响.
/// Sample::default() 即 cpu_pct=0 (captured_secs 是 crate-private, 不可外部构造).
fn fresh_registry() -> GovernorRegistry {
    let mock = MockMonitor::new(Sample::default());
    GovernorRegistry::with_monitor(Arc::new(mock))
}

/// 启动一个 cooperative worker，模拟生产代码模式：
///   while running { if !governor.should_run() { sleep; continue; } work; sleep(after_work); }
/// 返回 (counter, stop_flag, join_handle)。worker 每完成一次 work 就 counter += 1。
fn spawn_cooperative_worker(
    registry: &GovernorRegistry,
    kind: TaskKind,
) -> (Arc<AtomicUsize>, Arc<std::sync::atomic::AtomicBool>, thread::JoinHandle<()>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let governor = registry.register(kind);
    let counter_c = Arc::clone(&counter);
    let stop_c = Arc::clone(&stop);

    let handle = thread::spawn(move || {
        while !stop_c.load(Ordering::SeqCst) {
            if !governor.should_run() {
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            // 模拟极轻量的 "work" — 不真烧 CPU，避免污染 sysinfo 采样
            counter_c.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(5));
            // governor.after_work() 在没有真 CPU 烧热时返回最小退让 10ms
            thread::sleep(governor.after_work());
        }
    });

    (counter, stop, handle)
}

#[test]
fn pause_all_stops_workers_within_one_second() {
    let registry = fresh_registry();
    let (c1, s1, h1) = spawn_cooperative_worker(&registry, TaskKind::EmbeddingQueue);
    let (c2, s2, h2) = spawn_cooperative_worker(&registry, TaskKind::FileScanner);
    let (c3, s3, h3) = spawn_cooperative_worker(&registry, TaskKind::AiAnnotator);

    // 让 worker 跑 200ms 累积一些 count
    thread::sleep(Duration::from_millis(200));
    let pre_pause: Vec<usize> = [&c1, &c2, &c3].iter().map(|c| c.load(Ordering::SeqCst)).collect();
    assert!(pre_pause.iter().all(|&n| n > 0), "all workers should have made progress: {pre_pause:?}");

    // 全局 pause
    let pause_at = Instant::now();
    registry.pause_all();

    // 等待 1s 后，再观察 200ms — 这 200ms 内不应有显著新增
    thread::sleep(Duration::from_millis(1000));
    let after_pause: Vec<usize> = [&c1, &c2, &c3].iter().map(|c| c.load(Ordering::SeqCst)).collect();
    thread::sleep(Duration::from_millis(200));
    let after_settle: Vec<usize> = [&c1, &c2, &c3].iter().map(|c| c.load(Ordering::SeqCst)).collect();

    // 在 pause 之后的 200ms 观察窗口：每个 worker 增量 ≤ 2（1 round in flight + 1 jitter）
    for i in 0..3 {
        let delta = after_settle[i].saturating_sub(after_pause[i]);
        assert!(
            delta <= 2,
            "worker {i} kept running after pause_all: pre={} post={} settle={}, delta={delta}",
            pre_pause[i], after_pause[i], after_settle[i]
        );
    }
    let _ = pause_at; // 已经验证 pause 生效

    // resume 后能恢复处理 — 用 polling retry 替代 fixed sleep，避免重负载机器 flake。
    // 在 CI / 本地 cargo 满载机器上，单次 200ms 不够 worker 醒来处理一次任务；
    // 改为最多 wait 2s，每 50ms 采样一次，全部 worker 都开始增长后即通过。
    registry.resume_all();
    let resume_deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut after_resume = vec![0usize; 3];
    let counters = [&c1, &c2, &c3];
    loop {
        for i in 0..3 {
            after_resume[i] = counters[i].load(Ordering::SeqCst);
        }
        let all_resumed = (0..3).all(|i| after_resume[i] > after_settle[i]);
        if all_resumed {
            break;
        }
        if std::time::Instant::now() >= resume_deadline {
            break; // 让 assert 给出失败信息
        }
        thread::sleep(Duration::from_millis(50));
    }
    for i in 0..3 {
        assert!(
            after_resume[i] > after_settle[i],
            "worker {i} did not resume within 2s: settle={} resume={}",
            after_settle[i], after_resume[i]
        );
    }

    s1.store(true, Ordering::SeqCst);
    s2.store(true, Ordering::SeqCst);
    s3.store(true, Ordering::SeqCst);
    h1.join().unwrap();
    h2.join().unwrap();
    h3.join().unwrap();
}

#[test]
fn profile_change_reflects_in_budget_immediately() {
    let registry = fresh_registry();
    let g = registry.register(TaskKind::EmbeddingQueue);
    assert_eq!(g.current_profile(), Profile::Balanced);
    assert_eq!(g.current_budget().cpu_pct_max, 25.0);

    registry.set_profile(Profile::Aggressive);
    assert_eq!(g.current_profile(), Profile::Aggressive);
    assert_eq!(g.current_budget().cpu_pct_max, 60.0);

    registry.set_profile(Profile::Conservative);
    assert_eq!(g.current_budget().cpu_pct_max, 15.0);
}

#[test]
fn multiple_governors_register_independently() {
    let registry = fresh_registry();
    let kinds = [
        TaskKind::EmbeddingQueue,
        TaskKind::SkillEvolution,
        TaskKind::FileScanner,
        TaskKind::WebDavSync,
        TaskKind::PatentScanner,
        TaskKind::BrowserSearch,
        TaskKind::AiAnnotator,
        TaskKind::BrowseSignalIngest,
        TaskKind::AutoBookmark,
        TaskKind::MemoryConsolidation,
    ];
    for k in kinds {
        let _g = registry.register(k);
    }
    let snap = registry.snapshot();
    assert_eq!(snap.len(), kinds.len());
}

#[test]
fn global_registry_singleton_smoke() {
    // 仅验证全局 registry 可被多次调用且每次返回同一实例
    let r1 = global_registry();
    let r2 = global_registry();
    assert!(std::ptr::eq(r1, r2));
}

/// 集成验证：registry → Conservative profile 广播 → governor 在 CPU 超 budget 时
/// throttle（`should_run()` 变 false、`after_work()` 返回退让时长）。
///
/// 历史：旧版本真烧 N 个 CPU burner 线程，但 `fresh_registry()` 注入的是固定
/// `MockMonitor(cpu_pct=0)`——governor 永远看不到真负载，`should_run()` 恒 true，
/// `throttled` 恒 0，断言 `throttled > 0` 永不可能成立（任何 runner 上都失败）。
/// 该 file header 也明确禁止用 `SysinfoMonitor`（GHA 高负载 runner 抖动）。
/// 现改为通过 `MockMonitor::set()` 直接注入高 CPU sample —— 确定性、零真负载、
/// 跨平台稳定。budget 阈值的纯单测见 `governor.rs` 的 `governor_with_cpu` 系列。
#[test]
fn cpu_over_budget_triggers_throttle() {
    // 保留具体 MockMonitor 句柄以便后续 set()；registry 内部按 trait object 持有。
    let mock = Arc::new(MockMonitor::new(Sample::default()));
    let registry = GovernorRegistry::with_monitor(mock.clone());
    // Conservative 档：EmbeddingQueue cpu cap = 15%，throttle_on_exceed_ms = 2000。
    registry.set_profile(Profile::Conservative);
    let governor = registry.register(TaskKind::EmbeddingQueue);

    // 低于 budget（10% < 15% cap）→ 允许运行，after_work 仅最小退让（10ms）。
    mock.set(Sample::new(10.0, 0));
    assert!(governor.should_run(), "10% CPU 低于 15% cap，should_run 应为 true");
    assert_eq!(
        governor.after_work(),
        Duration::from_millis(10),
        "未近 budget 时 after_work 应返回最小退让 10ms"
    );

    // 超过 budget（95% > 15% cap）→ should_run 变 false。
    mock.set(Sample::new(95.0, 0));
    assert!(
        !governor.should_run(),
        "95% CPU 超 Conservative EmbeddingQueue 15% cap，should_run 必须 throttle 为 false"
    );
    // should_run() 已刷新 last_sample，after_work 现应返回完整退让（2000ms）。
    assert_eq!(
        governor.after_work(),
        Duration::from_millis(2000),
        "CPU 超 budget 时 after_work 应返回 Conservative throttle_on_exceed_ms"
    );

    // 恢复低负载 → governor 立即放行（无 sticky throttle）。
    mock.set(Sample::new(5.0, 0));
    assert!(governor.should_run(), "CPU 回落后 governor 应立即恢复放行");
}
