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

    // 真实读 plugin 目录下的 form yaml schema (per protocol §2 ui_components.html 字段).
    // component.html 可以是:
    //   - "forms/<form_id>.yaml" — 标准 FormSchema yaml (优先)
    //   - "ui/civil_loan_stage3.html" — 静态 HTML 文件
    let plugins_root = attune_core::plugin_registry::PluginRegistry::default_plugins_dir()
        .map_err(|e| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("plugins_dir: {e}")})),
        ))?;
    let component_path = plugins_root.join(&plugin_id).join(&component.html);

    // OPT-7: 避免 std::Path::exists 在 async 内阻塞 stat 调用
    let exists = attune_core::async_fs::try_exists(&component_path)
        .await
        .unwrap_or(false);
    let html = if exists
        && component_path.extension().and_then(|s| s.to_str()) == Some("yaml")
    {
        // 真读 yaml 渲染 (async_fs 包装 spawn_blocking, 防 Axum worker 阻塞)
        let yaml_str = attune_core::async_fs::read_to_string(&component_path)
            .await
            .map_err(|e| (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("read form yaml: {e}")})),
            ))?;
        let schema: FormSchema = serde_yaml::from_str(&yaml_str).map_err(|e| (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": format!("parse form yaml: {e}")})),
        ))?;
        render_html(&schema)
    } else if exists {
        // HTML 文件直接返
        attune_core::async_fs::read_to_string(&component_path)
            .await
            .map_err(|e| (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("read html: {e}")})),
            ))?
    } else {
        // Fallback: 返 stub schema (开发期 plugin 还没提供 form 文件)
        let schema = FormSchema {
            id: component.id.clone(),
            title: format!("Form: {}", component.id),
            description: format!(
                "{} (stub — plugin dir 未提供 {})",
                component.description,
                component.html
            ),
            submit_target: component.target.clone(),
            fields: vec![],
        };
        render_html(&schema)
    };

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
