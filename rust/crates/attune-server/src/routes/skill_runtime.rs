//! Skills orchestration-runtime routes (spec §5.2) — list / estimate / run a declarative skill.
//!
//! These are **distinct** from `routes::skills` (the SkillClaw plugin listing); the runtime
//! lives under `/api/v1/skill-runtime/*` to avoid that namespace collision. A skill run is a
//! 💰 tier-3 multi-step LLM op:
//!
//! - `GET  /api/v1/skill-runtime/skills`            — list registered skills (🆓).
//! - `POST /api/v1/skill-runtime/skills/{id}/estimate` — zero-LLM static cost estimate (🆓).
//! - `POST /api/v1/skill-runtime/skills/{id}/run`   — run it (💰, user-triggered, gated).
//!
//! **Cost contract (CLAUDE.md §成本契约)**: `/run` requires `confirm_cost: true` for a paid
//! skill (the runtime also enforces it); the response carries the actual `token_bill` so the UI
//! shows the post-run bill.
//!
//! **Privacy + member gate**: a skill with an LLM step is tier-3 — it reuses doc-intel's
//! `cloud_llm_or_refuse` (I2 egress gate + I1 PII-redacting wrapper) and the paid member gate,
//! so the same guarantees as `/documents/compare` apply (no raw content egress, paid-only).
//! The downloadable artifact is returned **inline** (base64) in the run response so no
//! server-side artifact store / TTL subsystem is needed (spec §2.2 OUT: no cloud storage).

use axum::extract::{Path, State};
use axum::Json;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use attune_core::export::sanitize::download_filename;
use attune_core::llm::LlmProvider;
use attune_core::skill_runtime::{
    self, run_skill, MapResolver, RagResolver, SkillError, SkillRegistry,
};

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

// ───────────────────────────── list ─────────────────────────────

#[derive(Serialize)]
pub struct SkillInfo {
    pub id: String,
    pub version: String,
    pub title: String,
    pub description: String,
    pub cost_tier: String,
    /// `oss` or `pro:<vertical>`.
    pub source: String,
    pub inputs: Vec<SkillInputInfo>,
}

#[derive(Serialize)]
pub struct SkillInputInfo {
    pub name: String,
    pub ty: String,
    pub required: bool,
}

/// GET /api/v1/skill-runtime/skills — list registered skills (🆓, no LLM).
pub async fn list_runtime_skills(State(_state): State<SharedState>) -> Json<Value> {
    let reg = SkillRegistry::with_builtins();
    let skills: Vec<SkillInfo> = reg
        .list()
        .into_iter()
        .map(|r| SkillInfo {
            id: r.skill.id.clone(),
            version: r.skill.version.clone(),
            title: r.skill.title.clone(),
            description: r.skill.description.clone(),
            cost_tier: r.skill.cost_tier.as_str().to_string(),
            source: r.source.clone(),
            inputs: r
                .skill
                .inputs
                .iter()
                .map(|i| SkillInputInfo {
                    name: i.name.clone(),
                    ty: format!("{:?}", i.ty).to_lowercase(),
                    required: i.required,
                })
                .collect(),
        })
        .collect();
    Json(json!({ "skills": skills }))
}

// ───────────────────────────── estimate ─────────────────────────────

#[derive(Deserialize)]
pub struct EstimateRequest {
    #[serde(default)]
    pub inputs: Value,
}

/// POST /api/v1/skill-runtime/skills/{id}/estimate — static cost estimate (🆓, no LLM call).
pub async fn estimate_skill(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<EstimateRequest>,
) -> AppResult<Json<Value>> {
    let reg = SkillRegistry::with_builtins();
    let reg_skill = reg
        .get(&id)
        .ok_or_else(|| skill_not_found(&id))?;

    // The estimate's input size = the total chars of the referenced items (best-effort; a
    // missing item just contributes 0). Reading item text is a 🆓 local op.
    let input_chars = referenced_input_chars(&state, reg_skill, &req.inputs);
    let model = model_name(&state);
    let est = skill_runtime::estimate(&reg_skill.skill, input_chars, &model);
    Ok(Json(serde_json::to_value(&est).unwrap_or_else(|_| json!({}))))
}

// ───────────────────────────── run ─────────────────────────────

#[derive(Deserialize)]
pub struct RunRequest {
    #[serde(default)]
    pub inputs: Value,
    /// Must be true for a paid skill (guards accidental spend, spec §8).
    #[serde(default)]
    pub confirm_cost: bool,
}

#[derive(Serialize)]
pub struct RunResponse {
    pub skill_id: String,
    pub filename: String,
    pub format: String,
    pub mime: String,
    pub size_bytes: usize,
    /// The artifact file, base64-encoded (download inline — no server-side store).
    pub artifact_base64: String,
    /// The artifact IR (so the UI may re-render another format via /export).
    pub artifact: Value,
    pub token_bill: Value,
    pub warnings: Vec<String>,
    pub partial: bool,
}

/// POST /api/v1/skill-runtime/skills/{id}/run — execute a skill (💰, user-triggered, gated).
pub async fn run_runtime_skill(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<RunRequest>,
) -> AppResult<Json<RunResponse>> {
    let reg = SkillRegistry::with_builtins();
    let reg_skill = reg.get(&id).ok_or_else(|| skill_not_found(&id))?;
    let skill = &reg_skill.skill;

    // A skill with an LLM step is tier-3: enforce the paid member gate + privacy I1/I2.
    let has_llm = skill.steps.iter().any(|s| s.is_llm());
    if has_llm {
        if !is_paid(&state) {
            return Err(membership_required());
        }
    }

    // Pre-fetch the referenced item text into an in-memory resolver (so the runner never holds
    // the vault lock during the LLM call — lock-ordering safety). Item read is 🆓.
    let resolver = build_resolver(&state, skill, &req.inputs)?;

    // The LLM handle goes through the redacting + egress-gated wrapper for any LLM step.
    let llm: Arc<dyn LlmProvider> = if has_llm {
        cloud_llm_or_refuse(&state)?
    } else {
        // A purely deterministic skill still needs *a* handle; the no-op never gets called.
        state.llm().ok_or_else(llm_unavailable)?
    };
    let model = model_name(&state);

    let result = run_skill(skill, &req.inputs, req.confirm_cost, resolver.as_ref(), llm.as_ref(), &model)
        .map_err(map_skill_err)?;

    let filename = download_filename(
        result.artifact.title().unwrap_or(&skill.title),
        result.format.extension(),
    );
    let b64 = base64::engine::general_purpose::STANDARD.encode(&result.artifact_bytes);

    Ok(Json(RunResponse {
        skill_id: result.skill_id,
        filename,
        format: result.format.extension().to_string(),
        mime: result.format.mime().to_string(),
        size_bytes: result.artifact_bytes.len(),
        artifact_base64: b64,
        artifact: serde_json::to_value(&result.artifact).unwrap_or_else(|_| json!(null)),
        token_bill: serde_json::to_value(&result.token_bill).unwrap_or_else(|_| json!({})),
        warnings: result.warnings,
        partial: result.partial,
    }))
}

// ───────────────────────────── helpers ─────────────────────────────

/// Build an in-memory [`RagResolver`] by pre-decrypting every item id the skill's inputs name.
/// Reads under the vault lock once, then releases it before the LLM step runs.
fn build_resolver(
    state: &SharedState,
    skill: &skill_runtime::Skill,
    inputs: &Value,
) -> AppResult<Box<dyn RagResolver>> {
    let item_ids = collect_item_id_inputs(skill, inputs);
    let mut map = std::collections::BTreeMap::new();
    if !item_ids.is_empty() {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|_| vault_locked())?;
        for id in &item_ids {
            if let Ok(Some(item)) = vault.store().get_item(&dek, id) {
                map.insert(id.clone(), item.content);
            }
        }
    }
    Ok(Box::new(MapResolver(map)))
}

/// The set of item ids the skill's `item_id`-typed inputs resolve to in this run payload.
fn collect_item_id_inputs(skill: &skill_runtime::Skill, inputs: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for spec in &skill.inputs {
        if spec.ty == skill_runtime::InputType::ItemId {
            if let Some(s) = inputs.get(&spec.name).and_then(|v| v.as_str()) {
                ids.push(s.to_string());
            }
        }
    }
    ids
}

/// Total chars of the items referenced by this skill+inputs (for the static estimate).
fn referenced_input_chars(
    state: &SharedState,
    reg_skill: &skill_runtime::RegisteredSkill,
    inputs: &Value,
) -> usize {
    let ids = collect_item_id_inputs(&reg_skill.skill, inputs);
    if ids.is_empty() {
        return 0;
    }
    let vault = match state.vault.lock() {
        Ok(v) => v,
        Err(e) => e.into_inner(),
    };
    let Ok(dek) = vault.dek_db() else { return 0 };
    ids.iter()
        .filter_map(|id| vault.store().get_item(&dek, id).ok().flatten())
        .map(|item| item.content.chars().count())
        .sum()
}

fn model_name(state: &SharedState) -> String {
    let settings = {
        let bytes = match state.vault.lock() {
            Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
            Err(_) => None,
        };
        bytes
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or_else(|| json!({}))
    };
    attune_core::document_intelligence::model_routing::ModelRouter::from_settings(&settings)
        .pick(attune_core::document_intelligence::model_routing::ModelRole::Cheap)
        .to_string()
}

fn is_paid(state: &SharedState) -> bool {
    state.member_state.lock().map(|g| g.is_paid()).unwrap_or(false)
}

/// I2 egress gate (default off) — has the user opted into cloud-LLM egress?
fn cloud_llm_egress_enabled(state: &SharedState) -> bool {
    let bytes = match state.vault.lock() {
        Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
        Err(_) => None,
    };
    bytes
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|s| s.get("privacy").and_then(|p| p.get("llm")).and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Resolve a tier-3 LLM handle, enforcing I2 egress gate + I1 PII-redaction (parity with
/// `documents::cloud_llm_or_refuse`).
fn cloud_llm_or_refuse(state: &SharedState) -> AppResult<Arc<dyn LlmProvider>> {
    if !cloud_llm_egress_enabled(state) {
        return Err(AppError::detailed(
            axum::http::StatusCode::FORBIDDEN,
            json!({ "error": "cloud LLM egress is disabled in Privacy settings", "code": "cloud-llm-disabled" }),
        ));
    }
    let inner = state.llm().ok_or_else(llm_unavailable)?;
    Ok(Arc::new(
        attune_core::redacting_llm::RedactingLlmProvider::with_default_redactor(inner),
    ))
}

fn map_skill_err(e: SkillError) -> AppError {
    use axum::http::StatusCode;
    let status = match e {
        SkillError::InputInvalid(_) | SkillError::CostNotConfirmed => StatusCode::BAD_REQUEST,
        SkillError::CostCapExceeded { .. } => StatusCode::BAD_REQUEST,
        SkillError::StepFailed { .. } | SkillError::NoArtifact => StatusCode::UNPROCESSABLE_ENTITY,
    };
    AppError::detailed(status, json!({ "error": e.to_string(), "code": e.code() }))
}

fn skill_not_found(id: &str) -> AppError {
    AppError::detailed(
        axum::http::StatusCode::NOT_FOUND,
        json!({ "error": format!("skill not found: {id}"), "code": "skill-not-found" }),
    )
}

fn membership_required() -> AppError {
    AppError::detailed(
        axum::http::StatusCode::PAYMENT_REQUIRED,
        json!({ "error": "this skill requires a paid membership", "code": "member-required" }),
    )
}

fn llm_unavailable() -> AppError {
    AppError::detailed(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": "no LLM provider configured", "code": "llm-unavailable" }),
    )
}

fn vault_locked() -> AppError {
    AppError::detailed(
        axum::http::StatusCode::UNAUTHORIZED,
        json!({ "error": "vault is locked", "code": "vault-locked" }),
    )
}
