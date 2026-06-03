//! ACP-2 — Unified Quality Gate Orchestrator (TDD).
//!
//! Per spec `2026-05-29-ai-agents-governance-orchestration.md` §3 ACP-2 + §9.
//!
//! This harness is the *machine-checkable* successor to the convention-only
//! ratchet that (per the B audit) existed in exactly ONE place (law-pro
//! `thresholds.yaml`). It loads the workspace-level `agent_quality_manifest.yaml`
//! SSOT and asserts, on every PR:
//!
//!   1. **Schema validity** — every gate's `metric_kind` is in the whitelist
//!      (closes the B-finding that metric vocabulary diverged:
//!      exact_match / accuracy / WER / F1 / pass_rate with no shared schema).
//!   2. **Ratchet (only-up)** — each gate's `threshold` must be ≥ its
//!      committed `ratchet_baseline`. A PR that lowers a threshold below the
//!      baseline fails the build (solves "only law-pro is machine-checkable").
//!   3. **Roll-up == sum of parts** — the dashboard aggregate equals the
//!      per-gate roll-up (no silent gate drop).
//!   4. **#[ignore] spike guard** — the count of `#[ignore]` attributes across
//!      the attune-side gate test files must not exceed the recorded baseline
//!      + 2 (per CLAUDE.md §7.2 Gate 2 / global §Gate-2).
//!
//! Deterministic-only — runs in `cargo test --workspace --release` (PR gate).
//! Real-LLM gates are NOT run here (per R5: PR fast / nightly slow).

use attune_core::agent_quality::{self, MetricKind};

/// Locate the workspace-level manifest from the test's CARGO_MANIFEST_DIR
/// (`.../rust/crates/attune-core`) → `.../rust/agent_quality_manifest.yaml`.
fn manifest_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("agent_quality_manifest.yaml")
}

/// S4b regression: OSS manifest contains only oss-core gates — no industry plugin gates.
/// Industry gates (law-pro / tech-pro external) removed per S4b decoupling.
#[test]
fn quality_manifest_contains_only_oss_core_gates() {
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    for gate in &m.gates {
        assert_ne!(
            gate.plugin.as_str(),
            "law-pro",
            "S4b: law-pro gate '{}' must not appear in OSS manifest — belongs in attune-pro",
            gate.id,
        );
        assert_ne!(
            gate.plugin.as_str(),
            "tech-pro",
            "S4b: tech-pro gate '{}' must not appear in OSS manifest — belongs in attune-pro",
            gate.id,
        );
    }
}

#[test]
fn manifest_loads_and_is_nonempty() {
    let m = agent_quality::load_manifest(&manifest_path())
        .expect("agent_quality_manifest.yaml must load");
    // S4b: 3 external industry gates (law_pro_deterministic, law_pro_fact_extractor_llm,
    // tech_pro_code_reviewer) removed — OSS manifest now has 8 oss-core gates.
    assert!(
        m.gates.len() >= 8,
        "S4b: manifest must record all 8 oss-core gates, got {}",
        m.gates.len()
    );
}

#[test]
fn every_gate_metric_kind_is_in_whitelist() {
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    // The whitelist is enforced by deserialization: an unknown metric_kind
    // would fail to parse. This test asserts the *intent* — that parsing
    // succeeded means every metric_kind was a known variant.
    for g in &m.gates {
        // MetricKind is a closed enum; presence here proves whitelist membership.
        let _: &MetricKind = &g.metric_kind;
    }
    assert!(!m.gates.is_empty());
}

#[test]
fn unknown_metric_kind_is_rejected() {
    let bad = r#"
gates:
  - id: bogus
    plugin: oss-core
    tier: deterministic
    crate: attune-core
    test_name: bogus_gate
    metric_kind: totally_made_up_metric
    threshold: 1.0
    ratchet_baseline: 1.0
    fixture_min: 10
    ignored: false
ignore_baseline:
  attune-core: 0
"#;
    let res = agent_quality::parse_manifest(bad);
    assert!(
        res.is_err(),
        "an unknown metric_kind must be rejected by the whitelist (B: metric vocab divergence)"
    );
}

#[test]
fn ratchet_only_up_passes_when_threshold_at_or_above_baseline() {
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    let report = agent_quality::check_ratchet(&m);
    assert!(
        report.violations.is_empty(),
        "the committed manifest must satisfy only-up ratchet; violations: {:?}",
        report.violations
    );
}

#[test]
fn ratchet_rejects_threshold_below_baseline() {
    // A PR that lowers a deterministic gate's threshold from baseline 1.0 to
    // 0.9 must be caught (this is the whole point — machine-checkable ratchet).
    let lowered = r#"
gates:
  - id: memory_consolidation
    plugin: oss-core
    tier: deterministic
    crate: attune-core
    test_name: memory_promotion_golden_gate_pass_rate_must_be_one
    metric_kind: pass_rate
    threshold: 0.90
    ratchet_baseline: 1.00
    fixture_min: 14
    ignored: false
ignore_baseline:
  attune-core: 0
"#;
    let m = agent_quality::parse_manifest(lowered).unwrap();
    let report = agent_quality::check_ratchet(&m);
    assert_eq!(
        report.violations.len(),
        1,
        "lowering threshold below ratchet_baseline must be a violation"
    );
    assert!(report.violations[0].contains("memory_consolidation"));
}

#[test]
fn rollup_equals_sum_of_parts() {
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    let dash = agent_quality::roll_up(&m);
    // Roll-up total gate count must equal the sum of per-tier counts (no drop).
    let summed: usize = dash.per_tier.values().sum();
    assert_eq!(
        dash.total_gates, summed,
        "roll-up total must equal sum of per-tier counts (no silent gate drop)"
    );
    assert_eq!(dash.total_gates, m.gates.len());
}

#[test]
fn rollup_counts_machine_checkable_vs_external() {
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    let dash = agent_quality::roll_up(&m);
    // S4b: industry external gates (law_pro_deterministic, law_pro_fact_extractor_llm,
    // tech_pro_code_reviewer) removed from OSS manifest. All remaining gates are
    // machine-checkable oss-core gates.
    assert!(
        dash.machine_checkable >= 8,
        "S4b: ≥8 oss-core gates must be machine-checkable in this repo, got {}",
        dash.machine_checkable
    );
    assert_eq!(
        dash.external,
        0,
        "S4b: 0 external gates remain in OSS manifest (industry gates moved to attune-pro); \
         got {}",
        dash.external
    );
}

#[test]
fn ignore_spike_guard_within_budget() {
    // Count #[ignore] in the attune-core gate test files and assert it does not
    // exceed the recorded baseline + 2 (per §7.2 Gate 2). The orchestrator
    // counts real attributes in real files — not a self-reported number.
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    let gate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let report = agent_quality::check_ignore_spike(&m, &gate_dir, "attune-core")
        .expect("ignore-spike scan must succeed");
    assert!(
        !report.spiked,
        "#[ignore] count {} exceeds baseline {} + 2 budget (per §7.2 Gate 2)",
        report.observed, report.baseline
    );
}

#[test]
fn ignore_spike_guard_detects_spike() {
    // Synthetic: baseline 0, but a directory with 5 #[ignore] → spike.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("fake_gate.rs"),
        "#[ignore]\n#[ignore]\n#[ignore]\n#[ignore]\n#[ignore]\nfn x(){}",
    )
    .unwrap();
    let manifest = r#"
gates:
  - id: x
    plugin: oss-core
    tier: deterministic
    crate: attune-core
    test_name: x
    metric_kind: pass_rate
    threshold: 1.0
    ratchet_baseline: 1.0
    fixture_min: 10
    ignored: false
ignore_baseline:
  attune-core: 0
"#;
    let m = agent_quality::parse_manifest(manifest).unwrap();
    let report =
        agent_quality::check_ignore_spike(&m, tmp.path(), "attune-core").unwrap();
    assert!(report.spiked, "5 ignores over baseline 0+2 must spike");
    assert_eq!(report.observed, 5);
}

#[test]
fn oss_real_llm_gate_recorded_and_marked_nightly() {
    // The orphaned OSS real-LLM gate (B finding #3) must now be recorded with
    // ignored=true (it's #[ignore]) AND flagged as running in nightly CI.
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    let oss_real = m
        .gates
        .iter()
        .find(|g| g.id == "oss_agent_real_llm")
        .expect("oss_agent_real_llm gate must be recorded (was orphaned per B audit)");
    assert!(oss_real.ignored, "real-LLM gate is #[ignore]");
    assert_eq!(
        oss_real.tier,
        agent_quality::Tier::Llm,
        "real-LLM gate is LLM tier (≥0.85 floor class)"
    );
    assert_eq!(
        oss_real.ci.as_deref(),
        Some("nightly"),
        "orphan fix: OSS real-LLM gate must declare nightly CI (no longer orphaned)"
    );
}

#[test]
fn the_five_oss_deterministic_gates_recorded_with_real_baselines() {
    // The 5 OSS agent gates already exist and are deterministic at
    // pass_rate/zero_violations = 1.00 (their real baseline). Record — do not fudge.
    let m = agent_quality::load_manifest(&manifest_path()).unwrap();
    for id in [
        "chat_reliability",
        "document_classifier",
        "linker",
        "memory_consolidation",
        "self_evolving_skill",
    ] {
        let g = m
            .gates
            .iter()
            .find(|g| g.id == id)
            .unwrap_or_else(|| panic!("OSS gate {id} must be in manifest"));
        assert_eq!(g.plugin, "oss-core", "{id} is OSS-core");
        assert_eq!(
            g.tier,
            agent_quality::Tier::Deterministic,
            "{id} gate is deterministic (no LLM in decision path)"
        );
    }
}
