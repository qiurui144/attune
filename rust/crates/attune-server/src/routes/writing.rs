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
use attune_core::writing::rewrite::{self, RewriteOutput, RewriteRequest};
use attune_core::writing::{SourceMaterial, StyleTarget, WritingError, WritingResult};
use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::json;
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
}
