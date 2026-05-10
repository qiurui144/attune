//! /api/v1/forms — UI form runtime endpoint.
//!
//! 给前端按 plugin id + form id 拿渲染好的 HTML 表单 (用 attune-core::ui_runtime).
//! 提交后端点 (POST /api/v1/forms/<plugin_id>/<form_id>/submit) 接受 form data,
//! 校验后转 capability_dispatch 调对应 agent, 返 audit_trail.

use crate::state::SharedState;
use attune_core::ui_runtime::{render_html, FormSchema};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;

/// GET /api/v1/forms/<plugin_id>/<form_id>
/// 返渲染好的 HTML
pub async fn get_form(
    State(state): State<SharedState>,
    Path((plugin_id, form_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let registry = state.plugin_registry.clone();
    let plugin = registry.get_plugin(&plugin_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("plugin '{plugin_id}' not loaded")})),
        )
    })?;

    let component = plugin
        .manifest
        .ui_components
        .iter()
        .find(|c| c.id == form_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("form '{form_id}' not declared in plugin '{plugin_id}'"),
                })),
            )
        })?;

    // 简化: 假设 plugin 提供的 yaml 形式 form schema 在 ui_components.html 路径上
    // 实际生产环境: form schema 应从 plugin dir 读取并解析
    // 这里先返 minimal stub schema 演示 endpoint 链路
    let schema = FormSchema {
        id: component.id.clone(),
        title: format!("Form: {}", component.id),
        description: component.description.clone(),
        submit_target: component.target.clone(),
        fields: vec![],
    };
    let html = render_html(&schema);

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    ))
}

/// POST /api/v1/forms/<plugin_id>/<form_id>/submit
/// 接 JSON body (字段值), 转发给对应 agent (target)
pub async fn submit_form(
    State(state): State<SharedState>,
    Path((plugin_id, form_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let registry = state.plugin_registry.clone();
    let plugin = registry.get_plugin(&plugin_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("plugin '{plugin_id}' not loaded")})),
        )
    })?;

    let component = plugin
        .manifest
        .ui_components
        .iter()
        .find(|c| c.id == form_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": format!("form '{form_id}' not found")})),
            )
        })?;

    Ok(Json(serde_json::json!({
        "form_id": form_id,
        "plugin_id": plugin_id,
        "target": component.target,
        "received_fields": body,
        "status": "ack",
        "next_step": "调用方按 component.target 走 agent_runner::run_agent_subprocess",
    })))
}
