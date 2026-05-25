# 资源治理框架（Resource Governor / H1）设计稿

**日期**：2026-04-27
**对应路线图**：12-week 战略 v2，Phase 1 W1 F-P0a
**依赖谁**：所有 Phase 1 P0 后台任务（A1 / G1 / G2 等）
**被谁依赖**：H2 三档档位、H3 顶栏 Pause、H4 自动降档、H5 `attune --diag`、H6 telemetry 图表

---

## 1. 为什么做（Why）

attune 当前所有后台任务（embedding 队列、SkillEvolver、文件扫描、WebDAV 同步、专利扫描、浏览器自动化搜索、AI 批注）直接 `std::thread::spawn` 后无 CPU/RAM/IO 上限，导致：

- 100 文件批量 embedding 时桌面应用感知卡顿
- 笔记本电池供电时后台任务持续耗电
- 用户演示 / 全屏游戏 / 直播时无暂停渠道
- README 自称 production-ready 但缺硬证据

竞品反例：Obsidian / Logseq 因索引重建拖系统被吐槽，attune 把"系统友好"做成产品承诺 + 显式可控阈值 = 差异化卖点。

## 2. 核心设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 治理粒度 | **任务级**（每个后台任务一个 governor） | 全局级会出现"embedding 卡死时 SkillEvolver 也饿死"的优先级倒置；任务级允许关键路径开绿灯、批处理红灯 |
| 兼容现有线程模型 | **不强制迁移 Tokio**，governor 兼容 `std::thread::spawn` 的 `Arc<AtomicBool>` 模式 | 全部迁移 Tokio 是 2-4 周大重构，远超 W1 工作量；governor 用 trait 适配，新旧并存 |
| 监控数据来源 | `sysinfo` crate（跨平台），**全局 CPU%** 不是单进程 | sysinfo 0.32 `Process::cpu_usage()` 对自身进程在多平台 quirky（部分平台返回 0）；`sys.cpus()` 全局占用稳定。**语义也更好**：「系统忙就让让」比「我用了多少」更符合好公民意图。多 governor 共享一个全局指标，自动避免每任务 budget 累加 > 100%。 |
| 三档预设 | Conservative / Balanced / Aggressive | 与 1Password / Logseq 用户认知一致；Balanced 默认；电池供电自动 Conservative（H4） |
| 全局 Pause | 中央 registry + 每个 governor 检查 | 顶栏一键暂停所有任务，原子性 |
| Telemetry | 仅本地，无上报 | 与 D1 No-telemetry 强一致 |

## 3. 模块结构

```
rust/crates/attune-core/src/resource_governor/
├── mod.rs           # 公开 API：TaskGovernor, GovernorRegistry, Profile
├── budget.rs        # CpuBudget / RamBudget / IoBudget 数据结构
├── profiles.rs      # Conservative / Balanced / Aggressive 三档预设
├── governor.rs      # TaskGovernor 核心：should_pause / throttle / sample
├── monitor.rs       # CPU/RAM/IO 采样（封装 sysinfo）
├── registry.rs      # 全局 GovernorRegistry：注册 / 列出 / 全局 pause
└── tests.rs         # 单元测试（含 mock monitor，无需真实 CPU）
```

## 4. 核心 trait & 数据结构

```rust
// budget.rs
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// **系统全局 CPU% 阈值**（0.0-100.0）— 当全局 CPU 高于此值时本任务暂缓。
    /// 协作式语义，不是单进程上限；多任务共享同一全局指标。
    pub cpu_pct_max: f32,
    /// 此任务允许占用的最大常驻内存（bytes）— 记录用，不强制
    pub ram_bytes_max: u64,
    /// IO 优先级（仅 Linux 生效，其他平台 best-effort）
    pub io_priority: IoPriority,
    /// 当全局 CPU 超 cpu_pct_max 时，throttle 多少毫秒再继续
    pub throttle_on_exceed_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum IoPriority { Idle, BestEffort, Realtime }

// profiles.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Profile { Conservative, Balanced, Aggressive }

impl Profile {
    /// 给定任务类型 + 当前档位，返回 Budget
    pub fn budget_for(self, task: TaskKind) -> Budget { /* 见 §6 */ }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum TaskKind {
    EmbeddingQueue,
    SkillEvolution,
    FileScanner,
    WebDavSync,
    PatentScanner,
    BrowserSearch,
    AiAnnotator,
    BrowseSignalIngest,  // G1
    AutoBookmark,        // G2
    MemoryConsolidation, // A1
}

// governor.rs
pub struct TaskGovernor {
    pub id: TaskKind,
    profile: Arc<RwLock<Profile>>,
    budget: Arc<RwLock<Budget>>,
    paused: Arc<AtomicBool>,
    monitor: Arc<dyn ResourceMonitor>,
    last_sample: Arc<Mutex<Sample>>,
}

impl TaskGovernor {
    /// Worker loop 每次迭代头部调用。返回 false → worker 应跳过本次工作并 sleep。
    pub fn should_run(&self) -> bool { /* paused? + budget exceeded? */ }

    /// 批处理后调用，更新采样、决定是否需要 throttle 延迟。
    pub fn after_work(&self) -> Duration { /* 返回需要 sleep 的时间 */ }

    /// 用户切档时调用
    pub fn set_profile(&self, p: Profile) { /* 重算 budget */ }

    /// 顶栏 Pause 按钮触发
    pub fn pause(&self) { self.paused.store(true, SeqCst); }
    pub fn resume(&self) { self.paused.store(false, SeqCst); }
}

// monitor.rs
pub trait ResourceMonitor: Send + Sync {
    fn sample_self(&self) -> Sample;  // 当前进程 CPU% / RSS / IO
}

pub struct SysinfoMonitor { /* sysinfo::System refresh */ }
pub struct MockMonitor { sample: Mutex<Sample> }  // 测试用

// registry.rs
pub struct GovernorRegistry {
    governors: RwLock<HashMap<TaskKind, Arc<TaskGovernor>>>,
    profile: RwLock<Profile>,
}

impl GovernorRegistry {
    pub fn register(&self, kind: TaskKind) -> Arc<TaskGovernor>;
    pub fn pause_all(&self);
    pub fn resume_all(&self);
    pub fn set_profile(&self, p: Profile);  // 全局切档，影响所有 governor
    pub fn snapshot(&self) -> Vec<TaskStatus>;  // for attune --diag (H5)
}

/// 全局单例 registry（lazy_static / once_cell）
pub fn global_registry() -> &'static GovernorRegistry;
```

## 5. Worker 接入模式（retrofit reference）

现有 `queue.rs` 改造前后对比：

```rust
// BEFORE
std::thread::spawn(move || {
    while running.load(Ordering::SeqCst) {
        match Self::process_batch(...) {
            Ok(0) => std::thread::sleep(POLL_INTERVAL),
            Ok(_) => {/* 立即下一轮 */}
            Err(_) => std::thread::sleep(POLL_INTERVAL),
        }
    }
});

// AFTER
let governor = global_registry().register(TaskKind::EmbeddingQueue);
std::thread::spawn(move || {
    while running.load(Ordering::SeqCst) {
        if !governor.should_run() {
            // 全局 paused 或超 budget，sleep 短间隔后重试
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        match Self::process_batch(...) {
            Ok(0) => std::thread::sleep(POLL_INTERVAL),
            Ok(_) => std::thread::sleep(governor.after_work()),  // throttle 决定下次 sleep
            Err(_) => std::thread::sleep(POLL_INTERVAL),
        }
    }
});
```

每个被 retrofit 的 worker 改动量：~10 行（注册 governor + 循环头检查 + 循环尾 sleep 替换），不破坏现有逻辑。

## 6. 三档预设值（首版）

> **语义**：`cpu_pct_max` 是**系统全局 CPU% 阈值**。表中"EmbeddingQueue Balanced 25%"
> 意为"全局 CPU 占用超过 25% 时此 worker 暂缓"，**不是**"此 worker 单独占用上限 25%"。
> 所有 worker 共享同一全局指标。

| 任务 | Conservative | Balanced (默认) | Aggressive |
|------|-------------|----------------|-----------|
| EmbeddingQueue | CPU 15% / 512MB / Idle IO / throttle 2000ms | CPU 25% / 1GB / BestEffort / 1000ms | CPU 60% / 2GB / BestEffort / 100ms |
| SkillEvolution | CPU 10% / 256MB / Idle / 5000ms（限 LLM 调用 5/h） | CPU 20% / 512MB / Idle / 2000ms（10/h） | CPU 40% / 1GB / BestEffort / 500ms（30/h） |
| FileScanner | CPU 10% / 256MB / Idle / 1000ms | CPU 20% / 512MB / Idle / 500ms | CPU 50% / 1GB / BestEffort / 100ms |
| WebDavSync | CPU 10% / 128MB / Idle / 5000ms | CPU 15% / 256MB / Idle / 2000ms | CPU 30% / 512MB / BestEffort / 500ms |
| BrowserSearch | CPU 30% / 1GB / BestEffort / 1000ms | CPU 50% / 1.5GB / BestEffort / 500ms | CPU 80% / 2GB / BestEffort / 100ms |
| AiAnnotator | CPU 10% / 256MB / Idle / 3000ms | CPU 20% / 512MB / Idle / 1000ms | CPU 50% / 1GB / BestEffort / 200ms |
| BrowseSignalIngest (G1) | CPU 5% / 64MB / Idle / 5000ms | CPU 10% / 128MB / Idle / 2000ms | CPU 20% / 256MB / Idle / 500ms |
| AutoBookmark (G2) | CPU 10% / 256MB / Idle / 5000ms | CPU 20% / 512MB / Idle / 2000ms | CPU 40% / 1GB / BestEffort / 500ms |
| MemoryConsolidation (A1) | CPU 15% / 512MB / Idle / 10000ms（限 LLM 5/h） | CPU 25% / 1GB / Idle / 5000ms（10/h） | CPU 50% / 2GB / BestEffort / 1000ms（30/h） |

数值来自 RK3588 / Radeon 780M / 普通笔记本三档实测的合理推断，正式发版前要在 H1 测试 + 实际负载 baseline 后微调，写入 `docs/system-impact.md`。

## 7. 跨平台兼容

| 平台 | CPU 采样 | RAM 采样 | IO Priority |
|------|---------|---------|------------|
| Linux | sysinfo `/proc/self/stat` | `/proc/self/status` VmRSS | `ioprio_set` (libc) |
| Windows | sysinfo PDH | `GetProcessMemoryInfo` | best-effort（无 ioprio，仅记录） |
| macOS | sysinfo task_info | task_info | best-effort |

**降级策略**：IoPriority::Idle 在不支持平台仅记录，不报错。

## 8. 测试策略

per CLAUDE.md 测试规范：

**Unit（attune-core/src/resource_governor/tests.rs）**：
- `MockMonitor` 注入特定 sample，验证 `should_run()` / `after_work()` 决策正确
- profile 切换后 budget 立即生效
- pause/resume 状态转换
- 三档预设值快照（防止意外修改）
- 全部使用确定输入 + 精确断言，无 random

**Integration（rust/tests/governor_integration.rs）**：
- 真实 sysinfo monitor + 真实 std::thread worker
- Spin 一个 CPU 烧热任务在 governor 下 → 实测 CPU% ≤ budget 上限
- 并发 5 个 worker 同时 pause → 全部停在 ≤ 100ms 内
- 用 tempfile + 真实 Store，无 mock 数据库

**Corpus Integration（W4 测试 enhanced）**：
- 100 文件（rust-lang/book pinned tag）批量 embedding 启用 governor → 24h 跑完不超 25% CPU 平均

## 9. 文档输出

- `docs/system-impact.md` — 用户面：三档说明 + 实测占用承诺
- `rust/DEVELOP.md` 章节 — 开发者：如何 retrofit 一个新 worker（5 步）
- `RELEASE.md` — Phase 1 W1 changelog

## 10. 不做的事（避免 scope creep）

- ❌ 不做 cgroups / namespace 隔离（Linux only，复杂度高，后续可加）
- ❌ 不做 GPU / NPU 占用治理（推理走 Ollama，由 Ollama 自己控制）
- ❌ 不做跨进程资源协调（attune 一个进程内即可，attune-mcp-server 各自管各自）
- ❌ 不做动态学习预设（读用户实际负载自动调）— H6 之后再考虑

## 10.1. W1 范围（已 retrofit 的 worker）

W1 retrofit 了 `attune-server/src/state.rs` 中**生产实际运行的 worker**：
- `start_classify_worker` → `TaskKind::AiAnnotator`（LLM 分类路径）
- `start_rescan_worker` → `TaskKind::FileScanner`（30 分钟目录重扫）
- `start_queue_worker` → `TaskKind::EmbeddingQueue`（生产 embedding 路径）
- `start_skill_evolver` → `TaskKind::SkillEvolution`（4 小时 LLM 扩词周期，含 `allow_llm_call` 配额检查）

外加 `attune-core::queue::QueueWorker`（test-only / 库路径，retrofit 是为完整性）。

**W1 不做**（推迟到 W2/W3 各 feature 周）：
- `web_search_browser.rs` — 由 `web_search.rs` on-demand 调用，不是常驻 worker。等网络搜索变成长跑 poller 时再 retrofit
- `scanner_webdav.rs` / `scanner_patent.rs` — 从 rescan_worker 内部调用，治理通过调用方 worker 继承
- `ai_annotator.rs` — 从 `routes/upload.rs` 异步任务调用（请求级短任务）；H1 只治理常驻后台 worker
- `workflow/` — 由 routes 编排，按执行实例生命周期管理，不是常驻 worker

## 11. 验收清单（W1 末）

- [ ] `cargo test -p attune-core resource_governor::` 全绿
- [ ] `cargo test --test governor_integration` 全绿
- [ ] queue.rs retrofit 后 `cargo test -p attune-core queue::` 全绿（保持原有行为不破坏）
- [ ] `docs/system-impact.md` + `.zh.md` 完成
- [ ] `tests/MANUAL_TEST_CHECKLIST.md` 加入"H1 治理验证"章节
- [ ] git commit + push develop，SHA 报告
