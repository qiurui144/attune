//! Document Intelligence routes (spec §5, T-07) + member-gate & secret-no-leak (T-08, SECURITY).
//!
//! Three endpoints under `/api/v1/documents/` (kebab, OPT-5):
//!   - `POST /compare`   — document comparison (semantic mode member-gated)
//!   - `POST /summarize` — flagship token-thrift deep summary (member-gated)
//!   - `POST /chapters`  — chapter list (free) / summarize|ask (member-gated)
//!
//! **§3.5 Output-Mode Contract**: every response is the unified envelope
//! `{ output_mode, result, annotations?, narrative?, token_bill }`. The default mode is
//! per-capability (compare=marked / summarize=narrative / chapters=review); a caller may
//! request `output_mode` in the body to override.
//!
//! **T-08 member-gate (high-risk, G1-flagged)**: tier-3 LLM operations
//! (`compare mode=semantic`, `summarize`, `chapters action∈{summarize,ask}`) are gated at the
//! handler entry on `MemberState::is_paid()` → `403 { code: "membership-required" }`. The gate
//! is NOT UI-only: a direct request from any client is rejected the same way.
//!
//! **T-08 secret no-leak (high-risk, G1-flagged, CLAUDE.md §1.4)**: the new-api/gateway token
//! is a credential. It is never placed into a request prompt, never serialized into the
//! response envelope, and never logged. `token_bill` carries only counts/USD/model-names — no
//! credential field (enforced structurally in `token_bill.rs::test_no_secret_field_only_counts`
//! and behaviourally in `tests/documents_member_gate.rs`).

use crate::error::AppError;
use crate::state::SharedState;
use attune_core::document_intelligence::chapters::{self, ChapterReadResult};
use attune_core::document_intelligence::compare::{self, CompareMode, DiffReport};
use attune_core::document_intelligence::deep_summary::{self, DeepSummaryConfig, Summary, SummaryLevel};
use attune_core::document_intelligence::model_routing::ModelRouter;
use attune_core::document_intelligence::token_bill::TokenBill;
use attune_core::llm::LlmProvider;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

type AppResult<T> = std::result::Result<T, AppError>;

/// Spec §7 stable error codes (kebab), emitted via `AppError::detailed` so the wire `code`
/// matches the spec table exactly (the generic `AppError::Forbidden` would emit `forbidden`).
fn doc_err(status: axum::http::StatusCode, code: &str, msg: &str) -> AppError {
    AppError::detailed(status, json!({ "error": msg, "code": code }))
}

fn membership_required() -> AppError {
    doc_err(
        axum::http::StatusCode::FORBIDDEN,
        "membership-required",
        "this operation requires a paid membership",
    )
}
fn invalid_input(msg: &str) -> AppError {
    doc_err(axum::http::StatusCode::BAD_REQUEST, "invalid-input", msg)
}
fn item_not_found() -> AppError {
    doc_err(axum::http::StatusCode::NOT_FOUND, "item-not-found", "document not found")
}
fn document_too_large() -> AppError {
    doc_err(
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        "document-too-large",
        "document exceeds the maximum size",
    )
}
fn llm_unavailable() -> AppError {
    doc_err(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "llm-unavailable",
        "no LLM provider is configured",
    )
}
fn vault_locked() -> AppError {
    doc_err(axum::http::StatusCode::UNAUTHORIZED, "vault-locked", "vault is locked")
}

/// Hard upper bound on input chars (defends against OOM; spec §7 document-too-large).
const MAX_DOC_CHARS: usize = 2_000_000;

// ─────────────────────────── request payloads (spec §5) ───────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocRef {
    pub item_id: Option<String>,
    pub text: Option<String>,
    #[allow(dead_code)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareRequest {
    pub left: DocRef,
    pub right: DocRef,
    #[serde(default = "default_compare_mode")]
    pub mode: String,
    /// §3.5 — caller may request the output mode; default per-capability (marked).
    pub output_mode: Option<String>,
}
fn default_compare_mode() -> String {
    "semantic".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummarizeRequest {
    pub source: DocRef,
    #[serde(default = "default_level")]
    pub level: String,
    pub output_mode: Option<String>,
}
fn default_level() -> String {
    "standard".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaptersRequest {
    pub item_id: Option<String>,
    pub text: Option<String>,
    pub action: String,
    pub chapter_idx: Option<usize>,
    pub question: Option<String>,
    pub output_mode: Option<String>,
}

// ─────────────────────────── §3.5 unified envelope ───────────────────────────

/// The §3.5 response envelope. `result` is the capability-specific structured payload;
/// `annotations`/`narrative` are populated per the active output mode.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocEnvelope {
    pub output_mode: String,
    pub result: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
    pub token_bill: TokenBill,
}

// ─────────────────────────── gate (T-08) — pure, unit-testable ───────────────────────────

/// Whether an operation is a tier-3 (member-gated) LLM op. Pure so the gate decision is
/// testable without a server boot (T-08 adversarial / 3-state).
pub fn is_tier3_compare(mode: &str) -> bool {
    mode.eq_ignore_ascii_case("semantic")
}
pub fn is_tier3_chapters(action: &str) -> bool {
    matches!(action, "summarize" | "ask")
}

/// Enforce the member gate: returns Err(membership_required) iff the op is tier-3 and !paid.
/// Free-tier ops (compare structural|textual, chapters list) pass regardless of login.
fn enforce_gate(is_tier3: bool, is_paid: bool) -> AppResult<()> {
    if is_tier3 && !is_paid {
        return Err(membership_required());
    }
    Ok(())
}

// ─────────────────────────── shared helpers ───────────────────────────

/// Resolve a DocRef to text: prefer inline `text`, else load the item by id (decrypt with the
/// vault DEK). `vault-locked` if the vault is not unlocked; `item-not-found` if absent.
fn resolve_doc(state: &SharedState, item_id: &Option<String>, text: &Option<String>) -> AppResult<String> {
    if let Some(t) = text {
        if t.chars().count() > MAX_DOC_CHARS {
            return Err(document_too_large());
        }
        return Ok(t.clone());
    }
    let id = item_id.as_deref().ok_or_else(|| invalid_input("either text or item_id is required"))?;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let item = vault
        .store()
        .get_item(&dek, id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(item_not_found)?;
    if item.content.chars().count() > MAX_DOC_CHARS {
        return Err(document_too_large());
    }
    Ok(item.content)
}

/// The router + LLM handle the route uses. The per-stage routing DECISION is client-side
/// (spec §8.2): the router maps roles → logical model names. In the degenerate server config
/// (single configured provider) both Cheap and Reasoning resolve to the same provider handle —
/// per-stage cheap/reasoning separation is unit-proven with `RecordingMockLlm` in
/// attune-core; the route's job is to wire + gate + shape the envelope.
fn router_from_state(state: &SharedState) -> ModelRouter {
    // Read the model_routing block from app_settings if present; else degenerate fallback.
    let settings = {
        let bytes = match state.vault.lock() {
            Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
            Err(_) => None,
        };
        bytes
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or_else(|| json!({}))
    };
    ModelRouter::from_settings(&settings)
}

fn llm_or_503(state: &SharedState) -> AppResult<Arc<dyn LlmProvider>> {
    state.llm().ok_or_else(llm_unavailable)
}

fn is_paid(state: &SharedState) -> bool {
    state.member_state.lock().map(|g| g.is_paid()).unwrap_or(false)
}

fn parse_level(s: &str) -> AppResult<SummaryLevel> {
    match s {
        "brief" => Ok(SummaryLevel::Brief),
        "standard" => Ok(SummaryLevel::Standard),
        "detailed" => Ok(SummaryLevel::Detailed),
        other => Err(invalid_input(&format!("invalid level: {other}"))),
    }
}

fn parse_compare_mode(s: &str) -> AppResult<CompareMode> {
    match s {
        "structural" => Ok(CompareMode::Structural),
        "textual" => Ok(CompareMode::Textual),
        "semantic" => Ok(CompareMode::Semantic),
        other => Err(invalid_input(&format!("invalid mode: {other}"))),
    }
}

// ─────────────────────────── handlers (T-07) ───────────────────────────

/// POST /api/v1/documents/compare
pub async fn compare_docs(
    State(state): State<SharedState>,
    Json(body): Json<CompareRequest>,
) -> AppResult<Json<DocEnvelope>> {
    let mode = parse_compare_mode(&body.mode)?;
    // T-08 gate: semantic compare is tier-3.
    enforce_gate(is_tier3_compare(&body.mode), is_paid(&state))?;

    let left = resolve_doc(&state, &body.left.item_id, &body.left.text)?;
    let right = resolve_doc(&state, &body.right.item_id, &body.right.text)?;

    // §3.5 default = marked; structured override accepted.
    let output_mode = match body.output_mode.as_deref() {
        Some("structured") => compare::OutputMode::Structured,
        _ => compare::OutputMode::Marked,
    };

    let router = router_from_state(&state);
    let llm = llm_or_503(&state)?;
    let llms = compare::StageLlms { cheap: llm.as_ref(), reasoning: llm.as_ref() };

    let report: DiffReport = compare::compare(
        &left,
        &right,
        mode,
        output_mode,
        is_paid(&state),
        &router,
        &llms,
    )
    .map_err(map_core_err)?;

    let token_bill = report.token_bill.clone();
    let annotations = if report.output_mode == "marked" {
        Some(serde_json::to_value(&report.annotations).unwrap_or(Value::Null))
    } else {
        None
    };
    Ok(Json(DocEnvelope {
        output_mode: report.output_mode.clone(),
        result: serde_json::to_value(&report).unwrap_or(Value::Null),
        annotations,
        narrative: report.summary.clone(),
        token_bill,
    }))
}

/// POST /api/v1/documents/summarize (flagship)
pub async fn summarize_doc(
    State(state): State<SharedState>,
    Json(body): Json<SummarizeRequest>,
) -> AppResult<Json<DocEnvelope>> {
    // T-08 gate: summarize is always tier-3.
    enforce_gate(true, is_paid(&state))?;
    let level = parse_level(&body.level)?;
    let full_text = resolve_doc(&state, &body.source.item_id, &body.source.text)?;
    if full_text.trim().is_empty() {
        return Err(doc_err(
            axum::http::StatusCode::BAD_REQUEST,
            "empty-document",
            "document is empty",
        ));
    }
    let item_id = body.source.item_id.clone().unwrap_or_default();

    let router = router_from_state(&state);
    let llm = llm_or_503(&state)?;
    let llms = deep_summary::StageLlms { cheap: llm.as_ref(), reasoning: llm.as_ref() };

    // store + dek for the cache layer (only when a real item_id was given).
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let cfg = DeepSummaryConfig::default();
    let (summary, bill): (Summary, TokenBill) =
        deep_summary::summarize(&full_text, level, &item_id, &router, &llms, vault.store(), &dek, &cfg)
            .map_err(map_core_err)?;
    drop(vault);

    // §3.5 default for summarize = narrative.
    let structured = matches!(body.output_mode.as_deref(), Some("structured"));
    let narrative = if structured { None } else { Some(render_narrative(&summary)) };
    Ok(Json(DocEnvelope {
        output_mode: if structured { "structured".into() } else { "narrative".into() },
        result: serde_json::to_value(&summary).unwrap_or(Value::Null),
        annotations: None,
        narrative,
        token_bill: bill,
    }))
}

/// POST /api/v1/documents/chapters
pub async fn chapters_doc(
    State(state): State<SharedState>,
    Json(body): Json<ChaptersRequest>,
) -> AppResult<Json<DocEnvelope>> {
    // T-08 gate: summarize/ask are tier-3; list is free.
    enforce_gate(is_tier3_chapters(&body.action), is_paid(&state))?;
    let full_text = resolve_doc(&state, &body.item_id, &body.text)?;

    match body.action.as_str() {
        "list" => {
            let entries = chapters::list(&full_text, 0.5);
            Ok(Json(DocEnvelope {
                output_mode: "structured".into(),
                result: json!({ "chapters": entries }),
                annotations: None,
                narrative: None,
                token_bill: TokenBill::default(),
            }))
        }
        "summarize" | "ask" => {
            let idx = body.chapter_idx.ok_or_else(|| invalid_input("chapter_idx is required"))?;
            let chs = chapters::split_chapters(&full_text);
            let router = router_from_state(&state);
            let llm = llm_or_503(&state)?;
            let structured = matches!(body.output_mode.as_deref(), Some("structured"));
            let om = if structured { chapters::OutputMode::Structured } else { chapters::OutputMode::Review };

            let res: ChapterReadResult = if body.action == "summarize" {
                chapters::summarize_chapter(&chs, idx, om, llm.as_ref(), &router)
            } else {
                let q = body.question.as_deref().ok_or_else(|| invalid_input("question is required for ask"))?;
                chapters::ask(&chs, idx, q, om, llm.as_ref(), &router)
            }
            .map_err(map_core_err)?;

            let annotations = if res.output_mode == "review" {
                let mut all = res.annotations.clone();
                all.extend(res.citations.clone());
                Some(serde_json::to_value(&all).unwrap_or(Value::Null))
            } else {
                None
            };
            let token_bill = res.token_bill.clone();
            Ok(Json(DocEnvelope {
                output_mode: res.output_mode.clone(),
                result: serde_json::to_value(&res).unwrap_or(Value::Null),
                annotations,
                narrative: None,
                token_bill,
            }))
        }
        other => Err(invalid_input(&format!("invalid action: {other}"))),
    }
}

/// Render a summary as a layered narrative report (§3.5 narrative mode).
fn render_narrative(s: &Summary) -> String {
    let mut out = String::new();
    out.push_str(&s.overview);
    if !s.per_chapter.is_empty() {
        out.push_str("\n\n");
        for ch in &s.per_chapter {
            let h = if ch.heading_path.is_empty() { "(无标题)" } else { ch.heading_path.as_str() };
            out.push_str(&format!("• 【{h}】{}\n", ch.summary));
        }
    }
    out
}

/// Map an attune-core VaultError to the spec §7 route error codes.
fn map_core_err(e: attune_core::error::VaultError) -> AppError {
    use attune_core::error::VaultError;
    match e {
        VaultError::NotFound(_) => item_not_found(),
        VaultError::Locked => vault_locked(),
        VaultError::InvalidInput(m) => invalid_input(&m),
        other => AppError::Internal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_tier3_classification() {
        assert!(is_tier3_compare("semantic"));
        assert!(!is_tier3_compare("structural"));
        assert!(!is_tier3_compare("textual"));
        assert!(is_tier3_chapters("summarize"));
        assert!(is_tier3_chapters("ask"));
        assert!(!is_tier3_chapters("list"));
    }

    #[test]
    fn test_enforce_gate_unpaid_tier3_blocked() {
        // tier3 + unpaid → membership-required.
        let err = enforce_gate(true, false).unwrap_err();
        if let AppError::Detailed { status, body } = err {
            assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
            assert_eq!(body["code"], "membership-required");
        } else {
            panic!("expected Detailed membership-required");
        }
    }

    #[test]
    fn test_enforce_gate_paid_tier3_allowed() {
        assert!(enforce_gate(true, true).is_ok());
    }

    #[test]
    fn test_enforce_gate_free_op_always_allowed() {
        // non-tier3 (free) ops pass regardless of paid state.
        assert!(enforce_gate(false, false).is_ok());
        assert!(enforce_gate(false, true).is_ok());
    }

    #[test]
    fn test_error_codes_match_spec() {
        for (e, code) in [
            (membership_required(), "membership-required"),
            (item_not_found(), "item-not-found"),
            (invalid_input("x"), "invalid-input"),
            (document_too_large(), "document-too-large"),
            (llm_unavailable(), "llm-unavailable"),
            (vault_locked(), "vault-locked"),
        ] {
            if let AppError::Detailed { body, .. } = e {
                assert_eq!(body["code"], code, "spec §7 stable code");
            } else {
                panic!("expected Detailed");
            }
        }
    }

    #[test]
    fn test_envelope_skips_none_fields() {
        let env = DocEnvelope {
            output_mode: "narrative".into(),
            result: json!({"x": 1}),
            annotations: None,
            narrative: Some("报告".into()),
            token_bill: TokenBill::default(),
        };
        let js = serde_json::to_value(&env).unwrap();
        assert!(js.get("annotations").is_none(), "None annotations omitted");
        assert_eq!(js["narrative"], "报告");
        assert_eq!(js["outputMode"], "narrative");
    }
}
