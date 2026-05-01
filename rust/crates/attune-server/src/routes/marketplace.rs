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

pub async fn list_plugins(
    State(state): State<SharedState>,
) -> Result<Json<ListResponse>, (StatusCode, String)> {
    let hub = state.plugin_hub.clone();
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
    let hub = state.plugin_hub.clone();
    let resp = hub
        .install_plugin(&plugin_id, req.device_fp.as_deref())
        .map_err(|e| {
            // mock / hub 都用 ModelLoad 表达 plan_required / not_found；客户端按 message 区分
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
    Ok(Json(resp))
}
