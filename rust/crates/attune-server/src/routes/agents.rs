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

use crate::error::{AppError, AppResult};
use crate::routes::errors::internal;
use crate::state::SharedState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// 从 app_settings JSON 中提取 LLM env vars，供 agent subprocess 使用。
///
/// 只显式 set 这几个 LLM_* 变量，不 inherit parent env（避免泄露无关变量）。
/// 变量名必须与 agent binary 的读取约定一致：attune-agent-sdk
/// `agent_main::prepare_llm_env` 读取裸前缀 `LLM_ENDPOINT` / `LLM_API_KEY` /
/// `LLM_MODEL`（见 capability_dispatch 协议注释 "LLM_ENDPOINT / LLM_API_KEY"）。
/// 历史 bug：曾发 `ATTUNE_LLM_*`，与 agent 读取的裸 `LLM_*` 不匹配 → LLM agent
/// 即使配置了 LLM 也恒 exit 4 "LLM_ENDPOINT not set"（§7.3 env-wiring trap）。
fn llm_env_from_settings(settings: &serde_json::Value) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let Some(llm) = settings.get("llm") else { return env };
    if let Some(v) = llm.get("provider").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("LLM_PROVIDER".into(), v.into()));
    }
    if let Some(v) = llm.get("endpoint").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("LLM_ENDPOINT".into(), v.into()));
    }
    if let Some(v) = llm.get("model").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("LLM_MODEL".into(), v.into()));
    }
    if let Some(v) = llm.get("api_key").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        env.push(("LLM_API_KEY".into(), v.into()));
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

/// Trust-chain T10 (spec §7.2): entitlement dispatch gate. Returns `Ok(())` when the
/// owning plugin is entitled to run (free / active / trial / paid-grace / degraded);
/// returns a kebab-coded `AppError::Forbidden` when blocked (`trial-expired` /
/// `license-revoked`). The plugin's installed data is NOT touched — only the run is
/// refused, so re-subscribing re-enables it (spec §7.2 "插件保留已装"). Pure read of
/// the in-memory cache (O(1) keyed lookup); no vault / network.
fn entitlement_gate(state: &SharedState, plugin_id: &str) -> AppResult<()> {
    let now = chrono::Utc::now();
    let decision = state.entitlement_cache.is_entitled(plugin_id, &now);
    gate_decision_to_result(plugin_id, decision)
}

/// Map an [`attune_core::entitlement::EntitlementDecision`] to the dispatch route's
/// result. `Allow` → `Ok`; `Reject(code)` → kebab-coded 403 `AppError::detailed`.
/// Pure (no state) so the dispatch-gate behavior is unit-testable per §7.2.
fn gate_decision_to_result(
    plugin_id: &str,
    decision: attune_core::entitlement::EntitlementDecision,
) -> AppResult<()> {
    use attune_core::entitlement::EntitlementDecision;
    match decision {
        EntitlementDecision::Allow => Ok(()),
        EntitlementDecision::Reject(code) => Err(AppError::detailed(
            StatusCode::FORBIDDEN,
            json!({
                "error": "agent dispatch blocked by entitlement",
                "code": code,
                "plugin_id": plugin_id,
            }),
        )),
    }
}

/// POST /api/v1/agents/{agent_id}/run
pub async fn run_agent(
    State(state): State<SharedState>,
    Path(agent_id): Path<String>,
    Json(body): Json<RunAgentRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if agent_id.len() > 128 {
        return Err(AppError::BadRequest("agent_id too long".into()));
    }
    let registry = state.plugin_registry.clone();

    // 1. agent_id → 所属 plugin_id + agent spec (registry.list_agents 返回 (plugin_id, AgentSpec))
    let (plugin_id, agent_runtime) = registry
        .list_agents()
        .iter()
        .find(|(_, a)| a.id == agent_id)
        .map(|(pid, a)| (pid.to_string(), a.runtime.clone()))
        .ok_or_else(|| {
            AppError::NotFound(format!("agent '{agent_id}' not found in any loaded plugin"))
        })?;

    // Trust-chain T10 (spec §7.2): entitlement gate BEFORE any dispatch work. A
    // trial-expired / revoked license rejects with a kebab code (plugin data is
    // preserved — only the run is blocked); paid-grace / degraded allow (fail-open);
    // free / unregistered plugins always allow. O(1) EntitlementCache keyed lookup.
    entitlement_gate(&state, &plugin_id)?;

    // Bug-D: runtime: library 的 agent (如 interest_calculator) 不暴露独立 binary —
    // 由其他 agent 内部以 lib 方式调用,不应通过 HTTP route 直接 dispatch。
    // 之前 fallthrough 到 run_agent_subprocess 找不到 binary 时返 500;改为 400 with
    // 明确 schema/runtime 错误,告诉调用方 "这个 id 不是可独立 subprocess 的能力"。
    if agent_runtime == "library" {
        // rich error: 结构化 code/message/agent_id/runtime, 走 Detailed 保完整 body
        return Err(AppError::detailed(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "invalid-input",
                "code": "agent-not-callable",
                "message": format!("agent '{agent_id}' has runtime=library and is not directly invokable via HTTP; it is called internally by other agents (e.g. civil_loan_agent)"),
                "agent_id": agent_id,
                "runtime": agent_runtime,
            }),
        ));
    }

    // Bug-D extension: 空 input ({} 或 null) 视为 schema 错误,返 400 而非 500/2/3 混乱。
    // 业务 agent binary 普遍至少需要 1 个必填字段, body.input 完全空通常是客户端 bug。
    let input_is_empty = match &body.input {
        serde_json::Value::Null => true,
        serde_json::Value::Object(m) => m.is_empty(),
        _ => false,
    };
    if input_is_empty {
        // rich error: 结构化 code/message/agent_id, 走 Detailed 保完整 body
        return Err(AppError::detailed(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "invalid-input",
                "code": "empty-agent-input",
                "message": format!("agent '{agent_id}' requires non-empty input object"),
                "agent_id": agent_id,
            }),
        ));
    }

    // 2. plugin_dir = plugins_root/<plugin_id>（plugin-install 装载约定子目录名 = plugin_id）
    let plugins_root = attune_core::plugin_registry::PluginRegistry::default_plugins_dir()
        .map_err(|e| internal("default_plugins_dir", e))?;
    let plugin_dir = plugins_root.join(&plugin_id);

    // 3. agent stdin JSON
    let stdin_json = serde_json::to_string(&body.input)
        .map_err(|e| internal("serialize agent input", e))?;

    // 3b. 从 app_settings 读取 LLM env vars，转发给 agent subprocess。
    // LLM-heavy agents (fact_extractor 等) 依赖裸 `LLM_*` env 来初始化 LLM client；
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
        return Err(AppError::ServiceUnavailable(format!(
            "agent '{agent_id}' timed out (>{}s)",
            AGENT_RUN_TIMEOUT.as_secs()
        )));
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
        3 => Err(AppError::BadRequest(format!(
            "agent '{agent_id}' rejected input: {}",
            result.stderr.trim()
        ))),
        other => Err(internal(
            "agent run",
            format!("agent '{agent_id}' exit {other}: {}", result.stderr),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::entitlement::{EntitlementCache, EntitlementDecision};
    use attune_core::store::plugin_entitlements::EntitlementRow;
    use chrono::{DateTime, Utc};

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn row(
        plugin_id: &str,
        tier: &str,
        status: &str,
        trial_expires: Option<&str>,
        grace_started: Option<&str>,
    ) -> EntitlementRow {
        EntitlementRow {
            plugin_id: plugin_id.into(),
            license_id: "lic-x".into(),
            tier: tier.into(),
            status: status.into(),
            trial_expires: trial_expires.map(|s| s.into()),
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: "2026-06-12T00:00:00+00:00".into(),
            grace_started_at: grace_started.map(|s| s.into()),
            updated_at: "2026-06-12T00:00:00+00:00".into(),
        }
    }

    // ── T10: dispatch gate (spec §7.2) ──────────────────────────────────────

    /// trial-expired → dispatch blocked with kebab code `trial-expired`; the plugin's
    /// installed data is untouched (gate only refuses the run, cache row still present).
    #[test]
    fn dispatch_blocked_when_trial_expired() {
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "trial", "active", Some("2026-06-10T00:00:00+00:00"), None));
        let now = ts("2026-06-12T00:00:00+00:00"); // past trial_expires
        let decision = cache.is_entitled("law-pro", &now);
        assert_eq!(decision, EntitlementDecision::Reject("trial-expired"));
        let res = gate_decision_to_result("law-pro", decision);
        let err = res.unwrap_err();
        // 403 + kebab code via AppError::detailed; plugin row still in cache (data preserved).
        assert!(matches!(err, AppError::Detailed { status, .. } if status == StatusCode::FORBIDDEN));
        assert_eq!(cache.snapshot().len(), 1, "trial-expired must NOT delete the plugin row");
    }

    /// paid in-grace (cloud unreachable, < 14d) → dispatch ALLOWED (fail-open).
    #[test]
    fn dispatch_allowed_in_grace_paid() {
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "paid", "active", None, Some("2026-06-10T00:00:00+00:00")));
        let now = ts("2026-06-12T00:00:00+00:00"); // 2d into grace, < 14d
        let decision = cache.is_entitled("law-pro", &now);
        assert_eq!(decision, EntitlementDecision::Allow);
        assert!(gate_decision_to_result("law-pro", decision).is_ok());
    }

    /// revoked → dispatch blocked with kebab code `license-revoked` (fail-closed).
    #[test]
    fn dispatch_blocked_when_revoked() {
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "paid", "revoked", None, None));
        let now = ts("2026-06-12T00:00:00+00:00");
        let decision = cache.is_entitled("law-pro", &now);
        assert_eq!(decision, EntitlementDecision::Reject("license-revoked"));
        let res = gate_decision_to_result("law-pro", decision);
        assert!(matches!(res.unwrap_err(), AppError::Detailed { status, .. } if status == StatusCode::FORBIDDEN));
        assert_eq!(cache.snapshot().len(), 1, "revoked must NOT delete the plugin row (data preserved)");
    }

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
        // Bare-prefix names must match attune-agent-sdk prepare_llm_env reader
        // (§7.3 env-wiring trap: ATTUNE_LLM_* mismatch caused recurring exit-4).
        assert!(env.iter().any(|(k, v)| k == "LLM_PROVIDER" && v == "openai_compat"));
        assert!(env.iter().any(|(k, v)| k == "LLM_ENDPOINT" && v == "https://api.deepseek.com/v1"));
        assert!(env.iter().any(|(k, v)| k == "LLM_MODEL" && v == "deepseek-chat"));
        assert!(env.iter().any(|(k, v)| k == "LLM_API_KEY" && v == "sk-test123"));
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
        // endpoint and api_key are empty → excluded
        assert!(env.iter().any(|(k, _)| k == "LLM_PROVIDER"));
        assert!(env.iter().any(|(k, _)| k == "LLM_MODEL"));
        assert!(!env.iter().any(|(k, _)| k == "LLM_ENDPOINT"));
        assert!(!env.iter().any(|(k, _)| k == "LLM_API_KEY"));
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

    // ── 增强覆盖: edge / adversarial ─────────────────────────────────────

    // Adversarial: llm 字段是 null 而不是 object — 不 panic, 返回 empty
    #[test]
    fn llm_env_from_settings_null_llm_returns_empty() {
        let settings = serde_json::json!({ "llm": null });
        let env = llm_env_from_settings(&settings);
        assert!(env.is_empty());
    }

    // Adversarial: llm 字段是 array 而不是 object — 不 panic
    #[test]
    fn llm_env_from_settings_array_llm_returns_empty() {
        let settings = serde_json::json!({ "llm": [1, 2, 3] });
        let env = llm_env_from_settings(&settings);
        assert!(env.is_empty(), "非 object llm 应被忽略");
    }

    // Adversarial: llm 子字段类型错 (number 不是 string) — 应跳过
    #[test]
    fn llm_env_from_settings_wrong_field_type_skipped() {
        let settings = serde_json::json!({
            "llm": {
                "provider": 123,        // number, 应跳过
                "endpoint": true,        // bool, 应跳过
                "model": "deepseek-chat",
                "api_key": ["arr"]       // array, 应跳过
            }
        });
        let env = llm_env_from_settings(&settings);
        assert_eq!(env.len(), 1, "只有 model 是合法字符串");
        assert!(env.iter().any(|(k, _)| k == "LLM_MODEL"));
    }

    // Edge: 空字符串 key 也算 (api_key 是空但不是缺失) — 看 filter 行为
    #[test]
    fn llm_env_from_settings_whitespace_only_value_kept() {
        // 当前实现仅 filter empty (!s.is_empty()), 不 trim, 所以 "  " 应保留
        // 这是当前行为 — 这测试锁定它,如果将来想改成 trim 则该测试失败提醒
        let settings = serde_json::json!({
            "llm": {
                "provider": "  ",
                "model": "x"
            }
        });
        let env = llm_env_from_settings(&settings);
        // current behavior: only empty string excluded, whitespace kept
        assert_eq!(env.len(), 2);
    }

    // Adversarial: 极深嵌套 settings — 不 stack overflow
    #[test]
    fn llm_env_from_settings_handles_huge_settings() {
        let mut huge = serde_json::Map::new();
        for i in 0..1000 {
            huge.insert(format!("k{i}"), serde_json::json!(i));
        }
        huge.insert(
            "llm".into(),
            serde_json::json!({
                "provider": "openai",
                "endpoint": "https://api.openai.com/v1",
                "model": "gpt-4",
                "api_key": "sk-xxx"
            }),
        );
        let env = llm_env_from_settings(&serde_json::Value::Object(huge));
        assert_eq!(env.len(), 4);
    }

    // Bug-D: empty input detection covers {}, null, but NOT non-empty objects
    #[test]
    fn input_is_empty_classification() {
        // 这里复刻路由内的判定逻辑,作为 unit test 锁定行为(避免重构时漂移)。
        fn is_empty(v: &serde_json::Value) -> bool {
            match v {
                serde_json::Value::Null => true,
                serde_json::Value::Object(m) => m.is_empty(),
                _ => false,
            }
        }
        assert!(is_empty(&serde_json::json!({})), "empty object should be empty");
        assert!(is_empty(&serde_json::Value::Null), "null should be empty");
        assert!(!is_empty(&serde_json::json!({"x": 1})), "non-empty object should not be empty");
        assert!(!is_empty(&serde_json::json!([])), "empty array is non-object, not treated as empty (agent decides)");
        assert!(!is_empty(&serde_json::json!("string")), "string non-empty");
        assert!(!is_empty(&serde_json::json!(42)), "number non-empty");
    }

    // I18n: API key 含 Unicode (虽然不该, 但不 panic)
    #[test]
    fn llm_env_from_settings_unicode_values_pass_through() {
        let settings = serde_json::json!({
            "llm": {
                "provider": "本地",
                "endpoint": "http://本地.test/v1",
                "model": "qwen-中文",
                "api_key": "sk-🔑"
            }
        });
        let env = llm_env_from_settings(&settings);
        assert_eq!(env.len(), 4);
        assert!(env.iter().any(|(_, v)| v == "qwen-中文"));
    }
}
