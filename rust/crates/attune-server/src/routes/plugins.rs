//! Plugin marketplace routes.
//! W4 E1 (2026-04-27): 加 enabled 字段 + toggle 端点支持 marketplace UI。

use crate::error::{AppError, AppResult};
use crate::routes::errors::{internal, vault_locked};
use crate::state::SharedState;
use attune_core::plugin_sig::TrustMode;
use attune_core::taxonomy::Taxonomy;
use axum::extract::{Path, State};
use axum::Json;
use std::collections::HashMap;
use std::sync::Arc;

const SETTINGS_KEY: &str = "app_settings";

/// 从 settings.json 读 plugins.disabled 数组。vault locked 时返回空（默认全启用）。
fn load_disabled_plugin_ids(state: &SharedState) -> Vec<String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek_ok = vault.dek_db().is_ok();
    if !dek_ok {
        return Vec::new();
    }
    let raw = match vault.store().get_meta(SETTINGS_KEY) {
        Ok(Some(b)) => b,
        _ => return Vec::new(),
    };
    let json: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    json.get("plugins")
        .and_then(|p| p.get("disabled"))
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn is_safe_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !id.contains("..")
}

pub(crate) fn plugin_decrypt_keys(state: &SharedState) -> HashMap<String, Vec<u8>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(dek) = vault.dek_db() else {
        return HashMap::new();
    };
    vault
        .store()
        .list_entitlements(&dek)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            row.decrypt_key
                .filter(|key| !key.is_empty())
                .map(|key| (row.plugin_id, key.into_bytes()))
        })
        .collect()
}

pub(crate) fn plugin_trust_settings(state: &SharedState) -> (TrustMode, Vec<String>) {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let raw = match vault.store().get_meta(SETTINGS_KEY) {
        Ok(Some(b)) => b,
        _ => return (TrustMode::Warn, Vec::new()),
    };
    let json: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(_) => return (TrustMode::Warn, Vec::new()),
    };
    let mode = json
        .get("plugin_trust_mode")
        .and_then(|m| serde_json::from_value::<TrustMode>(m.clone()).ok())
        .unwrap_or(TrustMode::Warn);
    let pubkeys = json
        .get("plugin_trusted_pubkeys")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .collect()
        })
        .unwrap_or_default();
    (mode, pubkeys)
}

pub(crate) fn current_plugin_registry(
    state: &SharedState,
) -> Arc<attune_core::plugin_registry::PluginRegistry> {
    match attune_core::plugin_registry::PluginRegistry::default_plugins_dir().and_then(|dir| {
        let decrypt_keys = plugin_decrypt_keys(state);
        let (trust_mode, trusted_pubkeys) = plugin_trust_settings(state);
        attune_core::plugin_registry::PluginRegistry::scan_with_keys_and_trust(
            &dir,
            &decrypt_keys,
            trust_mode,
            &trusted_pubkeys,
        )
    }) {
        Ok((registry, warnings)) => {
            for warning in warnings {
                tracing::warn!("plugin live-scan warning before plugin list: {warning}");
            }
            Arc::new(registry)
        }
        Err(e) => {
            tracing::warn!(
                "plugin live-scan failed before plugin list; using startup registry: {e}"
            );
            state.plugin_registry.clone()
        }
    }
}

/// GET /api/v1/plugins — 列出所有可用插件（内置 taxonomy + plugin_registry 装载）+ enabled 状态
pub async fn list(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let disabled = load_disabled_plugin_ids(&state);
    let is_enabled = |id: &str| !disabled.iter().any(|d| d == id);
    let registry = current_plugin_registry(&state);

    // Trust-chain T10 (spec §5.1): each plugin reports `trust` (real verify result,
    // T9 — not hardcoded) + `entitlement_status` (runtime state from EntitlementCache).
    // Free / unregistered plugins → entitlement_status "free"; trust defaults to
    // "unsigned" when the registry has no verified label (builtin taxonomy plugins).
    let now = chrono::Utc::now();
    let trust_of = |id: &str| -> &'static str {
        registry
            .plugin_trust(id)
            .map(|t| t.as_api_str())
            .unwrap_or("unsigned")
    };
    let entitlement_status_of = |id: &str, requires_entitlement: bool| -> &'static str {
        if requires_entitlement && !state.entitlement_cache.contains(id) {
            return "unlicensed";
        }
        let tier = state.entitlement_cache.tier(id);
        if requires_entitlement
            && tier
                .as_deref()
                .map(|t| t.trim().eq_ignore_ascii_case("free"))
                .unwrap_or(true)
        {
            return "unlicensed";
        }
        state.entitlement_cache.status(id, &now).as_api_str()
    };

    // 收集两个数据源:
    // 1. taxonomy.plugins (内置 dimensions yaml)
    // 2. plugin_registry (用户从 plugins/ 目录加载的, e.g. law-pro)
    let mut list: Vec<serde_json::Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Some(tax) = state
        .taxonomy
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        for p in &tax.plugins {
            seen.insert(p.id.clone());
            list.push(serde_json::json!({
                "id": p.id,
                "name": p.name,
                "version": p.version,
                "description": p.description,
                "source": if ["tech", "law", "presales", "patent"].contains(&p.id.as_str()) { "builtin" } else { "user" },
                "enabled": is_enabled(&p.id),
                "trust": trust_of(&p.id),
                "entitlement_status": entitlement_status_of(&p.id, false),
                "type": "taxonomy",
                "dimensions": p.dimensions.iter().map(|d| serde_json::json!({
                    "name": d.name,
                    "label": d.label,
                    "description": d.description,
                })).collect::<Vec<_>>(),
            }));
        }
    }

    // plugin_registry: 用户安装的 plugins (attune-pro vertical 等)
    for plugin in registry.plugins() {
        let m = &plugin.manifest;
        if seen.contains(&m.id) {
            continue; // 避免重复
        }
        let requires_entitlement =
            crate::routes::agents::plugin_requires_entitlement(&registry, &m.id);
        let agents = m
            .agents
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "description": a.description,
                    // case_kinds 非空 = 项目级 agent（绑 project.kind）；空 = 独立。
                    // 前端据此把触发入口路由到 Project 详情 vs Skills。
                    "case_kinds": a.case_kinds,
                })
            })
            .collect::<Vec<_>>();
        // ui_components：插件声明的 plugin-form（PluginForm 渲染 + agent-run 触发）
        let ui_components = m
            .ui_components
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "target": c.target,
                    "description": c.description,
                })
            })
            .collect::<Vec<_>>();
        list.push(serde_json::json!({
            "id": m.id,
            "name": m.name,
            "version": m.version,
            "description": m.description,
            "source": "user",
            "enabled": is_enabled(&m.id),
            "trust": trust_of(&m.id),
            "entitlement_status": entitlement_status_of(&m.id, requires_entitlement),
            "type": m.plugin_type.clone(),
            "agents": agents,
            "ui_components": ui_components,
            "tier": m.pricing.as_ref().map(|p| p.tier.clone()).unwrap_or_else(|| "free".into()),
        }));
    }

    if !list.is_empty() {
        return Ok(Json(serde_json::json!({
            "plugins": list,
            "official_pubkeys": attune_core::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS,
        })));
    }

    // Fallback: vault locked, only return builtins (assumed enabled)
    let plugins =
        Taxonomy::load_builtin_plugins().map_err(|e| internal("load_builtin_plugins", e))?;
    let list: Vec<serde_json::Value> = plugins
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "version": p.version,
                "description": p.description,
                "source": "builtin",
                "enabled": true,
                "trust": trust_of(&p.id),
                "entitlement_status": entitlement_status_of(&p.id, false),
                "dimensions": p.dimensions.iter().map(|d| serde_json::json!({
                    "name": d.name,
                    "label": d.label,
                    "description": d.description,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "plugins": list,
        "official_pubkeys": attune_core::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS,
    })))
}

/// POST /api/v1/plugins/{id}/toggle — 翻转 enabled 状态。返回新 enabled 值。
/// 修改 settings.plugins.disabled 数组并落盘。Vault 必须 unlocked。
pub async fn toggle(
    State(state): State<SharedState>,
    Path(plugin_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;

    // 读 → 修改 → 写
    let mut json: serde_json::Value = vault
        .store()
        .get_meta(SETTINGS_KEY)
        .map_err(|e| internal("get_meta settings", e))?
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    // 确保 plugins.disabled 路径存在
    let plugins = json
        .as_object_mut()
        .ok_or_else(|| internal("settings malformed", "expected object"))?
        .entry("plugins")
        .or_insert_with(|| serde_json::json!({"disabled": []}));
    let disabled = plugins
        .as_object_mut()
        .ok_or_else(|| internal("settings.plugins malformed", "expected object"))?
        .entry("disabled")
        .or_insert_with(|| serde_json::json!([]));
    let arr = disabled
        .as_array_mut()
        .ok_or_else(|| internal("settings.plugins.disabled malformed", "expected array"))?;

    let pos = arr.iter().position(|v| v.as_str() == Some(&plugin_id));
    let now_enabled = if let Some(idx) = pos {
        arr.remove(idx);
        true
    } else {
        arr.push(serde_json::Value::String(plugin_id.clone()));
        false
    };

    let bytes = serde_json::to_vec(&json).map_err(|e| internal("serialize settings", e))?;
    vault
        .store()
        .set_meta(SETTINGS_KEY, &bytes)
        .map_err(|e| internal("set_meta settings", e))?;

    Ok(Json(serde_json::json!({
        "id": plugin_id,
        "enabled": now_enabled,
    })))
}

/// DELETE /api/v1/plugins/{id} — 卸载本机插件目录并清理本地 entitlement。
///
/// 只删除 default plugins dir 下与 id 同名的目录；id 经字符白名单校验，避免路径穿越。
/// Paid 会员按 SettingsLocks 锁定卸载，防误删 cloud 自动同步的付费插件。
pub async fn uninstall(
    State(state): State<SharedState>,
    Path(plugin_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let member = state
        .member_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if !attune_core::member_session::SettingsLocks::for_state(&member).can_edit("plugin_uninstall")
    {
        return Err(AppError::Forbidden(
            "plugin uninstall is locked for paid members".into(),
        ));
    }
    if !is_safe_plugin_id(&plugin_id) {
        return Err(AppError::BadRequest("invalid plugin id".into()));
    }
    let plugins_dir = attune_core::plugin_registry::PluginRegistry::default_plugins_dir()
        .map_err(|e| AppError::Internal(format!("plugins dir unavailable: {e}")))?;
    let target = plugins_dir.join(&plugin_id);
    if !target.is_dir() {
        return Err(AppError::NotFound(format!(
            "plugin '{plugin_id}' is not installed"
        )));
    }

    let target_for_delete = target.clone();
    tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&target_for_delete))
        .await
        .map_err(|e| AppError::Internal(format!("plugin uninstall task join error: {e}")))?
        .map_err(|e| AppError::Internal(format!("remove plugin directory failed: {e}")))?;

    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        if vault.dek_db().is_ok() {
            if let Err(e) = vault.store().delete_entitlement(&plugin_id) {
                tracing::warn!("plugins: failed to delete entitlement for {plugin_id}: {e}");
            }
        }
    }
    state
        .entitlement_cache
        .set_status(&plugin_id, "revoked", &chrono::Utc::now().to_rfc3339());

    Ok(Json(serde_json::json!({
        "status": "ok",
        "plugin_id": plugin_id,
        "removed_path": target.display().to_string(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::store::plugin_entitlements::EntitlementRow;
    use std::sync::Arc;

    fn ent_row(plugin_id: &str, tier: &str, status: &str) -> EntitlementRow {
        EntitlementRow {
            plugin_id: plugin_id.into(),
            license_id: "lic-x".into(),
            decrypt_key: None,
            tier: tier.into(),
            status: status.into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: "2026-06-12T00:00:00+00:00".into(),
            grace_started_at: None,
            updated_at: "2026-06-12T00:00:00+00:00".into(),
        }
    }

    /// Trust-chain T10 (spec §5.1): GET /api/v1/plugins lists each plugin with `trust`
    /// (real verify result — an unsigned user plugin → "unsigned") + `entitlement_status`
    /// (runtime state from EntitlementCache — a seeded active paid license → "active").
    /// Drives the real `list` handler against an AppState that scanned a temp plugins dir.
    #[tokio::test]
    async fn list_returns_trust_and_status() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        // Pin attune's data dir to a temp dir via the thread-local override seam —
        // works on Windows too (dirs ignores XDG_DATA_HOME there). Restored on drop.
        let _dir = crate::test_support::override_data_dir(tmp.path().join("attune"));
        let plugin_dir = tmp.path().join("attune").join("plugins").join("test-pro");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: test-pro\nname: Test Pro\ntype: industry\nversion: \"1.0.0\"\n",
        )
        .expect("write plugin.yaml");

        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-plugins-trust-not-real").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));
        // Seed entitlement cache: test-pro is a paid, active license → status "active".
        state
            .entitlement_cache
            .upsert(ent_row("test-pro", "paid", "active"));

        let resp = list(axum::extract::State(state)).await.expect("list ok");
        let body = resp.0; // Json(Value)
        let plugins = body["plugins"].as_array().expect("plugins array");
        let entry = plugins
            .iter()
            .find(|p| p["id"] == "test-pro")
            .expect("test-pro listed");

        // Unsigned user plugin (no plugin.sig) → real verify yields trust "unsigned".
        assert_eq!(
            entry["trust"], "unsigned",
            "trust is the real verify result, not hardcoded"
        );
        // Seeded paid active license → entitlement_status "active".
        assert_eq!(entry["entitlement_status"], "active");
    }

    #[tokio::test]
    async fn list_live_scans_plugins_installed_after_startup() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("attune");
        let _dir = crate::test_support::override_data_dir(data_dir.clone());

        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-plugin-live-scan").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));

        let plugin_dir = data_dir.join("plugins").join("late-pro");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: late-pro\nname: Late Pro\ntype: industry\nversion: \"1.2.3\"\n",
        )
        .expect("write plugin.yaml");

        let resp = list(axum::extract::State(state)).await.expect("list ok");
        let body = resp.0;
        let plugins = body["plugins"].as_array().expect("plugins array");
        let entry = plugins
            .iter()
            .find(|p| p["id"] == "late-pro")
            .expect("late-pro listed after startup");

        assert_eq!(entry["version"], "1.2.3");
        assert_eq!(entry["entitlement_status"], "unlicensed");
    }

    #[tokio::test]
    async fn list_honors_strict_trust_mode_at_runtime() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("attune");
        let _dir = crate::test_support::override_data_dir(data_dir.clone());

        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-plugin-strict").expect("setup");
        let state = Arc::new(crate::state::AppState::new(vault, false));
        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            vault
                .store()
                .set_meta(
                    SETTINGS_KEY,
                    br#"{"plugin_trust_mode":"strict","plugin_trusted_pubkeys":[]}"#,
                )
                .expect("set strict trust settings");
        }

        let plugin_dir = data_dir.join("plugins").join("unsigned-runtime");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: unsigned-runtime\nname: Unsigned Runtime\ntype: industry\nversion: \"1.0.0\"\n",
        )
        .expect("write plugin.yaml");

        let resp = list(axum::extract::State(state)).await.expect("list ok");
        let body = resp.0;
        let plugins = body["plugins"].as_array().expect("plugins array");
        assert!(
            !plugins.iter().any(|p| p["id"] == "unsigned-runtime"),
            "runtime live scan must enforce plugin_trust_mode=strict"
        );
    }
}
