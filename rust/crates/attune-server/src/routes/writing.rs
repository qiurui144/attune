//! Writing Engine routes (spec §5) — grounded narrative generation.
//!
//! Endpoints under `/api/v1/writing/` (kebab, OPT-5):
//!   - `POST /draft`   — W1: outline + KB material → grounded draft (tier-3, member-gated)
//!   - `POST /rewrite` — W2: rewrite/polish preserving facts (tier-3, member-gated)
//!
//! **Member gate (tier-3)**: generation is always 💰; both endpoints gate on
//! `MemberState::is_paid()` → `403 { code: "membership-required" }` (parity with doc-intel).
//! The gate is NOT UI-only — a direct request is rejected the same way.
//!
//! **Privacy (I1/I2)**: generation sends content to the cloud LLM, so it requires the user
//! to have enabled cloud-LLM egress (`privacy.llm`), and the provider is PII-redacted. The
//! writing engine itself redacts again via the hardened helper (defense in depth).
//!
//! **No secret leak (CLAUDE.md §1.4)**: `token_bill` carries only counts/USD/model-names.

use crate::error::AppError;
use crate::state::SharedState;
use attune_core::llm::LlmProvider;
use attune_core::writing::draft::{self, DraftRequest};
use attune_core::writing::outline::{self, OutlineResult};
use attune_core::writing::rewrite::{self, RewriteOutput, RewriteRequest};
use attune_core::writing::synthesis::{self, SynthLlms, SynthesisRequest, SynthesisStructure};
use attune_core::writing::templates::{fill_template, FillResult, TemplateRegistry};
use attune_core::writing::{
    build_citations, find_inline_anchors, Citation, CiteError, CiteStyle, InlineAnchor, SourceMaterial,
    SourceMeta, StyleTarget, WritingError, WritingResult,
};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

type AppResult<T> = std::result::Result<T, AppError>;

fn werr(status: axum::http::StatusCode, code: &str, msg: &str) -> AppError {
    AppError::detailed(status, json!({ "error": msg, "code": code }))
}

/// Map a [`WritingError`] to its stable HTTP status + kebab code (spec §7).
fn map_writing_err(e: WritingError) -> AppError {
    use axum::http::StatusCode;
    let status = match e {
        WritingError::NoSourceMaterial | WritingError::EmptyInput => StatusCode::BAD_REQUEST,
        WritingError::SourceInjection => StatusCode::BAD_REQUEST,
        WritingError::LlmUnavailable | WritingError::GenerationUnavailable(_) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
    };
    werr(status, e.code(), &e.to_string())
}

fn membership_required() -> AppError {
    werr(
        axum::http::StatusCode::FORBIDDEN,
        "membership-required",
        "this operation requires a paid membership",
    )
}
fn vault_locked() -> AppError {
    werr(axum::http::StatusCode::UNAUTHORIZED, "vault-locked", "vault is locked")
}
fn cloud_llm_disabled() -> AppError {
    werr(
        axum::http::StatusCode::FORBIDDEN,
        "cloud-llm-disabled",
        "cloud LLM is disabled in Privacy settings; enable it to use this operation",
    )
}
fn llm_unavailable() -> AppError {
    werr(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "llm-unavailable",
        "no LLM provider is configured",
    )
}

fn is_paid(state: &SharedState) -> bool {
    state.member_state.lock().map(|g| g.is_paid()).unwrap_or(false)
}

/// Generation is always tier-3 → enforce the member gate.
fn enforce_member_gate(state: &SharedState) -> AppResult<()> {
    if is_paid(state) {
        Ok(())
    } else {
        Err(membership_required())
    }
}

/// Has the user enabled cloud-LLM egress in Privacy settings? (parity with documents.rs I2).
fn cloud_llm_egress_enabled(state: &SharedState) -> bool {
    let bytes = match state.vault.lock() {
        Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
        Err(_) => None,
    };
    bytes
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|s| s.get("privacy").and_then(|p| p.get("llm")).and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Resolve the tier-3 cloud LLM provider, enforcing privacy egress (I2) + PII redact (I1).
fn cloud_llm_or_refuse(state: &SharedState) -> AppResult<Arc<dyn LlmProvider>> {
    if !cloud_llm_egress_enabled(state) {
        return Err(cloud_llm_disabled());
    }
    let inner = state.llm().ok_or_else(llm_unavailable)?;
    Ok(Arc::new(
        attune_core::redacting_llm::RedactingLlmProvider::with_default_redactor(inner),
    ))
}

/// Load KB item contents (decrypted) into [`SourceMaterial`]. Missing items are skipped (the
/// engine grounds against whatever it gets; a draft with zero resolvable items + empty outline
/// fails downstream with `no-source-material`).
fn load_sources(state: &SharedState, item_ids: &[String]) -> AppResult<Vec<SourceMaterial>> {
    if item_ids.is_empty() {
        return Ok(vec![]);
    }
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let mut out = Vec::new();
    for id in item_ids {
        if let Ok(Some(item)) = vault.store().get_item(&dek, id) {
            out.push(SourceMaterial::new(id.clone(), item.content));
        }
    }
    Ok(out)
}

// ─────────────────────────── request bodies ───────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftBody {
    #[serde(default)]
    pub outline: String,
    #[serde(default)]
    pub item_ids: Vec<String>,
    /// Inline external/user-supplied material (not from the vault).
    #[serde(default)]
    pub extra_sources: Vec<InlineSource>,
    #[serde(default)]
    pub tone: Option<String>,
    #[serde(default)]
    pub length: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub style: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineSource {
    /// Optional external reference id (empty ⇒ grounds as External kind).
    #[serde(default)]
    pub external_ref: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewriteBody {
    pub text: String,
    #[serde(default)]
    pub tone: Option<String>,
    #[serde(default)]
    pub length: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub style: Option<String>,
    /// `"narrative"` (default) or `"review"`.
    #[serde(default)]
    pub output_mode: Option<String>,
}

fn style_from(
    tone: Option<String>,
    length: Option<String>,
    audience: Option<String>,
    style: Option<String>,
) -> StyleTarget {
    StyleTarget {
        tone,
        length,
        audience,
        style,
    }
}

// ─────────────────────────── handlers ───────────────────────────

/// POST /api/v1/writing/draft (W1, tier-3)
pub async fn draft_writing(
    State(state): State<SharedState>,
    Json(body): Json<DraftBody>,
) -> AppResult<Json<WritingResult>> {
    enforce_member_gate(&state)?;
    let llm = cloud_llm_or_refuse(&state)?;

    let mut sources = load_sources(&state, &body.item_ids)?;
    for ex in &body.extra_sources {
        // External/user-supplied: empty item_id ⇒ engine grounds it as External kind.
        sources.push(SourceMaterial::new(ex.external_ref.clone(), ex.text.clone()));
    }

    let req = DraftRequest {
        outline: body.outline,
        sources,
        style: style_from(body.tone, body.length, body.audience, body.style),
        structured: true,
    };
    let result = draft::draft(llm.as_ref(), &req).map_err(map_writing_err)?;
    Ok(Json(result))
}

/// POST /api/v1/writing/rewrite (W2, tier-3)
pub async fn rewrite_writing(
    State(state): State<SharedState>,
    Json(body): Json<RewriteBody>,
) -> AppResult<Json<WritingResult>> {
    enforce_member_gate(&state)?;
    let llm = cloud_llm_or_refuse(&state)?;

    let output = match body.output_mode.as_deref() {
        Some("review") => RewriteOutput::Review,
        _ => RewriteOutput::Narrative,
    };
    let req = RewriteRequest {
        text: body.text,
        target: style_from(body.tone, body.length, body.audience, body.style),
        output,
    };
    let result = rewrite::rewrite(llm.as_ref(), &req).map_err(map_writing_err)?;
    Ok(Json(result))
}

// ─────────────────────────── W3 outline (spec §5.2 /outline) ───────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineBody {
    /// Forward: the topic. Reverse: ignored.
    #[serde(default)]
    pub topic: String,
    /// Forward: optional KB material item ids.
    #[serde(default)]
    pub item_ids: Vec<String>,
    /// Reverse mode: the existing draft text to extract structure from. When present, runs the
    /// zero-LLM reverse path (no member gate — reverse is free).
    #[serde(default)]
    pub from_draft: Option<String>,
}

/// POST /api/v1/writing/outline (W3).
///
/// Reverse (`from_draft` present) is zero-LLM and ungated. Forward (topic → tree) is 💰 tier-3
/// and member-gated.
pub async fn outline_writing(
    State(state): State<SharedState>,
    Json(body): Json<OutlineBody>,
) -> AppResult<Json<OutlineResult>> {
    if let Some(draft_text) = body.from_draft.as_deref().filter(|s| !s.trim().is_empty()) {
        // Reverse: deterministic, zero LLM → no member gate, no privacy egress.
        let result = outline::outline_reverse(draft_text).map_err(map_writing_err)?;
        return Ok(Json(result));
    }
    // Forward: generation → tier-3 gate + cloud LLM.
    enforce_member_gate(&state)?;
    let llm = cloud_llm_or_refuse(&state)?;
    let sources = load_sources(&state, &body.item_ids)?;
    let result =
        outline::outline_forward(llm.as_ref(), &body.topic, &sources).map_err(map_writing_err)?;
    Ok(Json(result))
}

// ─────────────────────────── W4 cite (spec §5.2 /cite) — zero LLM ───────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CiteBody {
    /// Bibliographic metadata for each source to cite.
    pub sources: Vec<SourceMeta>,
    /// Citation style: `gbt7714` | `apa` | `ieee` | `mla`.
    pub style: String,
    /// Optional draft text to locate `[cite:<id>]` inline anchors in.
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiteResponse {
    pub citations: Vec<Citation>,
    pub inline_anchors: Vec<InlineAnchor>,
}

fn map_cite_err(e: CiteError) -> AppError {
    use axum::http::StatusCode;
    werr(StatusCode::BAD_REQUEST, e.code(), &e.to_string())
}

/// POST /api/v1/writing/cite (W4). Zero LLM (pure formatting), so NO member gate — citation
/// formatting is a 🆓 local operation available to everyone (spec §8).
pub async fn cite_writing(Json(body): Json<CiteBody>) -> AppResult<Json<CiteResponse>> {
    let style = CiteStyle::parse(&body.style).ok_or_else(|| {
        let supported: Vec<&str> = CiteStyle::all().iter().map(|s| s.as_str()).collect();
        werr(
            axum::http::StatusCode::BAD_REQUEST,
            "unsupported-cite-style",
            &format!("unsupported cite style; supported: {}", supported.join(", ")),
        )
    })?;
    let citations = build_citations(&body.sources, style).map_err(map_cite_err)?;
    let inline_anchors = match body.text.as_deref() {
        Some(t) => {
            let ids: Vec<String> = citations.iter().map(|c| c.id.clone()).collect();
            find_inline_anchors(t, &ids)
        }
        None => vec![],
    };
    Ok(Json(CiteResponse {
        citations,
        inline_anchors,
    }))
}

// ─────────────────────────── W5 synthesis (spec §5.2 /synthesis) ───────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SynthesisBody {
    #[serde(default)]
    pub item_ids: Vec<String>,
    #[serde(default)]
    pub extra_sources: Vec<InlineSource>,
    /// `"thematic"` (default) or `"chronological"`.
    #[serde(default)]
    pub structure: Option<String>,
    /// Cap on sources fed to the model (0 ⇒ no cap).
    #[serde(default)]
    pub max_sources: usize,
}

/// POST /api/v1/writing/synthesis (W5, tier-3). Map-reduce across N sources.
pub async fn synthesis_writing(
    State(state): State<SharedState>,
    Json(body): Json<SynthesisBody>,
) -> AppResult<Json<WritingResult>> {
    enforce_member_gate(&state)?;
    let llm = cloud_llm_or_refuse(&state)?;

    let mut sources = load_sources(&state, &body.item_ids)?;
    for ex in &body.extra_sources {
        sources.push(SourceMaterial::new(ex.external_ref.clone(), ex.text.clone()));
    }

    let structure = match body.structure.as_deref() {
        Some("chronological") => SynthesisStructure::Chronological,
        _ => SynthesisStructure::Thematic,
    };
    let req = SynthesisRequest {
        sources,
        structure,
        max_sources: body.max_sources,
    };
    // Same single cloud provider drives both the cheap MAP and reasoning REDUCE legs (parity with
    // documents.rs deep_summary; per-stage model selection is a later routing slice).
    let llms = SynthLlms {
        cheap: llm.as_ref(),
        reasoning: llm.as_ref(),
    };
    let result = synthesis::synthesize(&llms, &req).map_err(map_writing_err)?;
    Ok(Json(result))
}

// ─────────────────────────── W6 terms / template fill (spec §5.2 /terms) — zero LLM ──────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermsBody {
    /// The template body containing `{{slot}}` placeholders.
    pub text: String,
    /// Slot → value map for fill mode.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

/// POST /api/v1/writing/terms (W6 fill action). Pure `{{slot}}` substitution — zero LLM, no gate.
pub async fn terms_writing(Json(body): Json<TermsBody>) -> AppResult<Json<FillResult>> {
    Ok(Json(fill_template(&body.text, &body.values)))
}

/// GET /api/v1/writing/templates — list available OSS writing templates (id + display name +
/// placeholder marker). Zero LLM, no gate.
pub async fn templates_writing() -> AppResult<Json<serde_json::Value>> {
    let reg = TemplateRegistry::with_oss_defaults();
    let templates: Vec<serde_json::Value> = reg
        .ids()
        .iter()
        .filter_map(|id| reg.get(id))
        .map(|t| {
            json!({
                "id": t.id(),
                "displayName": t.display_name(),
                "placeholderMarker": t.placeholder_marker(),
                "citationStyles": t.citation_styles().iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "templates": templates })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writing_error_maps_to_stable_status_and_code() {
        let e = map_writing_err(WritingError::NoSourceMaterial);
        // AppError::detailed carries the status; verify the code via Display of the source.
        assert_eq!(WritingError::NoSourceMaterial.code(), "no-source-material");
        let _ = e; // status assertion is exercised in the subprocess integration test.

        assert_eq!(WritingError::EmptyInput.code(), "empty-input");
        assert_eq!(WritingError::SourceInjection.code(), "source-injection-detected");
        assert_eq!(WritingError::LlmUnavailable.code(), "llm-unavailable");
    }

    #[test]
    fn style_from_passes_through() {
        let st = style_from(Some("formal".into()), None, Some("expert".into()), None);
        assert_eq!(st.tone.as_deref(), Some("formal"));
        assert_eq!(st.audience.as_deref(), Some("expert"));
        assert!(st.length.is_none());
    }

    #[test]
    fn cite_err_maps_to_kebab_codes() {
        // The mapping builds an AppError; the kebab codes are the stable contract (HTTP status is
        // exercised in the subprocess integration test).
        let _ = map_cite_err(CiteError::NoSources);
        assert_eq!(CiteError::NoSources.code(), "no-source-material");
        assert_eq!(CiteError::MissingTitle("x".into()).code(), "citation-missing-title");
    }

    #[test]
    fn cite_style_parse_accepts_all_four() {
        for s in ["gbt7714", "apa", "ieee", "mla"] {
            assert!(CiteStyle::parse(s).is_some(), "should parse {s}");
        }
        assert!(CiteStyle::parse("chicago").is_none());
    }
}
