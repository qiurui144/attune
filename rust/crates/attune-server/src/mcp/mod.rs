//! G1 + G2 — 原生 MCP server (JSON-RPC 2.0)。
//!
//! attune-server 把私有 REST 业务以 MCP 工具形式暴露给外部 agent host (Hermes /
//! Claude Desktop / 任意 MCP client)。transport = HTTP `POST /mcp` (+ SSE `GET /mcp/sse`,
//! headless 24h 常驻)。工具名 + 参数 **兼容 attune-mcp-bridge 契约** → 桥退役 K3 零迁移。
//!
//! 每个 `tools/call` 经 G2 gate:scoped token 校验 → 高危黑名单 → 权限位 → 审计落盘
//! (allow/deny 都落, fail-closed)。
//!
//! 模块拆分:
//! - [`gate`]   纯决策 (denylist + scope)
//! - [`tools`]  6 工具定义 + 派发到既有 REST handler (不重写业务)
//! - [`transport`] axum 路由 (HTTP/SSE) + token 提取 + 协议组合

pub mod gate;
pub mod tools;
pub mod transport;

use serde_json::{json, Value};

use crate::state::SharedState;
use attune_core::store::agent_audit::AgentAuditEvent;

/// MCP / JSON-RPC 协议版本 (与 MCP spec 对齐的 date string)。
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// 一次已认证调用的上下文 (token 校验后的权限位 + 审计 label)。
#[derive(Debug, Clone)]
pub struct CallCtx {
    /// scoped token 的 label —— 审计 agent_source 字段。
    pub agent_source: String,
    /// 已校验的权限位集。
    pub scopes: Vec<String>,
}

/// 构造一个 JSON-RPC 成功响应。
fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// 构造一个 JSON-RPC 错误响应。
fn rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// 处理一个已认证的 JSON-RPC 请求 (transport 已校验 token → ctx)。
///
/// 支持的 method:`initialize` / `tools/list` / `tools/call`。
/// `tools/call` 走 [`call_tool`] (gate + audit + dispatch)。
pub async fn handle_request(state: &SharedState, ctx: &CallCtx, req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

    match method {
        "initialize" => rpc_ok(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "attune", "version": env!("CARGO_PKG_VERSION") }
            }),
        ),
        "notifications/initialized" => {
            // MCP client 握手后发的 notification (无 id) —— 无需响应体, 返回空 ack。
            rpc_ok(id, json!({}))
        }
        "tools/list" => rpc_ok(id, json!({ "tools": tools::list_tools(&ctx.scopes) })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(json!({}));
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(state, ctx, name, &args).await {
                Ok(content) => rpc_ok(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": content_to_text(&content) }],
                        "isError": false
                    }),
                ),
                Err((code, msg)) => rpc_err(id, code, &msg),
            }
        }
        other => rpc_err(id, gate::ERR_UNKNOWN_TOOL, &format!("unknown method '{other}'")),
    }
}

/// G2 闸门 + 派发 + 审计。返回 Ok(result_json) 或 Err((mcp_code, message))。
///
/// 顺序 (spec §3 信任不变量):denylist → scope → **审计 (allow/deny 都落, fail-closed)** →
/// 业务执行。审计写失败 → 拒绝放行 (防无痕调用)。
async fn call_tool(
    state: &SharedState,
    ctx: &CallCtx,
    tool: &str,
    args: &Value,
) -> Result<Value, (i64, String)> {
    let decision = gate::decide(tool, &ctx.scopes);

    // 先落审计 (allow 与 deny 都留痕)。fail-closed: 审计写失败 → 拒绝。
    let audit_event = match &decision {
        gate::GateDecision::Allow => AgentAuditEvent::allow(&ctx.agent_source, tool),
        gate::GateDecision::Deny(reason, _) => {
            AgentAuditEvent::deny(&ctx.agent_source, tool, *reason)
        }
    };
    if let Err(e) = record_audit(state, &audit_event) {
        return Err((
            -32603,
            format!("audit write failed (fail-closed, call refused): {e}"),
        ));
    }

    // 闸门拒绝 → 返回 MCP error (审计已留痕)。
    if let gate::GateDecision::Deny(reason, code) = decision {
        return Err((code, format!("denied: {reason}")));
    }

    // 放行 → 派发到既有 REST 业务。业务错误映射成应用层 MCP error (-32000)。
    match tools::dispatch(state, tool, args).await {
        Ok(v) => Ok(v),
        Err(app_err) => {
            let (_status, msg) = app_err.status_and_message();
            Err((-32000, msg))
        }
    }
}

/// 落一条 agent 审计 (短临界区:取 vault guard → store.record_agent_audit)。
fn record_audit(state: &SharedState, event: &AgentAuditEvent) -> Result<(), String> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault
        .store()
        .record_agent_audit(event)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 工具结果 JSON → MCP text content。结构化结果用紧凑 JSON 字符串呈现 (agent host 端解析)。
fn content_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(scopes: &[&str]) -> CallCtx {
        CallCtx {
            agent_source: "test-agent".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn state() -> SharedState {
        crate::test_support::unlocked_state()
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let s = state();
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let resp = handle_request(&s, &ctx(&["search"]), &req).await;
        assert_eq!(resp["result"]["serverInfo"]["name"], "attune");
        assert_eq!(resp["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn tools_list_filtered_by_scope() {
        let s = state();
        let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
        let resp = handle_request(&s, &ctx(&["search"]), &req).await;
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"vault_search"));
        assert!(!names.contains(&"vault_chat"));
    }

    #[tokio::test]
    async fn call_high_risk_denied_and_audited() {
        let s = state();
        // 安全对抗 a: 即使给全部三权, export/delete 必拒 + 落 deny 审计
        let req = json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"export","arguments":{}}});
        let resp = handle_request(&s, &ctx(&["search", "chat", "ingest"]), &req).await;
        assert_eq!(resp["error"]["code"], gate::ERR_HIGH_RISK);

        // 审计应有一条 deny / high-risk-denied
        let vault = s.vault.lock().unwrap();
        let rows = vault.store().list_agent_audit(None, None, 10).unwrap();
        assert!(rows
            .iter()
            .any(|r| r.tool == "export" && r.decision == "deny" && r.deny_reason == "high-risk-denied"));
    }

    #[tokio::test]
    async fn call_scope_missing_denied_and_audited() {
        let s = state();
        // 安全对抗 b: 无 chat 权限调 vault_chat → 拒 + 审计 deny/scope-missing
        let req = json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"vault_chat","arguments":{"message":"hi"}}});
        let resp = handle_request(&s, &ctx(&["search"]), &req).await;
        assert_eq!(resp["error"]["code"], gate::ERR_FORBIDDEN_SCOPE);

        let vault = s.vault.lock().unwrap();
        let rows = vault.store().list_agent_audit(None, None, 10).unwrap();
        assert!(rows
            .iter()
            .any(|r| r.tool == "vault_chat" && r.decision == "deny" && r.deny_reason == "scope-missing"));
    }

    #[tokio::test]
    async fn unknown_method_errors() {
        let s = state();
        let req = json!({"jsonrpc":"2.0","id":5,"method":"frobnicate"});
        let resp = handle_request(&s, &ctx(&["search"]), &req).await;
        assert!(resp["error"].is_object());
    }
}
