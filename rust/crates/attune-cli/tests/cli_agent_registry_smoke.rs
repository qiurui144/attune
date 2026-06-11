//! ACP-1 / ACP-3 — CLI smoke tests for `attune agent registry` (and the
//! `attune agent health` shape).
//!
//! `agent registry` needs neither a vault nor an LLM — it reads + validates the
//! committed `agents.registry.toml` and prints the directory. Runs in CI with no
//! external dependencies.

use assert_cmd::Command;
use predicates::prelude::*;

fn attune_cmd() -> Command {
    Command::cargo_bin("attune").expect("attune binary should build")
}

fn good_registry() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("agents.registry.toml")
}

/// S4b: updated from 22-agent (multi-plugin) to 6-agent (oss-core only).
/// Industry agents (law-pro / tech-pro) removed from OSS registry in S4b.
#[test]
fn agent_registry_lists_all_agents_exit_zero() {
    attune_cmd()
        .args(["agent", "registry", "--registry"])
        .arg(good_registry())
        .assert()
        .success()
        .stdout(predicate::str::contains("Agent Registry"))
        // S4b: 6 oss-core agents only.
        .stdout(predicate::str::contains("6 agents"))
        // representative oss-core agents
        .stdout(predicate::str::contains("document_classifier"))
        .stdout(predicate::str::contains("memory_consolidation"))
        // S4b: no industry agents in OSS registry
        .stdout(predicate::str::contains("oss-core"))
        // columns: tier + gate must be visible (directory view, §5.5)
        .stdout(predicate::str::contains("free"))
        .stdout(predicate::str::contains("gate="));
}

#[test]
fn agent_registry_missing_file_exits_nonzero() {
    attune_cmd()
        .args([
            "agent",
            "registry",
            "--registry",
            "/definitely/does/not/exist/__no_registry__.toml",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read registry"));
}

// ── ACP-3 Task 4: `attune agent tune` is wired and dry-run by default ──────────

#[test]
fn agent_tune_help_documents_dry_run_default() {
    // `--help` needs neither a vault nor the registry; it proves the subcommand
    // is wired and that dry-run is the safe default (R2).
    attune_cmd()
        .args(["agent", "tune", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FeedbackController"))
        .stdout(predicate::str::contains("dry-run"))
        .stdout(predicate::str::contains("auto_escalate"));
}
