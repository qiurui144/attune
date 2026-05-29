//! ACP-2 — Unified Agent Quality Gate Orchestration (workspace SSOT).
//!
//! Per spec `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §3 (ACP-2) + §5.5 (CLI) + §9 (test matrix).
//!
//! ## Why this exists (B audit `2026-05-29-quality-gating-telemetry-audit.md`)
//!
//! 11 golden-gate harnesses were **siloed**: thresholds hardcoded inline in 9 of
//! 11 `.rs` files, metric vocabulary inconsistent (`exact_match` / `accuracy` /
//! `WER` / `F1` / `pass_rate`), fixture floors scattered (≥10/11/13/14), and the
//! only machine-checkable ratchet lived in a single place (law-pro
//! `thresholds.yaml`). There was no roll-up dashboard and the OSS real-LLM gate
//! was orphaned (not in any workflow).
//!
//! This module lifts the law-pro `thresholds.yaml` pattern to a **workspace-level
//! manifest** (`rust/agent_quality_manifest.yaml`) and provides the shared
//! orchestration logic consumed by BOTH:
//!   - the CI hard-gate test (`tests/agent_gate_orchestrator.rs`), and
//!   - the CLI (`attune agent gate`).
//!
//! It does NOT re-run the 11 gates as subprocesses — those already run inside
//! `cargo test --workspace --release`. This module is the SSOT + ratchet enforcer
//! + roll-up dashboard.
//!
//! Per R5 (spec §11): PR runs deterministic (fast), nightly runs real-LLM
//! (slow); the manifest records which gate is which.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Closed whitelist of metric vocabularies. Deserialization of an unknown
/// `metric_kind` fails — this is the machine-checkable cure for the B-finding
/// that metric vocabulary diverged across gates with no shared schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// Whole-record exact match rate (law-pro civil_loan/labor/housing/sale).
    ExactMatchRate,
    /// Per-field exact match rate (law-pro bank/traffic/inheritance/divorce/defamation).
    FieldExactRate,
    /// Status-correctness rate (law-pro limitation).
    StatusCorrectnessRate,
    /// Relation/gap set match rate (law-pro evidence_chain).
    RelationGapSetMatchRate,
    /// pass_rate == threshold (memory_consolidation / self_evolving_skill).
    PassRate,
    /// Binary 0-violation facet gate (chat_reliability, document_classifier, linker).
    ZeroViolations,
    /// OCR per-field accuracy red line (office_ocr).
    MinFieldAccuracy,
    /// ASR word/char error rate red line — lower is better (office_asr).
    Wer,
    /// Real-time factor latency red line — lower is better (office_asr).
    Rtf,
    /// Micro-F1 over holdout (law-pro fact_extractor LLM lane).
    MicroF1,
    /// Real-LLM acceptance count `passed >= N` (oss_agent_real_llm).
    AcceptCount,
}

impl MetricKind {
    /// True when a *higher* metric value is better (ratchet raises the floor).
    /// For latency/error metrics (WER/RTF) a *lower* value is better, so the
    /// ratchet there caps the ceiling — but the only-up convention on the
    /// recorded `threshold` field is identical (a stricter threshold = lower
    /// number for these). We keep the comparison uniform on the stored number
    /// and document the direction here for readers.
    pub fn higher_is_better(self) -> bool {
        !matches!(self, MetricKind::Wer | MetricKind::Rtf)
    }
}

/// Quality tier: deterministic gates must hit 1.00; LLM gates ride a ≥0.85 floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Deterministic,
    Llm,
    /// Real OCR/ASR engine over fixtures — deterministic engine, numeric red line.
    Engine,
}

/// One agent quality gate entry. Mirrors law-pro `thresholds.yaml` rows but with
/// the full metadata the B audit found scattered: metric vocabulary, ratchet
/// baseline, fixture floor, tier, crate, test name, ignore status, CI lane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateEntry {
    /// Stable gate id (agent or engine scene group).
    pub id: String,
    /// Owning plugin: "oss-core" for attune, else "law-pro" / "tech-pro" / ...
    pub plugin: String,
    /// Quality tier.
    pub tier: Tier,
    /// Crate the gate test lives in (attune-core / attune-server / attune-pro:*).
    #[serde(rename = "crate")]
    pub krate: String,
    /// Name of the gate test fn (or pattern) — for traceability.
    pub test_name: String,
    /// Metric vocabulary (whitelisted).
    pub metric_kind: MetricKind,
    /// Current enforced threshold.
    pub threshold: f64,
    /// Committed ratchet baseline. `threshold` may only move toward stricter
    /// (≥ baseline for higher-is-better; ≤ baseline for WER/RTF). Lowering the
    /// floor below baseline (or raising the WER/RTF ceiling above it) fails CI.
    pub ratchet_baseline: f64,
    /// Minimum fixture count (six-class floor lower bound for this gate).
    pub fixture_min: u32,
    /// True if the gate test is `#[ignore]` (e.g. real-LLM requiring Ollama).
    pub ignored: bool,
    /// CI lane this gate runs in: "pr" (default), "nightly", or external repo.
    #[serde(default)]
    pub ci: Option<String>,
    /// True when the gate is enforced in a *different* repo (attune-pro) and is
    /// recorded here for the roll-up only (not machine-checked in this repo).
    #[serde(default)]
    pub external: bool,
    /// Optional free-text note (e.g. legacy-overlap markers).
    #[serde(default)]
    pub note: Option<String>,
}

/// The full workspace manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityManifest {
    pub gates: Vec<GateEntry>,
    /// Per-crate `#[ignore]` baseline counts (for the spike guard).
    #[serde(default)]
    pub ignore_baseline: BTreeMap<String, u32>,
}

/// Parse a manifest from a YAML string. Unknown `metric_kind` / `tier` values
/// fail here (whitelist enforcement).
pub fn parse_manifest(yaml: &str) -> Result<QualityManifest, String> {
    serde_yaml::from_str(yaml).map_err(|e| format!("manifest parse error: {e}"))
}

/// Load the manifest from a file path.
pub fn load_manifest(path: &Path) -> Result<QualityManifest, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read manifest {}: {e}", path.display()))?;
    parse_manifest(&raw)
}

/// Result of the ratchet (only-up) check.
#[derive(Debug, Default)]
pub struct RatchetReport {
    pub violations: Vec<String>,
}

/// Enforce the only-up ratchet against each gate's committed `ratchet_baseline`.
///
/// For higher-is-better metrics: `threshold >= ratchet_baseline`.
/// For WER/RTF (lower-is-better): `threshold <= ratchet_baseline` (stricter =
/// smaller). A violation is a PR that loosened a gate.
pub fn check_ratchet(m: &QualityManifest) -> RatchetReport {
    let mut report = RatchetReport::default();
    for g in &m.gates {
        let ok = if g.metric_kind.higher_is_better() {
            g.threshold >= g.ratchet_baseline - f64::EPSILON
        } else {
            g.threshold <= g.ratchet_baseline + f64::EPSILON
        };
        if !ok {
            report.violations.push(format!(
                "gate '{}' ({:?}) threshold {} loosened past ratchet_baseline {} — \
                 thresholds only move stricter (per ratchet rule 只升不降)",
                g.id, g.metric_kind, g.threshold, g.ratchet_baseline
            ));
        }
    }
    report
}

/// Roll-up dashboard aggregate.
#[derive(Debug)]
pub struct RollUp {
    pub total_gates: usize,
    pub per_tier: BTreeMap<String, usize>,
    pub per_plugin: BTreeMap<String, usize>,
    /// Gates machine-checked in *this* repo (not external).
    pub machine_checkable: usize,
    /// Gates recorded for roll-up but enforced in the attune-pro repo.
    pub external: usize,
    /// Gates that are `#[ignore]` (real-LLM lane).
    pub ignored: usize,
    /// Gates declaring a nightly CI lane.
    pub nightly: usize,
}

/// Compute the roll-up dashboard. `total_gates == sum(per_tier values)` is an
/// invariant asserted by the orchestrator test (no silent gate drop).
pub fn roll_up(m: &QualityManifest) -> RollUp {
    let mut per_tier: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_plugin: BTreeMap<String, usize> = BTreeMap::new();
    let (mut machine_checkable, mut external, mut ignored, mut nightly) = (0, 0, 0, 0);
    for g in &m.gates {
        *per_tier.entry(format!("{:?}", g.tier)).or_insert(0) += 1;
        *per_plugin.entry(g.plugin.clone()).or_insert(0) += 1;
        if g.external {
            external += 1;
        } else {
            machine_checkable += 1;
        }
        if g.ignored {
            ignored += 1;
        }
        if g.ci.as_deref() == Some("nightly") {
            nightly += 1;
        }
    }
    RollUp {
        total_gates: m.gates.len(),
        per_tier,
        per_plugin,
        machine_checkable,
        external,
        ignored,
        nightly,
    }
}

/// Result of the `#[ignore]` spike scan.
#[derive(Debug)]
pub struct IgnoreSpikeReport {
    pub observed: u32,
    pub baseline: u32,
    /// True when observed > baseline + 2 (per §7.2 Gate 2 budget).
    pub spiked: bool,
}

/// Count `#[ignore]` attributes across `.rs` files in `gate_dir` and compare to
/// the recorded baseline for `krate`. The count is from *real files* — it cannot
/// be defeated by self-reporting a lower number (adversarial review concern).
pub fn check_ignore_spike(
    m: &QualityManifest,
    gate_dir: &Path,
    krate: &str,
) -> Result<IgnoreSpikeReport, String> {
    let baseline = *m.ignore_baseline.get(krate).unwrap_or(&0);
    let observed = count_ignore_attrs(gate_dir)?;
    Ok(IgnoreSpikeReport {
        observed,
        baseline,
        spiked: observed > baseline + 2,
    })
}

/// Recursively count `#[ignore]` attribute lines in `.rs` files under `dir`.
/// Matches `#[ignore]` and `#[ignore = "..."]` (leading whitespace allowed),
/// ignoring occurrences inside line comments.
fn count_ignore_attrs(dir: &Path) -> Result<u32, String> {
    let mut total = 0u32;
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot scan {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            total += count_ignore_attrs(&path)?;
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        for line in content.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue; // skip commented-out attributes
            }
            if trimmed.starts_with("#[ignore]") || trimmed.starts_with("#[ignore ") {
                total += 1;
            }
        }
    }
    Ok(total)
}

/// Run the full orchestrator: validate ratchet + roll-up. Returns a human
/// dashboard string and a pass/fail. Used by `attune agent gate`.
pub fn run_orchestrator(manifest_path: &Path) -> Result<(String, bool), String> {
    let m = load_manifest(manifest_path)?;
    let ratchet = check_ratchet(&m);
    let dash = roll_up(&m);

    let mut out = String::new();
    out.push_str("Attune Agent Quality Gate — roll-up dashboard\n");
    out.push_str("=============================================\n");
    out.push_str(&format!("Total gates recorded : {}\n", dash.total_gates));
    out.push_str(&format!(
        "  machine-checkable  : {} (this repo)\n",
        dash.machine_checkable
    ));
    out.push_str(&format!(
        "  external           : {} (attune-pro repo)\n",
        dash.external
    ));
    out.push_str(&format!("  #[ignore] (real-LLM): {}\n", dash.ignored));
    out.push_str(&format!("  nightly CI lane    : {}\n", dash.nightly));
    out.push_str("\nBy tier:\n");
    for (tier, n) in &dash.per_tier {
        out.push_str(&format!("  {tier:<14}: {n}\n"));
    }
    out.push_str("\nBy plugin:\n");
    for (plugin, n) in &dash.per_plugin {
        out.push_str(&format!("  {plugin:<14}: {n}\n"));
    }
    out.push_str("\nRatchet (only-up): ");
    let pass = ratchet.violations.is_empty();
    if pass {
        out.push_str("OK — no threshold loosened\n");
    } else {
        out.push_str("FAIL\n");
        for v in &ratchet.violations {
            out.push_str(&format!("  - {v}\n"));
        }
    }
    Ok((out, pass))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_kind_direction() {
        assert!(MetricKind::PassRate.higher_is_better());
        assert!(MetricKind::MicroF1.higher_is_better());
        assert!(!MetricKind::Wer.higher_is_better());
        assert!(!MetricKind::Rtf.higher_is_better());
    }

    #[test]
    fn ratchet_wer_lower_is_stricter() {
        // For WER, threshold 0.10 vs baseline 0.15 means we tightened (ok).
        let m = parse_manifest(
            r#"
gates:
  - id: asr_en
    plugin: oss-core
    tier: engine
    crate: attune-server
    test_name: asr_en
    metric_kind: wer
    threshold: 0.10
    ratchet_baseline: 0.15
    fixture_min: 5
    ignored: false
ignore_baseline: {}
"#,
        )
        .unwrap();
        assert!(check_ratchet(&m).violations.is_empty());
    }

    #[test]
    fn ratchet_wer_loosened_ceiling_fails() {
        // Raising WER ceiling above baseline = loosening = violation.
        let m = parse_manifest(
            r#"
gates:
  - id: asr_en
    plugin: oss-core
    tier: engine
    crate: attune-server
    test_name: asr_en
    metric_kind: wer
    threshold: 0.20
    ratchet_baseline: 0.15
    fixture_min: 5
    ignored: false
ignore_baseline: {}
"#,
        )
        .unwrap();
        assert_eq!(check_ratchet(&m).violations.len(), 1);
    }

    #[test]
    fn unknown_tier_rejected() {
        let bad = r#"
gates:
  - id: x
    plugin: oss-core
    tier: quantum_superposition
    crate: attune-core
    test_name: x
    metric_kind: pass_rate
    threshold: 1.0
    ratchet_baseline: 1.0
    fixture_min: 10
    ignored: false
ignore_baseline: {}
"#;
        assert!(parse_manifest(bad).is_err());
    }

    #[test]
    fn count_ignore_skips_comments() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("a.rs"),
            "#[ignore]\n// #[ignore]\n    #[ignore = \"needs ollama\"]\nfn x(){}",
        )
        .unwrap();
        // 2 real (one bare, one with reason), 1 commented out → skipped.
        assert_eq!(count_ignore_attrs(tmp.path()).unwrap(), 2);
    }

    #[test]
    fn empty_manifest_rolls_up_to_zero() {
        let m = parse_manifest("gates: []\nignore_baseline: {}").unwrap();
        let d = roll_up(&m);
        assert_eq!(d.total_gates, 0);
        assert_eq!(d.machine_checkable, 0);
    }
}
