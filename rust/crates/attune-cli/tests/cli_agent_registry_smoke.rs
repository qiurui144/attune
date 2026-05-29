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

#[test]
fn agent_registry_lists_all_agents_exit_zero() {
    attune_cmd()
        .args(["agent", "registry", "--registry"])
        .arg(good_registry())
        .assert()
        .success()
        .stdout(predicate::str::contains("Agent Registry"))
        .stdout(predicate::str::contains("22 agents"))
        // representative agents from each owner
        .stdout(predicate::str::contains("document_classifier"))
        .stdout(predicate::str::contains("defamation_extractor"))
        .stdout(predicate::str::contains("code_reviewer"))
        // columns: tier + gate must be visible (directory view, §5.5)
        .stdout(predicate::str::contains("free"))
        .stdout(predicate::str::contains("paid"))
        .stdout(predicate::str::contains("gpt-4o-mini"));
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
