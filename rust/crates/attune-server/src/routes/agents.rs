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
//!   exit 3  → 400           客户端输入错误（stdin 空 / JSON 解析失败）
//!   exit 4  → 500           LLM_ENDPOINT not set（agent 需要 LLM 但未配置）
//!   exit 其他 → 500          内部错误（IO / 序列化）
//!   timeout → 503

use crate::routes::errors::{internal, RouteError};
use crate::state::SharedState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// 从 app_settings JSON 中提取 LLM env vars，供 agent subprocess 使用。
///
/// 只显式 set 4 个 ATTUNE_LLM_* 变量，不 inherit parent env（避免泄露无关变量）。
/// agent binary 读取这 4 个 env 来初始化 LLM client。
fn llm_env_from_settings(settings: &serde_json::Value) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let Some(llm) = settings.get("llm") else { return env };
    if let Some(v) = llm.get("provider").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("ATTUNE_LLM_PROVIDER".into(), v.into()));
    }
    if let Some(v) = llm.get("endpoint").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("ATTUNE_LLM_ENDPOINT".into(), v.into()));
    }
    if let Some(v) = llm.get("model").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("ATTUNE_LLM_MODEL".into(), v.into()));
    }
    if let Some(v) = llm.get("api_key").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("ATTUNE_LLM_API_KEY".into(), v.into()));
    }
    env
}

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

    // 3b. 从 app_settings 读取 LLM env vars，转发给 agent subprocess。
    // LLM-heavy agents (fact_extractor 等) 依赖 ATTUNE_LLM_* 来初始化 LLM client；
    // 未传递时 binary exit 4 "LLM_ENDPOINT not set"（per E2E spec P1:3）。
    let llm_env: Vec<(String, String)> = {
        let app_settings: serde_json::Value = state
            .vault
            .lock()
            .ok()
            .and_then(|vault| {
                vault.store().get_meta("app_settings").ok().flatten()
                    .and_then(|data| serde_json::from_slice(&data).ok())
            })
            .unwrap_or_else(|| serde_json::json!({}));
        llm_env_from_settings(&app_settings)
    };

    // 4. subprocess（run_agent_subprocess 是同步阻塞，spawn_blocking 包裹防阻塞 tokio worker）
    let agent_id_for_run = agent_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        attune_core::agent_runner::run_agent_subprocess(
            &registry,
            &agent_id_for_run,
            &plugin_dir,
            &stdin_json,
            llm_env,
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
        // exit 3 = 客户端输入错误（畸形 / 空 JSON）—— 是调用方的错，回 400 而非 500。
        3 => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("agent '{agent_id}' rejected input: {}", result.stderr.trim())})),
        )),
        other => Err(internal(
            "agent run",
            format!("agent '{agent_id}' exit {other}: {}", result.stderr),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_env_from_settings_extracts_all_four_vars() {
        let settings = serde_json::json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://api.deepseek.com/v1",
                "model": "deepseek-chat",
                "api_key": "sk-test123"
            }
        });
        let env = llm_env_from_settings(&settings);
        assert!(env.iter().any(|(k, v)| k == "ATTUNE_LLM_PROVIDER" && v == "openai_compat"));
        assert!(env.iter().any(|(k, v)| k == "ATTUNE_LLM_ENDPOINT" && v == "https://api.deepseek.com/v1"));
        assert!(env.iter().any(|(k, v)| k == "ATTUNE_LLM_MODEL" && v == "deepseek-chat"));
        assert!(env.iter().any(|(k, v)| k == "ATTUNE_LLM_API_KEY" && v == "sk-test123"));
        assert_eq!(env.len(), 4);
    }

    #[test]
    fn llm_env_from_settings_skips_empty_values() {
        let settings = serde_json::json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "",
                "model": "deepseek-chat",
                "api_key": ""
            }
        });
        let env = llm_env_from_settings(&settings);
        // endpoint と api_key は空なので除外される
        assert!(env.iter().any(|(k, _)| k == "ATTUNE_LLM_PROVIDER"));
        assert!(env.iter().any(|(k, _)| k == "ATTUNE_LLM_MODEL"));
        assert!(!env.iter().any(|(k, _)| k == "ATTUNE_LLM_ENDPOINT"));
        assert!(!env.iter().any(|(k, _)| k == "ATTUNE_LLM_API_KEY"));
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn llm_env_from_settings_no_llm_key_returns_empty() {
        let settings = serde_json::json!({ "theme": "dark" });
        let env = llm_env_from_settings(&settings);
        assert!(env.is_empty());
    }

    #[test]
    fn llm_env_from_settings_empty_settings_returns_empty() {
        let settings = serde_json::json!({});
        let env = llm_env_from_settings(&settings);
        assert!(env.is_empty());
    }
}
