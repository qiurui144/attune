//! ACP-5 Task 6 — CLI smoke tests for `attune agent flow list|run`.
//!
//! `agent flow` needs neither a vault nor an LLM — it reads + validates the
//! committed `agent_flows.toml` against `agents.registry.toml`, prints the flow
//! DAGs (`list`), and dry-runs the scheduling decision per step (`run`) without
//! calling any agent. Runs in CI with no external dependencies.

use assert_cmd::Command;
use predicates::prelude::*;

fn attune_cmd() -> Command {
    Command::cargo_bin("attune").expect("attune binary should build")
}

fn workspace_file(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(name)
}

/// S4b: OSS agent_flows.toml is intentionally empty — industry flows
/// (legal_defamation etc.) moved to attune-pro/plugins/law-pro/.
/// `agent flow list` exits zero and reports 0 flows in OSS mode.
#[test]
fn agent_flow_list_shows_empty_oss_flowset_exit_zero() {
    attune_cmd()
        .args(["agent", "flow", "list", "--flows"])
        .arg(workspace_file("agent_flows.toml"))
        .arg("--registry")
        .arg(workspace_file("agents.registry.toml"))
        .assert()
        .success()
        // S4b: OSS ships 0 flows; list must succeed (not crash) and report empty.
        .stdout(predicate::str::contains("Agent Flows"))
        // legal_defamation and its industry steps must NOT appear in OSS output.
        .stdout(predicate::str::contains("legal_defamation").not())
        .stdout(predicate::str::contains("defamation_extractor").not());
}

/// S4b: `agent flow run legal_defamation` exits non-zero (no such flow in OSS).
/// This is the graceful-degrade path — the command reports the unknown id and exits 1.
#[test]
fn agent_flow_run_unknown_industry_flow_exits_nonzero() {
    // legal_defamation is now an attune-pro flow; OSS must reject it gracefully.
    attune_cmd()
        .args(["agent", "flow", "run", "legal_defamation", "--paid", "--flows"])
        .arg(workspace_file("agent_flows.toml"))
        .arg("--registry")
        .arg(workspace_file("agents.registry.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("no flow with id"));
}

/// S4b: quota-zero path with an unknown industry flow also exits non-zero gracefully.
#[test]
fn agent_flow_run_quota_zero_unknown_industry_flow_exits_nonzero() {
    attune_cmd()
        .args([
            "agent", "flow", "run", "legal_defamation", "--paid", "--cloud-quota", "0", "--flows",
        ])
        .arg(workspace_file("agent_flows.toml"))
        .arg("--registry")
        .arg(workspace_file("agents.registry.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("no flow with id"));
}

#[test]
fn agent_flow_run_unknown_id_exits_nonzero() {
    attune_cmd()
        .args(["agent", "flow", "run", "no_such_flow", "--flows"])
        .arg(workspace_file("agent_flows.toml"))
        .arg("--registry")
        .arg(workspace_file("agents.registry.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("no flow with id"));
}

#[test]
fn agent_flow_list_help_is_wired() {
    attune_cmd()
        .args(["agent", "flow", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("run"));
}
