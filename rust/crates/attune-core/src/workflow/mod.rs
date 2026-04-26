//! Workflow 引擎（spec §3.3）— 跨证据链推理等多步任务编排。
//!
//! Phase C 范围：
//! - schema 解析（schema.rs）
//! - WorkflowRunner 执行引擎（runner.rs，Task 2）
//! - deterministic ops（ops.rs，Task 3）
//! - 内置 workflow（builtins.rs，Task 4）

pub mod schema;
pub mod runner;
pub mod ops;
// pub mod builtins;   // Task 4

pub use schema::{
    parse_workflow_yaml, DeterministicStep, SkillStep, Workflow, WorkflowStep, WorkflowTrigger,
};
pub use runner::{run_workflow, WorkflowError, WorkflowEvent, WorkflowResult};
