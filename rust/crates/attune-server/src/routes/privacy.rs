//! v0.6 Phase A.5.5 — Privacy tier 检测
//! v1.0.6 Privacy Logic Strategy — 5 outbound points 总览 + DSAR + 锁定 + wipe-cloud-session
//!
//! 端点：
//! - `GET  /api/v1/privacy/tier` — 返当前生产实现可用的脱敏层（v0.6 老接口）
//! - `GET  /api/v1/privacy/status` — 5 出网点状态 + vault state + redactor info（v1.0.6 新增）
//! - `PATCH /api/v1/privacy/settings` — 切换某一出网点开关（v1.0.6 新增）
//! - `POST /api/v1/privacy/lock` — 立即锁 vault（用户主动）（v1.0.6 新增）
//! - `POST /api/v1/privacy/wipe-cloud-session` — 吊销 cloud session + 清本地 token（v1.0.6 新增）
//!
//! 决策（用户 2026-04-28）：
//! - L1 正则脱敏 → OSS 免费层，所有 tier 都有
//! - L2 ONNX NER → 尚未接入生产路径，能力接口必须 fail-closed
//! - L3 LLM 脱敏 → 尚未接入生产路径，能力接口必须 fail-closed
//!
//! UI 用途：Settings → Privacy 页面根据该 endpoint 渲染 toggle 状态 + 升级提示。

use attune_core::doc_privacy::{
    enforce_artifact_egress, ArtifactEgressOutcome, DocPrivacyScanner, RedactMode,
};
use attune_core::embed::EmbeddingProvider;
use attune_core::export::Artifact;
use attune_core::llm::LlmProvider;
use attune_core::llm_settings::SETTINGS_META_KEY as SETTINGS_KEY;
use attune_core::pii::Redactor;
use attune_core::platform::{classify_hardware, Tier};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// 返当前生产路径实际可用的脱敏层。
///
/// `hardware_tier` 只是诊断信息，不等于功能已实现。L2 NER 目前仍是空
/// stub，L3 也没有接入生产脱敏路径，因此两者必须始终 fail-closed。
pub async fn tier(State(state): State<SharedState>) -> Json<serde_json::Value> {
    Json(tier_payload(classify_hardware(&state.hardware)))
}

fn tier_payload(tier: Tier) -> serde_json::Value {
    json!({
        "hardware_tier": tier.label(),
        "available_layers": ["L0", "L1"],
        "l1_regex_available": true,
        "l2_ner_available": false,
        "l3_llm_available": false,
        "l3_model_suggestion": null,
        "upgrade_hint": "L2 NER 与 L3 LLM 脱敏尚未接入生产路径；当前版本仅启用 L1 正则脱敏。",
        "implementation_pending_layers": ["L2", "L3"],
        "default_active_layers": ["L1"],
    })
}

// ─────────────────────────────────────────────────────────────────────────
// v1.0.6 Privacy Logic Strategy endpoints
// per docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md §5.1
// Task 2 of v1.0.6 Privacy Logic Implementation Plan
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tier_tests {
    use super::*;

    #[test]
    fn unwired_layers_are_fail_closed_for_every_hardware_tier() {
        for hardware_tier in [
            Tier::Unsupported,
            Tier::Low,
            Tier::Mid,
            Tier::High,
            Tier::Flagship,
        ] {
            let payload = tier_payload(hardware_tier);
            assert_eq!(
                payload["available_layers"],
                json!(["L0", "L1"]),
                "hardware tier {} must not unlock unwired layers",
                hardware_tier.label()
            );
            assert_eq!(payload["l2_ner_available"], json!(false));
            assert_eq!(payload["l3_llm_available"], json!(false));
            assert!(payload["l3_model_suggestion"].is_null());
        }
    }
}

const PRIVACY_KEYS: &[&str] = &["llm", "cloud_saas", "webdav", "web_search", "telemetry"];

type RouteResult = Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;

/// Helper — read the persisted `privacy` object from settings, falling back to all-false.
fn read_privacy_block(state: &SharedState) -> serde_json::Value {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let meta = vault.store().get_meta(SETTINGS_KEY).ok().flatten();
    let settings: serde_json::Value = match meta {
        Some(data) => serde_json::from_slice(&data).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };
    settings.get("privacy").cloned().unwrap_or_else(|| {
        json!({
            "llm": false,
            "cloud_saas": false,
            "webdav": false,
            "web_search": false,
            "telemetry": false,
            "privacy_tour_seen": false,
        })
    })
}

/// Helper — write a partial privacy patch into settings (merge, not overwrite).
pub(crate) fn write_privacy_patch(
    state: &SharedState,
    patch: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let meta = vault
        .store()
        .get_meta(SETTINGS_KEY)
        .map_err(|e| e.to_string())?;
    let mut settings: serde_json::Value = match meta {
        Some(data) => serde_json::from_slice(&data).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };
    let obj = settings.as_object_mut().ok_or("settings root not object")?;
    let privacy = obj
        .entry("privacy".to_string())
        .or_insert_with(|| {
            json!({
                "llm": false,
                "cloud_saas": false,
                "webdav": false,
                "web_search": false,
                "telemetry": false,
                "privacy_tour_seen": false,
            })
        })
        .as_object_mut()
        .ok_or("privacy block not object")?;
    let mut applied = serde_json::Map::new();
    for (k, v) in patch {
        // Only accept known keys (privacy_tour_seen included for tour modal).
        if PRIVACY_KEYS.contains(&k.as_str()) || k == "privacy_tour_seen" {
            privacy.insert(k.clone(), v.clone());
            applied.insert(k.clone(), v.clone());
        }
    }
    let data = serde_json::to_vec(&settings).map_err(|e| e.to_string())?;
    vault
        .store()
        .set_meta(SETTINGS_KEY, &data)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::Value::Object(applied))
}

/// Read one outbound consent flag. Missing or malformed values fail closed.
pub(crate) fn outbound_enabled(state: &SharedState, key: &str) -> bool {
    PRIVACY_KEYS.contains(&key)
        && read_privacy_block(state)
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

/// The single route-level boundary for content sent through the configured LLM.
/// Local providers bypass cloud consent and may process L0 content. Cloud
/// providers require explicit consent, reject any L0 source, and receive only
/// the redacting wrapper's payload. Callers should compute `contains_l0` while
/// materializing their vault inputs and release the vault lock before invoking
/// the returned provider.
pub(crate) fn governed_llm(
    state: &SharedState,
    contains_l0: bool,
) -> AppResult<Arc<dyn LlmProvider>> {
    let inner = state.llm();
    if let Some(local) = inner.as_ref().filter(|provider| provider.is_local()) {
        return Ok(local.clone());
    }
    if !outbound_enabled(state, "llm") {
        return Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            json!({
                "error": "cloud LLM is disabled in Privacy settings; enable it to use this operation",
                "code": "cloud-llm-disabled",
            }),
        ));
    }
    if contains_l0 {
        return Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            json!({
                "error": "L0 content cannot be sent to a cloud LLM",
                "code": "l0-cloud-blocked",
            }),
        ));
    }
    let inner = inner.ok_or_else(|| {
        AppError::detailed(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "no LLM provider is configured",
                "code": "llm-unavailable",
            }),
        )
    })?;
    Ok(Arc::new(
        attune_core::redacting_llm::RedactingLlmProvider::with_default_redactor(inner),
    ))
}

/// Embedding equivalent of [`governed_llm`]. Search queries and watch anchors
/// can contain PII even though they are not stored vault items, so a cloud
/// embedding provider is exposed only after consent and only through a wrapper
/// that sends redacted strings. `None` means the caller must degrade to a
/// non-vector path. L0 is never eligible for cloud embedding.
pub(crate) fn governed_embedding(
    state: &SharedState,
    contains_l0: bool,
) -> Option<Arc<dyn EmbeddingProvider>> {
    let (inner, is_local) = state.embedding_with_locality();
    let inner = inner?;
    if is_local {
        return Some(inner);
    }
    if contains_l0 || !outbound_enabled(state, "llm") {
        return None;
    }
    let vault_unlocked = state
        .vault
        .lock()
        .map(|vault| matches!(vault.state(), attune_core::vault::VaultState::Unlocked))
        .unwrap_or(false);
    if !vault_unlocked {
        return None;
    }
    Some(Arc::new(RedactingEmbeddingProvider {
        inner,
        redactor: Redactor::new(),
    }))
}

struct RedactingEmbeddingProvider {
    inner: Arc<dyn EmbeddingProvider>,
    redactor: Redactor,
}

impl EmbeddingProvider for RedactingEmbeddingProvider {
    fn embed(
        &self,
        texts: &[&str],
    ) -> attune_core::error::Result<(Vec<Vec<f32>>, attune_core::usage::TokenUsage)> {
        let (redacted, _) = self.redactor.redact_batch(texts);
        let wire: Vec<&str> = redacted.iter().map(String::as_str).collect();
        self.inner.embed(&wire)
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn model_name(&self) -> String {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod governed_provider_tests {
    use super::*;

    struct RecordingEmbedding {
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl EmbeddingProvider for RecordingEmbedding {
        fn embed(
            &self,
            texts: &[&str],
        ) -> attune_core::error::Result<(Vec<Vec<f32>>, attune_core::usage::TokenUsage)> {
            *self.seen.lock().unwrap_or_else(|e| e.into_inner()) =
                texts.iter().map(|text| (*text).to_string()).collect();
            Ok((
                vec![vec![0.0; 2]; texts.len()],
                attune_core::usage::TokenUsage::empty("test", "embedding"),
            ))
        }

        fn dimensions(&self) -> usize {
            2
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    fn test_state() -> SharedState {
        let dir = tempfile::tempdir().expect("tempdir");
        // Keep the directory alive for the process-long test state. SQLite has
        // already opened the file, but the vault also retains paths for reloads.
        let path = dir.keep();
        let vault =
            attune_core::vault::Vault::open(&path.join("vault.db"), &path).expect("open vault");
        vault.setup("P@ss-governed-llm").expect("setup vault");
        Arc::new(crate::state::AppState::new(vault, false))
    }

    fn code(error: AppError) -> String {
        match error {
            AppError::Detailed { body, .. } => body["code"].as_str().unwrap_or_default().into(),
            other => panic!("expected detailed refusal, got {other:?}"),
        }
    }

    #[test]
    fn cloud_requires_consent_and_rejects_l0() {
        let state = test_state();
        state.set_llm(Some(Arc::new(attune_core::llm::MockLlmProvider::new(
            "cloud",
        ))));
        assert_eq!(
            code(
                governed_llm(&state, false)
                    .err()
                    .expect("consent is required"),
            ),
            "cloud-llm-disabled"
        );
        set_outbound_enabled(&state, "llm", true).expect("enable consent");
        assert_eq!(
            code(
                governed_llm(&state, true)
                    .err()
                    .expect("L0 must be blocked"),
            ),
            "l0-cloud-blocked"
        );
        assert!(governed_llm(&state, false).is_ok());
    }

    #[test]
    fn local_provider_allows_l0_without_cloud_consent() {
        let state = test_state();
        state.set_llm(Some(Arc::new(attune_core::llm::OpenAiLlmProvider::new(
            "http://127.0.0.1:8090/v1",
            "",
            "llm-chat",
        ))));
        assert!(governed_llm(&state, true).is_ok());
    }

    #[test]
    fn cloud_embedding_requires_consent_redacts_and_blocks_l0() {
        let state = test_state();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        state.set_embedding(Some(Arc::new(RecordingEmbedding { seen: seen.clone() })));

        assert!(governed_embedding(&state, false).is_none());
        set_outbound_enabled(&state, "llm", true).expect("enable consent");
        assert!(governed_embedding(&state, true).is_none());

        let embedding = governed_embedding(&state, false).expect("consented cloud embedding");
        embedding
            .embed(&["联系 13800138000 或 zhangsan@example.com"])
            .expect("embed redacted query");
        let sent = seen.lock().unwrap_or_else(|e| e.into_inner()).join("\n");
        assert!(!sent.contains("13800138000"), "phone reached wire: {sent}");
        assert!(
            !sent.contains("zhangsan@example.com"),
            "email reached wire: {sent}"
        );
        assert!(sent.contains("PHONE_") || sent.contains("EMAIL_"));

        set_outbound_enabled(&state, "llm", false).expect("disable cloud consent");
        state.set_embedding_with_locality(
            Some(Arc::new(RecordingEmbedding { seen: seen.clone() })),
            true,
        );
        assert!(governed_embedding(&state, true).is_some());
    }
}

/// Persist one outbound consent flag through the same allowlisted merge path as
/// the public privacy settings endpoint.
pub(crate) fn set_outbound_enabled(
    state: &SharedState,
    key: &str,
    enabled: bool,
) -> Result<(), String> {
    if !PRIVACY_KEYS.contains(&key) {
        return Err(format!("unknown outbound privacy key: {key}"));
    }
    let mut patch = serde_json::Map::new();
    patch.insert(key.to_string(), serde_json::Value::Bool(enabled));
    write_privacy_patch(state, &patch).map(|_| ())
}

/// Helper — write a privacy-audit event into `audit_log` table.
/// We use category="privacy" + a kebab-case `kind` so the existing
/// `/api/v1/audit/log` endpoint surfaces these events for DSAR review.
///
/// **Contract**: redacted_meta MUST NOT contain prompts / responses /
/// API keys / passwords. We don't take a meta payload here — the existing
/// `audit_log` schema is fixed-shape (route + category + kind + counts).
fn record_privacy_event(state: &SharedState, kind: &str) {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    // store() returns &Store directly; if the underlying connection is sealed
    // the audit_log_record call will return an Err which we swallow.
    let _ = vault.store().audit_log_record(
        "/api/v1/privacy",
        "privacy",
        kind,
        0, // redacted_count: privacy events carry no PII payload
        0, // original_len: ditto
    );
}

/// `GET /api/v1/privacy/status` — Privacy dashboard snapshot.
///
/// Response shape:
/// ```json
/// {
///   "outbound": {
///     "llm":        { "enabled": false },
///     "cloud_saas": { "enabled": false },
///     "webdav":     { "enabled": false },
///     "web_search": { "enabled": false },
///     "telemetry":  { "enabled": false }
///   },
///   "vault":    { "state": "sealed" | "locked" | "unlocked" },
///   "redactor": { "patterns_active": 12 }
/// }
/// ```
pub async fn status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let privacy = read_privacy_block(&state);

    let mut outbound = serde_json::Map::new();
    for key in PRIVACY_KEYS {
        let enabled = privacy.get(*key).and_then(|v| v.as_bool()).unwrap_or(false);
        outbound.insert((*key).into(), json!({ "enabled": enabled }));
    }

    let vault_state_label = {
        let g = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        match g.state() {
            attune_core::vault::VaultState::Sealed => "sealed",
            attune_core::vault::VaultState::Locked => "locked",
            attune_core::vault::VaultState::Unlocked => "unlocked",
        }
    };

    Json(json!({
        "outbound": serde_json::Value::Object(outbound),
        "vault": { "state": vault_state_label },
        "redactor": {
            // L1 builtin patterns count — 12 per attune-core/src/pii/mod.rs
            "patterns_active": 12,
            "l1_active": true,
        },
        "privacy_tour_seen": privacy.get("privacy_tour_seen")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }))
}

/// `PATCH /api/v1/privacy/settings` — Toggle a subset of privacy keys.
///
/// Body: any subset of `{llm, cloud_saas, webdav, web_search, telemetry,
/// privacy_tour_seen}` with boolean values. Unknown keys are silently
/// dropped. Returns the applied diff.
///
/// Note: telemetry isolation guard (per `settings.rs::is_telemetry_path_allowed`)
/// does NOT apply here because this endpoint is privacy-only by construction —
/// it only accepts the 6 privacy keys.
pub async fn settings_patch(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> RouteResult {
    let patch = body.as_object().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "body must be an object"})),
        )
    })?;

    let applied = write_privacy_patch(&state, patch)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))))?;

    record_privacy_event(&state, "settings_changed");

    Ok(Json(json!({ "ok": true, "applied": applied })))
}

/// `POST /api/v1/privacy/lock` — Immediately lock the vault.
///
/// User-driven lock (vs idle timeout). Returns the new vault state.
pub async fn lock(State(state): State<SharedState>) -> RouteResult {
    let result = crate::routes::vault::lock_and_clear_runtime(&state).await;
    match result {
        Ok(()) => {
            record_privacy_event(&state, "vault_lock");
            Ok(Json(json!({ "ok": true, "vault_state": "locked" })))
        }
        Err(e) => Err((StatusCode::CONFLICT, Json(json!({"error": e.to_string()})))),
    }
}

/// `POST /api/v1/privacy/wipe-cloud-session` — Revoke the persisted cloud
/// session, remove membership-managed credentials, and disable Cloud SaaS
/// egress. This shares the same teardown as `/member/logout`.
pub async fn wipe_cloud_session(State(state): State<SharedState>) -> RouteResult {
    let outcome = crate::routes::member::perform_logout(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))))?;

    record_privacy_event(&state, "cloud_session_wiped");

    Ok(Json(json!({
        "ok": true,
        "cleared_local_token": outcome.removed_local_session,
        "cleared_member_credentials": outcome.cleared_member_credentials,
        "remote_logout_succeeded": outcome.remote_logout_succeeded,
        "cloud_saas": false,
    })))
}

// ─────────────────────────────────────────────────────────────────────────
// Document-privacy (INT-2) — classification + export-egress preview
// per docs/superpowers/specs/2026-06-20-privacy-layer-enhancement.md §5
// ─────────────────────────────────────────────────────────────────────────

/// Industry confidential markers (INT-2 pro write-end) — merged from installed
/// pro plugins (`plugin.yaml::confidential_keywords:`) + the optional
/// `settings.privacy.export_confidential_keywords` override. Mirrors
/// `routes::export::export_extra_keywords` exactly so the preview verdict matches
/// the real export. Empty on a bare OSS install (generic markers only).
fn export_extra_keywords(state: &SharedState) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let plugin_registry = crate::routes::plugins::current_plugin_registry(state);
    for kw in plugin_registry.all_confidential_keywords() {
        if seen.insert(kw.clone()) {
            out.push(kw);
        }
    }
    let bytes = {
        let g = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        g.store().get_meta("app_settings").ok().flatten()
    };
    if let Some(arr) = bytes
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|s| {
            s.get("privacy")
                .and_then(|p| p.get("export_confidential_keywords"))
                .and_then(|v| v.as_array().cloned())
        })
    {
        for kw in arr.iter().filter_map(|v| v.as_str()) {
            let s = kw.trim();
            if !s.is_empty() && seen.insert(s.to_string()) {
                out.push(s.to_string());
            }
        }
    }
    out
}

#[derive(Deserialize)]
pub struct DocScanRequest {
    /// Plain extracted document text to classify.
    pub text: String,
}

/// `POST /api/v1/doc-privacy/scan` — classify already-extracted document text.
///
/// Returns the document grade + PII summary (privacy-first: **no PII values**).
/// `blocked == true` ⇔ a confidential marker was found ⇔ export is fail-closed.
/// 🆓 zero-cost (regex + dictionary, no LLM, no vault DEK needed).
pub async fn doc_scan(Json(req): Json<DocScanRequest>) -> Json<serde_json::Value> {
    let redactor = Redactor::default();
    let scanner = DocPrivacyScanner::new(&redactor);
    let report = scanner.analyze_text(&req.text);
    Json(json!({
        "classification": report.classification.as_str(),
        "blocked": report.blocked,
        "block_reason": report.block_reason,
        "warning": report.warning,
        "pii_summary": report.summary,
        "pii_count": report.entities.len(),
    }))
}

#[derive(Deserialize)]
pub struct ExportPreviewRequest {
    /// The export IR (Table | Document) the UI is about to download.
    pub artifact: Artifact,
}

/// `POST /api/v1/doc-privacy/export-preview` — dry-run the export egress gate.
///
/// Tells the UI **before** it hits `/export` whether the artifact would be
/// blocked (confidential) or redacted (and how many PII spans), so it can show a
/// "this download will be redacted / cannot be exported" notice. Does NOT render
/// any file or leak PII values. 🆓 zero-cost.
pub async fn doc_export_preview(
    State(state): State<SharedState>,
    Json(req): Json<ExportPreviewRequest>,
) -> Json<serde_json::Value> {
    let redactor = Redactor::default();
    let extra = export_extra_keywords(&state);
    match enforce_artifact_egress(&redactor, &req.artifact, RedactMode::Reversible, &extra) {
        ArtifactEgressOutcome::Blocked { reason } => Json(json!({
            "decision": "blocked",
            "blocked": true,
            "reason": reason,
            "classification": "classified",
        })),
        ArtifactEgressOutcome::Allowed {
            redacted,
            classification,
            ..
        } => Json(json!({
            "decision": "allowed",
            "blocked": false,
            "will_redact": redacted > 0,
            "redacted_count": redacted,
            "classification": classification.as_str(),
        })),
    }
}
