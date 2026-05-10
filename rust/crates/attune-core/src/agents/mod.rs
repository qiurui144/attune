//! 内置 agents — 跨 skill 编排器, 输出业务可消费的结构化结果.
//!
//! 设计:
//! - 每个 agent 实现 Agent trait, 接受输入 → 调用多个 skill → 输出 AgentOutput
//! - 业务红线在 agent 层 enforce (不在 skill 层)
//! - audit_trail 含完整推理链 (调用方可审计)

pub mod document_classifier;

use serde::{Deserialize, Serialize};

/// 统一 agent 输出 schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutput<T> {
    /// agent 业务自定义输出 (借贷 = 金额; 婚姻 = 分割比例; 分类 = 证据列表)
    pub computation: T,
    /// 可审计推理链
    pub audit_trail: String,
    /// 硬阻塞: 任一不满足业务红线 → reject
    pub red_lines_violated: Vec<String>,
    /// 软追问: 缺失证据 (不阻塞)
    pub missing_evidence: Vec<String>,
    /// 后续行动建议 (调用方提示用户)
    pub followups: Vec<String>,
    /// 整体置信度 0.0 - 1.0
    pub confidence: f64,
}
