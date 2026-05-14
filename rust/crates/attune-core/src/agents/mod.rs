//! 内置 agents — 跨 skill 编排器, 输出业务可消费的结构化结果.
//!
//! 设计:
//! - 每个 agent 实现 Agent trait, 接受输入 → 调用多个 skill → 输出 AgentOutput
//! - 业务红线在 agent 层 enforce (不在 skill 层)
//! - audit_trail 含完整推理链 (调用方可审计)

pub mod document_classifier;

use crate::error::Result;
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

impl<T> AgentOutput<T> {
    /// 检查是否有业务红线被违反
    pub fn has_red_lines(&self) -> bool {
        !self.red_lines_violated.is_empty()
    }
    /// 检查是否需要后续追问 (软或硬)
    pub fn needs_attention(&self) -> bool {
        self.has_red_lines() || !self.missing_evidence.is_empty()
    }
}

/// Agent 统一接口. 内置 + 外部 plugin agent 都实现此 trait (内部直调) 或通过 capability_dispatch
/// subprocess 走 binary 模式 (跨进程 / 跨 plugin).
///
/// `Input` 业务自定义 (分类 agent = 文档列表; 借贷 agent = 证据集); `Output` 业务自定义.
pub trait Agent {
    type Input;
    type Output;

    /// agent 唯一 id (与 plugin.yaml agents[].id 对应)
    fn id(&self) -> &str;

    /// 简短描述
    fn description(&self) -> &str;

    /// 此 agent 能处理的案件类型 (空 = 任意)
    fn case_kinds(&self) -> &[&str];

    /// 主入口: 接受输入 → 输出 AgentOutput
    fn run(&self, input: Self::Input) -> Result<AgentOutput<Self::Output>>;
}
