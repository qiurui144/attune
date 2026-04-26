//! Workflow 引擎（spec §3.3）— 多步任务编排通用底座。
//!
//! 范围：
//! - schema 解析（schema.rs）
//! - WorkflowRunner 执行引擎（runner.rs）
//! - deterministic ops（ops.rs）
//!
//! 行业相关 workflow（如 law-pro 的 evidence_chain_inference）由 attune-pro 在
//! Sprint 2 plugin loader 中通过 plugin.yaml 注册，attune-core 不内置。

pub mod schema;
pub mod runner;
pub mod ops;

pub use schema::{
    parse_workflow_yaml, DeterministicStep, SkillStep, Workflow, WorkflowStep, WorkflowTrigger,
};
pub use runner::{run_workflow, WorkflowError, WorkflowEvent, WorkflowResult};
