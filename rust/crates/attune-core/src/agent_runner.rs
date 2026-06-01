//! Agent runner — 桥接 plugin_registry + capability_dispatch.
//!
//! 给 chat handler / Web UI 一层方便调用: 给定 agent_id + JSON 输入, 跑 binary 拿结果.
//! plugin_registry 查 agent.binary 路径 → capability_dispatch 起 subprocess → 返回结构化结果.

use crate::capability_dispatch::{
    dispatch_capability, parse_runtime, resolve_wasm, CapabilityInvocation, CapabilityResult,
    CapabilityRuntime,
};
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

    // 跨平台分流 (spec §3.2): 按 agent.runtime 解析执行体路径,统一走
    // dispatch_capability。RustBinary → resolve_binary(现有行为不变);
    // Wasm → resolve_wasm(.wasm 模块);data_only/python_subprocess → Err。
    // CapabilityInvocation.binary 字段复用为"执行体路径"(per 决策 D-a)。
    let runtime = parse_runtime(&agent.runtime)?;
    let entry = match runtime {
        CapabilityRuntime::RustBinary => {
            resolve_agent_binary(plugin_dir, &agent).ok_or_else(|| {
                VaultError::InvalidInput(format!(
                    "agent '{agent_id}' binary not found (declared {:?})",
                    agent.binary
                ))
            })?
        }
        CapabilityRuntime::Wasm => {
            let rel = agent.wasm.as_deref().ok_or_else(|| {
                VaultError::InvalidInput(format!("agent '{agent_id}' runtime=wasm but no wasm path"))
            })?;
            resolve_wasm(plugin_dir, rel).ok_or_else(|| {
                VaultError::InvalidInput(format!(
                    "agent '{agent_id}' wasm module not found: {rel}"
                ))
            })?
        }
        CapabilityRuntime::DataOnly => {
            return Err(VaultError::InvalidInput(format!(
                "agent '{agent_id}' is data_only; no executable to dispatch"
            )));
        }
    };

    let mut inv = CapabilityInvocation::new(entry)
        .stdin(stdin_json)
        .timeout(timeout);
    for (k, v) in env {
        inv = inv.env(k, v);
    }
    dispatch_capability(runtime, &inv)
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

    // ── 增强覆盖: success without audit / format edge / resolve_agent_binary ─

    #[test]
    fn format_success_without_audit_uses_stdout_only() {
        let r = CapabilityResult {
            exit_code: 0,
            stdout: r#"{"k":"v"}"#.into(),
            stderr: String::new(), // 无 audit
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "ag");
        assert!(s.contains("✅ ag"));
        assert!(s.contains("json"));
        assert!(s.contains("\"k\":\"v\""));
        // 不应含 "audit_trail" 那种 fallback 标签
        assert!(!s.contains("机器可读"));
    }

    // Edge: 空 stdout 也不 panic
    #[test]
    fn format_success_empty_stdout_no_panic() {
        let r = CapabilityResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: "audit".into(),
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "x");
        assert!(s.contains("✅"));
    }

    // Edge: stderr 含多行 — trim 后保留
    #[test]
    fn format_multiline_stderr_trimmed() {
        let r = CapabilityResult {
            exit_code: 0,
            stdout: "{}".into(),
            stderr: "line1\nline2\nline3\n".into(),
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "x");
        assert!(s.contains("line1"));
        assert!(s.contains("line3"));
    }

    // Edge: red line 用 exit 2 — Unicode error msg
    #[test]
    fn format_red_line_unicode_message() {
        let r = CapabilityResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: "证据链断裂 — 缺借条 🚨".into(),
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "civil_loan");
        assert!(s.contains("证据链断裂"));
        assert!(s.contains("🚨"));
    }

    // Edge: timed_out 优先于 exit_code 显示
    #[test]
    fn format_timeout_takes_precedence_over_exit_code() {
        let r = CapabilityResult {
            exit_code: 0, // 即使 exit 0
            stdout: "result".into(),
            stderr: String::new(),
            timed_out: true, // ...但 timed_out 优先
        };
        let s = format_agent_result_for_chat(&r, "x");
        assert!(s.contains("超时"));
        // 不应显示 "成功"
        assert!(!s.contains("✅"));
    }

    // run_agent: 空 agent_id (空字符串)
    #[test]
    fn run_agent_empty_id_returns_invalid_input() {
        let reg = PluginRegistry::new();
        let tmp = tempfile::TempDir::new().unwrap();
        let err = run_agent_subprocess(
            &reg,
            "",
            tmp.path(),
            "{}",
            vec![],
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    // resolve_agent_binary: 给一个不存在的 binary path
    #[test]
    fn resolve_agent_binary_returns_none_for_missing() {
        // 用 serde JSON 构造避免直接依赖所有 field
        let yaml = r#"id: test_agent
runtime: rust_binary
binary: bin/nonexistent
"#;
        let agent: AgentSpec = serde_yaml::from_str(yaml).expect("parse spec");
        let tmp = tempfile::TempDir::new().unwrap();
        // binary 不在 plugin_dir/bin/nonexistent → fallback 到 capability_dispatch::resolve_binary
        let result = resolve_agent_binary(tmp.path(), &agent);
        // 大概率 None (bin/nonexistent 不存在 + capability_dispatch 也找不到)
        assert!(result.is_none());
    }

    // resolve_agent_binary: 给一个存在的 binary path
    #[test]
    fn resolve_agent_binary_returns_some_when_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin_path = bin_dir.join("test_agent");
        std::fs::write(&bin_path, "dummy").unwrap();

        let yaml = r#"id: test_agent
runtime: rust_binary
binary: bin/test_agent
"#;
        let agent: AgentSpec = serde_yaml::from_str(yaml).expect("parse spec");
        let result = resolve_agent_binary(tmp.path(), &agent);
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("test_agent"));
    }

    // adversarial: super long stderr 不爆
    #[test]
    fn format_handles_huge_stderr() {
        let big = "x".repeat(100_000);
        let r = CapabilityResult {
            exit_code: 99,
            stdout: String::new(),
            stderr: big.clone(),
            timed_out: false,
        };
        let s = format_agent_result_for_chat(&r, "x");
        assert!(s.len() > 100_000);
        assert!(s.contains("exit=99"));
    }
}
