//! I5 — agent-invocable shared visual-understanding capability (spec 2026-06-16 §I5 / §5.4;
//! ADR-0008 impl delta).
//!
//! ADR-0008 makes the visual-understanding pass the ONE shared core for the whole product family:
//! OSS attune owns it, and pro plugins (law / medical / patent) MUST consume it rather than carry
//! their own VLM. Until now that core was reachable only via REST (`/api/v1/ocr/recognize`) + CLI;
//! a plugin/agent had to self-call REST. This module adds the in-process, typed capability entry so
//! any caller (OSS or pro) gets the SAME `RecognizePageResult` (regions + grounding + cross-validation
//! report) plus the §I4 `vlm_hint` — without an HTTP round-trip and without re-implementing vision.
//!
//! ## Zero vertical binding (R8 / X-Cutting)
//!
//! The capability name is `vision.recognize` and its output is the generic `RegionResult` schema.
//! There is NO law/medical/patent vocabulary anywhere in this module — domain semantics (what a
//! given table/stamp means in a vertical) are layered by the CALLING plugin ON TOP of this output.
//! That is the ADR-0008 contract: the shared core stays generic; plugins interpret, never re-detect.
//!
//! ## Reuses existing machinery (R9)
//!
//! This is NOT a new agent framework. `recognize` simply funnels through the existing
//! [`super::recognize_page`] orchestrator (Stage1→2→3 local + cross-validate) and surfaces the
//! existing [`super::vlm_router::VlmRouter`] failure aggregation as the cost/quality hint. Stage4
//! (💰 VLM escalation) stays the caller's gated step, exactly as for REST/CLI.

use super::vlm_router::VlmRouter;
use super::RecognizePageResult;
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The capability identifier any plugin/agent invokes (spec §5.4). Stable wire string; a
/// `schema_version` rides on the result so a caller can fail-fast on mismatch.
pub const CAPABILITY_ID: &str = "vision.recognize";

/// Schema version of the capability output (spec §10.2). Bumped only on a breaking change to
/// `VisionRecognizeResult` / `RegionResult`; callers compare and fail-fast on mismatch.
pub const SCHEMA_VERSION: u32 = 1;

/// The §I4 / §5.1 telemetry hint surfaced alongside the recognition result: whether the UI should
/// suggest a higher model tier, and the per-`<kind>:<model>` failure rates that drove it. Derived
/// from the [`VlmRouter`] aggregation — purely advisory, never blocks a result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VlmHint {
    /// True if ANY (kind × model) failure rate is above the §4.5-F threshold.
    pub suggest_higher_tier: bool,
    /// `"<kind>:<model>"` → failure rate (only entries with at least one call).
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub kind_failure_rates: HashMap<String, f64>,
}

impl VlmHint {
    /// Build the hint from a router's current aggregation snapshot (spec §5.1).
    pub fn from_router(router: &VlmRouter) -> Self {
        Self {
            suggest_higher_tier: router.any_suggest_higher_tier(),
            kind_failure_rates: router.failure_rate_snapshot(),
        }
    }
}

/// The typed result every `vision.recognize` caller sees (spec §5.4): the shared
/// [`RecognizePageResult`] plus the advisory [`VlmHint`], tagged with the [`SCHEMA_VERSION`] so a
/// plugin can verify compatibility before consuming. Zero vertical fields by contract (R8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionRecognizeResult {
    pub schema_version: u32,
    #[serde(flatten)]
    pub page: RecognizePageResult,
    pub vlm_hint: VlmHint,
}

/// Agent-invocable shared visual-understanding entry (spec §5.4 / ADR-0008). Runs the shared local
/// recognition + cross-validation pass and attaches the current VLM failure hint. The optional
/// `router` lets the host surface the §I4 telemetry hint; `None` yields an empty (no-data) hint.
///
/// This is a synchronous, dependency-light funnel — the SAME one REST and CLI use — so a plugin gets
/// identical results in-process. Domain interpretation is the caller's job; this returns only the
/// generic schema.
pub fn recognize(
    image_path: &Path,
    layout_model: &Path,
    table_model: &Path,
    ocr_lines: &[RawLine],
    router: Option<&VlmRouter>,
) -> VisionRecognizeResult {
    let page = super::recognize_page(image_path, layout_model, table_model, ocr_lines);
    let vlm_hint = router.map(VlmHint::from_router).unwrap_or_default();
    VisionRecognizeResult {
        schema_version: SCHEMA_VERSION,
        page,
        vlm_hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::nontext::vlm_router::{VlmCandidate, VlmRouter};
    use crate::ocr::nontext::RegionKind;
    use crate::vlm::MockVlmProvider;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn nonexistent_models() -> (PathBuf, PathBuf) {
        // Models absent → recognize_page degrades to a scaffold result (no panic). Good enough to
        // assert the capability shape + hint wiring without a bundled ONNX model.
        (
            PathBuf::from("/nonexistent/layout.onnx"),
            PathBuf::from("/nonexistent/table.onnx"),
        )
    }

    #[test]
    fn recognize_returns_typed_result_with_schema_version() {
        let (layout, table) = nonexistent_models();
        let img = PathBuf::from("/nonexistent/page.png");
        let out = recognize(&img, &layout, &table, &[], None);
        assert_eq!(out.schema_version, SCHEMA_VERSION);
        // No router → empty hint (no telemetry data).
        assert!(!out.vlm_hint.suggest_higher_tier);
        assert!(out.vlm_hint.kind_failure_rates.is_empty());
    }

    #[test]
    fn capability_id_and_schema_are_stable_contract() {
        assert_eq!(CAPABILITY_ID, "vision.recognize");
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn vlm_hint_reflects_router_aggregation() {
        // A 1-candidate router with no calls → hint is empty / no suggestion.
        let router = VlmRouter::new(vec![VlmCandidate::new(
            Arc::new(MockVlmProvider),
            "qwen-3.6-flash",
            5,
        )]);
        let hint = VlmHint::from_router(&router);
        assert!(!hint.suggest_higher_tier);
        // The router's no-data state yields no per-kind entries (boundary: never divides by zero).
        assert!(hint.kind_failure_rates.is_empty());
        let _ = RegionKind::Chart; // kind enum is the generic vocabulary (no vertical terms).
    }

    #[test]
    fn result_serializes_with_flattened_page_and_hint() {
        let (layout, table) = nonexistent_models();
        let img = PathBuf::from("/nonexistent/page.png");
        let out = recognize(&img, &layout, &table, &[], None);
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["schema_version"], 1);
        // #[serde(flatten)] lifts RecognizePageResult fields to the top level (REST shape, §5.1).
        assert!(json.get("regions").is_some(), "page fields are flattened");
        assert!(json.get("vlm_hint").is_some());
    }
}
