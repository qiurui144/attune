//! GET /api/v1/marketplace/plugins  — 列 hub 上对当前 license 可见的插件
//! POST /api/v1/marketplace/plugins/{id}/install — 启动 trial 或安装
//!
//! 默认走 Mock 后端；attune-pro 通过覆盖 AppState.plugin_hub 注入真客户端。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::state::SharedState;

#[derive(Serialize)]
pub struct ListResponse {
    pub hub_version: String,
    pub user_plan: String,
    pub upgrade_url: String,
    pub plugins: Vec<attune_core::plugin_hub::PluginListing>,
    pub provider: String,
}

#[derive(Deserialize)]
pub struct InstallRequest {
    #[serde(default)]
    pub device_fp: Option<String>,
}

fn _hub_arc(
    state: &SharedState,
) -> Result<std::sync::Arc<dyn attune_core::plugin_hub::PluginHubProvider>, (StatusCode, String)> {
    state
        .plugin_hub
        .lock()
        .map(|g| g.clone())
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "plugin_hub lock poisoned".into()))
}

pub async fn list_plugins(
    State(state): State<SharedState>,
) -> Result<Json<ListResponse>, (StatusCode, String)> {
    let hub = _hub_arc(&state)?;
    let resp = hub.list_plugins().map_err(|e| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("plugin hub unavailable: {e}"),
        )
    })?;
    Ok(Json(ListResponse {
        hub_version: resp.hub_version,
        user_plan: resp.user_plan,
        upgrade_url: resp.upgrade_url,
        plugins: resp.plugins,
        provider: hub.name().to_string(),
    }))
}

pub async fn install_plugin(
    State(state): State<SharedState>,
    Path(plugin_id): Path<String>,
    Json(req): Json<InstallRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let hub = _hub_arc(&state).map_err(|(code, msg)| {
        (
            code,
            Json(serde_json::json!({ "error": "pluginhub_unavailable", "detail": msg })),
        )
    })?;

    // P0 (2026-05-20): Mock provider 无真实包体 — 之前 fall-through 返回 HTTP 200 +
    // InstallResponse 让 UI 误判"安装成功"实际什么都没装. 改为 503 + actionable error
    // 让 UI 提示用户配 pluginhub.url + license_key 切到真 HttpPluginHubProvider.
    if hub.name() == "mock" {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "pluginhub_not_configured",
                "detail": format!(
                    "Plugin '{plugin_id}' cannot be installed: server is running with the offline Mock pluginhub provider. \
                     Configure 'pluginhub.url' and 'pluginhub.license_key' in Settings to switch to the real hub."
                ),
                "hint": "Settings → 插件市场 → 填入 pluginhub URL + license key (paid 会员见 Attune Pro 邮件)",
                "plugin_id": plugin_id,
                "provider": "mock",
            })),
        ));
    }

    let device_fp = req.device_fp;

    // hub 交互 + 下载 .attunepkg + 解压落地都是阻塞 IO，整体移出 async worker。
    // 真实 hub 才下载落地到 plugins 目录；新插件经一次 attune-server 重启由 registry 装载生效。
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

            let pkg = hub.download_plugin(&plugin_id, &resp.version).map_err(|e| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": "download_failed",
                        "detail": format!("plugin download failed: {e}"),
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
            let dst = attune_core::plugin_sync::install_plugin_package(
                &plugin_id,
                &pkg,
                &plugins_dir,
            )
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "install_failed",
                        "detail": format!("plugin install failed: {e}"),
                    })),
                )
            })?;
            tracing::info!("marketplace: 已安装插件 {plugin_id} → {}", dst.display());

            // 跨平台分发 version gate (spec §5/§10): 落盘后立即 scan, 若本插件因
            // min_attune_version 不满足而被 skip → 返回 plugin-incompatible-version
            // (不 panic, 清晰提示升级 attune)。scan 第二 Vec 含 [incompatible] /
            // [invalid-min-version] 前缀字符串, 匹配本 plugin_id 即拒绝。
            if let Ok((_, warnings)) =
                attune_core::plugin_registry::PluginRegistry::scan(&plugins_dir)
            {
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

    Ok(Json(serde_json::to_value(resp).unwrap_or_else(|_| serde_json::json!({}))))
}
