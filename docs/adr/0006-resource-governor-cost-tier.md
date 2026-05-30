# ADR 0006: Resource Governor + 三层成本治理契约

- **Status**: Accepted
- **Date**: 2026-04-27 (governor); 2026-05-28 (cost/token budget API 延伸)

## Context

attune 的后台任务（embedding 队列、SkillEvolver、文件扫描、WebDAV 同步、
浏览器自动化搜索、AI 批注、memory consolidation）原先直接 `std::thread::spawn`，
无 CPU/RAM/IO 上限，导致批量 embedding 卡顿、电池供电持续耗电、演示/游戏时无暂停渠道。
同时产品定位要求每次计算「谁在买单」必须对用户透明（零成本 CPU / 本地算力 GPU·NPU /
时间金钱 LLM 三层），不能后台偷跑第三层。

需要一个统一的治理层既约束系统资源，又承载成本分层契约。

## Decision

attune-core 引入 `resource_governor`（落地 `governor.rs`），核心决策：

1. **治理粒度 = 任务级**：每个后台任务一个 governor，避免全局级的优先级倒置
   （embedding 卡死时 SkillEvolver 饿死）。关键路径开绿灯、批处理红灯。
2. **兼容现有线程模型**：governor 用 trait 适配 `std::thread::spawn` 的
   `Arc<AtomicBool>` 模式，不强制迁移 Tokio（省 2-4 周重构）。
3. **监控用全局 CPU%（sysinfo），非单进程**：`sys.cpus()` 跨平台稳定，
   且语义更好 —「系统忙就让让」优于「我用了多少」；多 governor 共享一个全局指标，
   自动避免 budget 累加 > 100%。
4. **三档预设** Conservative / Balanced / Aggressive，Balanced 默认，
   电池供电自动降 Conservative；中央 registry 支持顶栏一键全局 Pause。
5. **三层成本契约**（与 governor 同源治理）：
   - 🆓 零成本（CPU，文件解析 / 分词 / BM25 / OCR）— 随便跑
   - ⚡ 本地算力（GPU·NPU，embedding / classify / 存档摘要）— 建库阶段自动跑，可暂停
   - 💰 时间金钱（LLM，Chat / 批注 / 深度分析）— **必须用户显式触发，永不后台偷跑**
   建库阶段永不升级到第三层；分析阶段永远等用户开口。UI 必须显示成本。
6. **Token/cache budget API（v1.1+ 延伸）**：cache/context/token 标准化为
   governor 之上的 budget 维度，task 级 token 预算路由 — 暂列 DRAFT，待 v1.1 实施。
7. **Telemetry 仅本地，无上报**，与 No-telemetry 隐私承诺一致。

## Consequences

**好处**：系统友好成为可量化产品承诺（差异化卖点 vs Obsidian/Logseq 索引拖系统）；
成本对用户透明，无意外账单；新旧线程模型并存，迁移成本可控。

**代价**：全局 CPU 指标在极端多进程负载下可能误判系统忙；任务级 governor 增加注册/
采样开销（µs 级，可接受）；token budget API 仍为 draft，v1.1 才闭环。

## Implementation 落地

- `rust/crates/attune-core/src/governor.rs` + `resource_governor/`（registry / monitor /
  profiles / budget），含 mock monitor 单测（无需真实 CPU）。
- 成本三层契约固化于项目 CLAUDE.md「成本感知与触发契约」节（产品最高优先级之一）。
- 本 ADR 取代并归档以下设计 spec（决策已落地，过程文档删除）：
  - `2026-04-27-resource-governor-design.md`
  - `2026-05-28-cache-context-token-standard-api.md`（DRAFT，token budget 延伸）
  - `2026-05-28-hybrid-token-strategy.md`（DRAFT，token agent 路由延伸）
