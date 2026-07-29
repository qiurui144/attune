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
//! **Privacy + member gate**: a skill with an LLM step is tier-3 — it reuses the shared
//! governed-LLM boundary and the paid member gate. Local providers stay local; cloud providers
//! require explicit egress consent, reject L0 content, and receive a redacted prompt.
//! The downloadable artifact is returned **inline** (base64) in the run response so no
//! server-side artifact store / TTL subsystem is needed (spec §2.2 OUT: no cloud storage).

use axum::extract::{Path, State};
use axum::Json;
use base64::Engine as _;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

use attune_core::export::sanitize::download_filename;
use attune_core::llm::LlmProvider;
use attune_core::skill_runtime::{
    self, run_skill_with_dispatcher, MapResolver, RagResolver, SkillError, SkillRegistry,
};

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

const SETTINGS_KEY: &str = "app_settings";
const SKILL_VERSION_PINS_KEY: &str = "skill_version_pins";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillVersionSnapshot {
    pub skill_id: String,
    pub version: String,
    pub title: String,
    pub source: String,
    pub hash: String,
    pub yaml: String,
    pub captured_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillVersionSnapshotInfo {
    pub skill_id: String,
    pub version: String,
    pub title: String,
    pub source: String,
    pub hash: String,
    pub captured_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl From<&SkillVersionSnapshot> for SkillVersionSnapshotInfo {
    fn from(snapshot: &SkillVersionSnapshot) -> Self {
        Self {
            skill_id: snapshot.skill_id.clone(),
            version: snapshot.version.clone(),
            title: snapshot.title.clone(),
            source: snapshot.source.clone(),
            hash: snapshot.hash.clone(),
            captured_at: snapshot.captured_at.clone(),
            note: snapshot.note.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SkillVersionStore {
    #[serde(default)]
    active: BTreeMap<String, SkillVersionSnapshot>,
    #[serde(default)]
    history: BTreeMap<String, Vec<SkillVersionSnapshot>>,
}

#[derive(Debug, Clone)]
struct ResolvedSkill {
    skill: skill_runtime::Skill,
    source: String,
    pinned: Option<SkillVersionSnapshot>,
}

/// Build the skill registry the runtime serves: OSS built-ins **plus** the declarative skills
/// every installed + enabled plugin registered via its `registers_skills:` manifest field.
///
/// This is the closure that makes pro deliverable skills (academic thesis draft, presales bid
/// doc, …) actually appear in `/skill-runtime/skills` and be runnable — without it the runtime
/// only ever exposed the 3 OSS built-ins.
///
/// **Trust + boundary**: a plugin only reaches `plugin_registry` after passing the scan-time
/// signature/trust gate (`scan_with_trust`), so anything here is already trust-allowed for the
/// configured mode. We additionally skip plugins the user has disabled in settings. Each skill is
/// namespaced by its plugin id (`pro:<plugin_id>` source) so two plugins can't shadow each other.
/// A single bad skill yaml is skipped with a warn — it never aborts the whole registry build.
pub fn build_skill_registry(state: &SharedState) -> SkillRegistry {
    let mut reg = SkillRegistry::with_builtins();
    let disabled = load_disabled_plugin_ids(state);
    let plugin_registry = crate::routes::plugins::current_plugin_registry(state);
    for (plugin_id, _trust, yamls) in plugin_registry.plugin_registered_skills() {
        if disabled.iter().any(|d| d == plugin_id) {
            continue;
        }
        for yaml in yamls {
            if let Err(e) = reg.register_plugin_skill(plugin_id, yaml) {
                tracing::warn!("skill-runtime: plugin '{plugin_id}' skill register failed: {e}");
            }
        }
    }
    reg
}

/// Read `plugins.disabled` from settings (same logic as `routes/scenarios`/`routes/plugins`).
/// Vault locked / unreadable → empty (default: everything enabled).
fn load_disabled_plugin_ids(state: &SharedState) -> Vec<String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    if vault.dek_db().is_err() {
        return Vec::new();
    }
    let raw = match vault.store().get_meta(SETTINGS_KEY) {
        Ok(Some(b)) => b,
        _ => return Vec::new(),
    };
    let json: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    json.get("plugins")
        .and_then(|p| p.get("disabled"))
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// ───────────────────────────── list ─────────────────────────────

#[derive(Serialize)]
pub struct SkillInfo {
    pub id: String,
    pub version: String,
    pub current_version: String,
    pub title: String,
    pub description: String,
    pub cost_tier: String,
    /// `oss` or `pro:<vertical>`.
    pub source: String,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_hash: Option<String>,
    pub inputs: Vec<SkillInputInfo>,
}

#[derive(Serialize)]
pub struct SkillInputInfo {
    pub name: String,
    pub ty: String,
    pub required: bool,
}

/// GET /api/v1/skill-runtime/skills — list registered skills (🆓, no LLM).
pub async fn list_runtime_skills(State(state): State<SharedState>) -> Json<Value> {
    let reg = build_skill_registry(&state);
    let pins = load_skill_version_store_optional(&state).unwrap_or_default();
    let skills: Vec<SkillInfo> = reg
        .list()
        .into_iter()
        .map(|r| SkillInfo {
            id: r.skill.id.clone(),
            version: pins
                .active
                .get(&r.skill.id)
                .map(|p| p.version.clone())
                .unwrap_or_else(|| r.skill.version.clone()),
            current_version: r.skill.version.clone(),
            title: pins
                .active
                .get(&r.skill.id)
                .map(|p| p.title.clone())
                .unwrap_or_else(|| r.skill.title.clone()),
            description: r.skill.description.clone(),
            cost_tier: r.skill.cost_tier.as_str().to_string(),
            source: pins
                .active
                .get(&r.skill.id)
                .map(|p| p.source.clone())
                .unwrap_or_else(|| r.source.clone()),
            pinned: pins.active.contains_key(&r.skill.id),
            active_hash: pins.active.get(&r.skill.id).map(|p| p.hash.clone()),
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

fn skill_info_from_resolved(resolved: &ResolvedSkill, current_version: &str) -> SkillInfo {
    SkillInfo {
        id: resolved.skill.id.clone(),
        version: resolved.skill.version.clone(),
        current_version: current_version.to_string(),
        title: resolved.skill.title.clone(),
        description: resolved.skill.description.clone(),
        cost_tier: resolved.skill.cost_tier.as_str().to_string(),
        source: resolved.source.clone(),
        pinned: resolved.pinned.is_some(),
        active_hash: resolved.pinned.as_ref().map(|p| p.hash.clone()),
        inputs: resolved
            .skill
            .inputs
            .iter()
            .map(|i| SkillInputInfo {
                name: i.name.clone(),
                ty: format!("{:?}", i.ty).to_lowercase(),
                required: i.required,
            })
            .collect(),
    }
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
    let resolved = resolve_skill_for_execution(&state, &id)?;

    // The estimate's input size = the total chars of the referenced items (best-effort; a
    // missing item just contributes 0). Reading item text is a 🆓 local op.
    let input_chars = referenced_input_chars(&state, &resolved.skill, &req.inputs);
    let model = model_name(&state);
    let est = skill_runtime::estimate(&resolved.skill, input_chars, &model);
    Ok(Json(
        serde_json::to_value(&est).unwrap_or_else(|_| json!({})),
    ))
}

// ───────────────────────────── dry-run ─────────────────────────────

#[derive(Serialize)]
pub struct DryRunStep {
    pub id: String,
    pub kind: String,
    pub tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Serialize)]
pub struct DryRunItemRef {
    pub id: String,
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub chars: usize,
}

#[derive(Serialize)]
pub struct DryRunResponse {
    pub skill: SkillInfo,
    pub valid_inputs: bool,
    pub can_run: bool,
    pub has_llm: bool,
    pub member_required: bool,
    pub privacy_llm_required: bool,
    pub referenced_items: Vec<DryRunItemRef>,
    pub steps: Vec<DryRunStep>,
    pub estimate: Value,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

/// POST /api/v1/skill-runtime/skills/{id}/dry-run — validate inputs and produce
/// an execution plan without calling LLMs, agents, or export renderers.
pub async fn dry_run_skill(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<EstimateRequest>,
) -> AppResult<Json<DryRunResponse>> {
    let reg = build_skill_registry(&state);
    let current_version = reg
        .get(&id)
        .map(|s| s.skill.version.clone())
        .unwrap_or_default();
    let resolved = resolve_skill_for_execution(&state, &id)?;
    let skill = &resolved.skill;
    let has_llm = skill.steps.iter().any(|s| s.is_llm());
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    let valid_inputs = match skill_runtime::validate_inputs(skill, &req.inputs) {
        Ok(()) => true,
        Err(e) => {
            blockers.push(e.to_string());
            false
        }
    };
    if has_llm && !is_paid(&state) {
        blockers.push("member-required".into());
    }
    let configured_llm = state.llm();
    let cloud_llm_configured = configured_llm
        .as_ref()
        .is_some_and(|provider| !provider.is_local());
    if has_llm && configured_llm.is_none() {
        blockers.push("llm-unavailable".into());
    }
    if has_llm && cloud_llm_configured && !crate::routes::privacy::outbound_enabled(&state, "llm") {
        blockers.push("cloud-llm-disabled".into());
    }
    if resolved.pinned.is_some() && current_version != skill.version {
        warnings.push(format!(
            "active skill snapshot v{} differs from installed v{}",
            skill.version, current_version
        ));
    }
    let referenced_items = referenced_items_for_dry_run(&state, skill, &req.inputs);
    for item in &referenced_items {
        if !item.found {
            warnings.push(format!("referenced item not found: {}", item.id));
        }
    }
    let input_chars = referenced_items.iter().map(|item| item.chars).sum();
    let model = model_name(&state);
    let est = skill_runtime::estimate(skill, input_chars, &model);
    let estimate = serde_json::to_value(&est).unwrap_or_else(|_| json!({}));
    let steps = skill.steps.iter().map(dry_run_step).collect();
    let can_run = valid_inputs && blockers.is_empty();
    Ok(Json(DryRunResponse {
        skill: skill_info_from_resolved(&resolved, &current_version),
        valid_inputs,
        can_run,
        has_llm,
        member_required: has_llm,
        privacy_llm_required: has_llm && cloud_llm_configured,
        referenced_items,
        steps,
        estimate,
        blockers,
        warnings,
    }))
}

// ───────────────────────────── version governance ─────────────────────────────

#[derive(Serialize)]
pub struct SkillVersionEntry {
    pub skill_id: String,
    pub current: SkillVersionSnapshotInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<SkillVersionSnapshotInfo>,
    pub history: Vec<SkillVersionSnapshotInfo>,
    pub drift: bool,
}

#[derive(Serialize)]
pub struct SkillVersionsResponse {
    pub skills: Vec<SkillVersionEntry>,
}

#[derive(Deserialize)]
pub struct CaptureSnapshotRequest {
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub activate: bool,
}

#[derive(Deserialize)]
pub struct ActivateSnapshotRequest {
    pub hash: String,
}

/// GET /api/v1/skill-runtime/versions — current installed skill versions plus
/// locally captured rollback snapshots.
pub async fn list_skill_versions(
    State(state): State<SharedState>,
) -> AppResult<Json<SkillVersionsResponse>> {
    let reg = build_skill_registry(&state);
    let store = load_skill_version_store(&state)?;
    let skills = reg
        .list()
        .into_iter()
        .map(|r| {
            let current = snapshot_from_registered(r);
            let active = store.active.get(&r.skill.id);
            let drift = active.map(|a| a.hash != current.hash).unwrap_or(false);
            let history = store
                .history
                .get(&r.skill.id)
                .map(|items| items.iter().map(SkillVersionSnapshotInfo::from).collect())
                .unwrap_or_default();
            SkillVersionEntry {
                skill_id: r.skill.id.clone(),
                current: SkillVersionSnapshotInfo::from(&current),
                active: active.map(SkillVersionSnapshotInfo::from),
                history,
                drift,
            }
        })
        .collect();
    Ok(Json(SkillVersionsResponse { skills }))
}

/// POST /api/v1/skill-runtime/skills/{id}/versions/snapshot — capture the
/// currently installed YAML as a rollback point.
pub async fn capture_skill_snapshot(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<CaptureSnapshotRequest>,
) -> AppResult<Json<SkillVersionEntry>> {
    let reg = build_skill_registry(&state);
    let reg_skill = reg.get(&id).ok_or_else(|| skill_not_found(&id))?;
    let mut snapshot = snapshot_from_registered(reg_skill);
    snapshot.note = req.note.filter(|s| !s.trim().is_empty());
    let mut store = load_skill_version_store(&state)?;
    let history = store.history.entry(id.clone()).or_default();
    history.retain(|s| s.hash != snapshot.hash);
    history.insert(0, snapshot.clone());
    history.truncate(20);
    if req.activate {
        store.active.insert(id.clone(), snapshot.clone());
    }
    save_skill_version_store(&state, &store)?;
    Ok(Json(skill_version_entry(&store, reg_skill)))
}

/// POST /api/v1/skill-runtime/skills/{id}/versions/activate — pin a captured
/// snapshot so estimate/dry-run/run use that YAML.
pub async fn activate_skill_snapshot(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<ActivateSnapshotRequest>,
) -> AppResult<Json<SkillVersionEntry>> {
    let reg = build_skill_registry(&state);
    let reg_skill = reg.get(&id).ok_or_else(|| skill_not_found(&id))?;
    let current = snapshot_from_registered(reg_skill);
    let mut store = load_skill_version_store(&state)?;
    let hash = req.hash.trim();
    let snapshot = if current.hash == hash {
        current
    } else {
        store
            .history
            .get(&id)
            .and_then(|items| items.iter().find(|s| s.hash == hash).cloned())
            .ok_or_else(|| {
                AppError::detailed(
                    axum::http::StatusCode::NOT_FOUND,
                    json!({"error": "skill snapshot not found", "code": "snapshot-not-found"}),
                )
            })?
    };
    let parsed = skill_runtime::parse_skill_yaml(&snapshot.yaml).map_err(|e| {
        AppError::detailed(
            axum::http::StatusCode::BAD_REQUEST,
            json!({"error": format!("snapshot is invalid: {e}"), "code": "snapshot-invalid"}),
        )
    })?;
    if parsed.id != id {
        return Err(AppError::detailed(
            axum::http::StatusCode::BAD_REQUEST,
            json!({"error": "snapshot id does not match skill id", "code": "snapshot-mismatch"}),
        ));
    }
    store.active.insert(id.clone(), snapshot);
    save_skill_version_store(&state, &store)?;
    Ok(Json(skill_version_entry(&store, reg_skill)))
}

/// DELETE /api/v1/skill-runtime/skills/{id}/versions/active — clear a pin and
/// return to the currently installed skill YAML.
pub async fn clear_skill_snapshot(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<SkillVersionEntry>> {
    let reg = build_skill_registry(&state);
    let reg_skill = reg.get(&id).ok_or_else(|| skill_not_found(&id))?;
    let mut store = load_skill_version_store(&state)?;
    store.active.remove(&id);
    save_skill_version_store(&state, &store)?;
    Ok(Json(skill_version_entry(&store, reg_skill)))
}

fn skill_version_entry(
    store: &SkillVersionStore,
    reg_skill: &skill_runtime::RegisteredSkill,
) -> SkillVersionEntry {
    let current = snapshot_from_registered(reg_skill);
    let active = store.active.get(&reg_skill.skill.id);
    let drift = active.map(|a| a.hash != current.hash).unwrap_or(false);
    let history = store
        .history
        .get(&reg_skill.skill.id)
        .map(|items| items.iter().map(SkillVersionSnapshotInfo::from).collect())
        .unwrap_or_default();
    SkillVersionEntry {
        skill_id: reg_skill.skill.id.clone(),
        current: SkillVersionSnapshotInfo::from(&current),
        active: active.map(SkillVersionSnapshotInfo::from),
        history,
        drift,
    }
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
    let resolved = resolve_skill_for_execution(&state, &id)?;
    let skill = &resolved.skill;

    // A skill with an LLM step is tier-3: enforce the paid member gate + privacy I1/I2.
    let has_llm = skill.steps.iter().any(|s| s.is_llm());
    if has_llm && !is_paid(&state) {
        return Err(membership_required());
    }

    // Pre-fetch the referenced item text into an in-memory resolver (so the runner never holds
    // the vault lock during the LLM call — lock-ordering safety). Item read is 🆓.
    let (resolver, contains_l0) = build_resolver(&state, skill, &req.inputs)?;

    // The LLM handle goes through the redacting + egress-gated wrapper for any LLM step.
    let llm: Arc<dyn LlmProvider> = if has_llm {
        crate::routes::privacy::governed_llm(&state, contains_l0)?
    } else {
        // A purely deterministic skill never invokes the provider and must not
        // depend on an unrelated LLM configuration.
        attune_core::llm::noop_llm()
    };
    let model = model_name(&state);

    // If the skill chains a **pro plugin agent** (any agent id outside the OSS namespaces), build
    // the subprocess-backed dispatcher (install + entitlement + timeout + LLM-env gated). The skill
    // run is blocking (a pro agent spawns a binary), so the whole `run_skill` moves into
    // spawn_blocking to avoid stalling a tokio worker. The dispatcher is rebuilt here (resolves
    // LLM env once, holds cloned Arc handles) so nothing borrows `state` across the blocking call.
    let dispatcher = crate::routes::skill_dispatch::SubprocessAgentDispatcher::from_state(&state);
    let confirm_cost = req.confirm_cost;
    let inputs = req.inputs.clone();
    let skill_owned = skill.clone();
    let result = tokio::task::spawn_blocking(move || {
        run_skill_with_dispatcher(
            &skill_owned,
            &inputs,
            confirm_cost,
            resolver.as_ref(),
            llm.as_ref(),
            &model,
            dispatcher
                .as_ref()
                .map(|d| d as &dyn attune_core::skill_runtime::AgentDispatcher),
        )
    })
    .await
    .map_err(|e| {
        AppError::detailed(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("skill run join: {e}"), "code": "internal" }),
        )
    })?
    .map_err(map_skill_err)?;

    // INT-2 file-egress gate: the rendered artifact (built from decrypted vault
    // content + LLM output) is a real file download. Scan it → fail-closed block a
    // confidential artifact (422 doc-classified), else PII-redact every text field
    // and **re-render** the redacted bytes so the download carries no plaintext PII.
    let filename = download_filename(
        result.artifact.title().unwrap_or(&skill.title),
        result.format.extension(),
    );
    let (artifact_bytes, gated_artifact, warnings) = {
        let redactor = attune_core::pii::Redactor::default();
        // INT-2 pro write-end: installed pro plugins inject industry confidential
        // markers via plugin.yaml `confidential_keywords:` so a pro-agent delivery
        // carrying an industry secret marker is fail-closed blocked. OSS-only →
        // empty → generic markers only (no industry leak).
        let plugin_registry = crate::routes::plugins::current_plugin_registry(&state);
        let extra_keywords = plugin_registry.all_confidential_keywords();
        match attune_core::doc_privacy::enforce_artifact_egress(
            &redactor,
            &result.artifact,
            attune_core::doc_privacy::RedactMode::Reversible,
            &extra_keywords,
        ) {
            attune_core::doc_privacy::ArtifactEgressOutcome::Blocked { reason } => {
                return Err(AppError::detailed(
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    json!({
                        "error": format!("skill output blocked: confidential document — {reason}"),
                        "code": "doc-classified",
                    }),
                ));
            }
            attune_core::doc_privacy::ArtifactEgressOutcome::Allowed {
                artifact, redacted, ..
            } => {
                let mut warns = result.warnings.clone();
                if redacted > 0 {
                    // Re-render the redacted IR so the downloadable bytes are clean.
                    // If re-render fails, fall closed: refuse rather than ship the
                    // original (plaintext-PII) bytes.
                    match artifact.render(result.format) {
                        Ok(bytes) => {
                            warns.push(format!("已对导出交付物脱敏 {redacted} 处 PII"));
                            (bytes, artifact, warns)
                        }
                        Err(e) => {
                            return Err(AppError::detailed(
                                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                                json!({
                                    "error": format!("redacted artifact re-render failed: {e}"),
                                    "code": "render-failed",
                                }),
                            ));
                        }
                    }
                } else {
                    (result.artifact_bytes.clone(), artifact, warns)
                }
            }
        }
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&artifact_bytes);

    Ok(Json(RunResponse {
        skill_id: result.skill_id,
        filename,
        format: result.format.extension().to_string(),
        mime: result.format.mime().to_string(),
        size_bytes: artifact_bytes.len(),
        artifact_base64: b64,
        artifact: serde_json::to_value(&gated_artifact).unwrap_or(Value::Null),
        token_bill: serde_json::to_value(&result.token_bill).unwrap_or_else(|_| json!({})),
        warnings,
        partial: result.partial,
    }))
}

// ───────────────────────────── helpers ─────────────────────────────

fn yaml_hash(yaml: &str) -> String {
    let digest = Sha256::digest(yaml.as_bytes());
    hex::encode(digest)
}

fn snapshot_from_registered(reg_skill: &skill_runtime::RegisteredSkill) -> SkillVersionSnapshot {
    SkillVersionSnapshot {
        skill_id: reg_skill.skill.id.clone(),
        version: reg_skill.skill.version.clone(),
        title: reg_skill.skill.title.clone(),
        source: reg_skill.source.clone(),
        hash: yaml_hash(&reg_skill.yaml),
        yaml: reg_skill.yaml.clone(),
        captured_at: Utc::now().to_rfc3339(),
        note: None,
    }
}

fn load_skill_version_store_optional(state: &SharedState) -> Option<SkillVersionStore> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().ok()?;
    let raw = vault
        .store()
        .get_meta(SKILL_VERSION_PINS_KEY)
        .ok()
        .flatten()?;
    let plain = attune_core::crypto::decrypt(&dek, &raw).ok()?;
    serde_json::from_slice(&plain).ok()
}

fn load_skill_version_store(state: &SharedState) -> AppResult<SkillVersionStore> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let raw = vault
        .store()
        .get_meta(SKILL_VERSION_PINS_KEY)
        .map_err(|e| AppError::Internal(format!("read skill version store: {e}")))?;
    let Some(raw) = raw else {
        return Ok(SkillVersionStore::default());
    };
    let plain = attune_core::crypto::decrypt(&dek, &raw)
        .map_err(|e| AppError::Internal(format!("decrypt skill version store: {e}")))?;
    serde_json::from_slice(&plain)
        .map_err(|e| AppError::Internal(format!("parse skill version store: {e}")))
}

fn save_skill_version_store(state: &SharedState, store: &SkillVersionStore) -> AppResult<()> {
    let data = serde_json::to_vec_pretty(store)
        .map_err(|e| AppError::Internal(format!("serialize skill version store: {e}")))?;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|_| vault_locked())?;
    let encrypted = attune_core::crypto::encrypt(&dek, &data)
        .map_err(|e| AppError::Internal(format!("encrypt skill version store: {e}")))?;
    vault
        .store()
        .set_meta(SKILL_VERSION_PINS_KEY, &encrypted)
        .map_err(|e| AppError::Internal(format!("write skill version store: {e}")))?;
    Ok(())
}

fn resolve_skill_for_execution(state: &SharedState, id: &str) -> AppResult<ResolvedSkill> {
    let reg = build_skill_registry(state);
    let current = reg.get(id).ok_or_else(|| skill_not_found(id))?;
    let Some(store) = load_skill_version_store_optional(state) else {
        return Ok(ResolvedSkill {
            skill: current.skill.clone(),
            source: current.source.clone(),
            pinned: None,
        });
    };
    let Some(pin) = store.active.get(id).cloned() else {
        return Ok(ResolvedSkill {
            skill: current.skill.clone(),
            source: current.source.clone(),
            pinned: None,
        });
    };
    let skill = skill_runtime::parse_skill_yaml(&pin.yaml).map_err(|e| {
        AppError::detailed(
            axum::http::StatusCode::CONFLICT,
            json!({
                "error": format!("pinned skill snapshot is invalid: {e}"),
                "code": "pinned-skill-invalid",
            }),
        )
    })?;
    if skill.id != id {
        return Err(AppError::detailed(
            axum::http::StatusCode::CONFLICT,
            json!({
                "error": "pinned skill snapshot id does not match request",
                "code": "pinned-skill-mismatch",
            }),
        ));
    }
    Ok(ResolvedSkill {
        skill,
        source: pin.source.clone(),
        pinned: Some(pin),
    })
}

fn dry_run_step(step: &skill_runtime::SkillStep) -> DryRunStep {
    match step {
        skill_runtime::SkillStep::Rag(s) => DryRunStep {
            id: s.id.clone(),
            kind: "rag".into(),
            tier: "local".into(),
            detail: Some(s.output.clone()),
        },
        skill_runtime::SkillStep::Agent(s) => DryRunStep {
            id: s.id.clone(),
            kind: "agent".into(),
            tier: "llm".into(),
            detail: Some(s.agent.clone()),
        },
        skill_runtime::SkillStep::Synthesize(s) => DryRunStep {
            id: s.id.clone(),
            kind: "synthesize".into(),
            tier: "llm".into(),
            detail: Some(s.output.clone()),
        },
        skill_runtime::SkillStep::Render(s) => DryRunStep {
            id: s.id.clone(),
            kind: "render".into(),
            tier: "local".into(),
            detail: Some(format!("{:?}", s.as_kind).to_lowercase()),
        },
        skill_runtime::SkillStep::Export(s) => DryRunStep {
            id: s.id.clone(),
            kind: "export".into(),
            tier: "local".into(),
            detail: Some(s.output.clone()),
        },
    }
}

fn referenced_items_for_dry_run(
    state: &SharedState,
    skill: &skill_runtime::Skill,
    inputs: &Value,
) -> Vec<DryRunItemRef> {
    let ids = collect_item_id_inputs(skill, inputs);
    if ids.is_empty() {
        return Vec::new();
    }
    let vault = match state.vault.lock() {
        Ok(v) => v,
        Err(e) => e.into_inner(),
    };
    let Ok(dek) = vault.dek_db() else {
        return ids
            .into_iter()
            .map(|id| DryRunItemRef {
                id,
                found: false,
                title: None,
                chars: 0,
            })
            .collect();
    };
    ids.into_iter()
        .map(
            |id| match vault.store().get_item(&dek, &id).ok().flatten() {
                Some(item) => DryRunItemRef {
                    id,
                    found: true,
                    title: Some(item.title),
                    chars: item.content.chars().count(),
                },
                None => DryRunItemRef {
                    id,
                    found: false,
                    title: None,
                    chars: 0,
                },
            },
        )
        .collect()
}

/// Build an in-memory [`RagResolver`] by pre-decrypting every item id the skill's inputs name.
/// Reads under the vault lock once, then releases it before the LLM step runs.
fn build_resolver(
    state: &SharedState,
    skill: &skill_runtime::Skill,
    inputs: &Value,
) -> AppResult<(Box<dyn RagResolver>, bool)> {
    let item_ids = collect_item_id_inputs(skill, inputs);
    let mut map = std::collections::BTreeMap::new();
    let mut contains_l0 = false;
    if !item_ids.is_empty() {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|_| vault_locked())?;
        for id in &item_ids {
            if let Ok(Some(item)) = vault.store().get_item(&dek, id) {
                map.insert(id.clone(), item.content);
                contains_l0 |= vault
                    .store()
                    .get_item_privacy_tier(id)
                    .map(|tier| matches!(tier, attune_core::store::audit::PrivacyTier::L0))
                    .unwrap_or(true);
            }
        }
    }
    Ok((Box::new(MapResolver(map)), contains_l0))
}

/// The set of item ids the skill's inputs resolve to in this run payload.
///
/// Covers both `item_id`-typed scalar inputs (e.g. `doc` / `reference`) AND `string_list`-typed
/// inputs that carry KB item ids (e.g. synthesis `item_ids`, reference `source_data`). The latter
/// is necessary so document skills' multi-item RAG bundle is pre-decrypted before the LLM step.
fn collect_item_id_inputs(skill: &skill_runtime::Skill, inputs: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for spec in &skill.inputs {
        match spec.ty {
            skill_runtime::InputType::ItemId => {
                if let Some(s) = inputs.get(&spec.name).and_then(|v| v.as_str()) {
                    ids.push(s.to_string());
                }
            }
            // A string_list MAY carry item ids (synthesis `item_ids` / reference `source_data`).
            // Pre-fetch them too; a non-id string just resolves to nothing (skipped downstream).
            skill_runtime::InputType::StringList => {
                if let Some(arr) = inputs.get(&spec.name).and_then(|v| v.as_array()) {
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            ids.push(s.to_string());
                        }
                    }
                }
            }
            skill_runtime::InputType::String => {}
        }
    }
    ids
}

/// Total chars of the items referenced by this skill+inputs (for the static estimate).
fn referenced_input_chars(
    state: &SharedState,
    skill: &skill_runtime::Skill,
    inputs: &Value,
) -> usize {
    let ids = collect_item_id_inputs(skill, inputs);
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
    state
        .member_state
        .lock()
        .map(|g| g.is_paid())
        .unwrap_or(false)
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

fn vault_locked() -> AppError {
    AppError::detailed(
        axum::http::StatusCode::UNAUTHORIZED,
        json!({ "error": "vault is locked", "code": "vault-locked" }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but *valid* declarative skill yaml (paid llm_multi_step, manual trigger so it
    /// passes the registry cost guard). `id` is templated so each plugin can have a unique id.
    fn skill_yaml(id: &str) -> String {
        skill_yaml_version(id, "1.0.0")
    }

    fn skill_yaml_version(id: &str, version: &str) -> String {
        format!(
            r#"id: {id}
type: skill
version: "{version}"
title: 测试技能 {id}
cost_tier: llm_multi_step
trigger: {{ on: manual, scope: project }}
inputs:
  - {{ name: item_ids, type: string_list, required: true }}
steps:
  - type: agent
    id: synth
    agent: writing.research_synthesis
    input: {{}}
    output: synthesis
  - type: render
    id: build
    as_kind: document
    input: {{}}
    output: artifact
  - type: export
    id: out
    input: {{}}
    output: file
"#
        )
    }

    fn insert_item(state: &SharedState, title: &str, content: &str) -> String {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().expect("dek");
        vault
            .store()
            .insert_item(&dek, title, content, None, "note", None, None)
            .expect("insert item")
    }

    /// Write a plugin dir with a plugin.yaml that registers the given skill yaml files.
    fn write_plugin(
        plugins_root: &std::path::Path,
        plugin_id: &str,
        skills: &[(&str, &str)], // (rel_path, yaml_content)
    ) {
        let dir = plugins_root.join(plugin_id);
        std::fs::create_dir_all(dir.join("skills")).expect("mkdir skills");
        let mut reg_lines = String::new();
        for (rel, content) in skills {
            std::fs::write(dir.join(rel), content).expect("write skill yaml");
            reg_lines.push_str(&format!("  - {rel}\n"));
        }
        let manifest = format!(
            "id: {plugin_id}\nname: {plugin_id}\ntype: industry\nversion: \"1.0.0\"\nregisters_skills:\n{reg_lines}"
        );
        std::fs::write(dir.join("plugin.yaml"), manifest).expect("write plugin.yaml");
    }

    fn state_with_plugins(tmp: &std::path::Path) -> SharedState {
        let vault = attune_core::vault::Vault::open_memory(tmp).expect("vault");
        vault.setup("P@ss-skillreg-not-real").expect("setup");
        std::sync::Arc::new(crate::state::AppState::new(vault, false))
    }

    /// 0 plugins → only the 3 OSS built-ins, no pro skills.
    #[test]
    fn build_registry_no_plugins_only_builtins() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let state = state_with_plugins(tmp.path());
        let reg = build_skill_registry(&state);
        // Built-ins present; none tagged pro.
        assert!(reg.get("research-synthesis").is_some());
        assert!(reg.list().iter().all(|r| r.source == "oss"));
    }

    /// Multi-plugin: two installed plugins each contribute their skill, both prefixed by plugin id.
    #[test]
    fn build_registry_multi_plugin_registers_all() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        write_plugin(
            &plugins,
            "academic-pro",
            &[("skills/a.yaml", &skill_yaml("acad-thesis"))],
        );
        write_plugin(
            &plugins,
            "presales-pro",
            &[("skills/b.yaml", &skill_yaml("presales-bid"))],
        );
        let state = state_with_plugins(tmp.path());
        let reg = build_skill_registry(&state);
        assert_eq!(reg.get("acad-thesis").unwrap().source, "pro:academic-pro");
        assert_eq!(reg.get("presales-bid").unwrap().source, "pro:presales-pro");
        // built-ins still there.
        assert!(reg.get("research-synthesis").is_some());
    }

    /// A malformed skill yaml is skipped — the rest of the registry (built-ins + good plugin) survives.
    #[test]
    fn build_registry_bad_yaml_skipped_not_fatal() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        // background-triggered paid skill → registry rejects it (cost guard), must be skipped.
        let bad = "id: sneaky\ntype: skill\nversion: \"1\"\ntitle: x\ncost_tier: llm_multi_step\ntrigger: { on: file_added, scope: project }\nsteps:\n  - type: export\n    id: out\n    input: {}\n    output: file\n";
        write_plugin(&plugins, "evil-pro", &[("skills/bad.yaml", bad)]);
        let state = state_with_plugins(tmp.path());
        let reg = build_skill_registry(&state);
        assert!(reg.get("sneaky").is_none(), "rejected skill not registered");
        assert!(
            reg.get("research-synthesis").is_some(),
            "built-ins survive bad plugin"
        );
    }

    /// Disabled plugins (settings.plugins.disabled) don't contribute skills.
    #[test]
    fn build_registry_skips_disabled_plugin() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        write_plugin(
            &plugins,
            "academic-pro",
            &[("skills/a.yaml", &skill_yaml("acad-thesis"))],
        );
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-skillreg-not-real").expect("setup");
        let settings = serde_json::json!({ "plugins": { "disabled": ["academic-pro"] } });
        vault
            .store()
            .set_meta(SETTINGS_KEY, settings.to_string().as_bytes())
            .expect("write settings");
        let state = std::sync::Arc::new(crate::state::AppState::new(vault, false));
        let reg = build_skill_registry(&state);
        assert!(
            reg.get("acad-thesis").is_none(),
            "disabled plugin skill not registered"
        );
    }

    /// list endpoint exposes a pro plugin's registered skill (end-to-end through the handler).
    #[tokio::test]
    async fn list_endpoint_includes_pro_skill() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        write_plugin(
            &plugins,
            "academic-pro",
            &[("skills/a.yaml", &skill_yaml("acad-thesis"))],
        );
        let state = state_with_plugins(tmp.path());
        let resp = list_runtime_skills(State(state)).await;
        let skills = resp.0["skills"].as_array().expect("skills array");
        let found = skills
            .iter()
            .any(|s| s["id"] == "acad-thesis" && s["source"] == "pro:academic-pro");
        assert!(
            found,
            "pro skill must appear in /skill-runtime/skills listing"
        );
    }

    /// Dry-run validates inputs, item references, member gating, privacy gating, and estimate
    /// without requiring a configured LLM or executing the skill.
    #[tokio::test]
    async fn dry_run_reports_blockers_and_referenced_items_without_execution() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        write_plugin(
            &plugins,
            "academic-pro",
            &[("skills/a.yaml", &skill_yaml("acad-thesis"))],
        );
        let state = state_with_plugins(tmp.path());
        let item_id = insert_item(
            &state,
            "合同材料",
            "这是一段用于估算的合同正文，包含足够字符。",
        );

        let resp = dry_run_skill(
            State(state),
            Path("acad-thesis".to_string()),
            Json(EstimateRequest {
                inputs: json!({ "item_ids": [item_id, "missing-item"] }),
            }),
        )
        .await
        .expect("dry-run ok")
        .0;

        assert!(resp.valid_inputs);
        assert!(!resp.can_run, "logged-out + privacy-off should block run");
        assert!(resp.has_llm);
        assert!(resp.blockers.iter().any(|b| b == "member-required"));
        assert!(resp.blockers.iter().any(|b| b == "llm-unavailable"));
        assert!(resp.warnings.iter().any(|w| w.contains("missing-item")));
        assert_eq!(resp.referenced_items.len(), 2);
        assert!(resp
            .referenced_items
            .iter()
            .any(|item| item.found && item.title.as_deref() == Some("合同材料")));
        assert!(resp
            .referenced_items
            .iter()
            .any(|item| !item.found && item.id == "missing-item"));
        assert!(resp.steps.iter().any(|step| step.kind == "agent"));
        assert!(
            resp.estimate["est_tokens"].as_u64().unwrap_or_default() > 0,
            "estimate should include the referenced item size"
        );
    }

    /// Version snapshots are local rollback points: pinning a snapshot keeps the runnable/listed
    /// skill on that YAML even after the installed plugin moves forward, and clearing the pin
    /// returns to the current installed version.
    #[tokio::test]
    async fn version_snapshot_pin_tracks_drift_and_can_clear() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        write_plugin(
            &plugins,
            "academic-pro",
            &[("skills/a.yaml", &skill_yaml_version("acad-thesis", "1.0.0"))],
        );
        let state = state_with_plugins(tmp.path());

        let pinned = capture_skill_snapshot(
            State(state.clone()),
            Path("acad-thesis".to_string()),
            Json(CaptureSnapshotRequest {
                note: Some("baseline".into()),
                activate: true,
            }),
        )
        .await
        .expect("capture snapshot")
        .0;
        assert_eq!(
            pinned.active.as_ref().map(|s| s.version.as_str()),
            Some("1.0.0")
        );
        let api_json = serde_json::to_string(&pinned).expect("serialize api entry");
        assert!(
            !api_json.contains("\"yaml\""),
            "version API must not expose commercial skill yaml"
        );
        let raw_store = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            vault
                .store()
                .get_meta(SKILL_VERSION_PINS_KEY)
                .expect("read raw version store")
                .expect("raw version store")
        };
        assert!(
            !String::from_utf8_lossy(&raw_store).contains("acad-thesis"),
            "version snapshots must not persist commercial skill yaml in plaintext"
        );

        std::fs::write(
            plugins.join("academic-pro").join("skills").join("a.yaml"),
            skill_yaml_version("acad-thesis", "2.0.0"),
        )
        .expect("write upgraded skill");

        let versions = list_skill_versions(State(state.clone()))
            .await
            .expect("list versions")
            .0;
        let entry = versions
            .skills
            .iter()
            .find(|s| s.skill_id == "acad-thesis")
            .expect("version entry");
        assert_eq!(entry.current.version, "2.0.0");
        assert_eq!(
            entry.active.as_ref().map(|s| s.version.as_str()),
            Some("1.0.0")
        );
        assert!(entry.drift);

        let listed = list_runtime_skills(State(state.clone())).await;
        let skills = listed.0["skills"].as_array().expect("skills");
        let listed_skill = skills
            .iter()
            .find(|s| s["id"] == "acad-thesis")
            .expect("listed skill");
        assert_eq!(listed_skill["version"], "1.0.0");
        assert_eq!(listed_skill["current_version"], "2.0.0");
        assert_eq!(listed_skill["pinned"], true);

        let cleared = clear_skill_snapshot(State(state.clone()), Path("acad-thesis".to_string()))
            .await
            .expect("clear active")
            .0;
        assert!(cleared.active.is_none());
        assert!(!cleared.drift);

        let listed = list_runtime_skills(State(state)).await;
        let skills = listed.0["skills"].as_array().expect("skills");
        let listed_skill = skills
            .iter()
            .find(|s| s["id"] == "acad-thesis")
            .expect("listed skill after clear");
        assert_eq!(listed_skill["version"], "2.0.0");
        assert_eq!(listed_skill["pinned"], false);
    }
}
