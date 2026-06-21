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
    self, run_skill_with_dispatcher, MapResolver, RagResolver, SkillError, SkillRegistry,
};

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

const SETTINGS_KEY: &str = "app_settings";

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
    for (plugin_id, _trust, yamls) in state.plugin_registry.plugin_registered_skills() {
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
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

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
pub async fn list_runtime_skills(State(state): State<SharedState>) -> Json<Value> {
    let reg = build_skill_registry(&state);
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
    let reg = build_skill_registry(&state);
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
    let reg = build_skill_registry(&state);
    let reg_skill = reg.get(&id).ok_or_else(|| skill_not_found(&id))?;
    let skill = &reg_skill.skill;

    // A skill with an LLM step is tier-3: enforce the paid member gate + privacy I1/I2.
    let has_llm = skill.steps.iter().any(|s| s.is_llm());
    if has_llm && !is_paid(&state) {
        return Err(membership_required());
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
            dispatcher.as_ref().map(|d| d as &dyn attune_core::skill_runtime::AgentDispatcher),
        )
    })
    .await
    .map_err(|e| AppError::detailed(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": format!("skill run join: {e}"), "code": "internal" }),
    ))?
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
        let extra_keywords = state.plugin_registry.all_confidential_keywords();
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
            attune_core::doc_privacy::ArtifactEgressOutcome::Allowed { artifact, redacted, .. } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but *valid* declarative skill yaml (paid llm_multi_step, manual trigger so it
    /// passes the registry cost guard). `id` is templated so each plugin can have a unique id.
    fn skill_yaml(id: &str) -> String {
        format!(
            r#"id: {id}
type: skill
version: "1.0.0"
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
        write_plugin(&plugins, "academic-pro", &[("skills/a.yaml", &skill_yaml("acad-thesis"))]);
        write_plugin(&plugins, "presales-pro", &[("skills/b.yaml", &skill_yaml("presales-bid"))]);
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
        assert!(reg.get("research-synthesis").is_some(), "built-ins survive bad plugin");
    }

    /// Disabled plugins (settings.plugins.disabled) don't contribute skills.
    #[test]
    fn build_registry_skips_disabled_plugin() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        write_plugin(&plugins, "academic-pro", &[("skills/a.yaml", &skill_yaml("acad-thesis"))]);
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-skillreg-not-real").expect("setup");
        let settings = serde_json::json!({ "plugins": { "disabled": ["academic-pro"] } });
        vault
            .store()
            .set_meta(SETTINGS_KEY, settings.to_string().as_bytes())
            .expect("write settings");
        let state = std::sync::Arc::new(crate::state::AppState::new(vault, false));
        let reg = build_skill_registry(&state);
        assert!(reg.get("acad-thesis").is_none(), "disabled plugin skill not registered");
    }

    /// list endpoint exposes a pro plugin's registered skill (end-to-end through the handler).
    #[tokio::test]
    async fn list_endpoint_includes_pro_skill() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let _g = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugins = tmp.path().join("attune").join("plugins");
        write_plugin(&plugins, "academic-pro", &[("skills/a.yaml", &skill_yaml("acad-thesis"))]);
        let state = state_with_plugins(tmp.path());
        let resp = list_runtime_skills(State(state)).await;
        let skills = resp.0["skills"].as_array().expect("skills array");
        let found = skills.iter().any(|s| s["id"] == "acad-thesis" && s["source"] == "pro:academic-pro");
        assert!(found, "pro skill must appear in /skill-runtime/skills listing");
    }
}
