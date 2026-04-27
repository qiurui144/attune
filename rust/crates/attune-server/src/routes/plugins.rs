//! Plugin marketplace routes.
//! W4 E1 (2026-04-27): 加 enabled 字段 + toggle 端点支持 marketplace UI。

use axum::extract::{Path, State};
use axum::Json;
use crate::routes::errors::{internal, vault_locked, RouteError};
use crate::state::SharedState;
use attune_core::taxonomy::Taxonomy;

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

/// GET /api/v1/plugins — 列出所有可用插件（内置 + 用户）+ enabled 状态
pub async fn list(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, RouteError> {
    let disabled = load_disabled_plugin_ids(&state);
    let is_enabled = |id: &str| !disabled.iter().any(|d| d == id);

    if let Some(tax) = state.taxonomy.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let list: Vec<serde_json::Value> = tax.plugins.iter().map(|p| serde_json::json!({
            "id": p.id,
            "name": p.name,
            "version": p.version,
            "description": p.description,
            "source": if ["tech", "law", "presales", "patent"].contains(&p.id.as_str()) { "builtin" } else { "user" },
            "enabled": is_enabled(&p.id),
            "dimensions": p.dimensions.iter().map(|d| serde_json::json!({
                "name": d.name,
                "label": d.label,
                "description": d.description,
            })).collect::<Vec<_>>(),
        })).collect();
        return Ok(Json(serde_json::json!({"plugins": list})));
    }

    // Fallback: vault locked, only return builtins (assumed enabled)
    let plugins = Taxonomy::load_builtin_plugins().map_err(|e| internal("load_builtin_plugins", e))?;
    let list: Vec<serde_json::Value> = plugins.iter().map(|p| serde_json::json!({
        "id": p.id,
        "name": p.name,
        "version": p.version,
        "description": p.description,
        "source": "builtin",
        "enabled": true,
        "dimensions": p.dimensions.iter().map(|d| serde_json::json!({
            "name": d.name,
            "label": d.label,
            "description": d.description,
        })).collect::<Vec<_>>(),
    })).collect();
    Ok(Json(serde_json::json!({"plugins": list})))
}

/// POST /api/v1/plugins/{id}/toggle — 翻转 enabled 状态。返回新 enabled 值。
/// 修改 settings.plugins.disabled 数组并落盘。Vault 必须 unlocked。
pub async fn toggle(
    State(state): State<SharedState>,
    Path(plugin_id): Path<String>,
) -> Result<Json<serde_json::Value>, RouteError> {
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
