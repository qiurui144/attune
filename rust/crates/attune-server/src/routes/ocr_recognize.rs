//! POST /api/v1/ocr/recognize + report/accept (spec §5.1). Office-helper semantics:
//! result is NOT auto-written to vault — user must explicitly accept (spec §2.2/§7).
//!
//! Gated behind the `nontext` feature (forwards to attune-core/nontext). When the
//! layout/recognizer models are missing the pass degrades to empty regions (never 500).

use crate::state::SharedState;
use attune_core::ocr::nontext::{EngineStatus, OcrCorrectionReport, Region};
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Request body for /api/v1/ocr/recognize (multipart file OR { item_id }).
#[derive(Debug, Deserialize, Default)]
pub struct RecognizeRequest {
    pub item_id: Option<String>,
    pub profile_id: Option<String>,
    pub kinds: Option<Vec<String>>,
    /// "off" | "on_discrepancy" | "aggressive"
    pub vlm_escalation: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OcrRecognizeResponse {
    pub regions: Vec<Region>,
    pub correction_report: OcrCorrectionReport,
    /// Per spec §8: surfaced cost summary for the UI.
    pub cost: RecognizeCost,
    /// HONEST engine status (I3 / C1): callers KNOW whether recognition is functional or a
    /// scaffold (no layout model bundled). Mirrors the core `EngineStatus`.
    pub engine_status: EngineStatus,
    /// The VLM escalation policy actually applied (I3: don't discard the parsed policy).
    /// Build-stage default is Off (never escalates, §8).
    pub vlm_escalation: attune_core::ocr::profile::VlmEscalationPolicy,
    /// Page-level warnings surfaced (e.g. a Stage1 inference error), empty on happy path.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub validation_warnings: Vec<String>,
    /// I4 (spec §5.1): the VLM failure-rate hint surfaced to the UI ("建议切高 tier"). This route
    /// runs Stage1-3 only (Stage4 VLM escalation is the caller's gated step), so when no VlmRouter
    /// telemetry has accumulated the hint is empty — we surface the CONTRACT field honestly rather
    /// than fabricate failure rates for calls that did not happen.
    #[serde(default)]
    pub vlm_hint: attune_core::ocr::nontext::vision_capability::VlmHint,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct RecognizeCost {
    pub local_regions: u32,
    pub escalated_regions: u32,
    // NOTE(I3): no `cache_hits` field. Escalation + a VLM result cache are NOT wired yet, so
    // reporting `cache_hits: 0` would fabricate a metric for a path that does not run. We omit
    // it entirely until the cache exists rather than emit a misleading always-zero number.
}

/// Map the profile vlm_escalation string → typed policy (defaults Off, §8 build-stage-safe).
pub fn parse_escalation(s: Option<&str>) -> attune_core::ocr::profile::VlmEscalationPolicy {
    use attune_core::ocr::profile::VlmEscalationPolicy::*;
    match s {
        Some("aggressive") => Aggressive,
        Some("on_discrepancy") => OnDiscrepancy,
        _ => Off,
    }
}

fn err(code: &str, msg: &str, status: StatusCode) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "error": msg, "code": code })),
    )
}

type RouteResult<T> = Result<T, (StatusCode, Json<serde_json::Value>)>;
const OCR_RECOGNIZE_SCHEDULER_TASK: &str = "kb.document.ocr_recognize";
const OCR_RECOGNIZE_TIMEOUT: Duration = Duration::from_secs(120);

/// POST /api/v1/ocr/recognize — sync, multipart/form-data (file + optional profile/kinds/vlm).
/// Runs Stage1 layout → Stage2 local recognizers → Stage3 cross-validate. VLM escalation
/// (Stage4) is gated by the profile's vlm_escalation; build-stage default Off never escalates.
/// Models missing → regions degrade to empty (200, never 500).
pub async fn post_recognize(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> RouteResult<Json<OcrRecognizeResponse>> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut profile_id: Option<String> = None;
    let mut vlm_escalation: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        err(
            "invalid-input",
            &format!("multipart parse: {e}"),
            StatusCode::BAD_REQUEST,
        )
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                let bytes = field.bytes().await.map_err(|e| {
                    err(
                        "invalid-input",
                        &format!("file read: {e}"),
                        StatusCode::BAD_REQUEST,
                    )
                })?;
                file_bytes = Some(bytes.to_vec());
            }
            "profile" | "profile_id" => profile_id = Some(field.text().await.unwrap_or_default()),
            "vlm_escalation" => vlm_escalation = Some(field.text().await.unwrap_or_default()),
            _ => {}
        }
    }

    let bytes =
        file_bytes.ok_or_else(|| err("invalid-input", "file required", StatusCode::BAD_REQUEST))?;
    if bytes.is_empty() {
        return Err(err("empty-file", "file is empty", StatusCode::BAD_REQUEST));
    }
    // vlm_escalation is parsed for policy (build-stage Off never escalates, §8). I3: the
    // policy is no longer discarded — it is threaded into run_recognize and echoed honestly.
    let policy = parse_escalation(vlm_escalation.as_deref());

    let outputs = match crate::scheduler_tasks::submit_kb_task_final(
        &state,
        OCR_RECOGNIZE_SCHEDULER_TASK,
        serde_json::json!({
            "profile_id": profile_id,
            "vlm_escalation": vlm_escalation,
            "file_base64": BASE64_STANDARD.encode(&bytes),
            "timeout_ms": OCR_RECOGNIZE_TIMEOUT.as_millis() as u64,
            "ttl_ms": OCR_RECOGNIZE_TIMEOUT.as_millis() as u64,
        }),
        true,
        OCR_RECOGNIZE_TIMEOUT,
    )
    .await
    {
        Ok(outputs) => outputs,
        Err(e) => {
            let response =
                empty_recognize_response(policy, vec![format!("local-scheduler-ocr-error: {e}")]);
            return Ok(Json(response));
        }
    };

    let response = recognize_response_from_scheduler_outputs(&outputs, policy);
    Ok(Json(response))
}

fn recognize_response_from_scheduler_outputs(
    outputs: &serde_json::Value,
    policy: attune_core::ocr::profile::VlmEscalationPolicy,
) -> OcrRecognizeResponse {
    let regions = parse_regions(outputs);
    let correction_report = parse_correction_report(outputs).unwrap_or_else(empty_report);
    let local_regions = outputs
        .get("local_regions")
        .and_then(|v| v.as_u64())
        .unwrap_or(regions.len() as u64) as u32;
    let escalated_regions = outputs
        .get("escalated_regions")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let engine_status = outputs
        .get("engine_status")
        .or_else(|| outputs.get("engine-status"))
        .cloned()
        .and_then(|v| serde_json::from_value::<EngineStatus>(v).ok())
        .unwrap_or(if regions.is_empty() {
            EngineStatus::ScaffoldNoLayoutModel
        } else {
            EngineStatus::Functional
        });
    let validation_warnings = outputs
        .get("validation_warnings")
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default();
    OcrRecognizeResponse {
        regions,
        correction_report,
        cost: RecognizeCost {
            local_regions,
            escalated_regions,
        },
        engine_status,
        vlm_escalation: policy,
        validation_warnings,
        vlm_hint: Default::default(),
    }
}

fn parse_regions(outputs: &serde_json::Value) -> Vec<Region> {
    for pointer in ["/regions", "/outputs/regions", "/result/regions", "/data/regions"] {
        if let Some(value) = outputs.pointer(pointer) {
            if let Ok(regions) = serde_json::from_value::<Vec<Region>>(value.clone()) {
                return regions;
            }
        }
    }
    Vec::new()
}

fn parse_correction_report(outputs: &serde_json::Value) -> Option<OcrCorrectionReport> {
    for pointer in [
        "/correction_report",
        "/outputs/correction_report",
        "/result/correction_report",
        "/data/correction_report",
    ] {
        if let Some(value) = outputs.pointer(pointer) {
            if let Ok(report) = serde_json::from_value::<OcrCorrectionReport>(value.clone()) {
                return Some(report);
            }
        }
    }
    None
}

fn empty_recognize_response(
    policy: attune_core::ocr::profile::VlmEscalationPolicy,
    validation_warnings: Vec<String>,
) -> OcrRecognizeResponse {
    OcrRecognizeResponse {
        regions: vec![],
        correction_report: empty_report(),
        cost: RecognizeCost::default(),
        engine_status: EngineStatus::ScaffoldNoLayoutModel,
        vlm_escalation: policy,
        validation_warnings,
        vlm_hint: Default::default(),
    }
}

fn empty_report() -> OcrCorrectionReport {
    OcrCorrectionReport {
        schema_version: 1,
        entries: vec![],
        summary: Default::default(),
    }
}

/// GET /api/v1/ocr/recognize/{item_id}/report — fetch a stored correction report.
/// Regions/reports are runtime products (not persisted in items, spec §10.3); without a
/// sidecar store this returns an empty report for the item (lazy-recompute is the entry above).
pub async fn get_report(
    State(_state): State<SharedState>,
    Path(_item_id): Path<String>,
) -> RouteResult<Json<OcrCorrectionReport>> {
    Ok(Json(OcrCorrectionReport {
        schema_version: 1,
        entries: vec![],
        summary: Default::default(),
    }))
}

/// POST /api/v1/ocr/recognize/{item_id}/accept — user explicitly accepts corrections.
/// Office-helper semantics: nothing is written until this is called (spec §2.2/§7).
pub async fn accept(
    State(_state): State<SharedState>,
    Path(_item_id): Path<String>,
) -> RouteResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "accepted": 0, "status": "ok" })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escalation_defaults_off() {
        use attune_core::ocr::profile::VlmEscalationPolicy;
        assert_eq!(parse_escalation(None), VlmEscalationPolicy::Off);
        assert_eq!(parse_escalation(Some("garbage")), VlmEscalationPolicy::Off);
        assert_eq!(
            parse_escalation(Some("aggressive")),
            VlmEscalationPolicy::Aggressive
        );
        assert_eq!(
            parse_escalation(Some("on_discrepancy")),
            VlmEscalationPolicy::OnDiscrepancy
        );
    }

    #[test]
    fn response_serializes_with_cost() {
        use attune_core::ocr::profile::VlmEscalationPolicy;
        let resp = OcrRecognizeResponse {
            regions: vec![],
            correction_report: OcrCorrectionReport {
                schema_version: 1,
                entries: vec![],
                summary: attune_core::ocr::nontext::CorrectionSummary::default(),
            },
            cost: RecognizeCost {
                local_regions: 3,
                escalated_regions: 1,
            },
            engine_status: EngineStatus::Functional,
            vlm_escalation: VlmEscalationPolicy::Off,
            validation_warnings: vec![],
            vlm_hint: Default::default(),
        };
        let j = serde_json::to_string(&resp).unwrap();
        assert!(j.contains(r#""local_regions":3"#));
        assert!(j.contains(r#""schema_version":1"#));
        // I3: no fabricated cache_hits field in the serialized response.
        assert!(
            !j.contains("cache_hits"),
            "cache_hits must not be fabricated; got {j}"
        );
        // I3: the applied policy is echoed honestly.
        assert!(j.contains(r#""vlm_escalation":"off""#));
    }

    #[test]
    fn no_models_degrades_to_empty_regions() {
        // Scheduler unavailable / no OCR output → empty (never panics, never 500 on this route).
        // I3/C1: response honestly reports the scaffold status + echoes the applied policy.
        use attune_core::ocr::profile::VlmEscalationPolicy;
        let resp = empty_recognize_response(
            VlmEscalationPolicy::OnDiscrepancy,
            vec!["scheduler unavailable".to_string()],
        );
        assert!(resp.regions.is_empty());
        assert_eq!(resp.cost.local_regions, 0);
        assert_eq!(resp.correction_report.summary.total, 0);
        assert_eq!(resp.engine_status, EngineStatus::ScaffoldNoLayoutModel);
        assert_eq!(resp.vlm_escalation, VlmEscalationPolicy::OnDiscrepancy);
        assert_eq!(resp.validation_warnings.len(), 1);
    }
}
