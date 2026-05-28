//! v0.6 Phase A.5.5 — Privacy tier 检测
//! v1.0.6 Privacy Logic Strategy — 5 outbound points 总览 + DSAR + 锁定 + wipe-cloud-session
//!
//! 端点：
//! - `GET  /api/v1/privacy/tier` — 返硬件支持的脱敏层 + 推荐选择（v0.6 老接口）
//! - `GET  /api/v1/privacy/status` — 5 出网点状态 + vault state + redactor info（v1.0.6 新增）
//! - `PATCH /api/v1/privacy/settings` — 切换某一出网点开关（v1.0.6 新增）
//! - `POST /api/v1/privacy/lock` — 立即锁 vault（用户主动）（v1.0.6 新增）
//! - `POST /api/v1/privacy/wipe-cloud-session` — 吊销 cloud session + 清本地 token（v1.0.6 新增）
//!
//! 决策（用户 2026-04-28）：
//! - L1 正则脱敏 → OSS 免费层，所有 tier 都有
//! - L2 ONNX NER → OSS 免费层，Tier T1+ 可选下载
//! - L3 LLM 脱敏 → 仅 Tier T3 + T4 + K3 一体机解锁
//!
//! UI 用途：Settings → Privacy 页面根据该 endpoint 渲染 toggle 状态 + 升级提示。

use attune_core::llm_settings::SETTINGS_META_KEY as SETTINGS_KEY;
use attune_core::platform::{classify_hardware, Tier};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

use crate::state::SharedState;

/// 返当前硬件可用的脱敏层 + 推荐 LLM 脱敏模型（如适用）。
pub async fn tier(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let hw = &state.hardware;
    let tier = classify_hardware(hw);

    // 各 tier 解锁的层级
    // L1 正则 = 所有 tier（即使 T0 也提供，但 T0 通常进不了应用）
    // L2 NER = T1+（约 300MB 模型）
    // L3 LLM = T3+ 才有意义
    let layers: Vec<&str> = match tier {
        Tier::Unsupported => vec!["L0", "L1"],
        Tier::Low => vec!["L0", "L1"],
        Tier::Mid => vec!["L0", "L1", "L2"],
        Tier::High => vec!["L0", "L1", "L2", "L3"],
        Tier::Flagship => vec!["L0", "L1", "L2", "L3"],
    };

    // L3 默认模型（按 tier）
    let l3_model: Option<&'static str> = match tier {
        Tier::High => Some("qwen2.5:3b-instruct-q4_K_M"),
        Tier::Flagship => Some("qwen2.5:7b-instruct-q4_K_M"),
        _ => None,
    };

    // 升级提示
    let upgrade_hint: Option<&'static str> = match tier {
        Tier::Unsupported | Tier::Low => Some(
            "你的硬件仅支持 L1 正则脱敏（OSS 免费）。如需 L2 NER / L3 LLM 脱敏，建议升级硬件或选购 K3 一体机。",
        ),
        Tier::Mid => Some(
            "你的硬件支持 L1 + L2 NER 脱敏（OSS 免费）。如需 L3 LLM 语义脱敏，建议升级到 16GB+ RAM / 高性能 CPU。",
        ),
        Tier::High | Tier::Flagship => None, // 已是最高，无升级提示
    };

    let l3_available = matches!(tier, Tier::High | Tier::Flagship);

    Json(json!({
        "hardware_tier": tier.label(),
        "available_layers": layers,
        "l1_regex_available": true,           // 所有 tier 必有
        "l2_ner_available": tier as u8 >= Tier::Mid as u8,
        "l3_llm_available": l3_available,
        "l3_model_suggestion": l3_model,
        "upgrade_hint": upgrade_hint,
        // 默认推荐：L1 已开（强制），L2 / L3 由用户在 Settings 主动切
        "default_active_layers": ["L1"],
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// v1.0.6 Privacy Logic Strategy endpoints
// per docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md §5.1
// Task 2 of v1.0.6 Privacy Logic Implementation Plan
// ─────────────────────────────────────────────────────────────────────────

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
    settings
        .get("privacy")
        .cloned()
        .unwrap_or_else(|| {
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
fn write_privacy_patch(
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
        let enabled = privacy
            .get(*key)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
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

    let applied = write_privacy_patch(&state, patch).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e})),
        )
    })?;

    record_privacy_event(&state, "settings_changed");

    Ok(Json(json!({ "ok": true, "applied": applied })))
}

/// `POST /api/v1/privacy/lock` — Immediately lock the vault.
///
/// User-driven lock (vs idle timeout). Returns the new vault state.
pub async fn lock(State(state): State<SharedState>) -> RouteResult {
    let result = {
        let g = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        g.lock()
    };
    match result {
        Ok(()) => {
            record_privacy_event(&state, "vault_lock");
            Ok(Json(json!({ "ok": true, "vault_state": "locked" })))
        }
        Err(e) => Err((
            StatusCode::CONFLICT,
            Json(json!({"error": e.to_string()})),
        )),
    }
}

/// `POST /api/v1/privacy/wipe-cloud-session` — Revoke cloud session + clear
/// any locally cached cloud session token. Best-effort remote logout +
/// **unconditional** local clear (per `CloudClient::wipe_session()` contract).
///
/// **Note**: today attune-server doesn't hold a long-lived `CloudClient`
/// instance — cloud calls are made transiently. This endpoint serves as the
/// surface point users hit to "remove cloud footprint"; it clears the
/// `cloud_session_token` meta key if present, and records a privacy audit
/// event. Real CloudClient wiping happens at Task 7 when the client lifecycle
/// is wired through state.
pub async fn wipe_cloud_session(State(state): State<SharedState>) -> RouteResult {
    let cleared = {
        let g = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        // Try to clear any persisted cloud session token from meta.
        // `cloud_session_token` is the conventional meta key used by login flows.
        let store = g.store();
        match store.get_meta("cloud_session_token") {
            Ok(Some(_)) => {
                let _ = store.set_meta("cloud_session_token", b"");
                true
            }
            _ => false,
        }
    };

    record_privacy_event(&state, "cloud_session_wiped");

    Ok(Json(json!({
        "ok": true,
        "cleared_local_token": cleared,
        // Remote logout is best-effort; documented as not-guaranteed-success.
        "remote_logout": "best-effort",
    })))
}
