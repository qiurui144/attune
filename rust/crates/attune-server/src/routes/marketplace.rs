//! GET /api/v1/marketplace/plugins  — 列 hub 上对当前 license 可见的插件
//! POST /api/v1/marketplace/plugins/{id}/install — 启动 trial 或安装
//!
//! 默认走未配置的离线 provider；登录/激活会员后切换到真实 PluginHub。
//!
//! Local-fs fallback: when the hub returns an empty plugin list and the provider
//! is "mock", this route also scans the filesystem plugins directory and merges
//! locally installed plugins into the response so the Marketplace UI shows
//! something useful instead of "no plugins available".

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::state::SharedState;

type MarketplaceError = (StatusCode, Json<serde_json::Value>);

#[derive(Serialize)]
pub struct ListResponse {
    pub hub_version: String,
    pub user_plan: String,
    pub upgrade_url: String,
    pub plugins: Vec<attune_core::plugin_hub::PluginListing>,
    pub provider: String,
    pub installed_versions: HashMap<String, String>,
}

#[derive(Deserialize)]
pub struct InstallRequest {
    #[serde(default)]
    pub device_fp: Option<String>,
}

fn hub_arc(
    state: &SharedState,
) -> Result<std::sync::Arc<dyn attune_core::plugin_hub::PluginHubProvider>, MarketplaceError> {
    state.plugin_hub.lock().map(|g| g.clone()).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "pluginhub_unavailable",
                "code": "pluginhub-lock-poisoned",
            })),
        )
    })
}

/// A mock PluginHub is an in-process catalog and never leaves the device. Every
/// other provider is an Attune Cloud/PluginHub egress point and therefore needs
/// the explicit `privacy.cloud_saas` consent bit before even listing metadata.
fn require_cloud_saas(provider: &str, enabled: bool) -> Result<(), MarketplaceError> {
    if provider == "mock" || enabled {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "cloud SaaS is disabled by privacy settings",
            "code": "cloud-saas-disabled",
            "provider": provider,
        })),
    ))
}

pub async fn list_plugins(
    State(state): State<SharedState>,
) -> Result<Json<ListResponse>, MarketplaceError> {
    let hub = hub_arc(&state)?;
    require_cloud_saas(
        hub.name(),
        crate::routes::privacy::outbound_enabled(&state, "cloud_saas"),
    )?;
    let resp = hub.list_plugins().map_err(|e| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "pluginhub_unavailable",
                "code": "pluginhub-list-failed",
                "detail": e.to_string(),
            })),
        )
    })?;

    // Local-fs fallback: when the hub is mock and returns an empty catalog,
    // surface locally installed plugins so the Marketplace UI isn't blank.
    let mut plugins = resp.plugins;
    let provider = hub.name().to_string();
    let registry = crate::routes::plugins::current_plugin_registry(&state);
    let installed_versions: HashMap<String, String> = registry
        .plugins()
        .map(|p| (p.manifest.id.clone(), p.manifest.version.clone()))
        .collect();
    if provider == "mock" && plugins.is_empty() {
        let local = attune_core::plugin_hub::local_plugin_listings_from_registry(&registry);
        if !local.is_empty() {
            plugins = local;
        }
    }

    Ok(Json(ListResponse {
        hub_version: resp.hub_version,
        user_plan: resp.user_plan,
        upgrade_url: resp.upgrade_url,
        plugins,
        provider,
        installed_versions,
    }))
}

pub async fn install_plugin(
    State(state): State<SharedState>,
    Path(plugin_id): Path<String>,
    Json(_req): Json<InstallRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let hub = hub_arc(&state)?;
    require_cloud_saas(
        hub.name(),
        crate::routes::privacy::outbound_enabled(&state, "cloud_saas"),
    )?;

    // P0 (2026-05-20): 未配置 provider 无真实包体 — 之前 fall-through 返回 HTTP 200 +
    // InstallResponse 让 UI 误判"安装成功"实际什么都没装. 改为 503 + actionable error
    // 让 UI 提示用户配 pluginhub.url + license_key 切到真 HttpPluginHubProvider.
    if hub.name() == "mock" {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "pluginhub_not_configured",
                "detail": format!(
                    "Plugin '{plugin_id}' cannot be installed: server is running with the offline/unconfigured pluginhub provider. \
                     Configure 'pluginhub.url' and 'pluginhub.license_key' in Settings to switch to the real hub."
                ),
                "hint": "Settings → 插件市场 → 填入 pluginhub URL + license key (paid 会员见 Attune Pro 邮件)",
                "plugin_id": plugin_id,
                "provider": "mock",
            })),
        ));
    }

    let device_fp = Some(attune_core::device_fingerprint::device_fingerprint().fingerprint_sig);

    // hub 交互 + 下载 .tar.gz + 解压落地都是阻塞 IO，整体移出 async worker。
    // 真实 hub 才下载落地到 plugins 目录；后续路由实时扫描，安装后立即可见。
    let state_for_install = state.clone();
    let resp = tokio::task::spawn_blocking(
        move || -> Result<attune_core::plugin_hub::InstallResponse, (StatusCode, Json<serde_json::Value>)> {
            let resp = hub
                .install_plugin(&plugin_id, device_fp.as_deref())
                .map_err(|e| {
                    // hub 用 ModelLoad 表达 plan_required / not_found；按 message 区分
                    let msg = e.to_string();
                    let code = if msg.contains("plan_required") || msg.contains("trial_already") {
                        StatusCode::PAYMENT_REQUIRED
                    } else if msg.contains("not found") {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    };
                    (code, Json(serde_json::json!({ "error": "install_failed", "detail": msg })))
                })?;

            if resp.decrypt_key.is_some() {
                let vault = state_for_install.vault.lock().unwrap_or_else(|e| e.into_inner());
                if vault.dek_db().is_err() {
                    return Err((
                        StatusCode::LOCKED,
                        Json(serde_json::json!({
                            "error": "vault_locked",
                            "detail": "encrypted plugin key requires an unlocked vault",
                            "plugin_id": plugin_id.clone(),
                        })),
                    ));
                }
            }

            let download_url = resp.download_url.trim().to_string();
            let pkg_result = if download_url.is_empty() {
                hub.download_plugin(&plugin_id, &resp.version)
            } else {
                hub.download_plugin_url(&download_url)
            };
            let pkg = pkg_result.map_err(|e| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": "download_failed",
                        "detail": format!("plugin download failed: {e}"),
                    })),
                )
            })?;
            attune_core::plugin_sync::verify_plugin_package_sha256(&pkg, &resp.sha256)
                .map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": "package_integrity_failed",
                            "detail": format!("plugin package integrity check failed: {e}"),
                        })),
                    )
                })?;
            let plugins_dir =
                attune_core::plugin_registry::PluginRegistry::default_plugins_dir()
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "plugins_dir",
                                "detail": e.to_string(),
                            })),
                        )
                    })?;
            let key_bytes = resp.decrypt_key.as_ref().map(|k| k.as_bytes().to_vec());
            let dst = attune_core::plugin_sync::install_official_plugin_package_with_key(
                &plugin_id,
                &pkg,
                &plugins_dir,
                key_bytes.as_deref(),
            )
            .map_err(|e| {
                let detail = format!("plugin install failed: {e}");
                let incompatible = detail.contains("plugin-incompatible-version");
                (
                    if incompatible {
                        StatusCode::CONFLICT
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR
                    },
                    Json(serde_json::json!({
                        "error": if incompatible { "plugin-incompatible-version" } else { "install_failed" },
                        "detail": detail,
                    })),
                )
            })?;
            tracing::info!("marketplace: 已安装插件 {plugin_id} → {}", dst.display());

            // 跨平台分发 version gate (spec §5/§10): 落盘后立即 scan, 若本插件因
            // min_attune_version 不满足而被 skip → 返回 plugin-incompatible-version
            // (不 panic, 清晰提示升级 attune)。scan 第二 Vec 含 [incompatible] /
            // [invalid-min-version] 前缀字符串, 匹配本 plugin_id 即拒绝。
            let (trust_mode, trusted_pubkeys) =
                crate::routes::plugins::plugin_trust_settings(&state_for_install);
            let scan_result = if let Some(key) = key_bytes.as_ref() {
                let mut keys = std::collections::HashMap::new();
                keys.insert(plugin_id.clone(), key.clone());
                attune_core::plugin_registry::PluginRegistry::scan_with_keys_and_trust(
                    &plugins_dir,
                    &keys,
                    trust_mode,
                    &trusted_pubkeys,
                )
            } else {
                attune_core::plugin_registry::PluginRegistry::scan_with_trust(
                    &plugins_dir,
                    None,
                    trust_mode,
                    &trusted_pubkeys,
                )
            };
            if let Ok((_, warnings)) = scan_result {
                if let Some(detail) = warnings.iter().find(|w| {
                    (w.starts_with("[incompatible]") || w.starts_with("[invalid-min-version]"))
                        && w.contains(&plugin_id)
                }) {
                    return Err((
                        StatusCode::CONFLICT,
                        Json(serde_json::json!({
                            "error": "plugin-incompatible-version",
                            "detail": detail,
                            "plugin_id": plugin_id,
                            "hint": "请升级 attune 到插件要求的版本后重试",
                        })),
                    ));
                }
            }
            Ok(resp)
        },
    )
    .await
    .map_err(|e| (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": "install_task", "detail": e.to_string() })),
    ))??;

    persist_marketplace_install_entitlement(&state, &resp)?;

    Ok(Json(serde_json::json!({
        "install_id": resp.install_id,
        "plugin_id": resp.plugin_id,
        "version": resp.version,
        "sha256": resp.sha256,
        "trial_started": resp.trial_started,
        "trial_expires": resp.trial_expires,
    })))
}

fn persist_marketplace_install_entitlement(
    state: &SharedState,
    resp: &attune_core::plugin_hub::InstallResponse,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let now = chrono::Utc::now().to_rfc3339();
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(dek) = vault.dek_db() else {
        tracing::warn!(
            "marketplace: vault locked — entitlement not persisted for {}",
            resp.plugin_id
        );
        return if resp.decrypt_key.is_some() {
            Err((
                StatusCode::LOCKED,
                Json(serde_json::json!({
                    "error": "vault_locked",
                    "detail": "encrypted plugin key could not be persisted; unlock the vault and retry",
                    "plugin_id": resp.plugin_id,
                })),
            ))
        } else {
            Ok(())
        };
    };
    let existing = vault
        .store()
        .get_entitlement(&dek, &resp.plugin_id)
        .ok()
        .flatten();
    let row = attune_core::store::plugin_entitlements::EntitlementRow {
        plugin_id: resp.plugin_id.clone(),
        license_id: existing
            .as_ref()
            .map(|row| row.license_id.clone())
            .unwrap_or_else(|| format!("pluginhub-install:{}", resp.install_id)),
        decrypt_key: resp
            .decrypt_key
            .clone()
            .or_else(|| existing.as_ref().and_then(|row| row.decrypt_key.clone())),
        tier: existing
            .as_ref()
            .map(|row| row.tier.clone())
            .unwrap_or_else(|| {
                if resp.trial_expires.is_some() {
                    "trial".into()
                } else {
                    "paid".into()
                }
            }),
        status: "active".into(),
        trial_expires: resp
            .trial_expires
            .clone()
            .or_else(|| existing.as_ref().and_then(|row| row.trial_expires.clone())),
        signing_pubkey_hex: existing
            .as_ref()
            .map(|row| row.signing_pubkey_hex.clone())
            .unwrap_or_default(),
        last_verified_at: now.clone(),
        grace_started_at: None,
        updated_at: now,
    };
    state.entitlement_cache.upsert(row.clone());
    if let Err(e) = vault.store().upsert_entitlement(&dek, &row) {
        tracing::warn!(
            "marketplace: failed to persist entitlement {}: {e}",
            resp.plugin_id
        );
        if resp.decrypt_key.is_some() {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "entitlement_persist_failed",
                    "detail": "encrypted plugin key could not be persisted",
                    "plugin_id": resp.plugin_id,
                })),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_catalog_does_not_require_cloud_consent() {
        assert!(require_cloud_saas("mock", false).is_ok());
    }

    #[test]
    fn real_hub_fails_closed_with_stable_json_when_cloud_saas_is_disabled() {
        let (status, Json(body)) =
            require_cloud_saas("real-hub", false).expect_err("real hub must be gated");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "cloud-saas-disabled");
        assert_eq!(body["provider"], "real-hub");
    }

    #[test]
    fn real_hub_is_allowed_only_after_explicit_cloud_consent() {
        assert!(require_cloud_saas("real-hub", true).is_ok());
    }
}
