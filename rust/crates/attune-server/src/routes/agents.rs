//! Agent run route — 前端触发 plugin agent binary 执行。
//!
//! per plan「law-pro 接入」阶段 2：打通前端（chat / 动态表单）→ agent binary。
//! 此前 chat 命中 plugin chat_trigger 只返回提示文本，agent_runner 无 HTTP 暴露。
//! 本路由让前端能端到端触发 agent（如 law-pro civil_loan_agent 算借贷金额）。
//!
//! POST /api/v1/agents/{agent_id}/run
//!   body: { "input": <agent stdin JSON> }   — 如 CivilLoanInput
//!   resp: { ok, agent_id, output, audit_trail, red_lines_violated? }
//!
//! 错误映射（agent binary exit code，per capability_dispatch 协议）：
//!   exit 0  → 200 ok=true   计算成功
//!   exit 2  → 200 ok=false  业务红线触发（red_lines_violated=true，非 HTTP 错误）
//!   exit 其他 → 500          IO / 解析错误
//!   timeout → 503

use crate::routes::errors::{internal, RouteError};
use crate::state::SharedState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// agent 单次执行超时。civil_loan_agent 等纯计算 agent 远快于此；
/// 上限防 binary 卡死拖垮 tokio。
const AGENT_RUN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
pub struct RunAgentRequest {
    /// agent stdin JSON。原样序列化后喂给 agent binary 的 stdin。
    pub input: serde_json::Value,
}

/// POST /api/v1/agents/{agent_id}/run
pub async fn run_agent(
    State(state): State<SharedState>,
    Path(agent_id): Path<String>,
    Json(body): Json<RunAgentRequest>,
) -> Result<Json<serde_json::Value>, RouteError> {
    if agent_id.len() > 128 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "agent_id too long"}))));
    }
    let registry = state.plugin_registry.clone();

    // 1. agent_id → 所属 plugin_id（registry.list_agents 返回 (plugin_id, AgentSpec)）
    let plugin_id = registry
        .list_agents()
        .iter()
        .find(|(_, a)| a.id == agent_id)
        .map(|(pid, _)| pid.to_string())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("agent '{agent_id}' not found in any loaded plugin")})),
            )
        })?;

    // 2. plugin_dir = plugins_root/<plugin_id>（plugin-install 装载约定子目录名 = plugin_id）
    let plugins_root = attune_core::plugin_registry::PluginRegistry::default_plugins_dir()
        .map_err(|e| internal("default_plugins_dir", e))?;
    let plugin_dir = plugins_root.join(&plugin_id);

    // 3. agent stdin JSON
    let stdin_json = serde_json::to_string(&body.input)
        .map_err(|e| internal("serialize agent input", e))?;

    // 4. subprocess（run_agent_subprocess 是同步阻塞，spawn_blocking 包裹防阻塞 tokio worker）
    let agent_id_for_run = agent_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        attune_core::agent_runner::run_agent_subprocess(
            &registry,
            &agent_id_for_run,
            &plugin_dir,
            &stdin_json,
            vec![],
            AGENT_RUN_TIMEOUT,
        )
    })
    .await
    .map_err(|e| internal("agent subprocess join", e))?
    .map_err(|e| internal("agent run", e))?;

    // 5. 错误映射
    if result.timed_out {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": format!("agent '{agent_id}' timed out (>{}s)", AGENT_RUN_TIMEOUT.as_secs())})),
        ));
    }
    // agent stdout 是机器可读 JSON；解析失败保留原文（不阻断响应）
    let output: serde_json::Value =
        serde_json::from_str(&result.stdout).unwrap_or_else(|_| json!(result.stdout));

    match result.exit_code {
        0 => Ok(Json(json!({
            "ok": true,
            "agent_id": agent_id,
            "output": output,
            "audit_trail": result.stderr,
        }))),
        // exit 2 = 业务红线触发。这是合法的业务结果（不是 HTTP 错误），
        // 前端据 red_lines_violated 渲染"需补证据 / 红线提示"。
        2 => Ok(Json(json!({
            "ok": false,
            "agent_id": agent_id,
            "red_lines_violated": true,
            "output": output,
            "audit_trail": result.stderr,
        }))),
        other => Err(internal(
            "agent run",
            format!("agent '{agent_id}' exit {other}: {}", result.stderr),
        )),
    }
}
