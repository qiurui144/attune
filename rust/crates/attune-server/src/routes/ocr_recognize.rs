//! POST /api/v1/ocr/recognize + report/accept (spec §5.1). Office-helper semantics:
//! result is NOT auto-written to vault — user must explicitly accept (spec §2.2/§7).
//!
//! Gated behind the `nontext` feature (forwards to attune-core/nontext). When the
//! layout/recognizer models are missing the pass degrades to empty regions (never 500).

use crate::state::SharedState;
use attune_core::ocr::nontext::{recognize_page, OcrCorrectionReport, Region};
use attune_core::ocr::{self, RawLine};
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct RecognizeCost {
    pub local_regions: u32,
    pub escalated_regions: u32,
    pub cache_hits: u32,
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
    (status, Json(serde_json::json!({ "error": msg, "code": code })))
}

type RouteResult<T> = Result<T, (StatusCode, Json<serde_json::Value>)>;

/// POST /api/v1/ocr/recognize — sync, multipart/form-data (file + optional profile/kinds/vlm).
/// Runs Stage1 layout → Stage2 local recognizers → Stage3 cross-validate. VLM escalation
/// (Stage4) is gated by the profile's vlm_escalation; build-stage default Off never escalates.
/// Models missing → regions degrade to empty (200, never 500).
pub async fn post_recognize(
    State(_state): State<SharedState>,
    mut multipart: Multipart,
) -> RouteResult<Json<OcrRecognizeResponse>> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut profile_id: Option<String> = None;
    let mut vlm_escalation: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        err("invalid-input", &format!("multipart parse: {e}"), StatusCode::BAD_REQUEST)
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                let bytes = field.bytes().await.map_err(|e| {
                    err("invalid-input", &format!("file read: {e}"), StatusCode::BAD_REQUEST)
                })?;
                file_bytes = Some(bytes.to_vec());
            }
            "profile" | "profile_id" => profile_id = Some(field.text().await.unwrap_or_default()),
            "vlm_escalation" => vlm_escalation = Some(field.text().await.unwrap_or_default()),
            _ => {}
        }
    }

    let bytes = file_bytes
        .ok_or_else(|| err("invalid-input", "file required", StatusCode::BAD_REQUEST))?;
    if bytes.is_empty() {
        return Err(err("empty-file", "file is empty", StatusCode::BAD_REQUEST));
    }
    // vlm_escalation is parsed for policy (build-stage Off never escalates, §8).
    let _policy = parse_escalation(vlm_escalation.as_deref());

    // Write to a tmp file (PP-OCR + layout models accept a path).
    let tmp = tempfile::NamedTempFile::new()
        .map_err(|e| err("internal-error", &format!("tempfile: {e}"), StatusCode::INTERNAL_SERVER_ERROR))?;
    std::fs::write(tmp.path(), &bytes)
        .map_err(|e| err("internal-error", &format!("write tmp: {e}"), StatusCode::INTERNAL_SERVER_ERROR))?;

    let ocr_profile = ocr::profile_for_id(profile_id.as_deref());

    // Second opinion: PP-OCR lines for cross-validation (best-effort; missing engine → none).
    let ocr_lines: Vec<RawLine> = match ocr::detect_default_provider() {
        Some(provider) => provider
            .extract_structured(tmp.path(), &ocr_profile)
            .ok()
            .and_then(|o| o.lines)
            .unwrap_or_default(),
        None => Vec::new(),
    };

    // Shared visual-understanding capability — ONE orchestration path (ADR-0008);
    // CLI + plugins go through the same `recognize_page`.
    let response = run_recognize(tmp.path(), &ocr_lines);
    Ok(Json(response))
}

/// Run the shared recognition pass and shape it into the HTTP response. The layout/table
/// model paths follow the PP-OCR model dir convention; when absent the pass degrades to
/// empty regions (plain OCR). Cost is surfaced for the UI (spec §8).
fn run_recognize(image_path: &std::path::Path, ocr_lines: &[RawLine]) -> OcrRecognizeResponse {
    let models_dir = ocr::ppocr::PpOcrProvider::models_dir();
    let layout_model = models_dir.join("layout").join("layout.onnx");
    let table_model = models_dir.join("table").join("slanet.onnx");
    let out = recognize_page(image_path, &layout_model, &table_model, ocr_lines);
    OcrRecognizeResponse {
        regions: out.regions,
        correction_report: out.correction_report,
        cost: RecognizeCost {
            local_regions: out.local_regions,
            escalated_regions: out.escalated_regions,
            cache_hits: 0,
        },
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
        assert_eq!(parse_escalation(Some("aggressive")), VlmEscalationPolicy::Aggressive);
        assert_eq!(parse_escalation(Some("on_discrepancy")), VlmEscalationPolicy::OnDiscrepancy);
    }

    #[test]
    fn response_serializes_with_cost() {
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
                cache_hits: 0,
            },
        };
        let j = serde_json::to_string(&resp).unwrap();
        assert!(j.contains(r#""local_regions":3"#));
        assert!(j.contains(r#""schema_version":1"#));
    }

    #[test]
    fn no_models_degrades_to_empty_regions() {
        // run_recognize with no layout model present → empty (never panics, never 500).
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let resp = run_recognize(tmp.path(), &[]);
        assert!(resp.regions.is_empty());
        assert_eq!(resp.cost.local_regions, 0);
        assert_eq!(resp.correction_report.summary.total, 0);
    }
}
