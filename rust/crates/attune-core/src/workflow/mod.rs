//! Workflow 引擎（spec §3.3）— 多步任务编排通用底座。
//!
//! 范围：
//! - schema 解析（schema.rs）
//! - WorkflowRunner 执行引擎（runner.rs）
//! - deterministic ops（ops.rs）
//!
//! 行业相关 workflow（如各 vertical 的跨实体推理 / 合同审查 / BANT 评估等）由对应
//! vertical 插件在 plugin.yaml 中注册并通过 plugin loader 加载，attune-core 不内置任何
//! 行业 workflow — 本文件仅提供通用引擎。

pub mod schema;
pub mod runner;
pub mod ops;

pub use schema::{
    parse_workflow_yaml, DeterministicStep, SkillStep, Workflow, WorkflowStep, WorkflowTrigger,
};
pub use runner::{run_workflow, WorkflowError, WorkflowEvent, WorkflowResult};
