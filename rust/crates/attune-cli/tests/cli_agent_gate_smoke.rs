//! ACP-2 — CLI smoke tests for `attune agent gate`.
//!
//! Verifies the §5.5 CLI contract via subprocess (no vault, no LLM):
//!   - `attune agent gate --manifest <good>` → exit 0, prints roll-up dashboard.
//!   - `attune agent gate --manifest <missing>` → exit non-zero, clear error.
//!   - `attune agent gate --manifest <loosened>` → exit non-zero (ratchet fail).
//!
//! These run in CI with no external dependencies.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn attune_cmd() -> Command {
    Command::cargo_bin("attune").expect("attune binary should build")
}

/// Path to the committed workspace manifest (relative to the cli crate dir).
fn good_manifest() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("agent_quality_manifest.yaml")
}

#[test]
fn agent_gate_good_manifest_prints_dashboard_exit_zero() {
    attune_cmd()
        .args(["agent", "gate", "--manifest"])
        .arg(good_manifest())
        .assert()
        .success()
        .stdout(predicate::str::contains("roll-up dashboard"))
        .stdout(predicate::str::contains("Total gates recorded"))
        .stdout(predicate::str::contains("Ratchet (only-up): OK"));
}

#[test]
fn agent_gate_missing_manifest_exits_nonzero() {
    attune_cmd()
        .args([
            "agent",
            "gate",
            "--manifest",
            "/definitely/does/not/exist/__no_manifest__.yaml",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read manifest"));
}

#[test]
fn agent_gate_loosened_ratchet_exits_nonzero() {
    // A manifest that lowers a deterministic gate below its baseline must FAIL
    // the gate (the whole point of the machine-checkable ratchet).
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    write!(
        tmp,
        r#"
gates:
  - id: memory_consolidation
    plugin: oss-core
    tier: deterministic
    crate: attune-core
    test_name: x
    metric_kind: pass_rate
    threshold: 0.50
    ratchet_baseline: 1.00
    fixture_min: 14
    ignored: false
ignore_baseline:
  attune-core: 35
"#
    )
    .unwrap();
    tmp.flush().unwrap();
    attune_cmd()
        .args(["agent", "gate", "--manifest"])
        .arg(tmp.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("Ratchet (only-up): FAIL"))
        .stderr(predicate::str::contains("gate FAILED"));
}
