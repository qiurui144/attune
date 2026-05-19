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
) -> Result<Json<attune_core::plugin_hub::InstallResponse>, (StatusCode, String)> {
    let hub = _hub_arc(&state)?;
    let device_fp = req.device_fp;

    // hub 交互 + 下载 .attunepkg + 解压落地都是阻塞 IO，整体移出 async worker。
    // 真实 hub 才下载落地到 plugins 目录；新插件经一次 attune-server 重启由 registry 装载生效。
    let resp = tokio::task::spawn_blocking(
        move || -> Result<attune_core::plugin_hub::InstallResponse, (StatusCode, String)> {
            let resp = hub
                .install_plugin(&plugin_id, device_fp.as_deref())
                .map_err(|e| {
                    // mock / hub 都用 ModelLoad 表达 plan_required / not_found；按 message 区分
                    let msg = e.to_string();
                    let code = if msg.contains("plan_required") || msg.contains("trial_already") {
                        StatusCode::PAYMENT_REQUIRED
                    } else if msg.contains("not found") {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    };
                    (code, msg)
                })?;

            // Mock 后端无真实包体（仅离线/测试用），跳过下载落地，保留旧元数据返回行为。
            if hub.name() != "mock" {
                let pkg = hub.download_plugin(&plugin_id, &resp.version).map_err(|e| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        format!("plugin download failed: {e}"),
                    )
                })?;
                let plugins_dir =
                    attune_core::plugin_registry::PluginRegistry::default_plugins_dir()
                        .map_err(|e| {
                            (StatusCode::INTERNAL_SERVER_ERROR, format!("plugins dir: {e}"))
                        })?;
                let dst = attune_core::plugin_sync::install_plugin_package(
                    &plugin_id,
                    &pkg,
                    &plugins_dir,
                )
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("plugin install failed: {e}"),
                    )
                })?;
                tracing::info!("marketplace: 已安装插件 {plugin_id} → {}", dst.display());
            }
            Ok(resp)
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("install task: {e}")))??;

    Ok(Json(resp))
}
