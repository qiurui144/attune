//! Agent runner — 桥接 plugin_registry + capability_dispatch.
//!
//! 给 chat handler / Web UI 一层方便调用: 给定 agent_id + JSON 输入, 跑 binary 拿结果.
//! plugin_registry 查 agent.binary 路径 → capability_dispatch 起 subprocess → 返回结构化结果.

use crate::capability_dispatch::{dispatch, CapabilityInvocation, CapabilityResult};
use crate::error::{Result, VaultError};
use crate::plugin_loader::AgentSpec;
use crate::plugin_registry::PluginRegistry;
use std::path::PathBuf;
use std::time::Duration;

/// 解析 agent 调用所需的 binary 路径
fn resolve_agent_binary(plugin_dir: &std::path::Path, agent: &AgentSpec) -> Option<PathBuf> {
    if let Some(rel) = &agent.binary {
        let full = plugin_dir.join(rel);
        if full.exists() {
            return Some(full);
        }
    }
    crate::capability_dispatch::resolve_binary(plugin_dir, &agent.id)
}

/// 调用 agent (通过 subprocess), 返回 raw CapabilityResult.
/// 调用方按 agent 业务 schema 解析 stdout JSON.
pub fn run_agent_subprocess(
    registry: &PluginRegistry,
    agent_id: &str,
    plugin_dir: &std::path::Path,
    stdin_json: &str,
    env: Vec<(String, String)>,
    timeout: Duration,
) -> Result<CapabilityResult> {
    let agent = registry
        .list_agents()
        .into_iter()
        .find(|(_, a)| a.id == agent_id)
        .map(|(_, a)| a.clone())
        .ok_or_else(|| {
            VaultError::InvalidInput(format!("agent '{agent_id}' not found in registry"))
        })?;

    let binary = resolve_agent_binary(plugin_dir, &agent).ok_or_else(|| {
        VaultError::InvalidInput(format!(
            "agent '{agent_id}' binary not found (declared {:?})",
            agent.binary
        ))
    })?;

    let mut inv = CapabilityInvocation::new(binary)
        .stdin(stdin_json)
        .timeout(timeout);
    for (k, v) in env {
        inv = inv.env(k, v);
    }
    dispatch(&inv)
}

/// 上层封装: agent 调用结果适配为统一 chat-friendly 字符串
pub fn format_agent_result_for_chat(result: &CapabilityResult, agent_id: &str) -> String {
    if result.timed_out {
        return format!("⚠️ agent '{agent_id}' 超时 (>=配置上限)");
    }
    match result.exit_code {
        0 => {
            // 成功: 优先回 stderr (audit_trail 通常打 stderr), stdout 是 JSON
            if !result.stderr.is_empty() {
                format!(
                    "✅ {agent_id} 成功:\n```\n{}\n```\n\n--- 机器可读 JSON (stdout) ---\n{}",
                    result.stderr.trim(),
                    result.stdout.trim()
                )
            } else {
                format!("✅ {agent_id} 成功:\n```json\n{}\n```", result.stdout.trim())
            }
        }
        2 => format!(
            "⚠️ {agent_id} 业务红线触发, 拒绝输出:\n{}",
            result.stderr.trim()
        ),
        other => format!(
            "❌ {agent_id} 错误 (exit={other}):\n{}",
            result.stderr.trim()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_success_result_includes_audit() {
        let r = CapabilityResult {
            exit_code: 0,
            stdout: r#"{"computed":42}"#.into(),
            stderr: "audit: 调 skill x → y\n".into(),
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "test_agent");
        assert!(s.contains("✅ test_agent"));
        assert!(s.contains("audit: 调 skill"));
        assert!(s.contains("\"computed\":42"));
    }

    #[test]
    fn format_red_line_uses_warning() {
        let r = CapabilityResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: "借条不存在".into(),
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "civil_loan_agent");
        assert!(s.contains("⚠️"));
        assert!(s.contains("业务红线"));
        assert!(s.contains("借条不存在"));
    }

    #[test]
    fn format_timeout_explicit() {
        let r = CapabilityResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        };
        let s = format_agent_result_for_chat(&r, "x");
        assert!(s.contains("超时"));
    }

    #[test]
    fn format_unknown_error_shows_exit_code() {
        let r = CapabilityResult {
            exit_code: 99,
            stdout: String::new(),
            stderr: "fatal: oops".into(),
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "broken_agent");
        assert!(s.contains("exit=99"));
        assert!(s.contains("fatal"));
    }

    #[test]
    fn run_agent_unknown_id_returns_error() {
        let reg = PluginRegistry::new();
        let tmp = tempfile::TempDir::new().unwrap();
        let err = run_agent_subprocess(
            &reg,
            "nonexistent_agent",
            tmp.path(),
            "{}",
            vec![],
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }
}
