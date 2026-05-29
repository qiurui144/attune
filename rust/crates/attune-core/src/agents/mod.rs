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

/// Locate a workspace SSOT file (`agents.registry.toml` / `agent_flows.toml`) by
/// walking up from CWD and the running executable's directory (ACP §5.5 / §5.3b —
/// the registry + flows are workspace files, not vault data). Returns `None` when
/// the file is absent (e.g. an OSS attune install that ships no agent registry —
/// the flow path then stays a no-op and chat falls back to free-form RAG).
///
/// Shared by the CLI (`attune agent flow …`) and the server's chat-path flow
/// wiring so both resolve the same files with identical semantics.
pub fn locate_workspace_file(name: &str) -> Option<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.to_path_buf());
        }
    }
    for root in roots {
        let mut cur: Option<&std::path::Path> = Some(root.as_path());
        while let Some(dir) = cur {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
            cur = dir.parent();
        }
    }
    None
}

/// Load the workspace agent registry + flow set and validate the typed-handoff
/// chain (ACP-5 guarantee ①). Returns `None` when either file is absent (graceful
/// — an OSS install with no agents has no flows to run) or fails to parse /
/// validate (the error is logged by the caller; the chat path must never hard-fail
/// because the optional flow layer could not load — spec §7 / §11 R8).
///
/// The two file names default to the workspace SSOT names but are parameterized so
/// tests can point at fixtures.
pub fn load_workspace_flows(
    registry_name: &str,
    flows_name: &str,
) -> std::result::Result<(flow::FlowSet, registry::AgentRegistry), String> {
    let reg_path = locate_workspace_file(registry_name)
        .ok_or_else(|| format!("{registry_name} not found in workspace"))?;
    let flows_path = locate_workspace_file(flows_name)
        .ok_or_else(|| format!("{flows_name} not found in workspace"))?;
    let reg = registry::AgentRegistry::from_path(&reg_path)?;
    let flows = flow::FlowSet::from_path(&flows_path)?;
    // Guarantee ① — typed handoff validated against the registry at load time.
    flows.validate_against(&reg)?;
    Ok((flows, reg))
}

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

    // ACP-5 chat wiring — workspace file locator finds the SSOT registry by
    // walking up from CWD (tests run with CWD inside the crate dir).
    #[test]
    fn locate_workspace_file_finds_registry() {
        let found = super::locate_workspace_file("agents.registry.toml");
        assert!(
            found.is_some(),
            "agents.registry.toml must be locatable from the workspace"
        );
    }

    // ACP-5 chat wiring — a missing file is a graceful None (not a panic / error).
    #[test]
    fn locate_workspace_file_missing_is_none() {
        assert!(super::locate_workspace_file("definitely-not-a-real-file.toml").is_none());
    }

    // ACP-5 chat wiring — load_workspace_flows validates the typed-handoff chain
    // against the registry (guarantee ①) and returns the canonical legal_defamation
    // flow from the workspace SSOT.
    #[test]
    fn load_workspace_flows_loads_and_validates() {
        let (flows, reg) =
            super::load_workspace_flows("agents.registry.toml", "agent_flows.toml")
                .expect("workspace flows must load + validate");
        assert!(!reg.is_empty(), "registry must have agents");
        assert!(
            flows.get("legal_defamation").is_some(),
            "the canonical legal_defamation flow must be present"
        );
    }

    // ACP-5 chat wiring — a missing registry/flows file is an Err (caller degrades),
    // never a panic.
    #[test]
    fn load_workspace_flows_missing_is_err() {
        let r = super::load_workspace_flows("nope-registry.toml", "nope-flows.toml");
        assert!(r.is_err());
    }
}
