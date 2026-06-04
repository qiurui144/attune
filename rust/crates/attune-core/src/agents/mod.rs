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

// Agent / AgentOutput / AgentError / AgentResult 抽到 wasm-safe leaf crate
// attune-agent-sdk(零 native 依赖,可编 wasm32-wasip1)。此处 re-export **同一类型**
// (非重定义)——保 `attune_core::agents::{Agent, AgentOutput}` 路径不变,且
// attune-pro `impl Agent for ...` 仍指向同一 trait。`From<AgentError> for VaultError`
// 在 crate::error 侧定义,内部 agent 仍可返回 crate::error::Result(? 自动桥接)。
pub use attune_agent_sdk::{Agent, AgentError, AgentOutput, AgentResult};
// Re-export the generic subprocess-ABI entry helper so plugin agents (law-pro
// ×12, future medical/patent) call it via the stable `attune_core::agents::
// agent_main` path. Same module — not a redefinition.
pub use attune_agent_sdk::agent_main;

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

#[cfg(test)]
mod tests {
    // AgentOutput / Agent / AgentError 的单元测试已随类型迁入 attune-agent-sdk leaf
    // crate(8 golden + 边界 + 异常 + proptest + JSON wire 断言)。此处只保留
    // attune-core 本地的 workspace-file locator 测试(locate_workspace_file /
    // load_workspace_flows 留在 attune-core,依赖 DB/网络外的文件系统漫游)。

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
    // against the registry (guarantee ①). S4b: OSS ships an intentionally empty
    // flow set — industry flows (legal_defamation etc.) live in attune-pro. The
    // loader must succeed (not Err) and return an empty FlowSet; the OSS registry
    // still has 6 oss-core agents.
    #[test]
    fn load_workspace_flows_loads_and_validates() {
        let (flows, reg) =
            super::load_workspace_flows("agents.registry.toml", "agent_flows.toml")
                .expect("workspace flows must load + validate");
        assert!(!reg.is_empty(), "registry must have oss-core agents");
        // S4b: legal_defamation moved to attune-pro — OSS flow set is empty.
        assert!(
            flows.get("legal_defamation").is_none(),
            "S4b: legal_defamation flow must not be present in OSS (moved to attune-pro)"
        );
        assert!(
            flows.is_empty(),
            "S4b: OSS agent_flows.toml is intentionally empty — industry flows live in attune-pro"
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

#[cfg(test)]
mod agent_main_reexport_tests {
    #[test]
    fn agent_main_helper_is_reachable_via_attune_core_path() {
        // Compile-level: these paths must resolve through attune-core.
        use crate::agents::agent_main::{AgentExit, EntryConfig, Section, SectionField};
        let _cfg = EntryConfig {
            red_line_exit2: true,
            sections: &[Section {
                title: "x",
                field: SectionField::RedLines,
                bullet: "⚠️  ",
            }],
            input_name: "X",
        };
        let _ = AgentExit::Success;
    }
}
