// 资源治理框架 (H1) — 任务级 CPU/RAM/IO 上限 + 全局 Pause + 三档预设。
//
// 设计：docs/superpowers/specs/2026-04-27-resource-governor-design.md
// 用法：
//   ```
//   let g = global_registry().register(TaskKind::EmbeddingQueue);
//   loop {
//       if !g.should_run() { sleep(500ms); continue; }
//       do_batch();
//       sleep(g.after_work());
//   }
//   ```

pub mod budget;
pub mod governor;
pub mod monitor;
pub mod profiles;
pub mod registry;

pub use budget::{Budget, IoPriority};
pub use governor::{BatteryPolicy, PowerPolicy, TaskGovernor, TaskStatus};
pub use monitor::{MockMonitor, ResourceMonitor, Sample, SysinfoMonitor};
pub use profiles::{Profile, TaskKind};
pub use registry::{global_registry, GovernorRegistry};
