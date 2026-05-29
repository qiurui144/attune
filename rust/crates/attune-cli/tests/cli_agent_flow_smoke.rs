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

#[test]
fn agent_flow_list_shows_typed_chain_exit_zero() {
    attune_cmd()
        .args(["agent", "flow", "list", "--flows"])
        .arg(workspace_file("agent_flows.toml"))
        .arg("--registry")
        .arg(workspace_file("agents.registry.toml"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Agent Flows"))
        .stdout(predicate::str::contains("legal_defamation"))
        // the typed-handoff chain must be visible (§5.5)
        .stdout(predicate::str::contains("CaseFacts"))
        .stdout(predicate::str::contains("DefamationFacts"))
        .stdout(predicate::str::contains("defamation_extractor"))
        .stdout(predicate::str::contains("defamation_agent"));
}

#[test]
fn agent_flow_run_dry_run_shows_schedule_decisions() {
    // Paid + default quota → the cloud extractors route Cloud, the deterministic
    // damages calc routes Local. Pure dry-run, no LLM.
    attune_cmd()
        .args(["agent", "flow", "run", "legal_defamation", "--paid", "--flows"])
        .arg(workspace_file("agent_flows.toml"))
        .arg("--registry")
        .arg(workspace_file("agents.registry.toml"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Flow dry-run: legal_defamation"))
        .stdout(predicate::str::contains("entitlement: paid"))
        .stdout(predicate::str::contains("schedule: Cloud"))
        .stdout(predicate::str::contains("Local"));
}

#[test]
fn agent_flow_run_quota_zero_shows_degrade_and_block() {
    // Quota exhausted: qwen3b-floor fact_extractor degrades to local; the
    // gpt-4o-mini-floor defamation_extractor blocks (never silently downgrade).
    attune_cmd()
        .args([
            "agent", "flow", "run", "legal_defamation", "--paid", "--cloud-quota", "0", "--flows",
        ])
        .arg(workspace_file("agent_flows.toml"))
        .arg("--registry")
        .arg(workspace_file("agents.registry.toml"))
        .assert()
        .success()
        .stdout(predicate::str::contains("degraded_from_cloud: true"))
        .stdout(predicate::str::contains("BlockedQuotaExhausted"));
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
