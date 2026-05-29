//! 内置 agents — 跨 skill 编排器, 输出业务可消费的结构化结果.
//!
//! 设计:
//! - 每个 agent 实现 Agent trait, 接受输入 → 调用多个 skill → 输出 AgentOutput
//! - 业务红线在 agent 层 enforce (不在 skill 层)
//! - audit_trail 含完整推理链 (调用方可审计)

pub mod document_classifier;
pub mod flow;
pub mod flow_runner;
pub mod registry;
pub mod scheduler;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_output<T>(comp: T, red: Vec<String>, missing: Vec<String>) -> AgentOutput<T> {
        AgentOutput {
            computation: comp,
            audit_trail: String::new(),
            red_lines_violated: red,
            missing_evidence: missing,
            followups: vec![],
            confidence: 1.0,
        }
    }

    #[test]
    fn has_red_lines_empty_is_false() {
        let o = make_output(42, vec![], vec![]);
        assert!(!o.has_red_lines());
    }

    #[test]
    fn has_red_lines_with_one_violation_is_true() {
        let o = make_output(42, vec!["red1".into()], vec![]);
        assert!(o.has_red_lines());
    }

    #[test]
    fn needs_attention_with_red_line() {
        let o = make_output(42, vec!["red".into()], vec![]);
        assert!(o.needs_attention());
    }

    #[test]
    fn needs_attention_with_missing_evidence() {
        let o = make_output(42, vec![], vec!["missing1".into()]);
        assert!(o.needs_attention());
    }

    #[test]
    fn needs_attention_when_clean_is_false() {
        let o = make_output(42, vec![], vec![]);
        assert!(!o.needs_attention());
    }

    #[test]
    fn needs_attention_with_both() {
        let o = make_output(42, vec!["red".into()], vec!["m".into()]);
        assert!(o.needs_attention());
    }

    // serde roundtrip for AgentOutput<T>
    #[test]
    fn agent_output_serde_roundtrip() {
        let o = AgentOutput {
            computation: serde_json::json!({"result": "ok"}),
            audit_trail: "step 1\nstep 2".into(),
            red_lines_violated: vec!["red".into()],
            missing_evidence: vec!["m1".into(), "m2".into()],
            followups: vec!["follow".into()],
            confidence: 0.85,
        };
        let json = serde_json::to_string(&o).expect("ser");
        assert!(json.contains("\"confidence\":0.85"));
        let back: AgentOutput<serde_json::Value> = serde_json::from_str(&json).expect("de");
        assert_eq!(back.red_lines_violated.len(), 1);
        assert_eq!(back.missing_evidence.len(), 2);
        assert_eq!(back.confidence, 0.85);
    }

    // generic T: 验证 String / Vec / Custom struct 都能 work
    #[test]
    fn agent_output_generic_over_types() {
        let s: AgentOutput<String> = make_output("hello".into(), vec![], vec![]);
        assert_eq!(s.computation, "hello");
        let v: AgentOutput<Vec<i32>> = make_output(vec![1, 2, 3], vec![], vec![]);
        assert_eq!(v.computation.len(), 3);
    }
}
