//! Stage 4 — 💰 VLM escalation. Reuses the existing `VlmProvider` (vqa) with a
//! schema-guided JSON prompt + a retry-validate loop (≤3, spec §4.5 B) + failure
//! telemetry (§4.5 F). Outbound is enforced by the caller through OutboundGate
//! (see gate_vlm_egress below); this module is pure orchestration over the provider.

use super::{RegionKind, RegionResult, Series};
use crate::error::{Result, VaultError};
use crate::ocr::profile::VlmEscalationPolicy;
use crate::outbound_gate::{OutboundError, OutboundGate, OutboundKind, OutboundPolicy};
use crate::pii::Redactor;
use crate::vlm::VlmProvider;
use std::path::Path;

/// Max retries when VLM JSON is invalid (spec §4.5 B).
pub const MAX_RETRIES: u32 = 3;

/// Should this region escalate, given policy + the cross-validate decision + budget?
/// `discrepant` = the region was flagged conflict/discrepancy or low-confidence.
pub fn should_escalate(policy: VlmEscalationPolicy, discrepant: bool, used: u32, budget: u32) -> bool {
    if used >= budget {
        return false; // R2 budget cap (escalate_budget)
    }
    match policy {
        VlmEscalationPolicy::Off => false, // build-stage / 保守 → never (§8)
        VlmEscalationPolicy::OnDiscrepancy => discrepant,
        VlmEscalationPolicy::Aggressive => true,
    }
}

/// Enforce the outbound gate for a VLM (cloud) escalation call. Returns Ok(()) when the
/// region crop may leave the device; Err(gate reason) → caller degrades to local (R7).
/// VLM egress is tracked under OutboundKind::Llm (the existing cloud-LLM egress point).
///
/// NOTE(plan-recon 2026-06-10): the plan's Task 13 referenced OutboundPolicy fields
/// `local_destination` / `contains_l0` and an `OutboundError::L0CloudBlocked` variant that
/// do NOT exist in the real outbound_gate.rs API. The real `OutboundPolicy` is
/// { kind, enabled, vault_unlocked, redactor } and errors are { Disabled, VaultLocked,
/// RedactorRequired }. We adapt to the real API while preserving R7's intent (VLM egress
/// passes through the REAL, non-no-op gate): the caller maps privacy settings to `enabled`
/// (a local-only / 保守-governor destination sets enabled=false to refuse cloud egress),
/// supplies a Redactor so PII is stripped before the crop descriptor leaves, and the gate
/// fails closed (RedactorRequired) when a non-empty descriptor lacks a redactor.
pub fn gate_vlm_egress(
    enabled: bool,
    vault_unlocked: bool,
    redactor: Option<&Redactor>,
    crop_descriptor: &str,
) -> std::result::Result<String, OutboundError> {
    let policy = OutboundPolicy {
        kind: OutboundKind::Llm,
        enabled,
        vault_unlocked,
        redactor,
    };
    OutboundGate::enforce(&policy, crop_descriptor)
}

/// One telemetry record per VLM call attempt (spec §7 / §4.5 F).
#[derive(Debug, Clone, PartialEq)]
pub struct VlmTelemetry {
    pub region_kind: RegionKind,
    pub vlm_model: String,
    pub error_kind: Option<String>, // None = success; "parse"/"provider"
    pub retry_count: u32,
}

/// Build the schema-guided VQA question for a region kind (spec §3.2 / §4.5 A).
pub fn escalation_prompt(kind: RegionKind) -> &'static str {
    match kind {
        RegionKind::Chart => "Extract the chart as JSON: {\"chart_type\":\"bar|line|pie\",\"series\":[{\"name\":\"...\",\"values\":[..]}],\"axis_labels\":[..]}. Reply with ONLY the JSON object.",
        RegionKind::Formula => "Transcribe the formula to LaTeX as JSON: {\"latex\":\"...\"}. Reply with ONLY the JSON object.",
        RegionKind::Handwriting => "Transcribe the handwriting as JSON: {\"text\":\"...\"}. Reply with ONLY the JSON object.",
        RegionKind::Figure => "Caption the figure as JSON: {\"caption\":\"...\"}. Reply with ONLY the JSON object.",
        RegionKind::Stamp => "Read the stamp as JSON: {\"text\":\"...\",\"stamp_type\":\"...\"}. Reply with ONLY the JSON object.",
        _ => "Describe this region as JSON: {\"text\":\"...\"}. Reply with ONLY the JSON object.",
    }
}

/// Validate + parse the VLM JSON answer into a RegionResult. Err on invalid JSON
/// or missing required field (drives the retry loop). Never fabricates.
pub fn parse_vlm_answer(kind: RegionKind, answer: &str) -> Result<RegionResult> {
    let json = extract_json(answer)
        .ok_or_else(|| VaultError::Io(std::io::Error::other("vlm-parse-fail: no json object")))?;
    let v: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| VaultError::Io(std::io::Error::other(format!("vlm-parse-fail: {e}"))))?;
    Ok(match kind {
        RegionKind::Formula => RegionResult::FormulaV1 {
            latex: v.get("latex").and_then(|x| x.as_str()).map(str::to_string),
            raw_ocr: None,
        },
        RegionKind::Handwriting => RegionResult::HandwritingV1 {
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
        },
        RegionKind::Figure => RegionResult::FigureV1 {
            class: "figure".into(),
            caption: v.get("caption").and_then(|x| x.as_str()).map(str::to_string),
        },
        RegionKind::Chart => RegionResult::ChartV1 {
            chart_type: v
                .get("chart_type")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string(),
            series: v.get("series").and_then(parse_series).unwrap_or_default(),
            axis_labels: v
                .get("axis_labels")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        },
        RegionKind::Stamp => RegionResult::StampV1 {
            present: true,
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
            stamp_type: v
                .get("stamp_type")
                .and_then(|x| x.as_str())
                .map(str::to_string),
        },
        _ => RegionResult::UnrecognizedV1 {
            reason: "vlm-unsupported-kind".into(),
        },
    })
}

fn parse_series(v: &serde_json::Value) -> Option<Vec<Series>> {
    Some(
        v.as_array()?
            .iter()
            .filter_map(|s| {
                Some(Series {
                    name: s.get("name")?.as_str()?.to_string(),
                    values: s
                        .get("values")?
                        .as_array()?
                        .iter()
                        .filter_map(|x| x.as_f64())
                        .collect(),
                })
            })
            .collect(),
    )
}

/// Extract the first {...} JSON object from a possibly-chatty answer.
fn extract_json(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(s[start..=end].to_string())
    } else {
        None
    }
}

/// Escalate one region to VLM. Retries up to MAX_RETRIES on parse failure, feeding the
/// validator error back into the question (spec §4.5 B). Returns (result, telemetry).
pub fn escalate_region(
    vlm: &dyn VlmProvider,
    region_crop_path: &Path,
    kind: RegionKind,
    model_name: &str,
) -> (Result<RegionResult>, VlmTelemetry) {
    let base = escalation_prompt(kind);
    let mut last_err = String::new();
    for attempt in 0..MAX_RETRIES {
        let q = if attempt == 0 {
            base.to_string()
        } else {
            format!("{base}\nPrevious answer was invalid: {last_err}. Return ONLY valid JSON.")
        };
        match vlm.vqa(region_crop_path, &q) {
            Ok(answer) => match parse_vlm_answer(kind, &answer) {
                Ok(r) => {
                    return (
                        Ok(r),
                        VlmTelemetry {
                            region_kind: kind,
                            vlm_model: model_name.into(),
                            error_kind: None,
                            retry_count: attempt,
                        },
                    )
                }
                Err(e) => last_err = e.to_string(),
            },
            Err(e) => {
                return (
                    Err(e),
                    VlmTelemetry {
                        region_kind: kind,
                        vlm_model: model_name.into(),
                        error_kind: Some("provider".into()),
                        retry_count: attempt,
                    },
                )
            }
        }
    }
    (
        Err(VaultError::Io(std::io::Error::other(
            "vlm-parse-fail after retries",
        ))),
        VlmTelemetry {
            region_kind: kind,
            vlm_model: model_name.into(),
            error_kind: Some("parse".into()),
            retry_count: MAX_RETRIES,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vlm::VlmProvider;
    use std::sync::Mutex;

    /// VLM mock that returns scripted answers in order (to drive retry).
    /// Uses Mutex (not RefCell) because VlmProvider requires Send + Sync.
    struct ScriptVlm {
        answers: Mutex<Vec<String>>,
    }
    impl VlmProvider for ScriptVlm {
        fn caption(&self, _: &Path) -> Result<String> {
            Ok("x".into())
        }
        fn vqa(&self, _: &Path, _: &str) -> Result<String> {
            Ok(self.answers.lock().unwrap().pop().unwrap_or_default())
        }
    }
    // ScriptVlm pops from the end → push in reverse order of expected calls.
    fn script(a: &[&str]) -> ScriptVlm {
        ScriptVlm {
            answers: Mutex::new(a.iter().rev().map(|s| s.to_string()).collect()),
        }
    }

    #[test]
    fn parse_valid_formula_json() {
        let r = parse_vlm_answer(RegionKind::Formula, r#"{"latex":"E=mc^2"}"#).unwrap();
        assert!(matches!(r, RegionResult::FormulaV1 { latex: Some(_), .. }));
    }
    #[test]
    fn parse_chatty_answer_extracts_json() {
        let r = parse_vlm_answer(RegionKind::Handwriting, r#"Sure! {"text":"hello"} done"#).unwrap();
        match r {
            RegionResult::HandwritingV1 { text } => assert_eq!(text.as_deref(), Some("hello")),
            _ => panic!(),
        }
    }
    #[test]
    fn invalid_json_errs() {
        assert!(parse_vlm_answer(RegionKind::Formula, "not json").is_err());
    }
    #[test]
    fn first_try_success_zero_retries() {
        let vlm = script(&[r#"{"text":"ok"}"#]);
        let (res, tel) = escalate_region(&vlm, Path::new("/x.png"), RegionKind::Handwriting, "qwen-vl");
        assert!(res.is_ok());
        assert_eq!(tel.retry_count, 0);
        assert_eq!(tel.error_kind, None);
    }
    #[test]
    fn retries_then_succeeds() {
        let vlm = script(&["garbage", r#"{"text":"ok"}"#]);
        let (res, tel) = escalate_region(&vlm, Path::new("/x.png"), RegionKind::Handwriting, "qwen-vl");
        assert!(res.is_ok());
        assert_eq!(tel.retry_count, 1);
    }
    #[test]
    fn three_failures_gives_parse_fail_telemetry() {
        let vlm = script(&["bad1", "bad2", "bad3", "bad4"]);
        let (res, tel) = escalate_region(&vlm, Path::new("/x.png"), RegionKind::Handwriting, "qwen-vl");
        assert!(res.is_err());
        assert_eq!(tel.retry_count, MAX_RETRIES);
        assert_eq!(tel.error_kind.as_deref(), Some("parse"));
    }

    #[test]
    fn off_policy_never_escalates() {
        assert!(!should_escalate(VlmEscalationPolicy::Off, true, 0, 100));
    }
    #[test]
    fn on_discrepancy_only_when_flagged() {
        assert!(should_escalate(VlmEscalationPolicy::OnDiscrepancy, true, 0, 8));
        assert!(!should_escalate(VlmEscalationPolicy::OnDiscrepancy, false, 0, 8));
    }
    #[test]
    fn budget_cap_blocks_when_exhausted() {
        assert!(!should_escalate(VlmEscalationPolicy::Aggressive, true, 8, 8));
        assert!(should_escalate(VlmEscalationPolicy::Aggressive, true, 7, 8));
    }

    // gate_vlm_egress tests use the REAL OutboundGate API (Disabled / VaultLocked /
    // RedactorRequired), not the plan's hypothetical L0CloudBlocked. A local-only /
    // 保守-governor destination is modeled by enabled=false (refuse cloud egress).
    #[test]
    fn gate_blocks_when_disabled() {
        // enabled=false models a local-only / privacy-off destination → refuse cloud egress.
        let err = gate_vlm_egress(false, true, None, "region-crop").unwrap_err();
        assert!(matches!(err, OutboundError::Disabled(OutboundKind::Llm)));
    }
    #[test]
    fn gate_blocks_when_vault_locked() {
        let err = gate_vlm_egress(true, false, None, "region-crop").unwrap_err();
        assert!(matches!(err, OutboundError::VaultLocked));
    }
    #[test]
    fn gate_fails_closed_without_redactor_on_nonempty() {
        // R7 / MEMORY P0: gate is non-no-op — a non-empty payload with no redactor fails closed.
        let err = gate_vlm_egress(true, true, None, "sensitive crop descriptor").unwrap_err();
        assert!(matches!(err, OutboundError::RedactorRequired));
    }
    #[test]
    fn gate_allows_with_redactor_and_redacts() {
        let redactor = Redactor::new();
        // PII in the descriptor is stripped before it leaves the device.
        let out = gate_vlm_egress(true, true, Some(&redactor), "联系电话 13800138000").unwrap();
        assert!(!out.contains("13800138000"), "phone must be redacted; got {out}");
    }
}
