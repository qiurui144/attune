//! G1 — MCP 6 工具定义 + 派发到既有 REST 业务。
//!
//! 工具名 + 参数 **兼容 attune-mcp-bridge 契约**(local-scheduler side 零迁移)。每个工具 **包装既有 REST
//! handler**(经 axum handler 函数直接调用,不重写业务逻辑)—— handler 是普通 `async fn`,
//! 接受 extractor 包装的参数,这里构造同样的 extractor 调用,取出内层 JSON。
//!
//! 不暴露任何高危工具(export/delete/settings) —— 那些被 gate.rs 黑名单永久挡。

use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// 6 工具名(兼容桥契约)。
pub const TOOL_NAMES: &[&str] = &[
    "vault_search",
    "vault_chat",
    "ingest",
    "annotate",
    "agent_invoke",
    "job_status",
];

/// 工具 → 所需权限位(spec §5.2)。返回 None = 未知工具。
pub fn required_scope(tool: &str) -> Option<&'static str> {
    match tool {
        "vault_search" => Some("search"),
        "job_status" => Some("search"),
        "ingest" => Some("ingest"),
        "annotate" => Some("ingest"),
        "vault_chat" => Some("chat"),
        "agent_invoke" => Some("chat"),
        _ => None,
    }
}

/// MCP `tools/list` 的工具描述(name + description + inputSchema)。
/// 只列出 `granted_scopes` 有权限的工具(spec §5.1:tools/list 按权限过滤)。
pub fn list_tools(granted_scopes: &[String]) -> Vec<Value> {
    all_tool_descriptors()
        .into_iter()
        .filter(|d| {
            let name = d["name"].as_str().unwrap_or("");
            required_scope(name).is_some_and(|req| granted_scopes.iter().any(|s| s == req))
        })
        .collect()
}

fn all_tool_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "vault_search",
            "description": "Search the private knowledge vault (hybrid BM25 + vector). Read-only, zero cost.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "search query" },
                    "top_k": { "type": "integer", "description": "max results (1-100)", "default": 10 }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "vault_chat",
            "description": "RAG chat over the vault. Triggers an LLM (cloud or local) — costs time/tokens.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message": { "type": "string" },
                    "session_id": { "type": "string" },
                    "history": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "role": { "type": "string" },
                                "content": { "type": "string" }
                            }
                        }
                    }
                },
                "required": ["message"]
            }
        }),
        json!({
            "name": "ingest",
            "description": "Ingest a text note into the vault (indexed + queued for embedding).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string" },
                    "source_type": { "type": "string", "default": "note" },
                    "url": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["title", "content"]
            }
        }),
        json!({
            "name": "annotate",
            "description": "Create an annotation on an existing vault item.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "item_id": { "type": "string" },
                    "offset_start": { "type": "integer" },
                    "offset_end": { "type": "integer" },
                    "text_snippet": { "type": "string" },
                    "label": { "type": "string" },
                    "color": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["item_id", "offset_start", "offset_end", "text_snippet"]
            }
        }),
        json!({
            "name": "agent_invoke",
            "description": "Invoke a loaded agent by id. Triggers an LLM subprocess — costs time/tokens.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "input": { "type": "object" }
                },
                "required": ["agent_id", "input"]
            }
        }),
        json!({
            "name": "job_status",
            "description": "Get the status of a durable job by id. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": { "job_id": { "type": "string" } },
                "required": ["job_id"]
            }
        }),
    ]
}

/// 派发已 gate 放行的工具调用到既有 REST 业务,返回工具结果 JSON。
///
/// **包装既有 handler**:axum handler 是普通 `async fn`,这里构造其 extractor 参数直接调用,
/// 取出内层 `Json(value)` 的 `value`。不重写任何业务逻辑。
pub async fn dispatch(state: &SharedState, tool: &str, args: &Value) -> AppResult<Value> {
    match tool {
        "vault_search" => call_search(state, args).await,
        "vault_chat" => call_chat(state, args).await,
        "ingest" => call_ingest(state, args).await,
        "annotate" => call_annotate(state, args).await,
        "agent_invoke" => call_agent_invoke(state, args).await,
        "job_status" => call_job_status(state, args).await,
        other => Err(AppError::NotFound(format!("unknown tool '{other}'"))),
    }
}

// ── 工具 → 既有 handler 包装 ────────────────────────────────────────────────

async fn call_search(state: &SharedState, args: &Value) -> AppResult<Value> {
    use crate::routes::search::{search, SearchQuery};
    use axum::extract::{Query, State};
    use axum::http::HeaderMap;

    let query = require_str(args, "query")?;
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let q = SearchQuery {
        q: query,
        top_k,
        initial_k: None,
        intermediate_k: None,
    };
    // search 返回 Result<Json<Value>, (StatusCode, Json<Value>)> —— 映射 tuple err → AppError
    match search(State(state.clone()), HeaderMap::new(), Query(q)).await {
        Ok(json) => Ok(json.0),
        Err((status, body)) => Err(AppError::detailed(status, body.0)),
    }
}

async fn call_chat(state: &SharedState, args: &Value) -> AppResult<Value> {
    use crate::routes::chat::{chat, ChatRequest, HistoryMessage};
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::Json;

    let message = require_str(args, "message")?;
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let history: Vec<HistoryMessage> = args
        .get("history")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    Some(HistoryMessage {
                        role: m.get("role")?.as_str()?.to_string(),
                        content: m.get("content")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let body = ChatRequest {
        message,
        history,
        session_id,
    };
    let json = chat(State(state.clone()), HeaderMap::new(), Json(body)).await?;
    Ok(json.0)
}

async fn call_ingest(state: &SharedState, args: &Value) -> AppResult<Value> {
    use crate::routes::ingest::{ingest, IngestRequest};
    use axum::extract::State;
    use axum::Json;

    let title = require_str(args, "title")?;
    let content = require_str(args, "content")?;
    let body = IngestRequest {
        title,
        content,
        source_type: args
            .get("source_type")
            .and_then(|v| v.as_str())
            .unwrap_or("note")
            .to_string(),
        url: args.get("url").and_then(|v| v.as_str()).map(String::from),
        domain: args
            .get("domain")
            .and_then(|v| v.as_str())
            .map(String::from),
        tags: args.get("tags").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        }),
    };
    let json = ingest(State(state.clone()), Json(body)).await?;
    Ok(json.0)
}

async fn call_annotate(state: &SharedState, args: &Value) -> AppResult<Value> {
    use crate::routes::annotations::{create_annotation, CreateAnnotationRequest};
    use axum::extract::State;
    use axum::Json;

    let body = CreateAnnotationRequest {
        item_id: require_str(args, "item_id")?,
        offset_start: args
            .get("offset_start")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        offset_end: args.get("offset_end").and_then(|v| v.as_i64()).unwrap_or(0),
        text_snippet: args
            .get("text_snippet")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        label: args.get("label").and_then(|v| v.as_str()).map(String::from),
        color: args.get("color").and_then(|v| v.as_str()).map(String::from),
        content: args
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from),
        // MCP 调用恒为 ai source (外部 agent 写入)
        source: Some("ai".to_string()),
    };
    let json = create_annotation(State(state.clone()), Json(body)).await?;
    Ok(json.0)
}

async fn call_agent_invoke(state: &SharedState, args: &Value) -> AppResult<Value> {
    use crate::routes::agents::{run_agent, RunAgentRequest};
    use axum::extract::{Path, State};
    use axum::Json;

    let agent_id = require_str(args, "agent_id")?;
    let input = args.get("input").cloned().unwrap_or(json!({}));
    let body = RunAgentRequest { input };
    let json = run_agent(State(state.clone()), Path(agent_id), Json(body)).await?;
    Ok(json.0)
}

async fn call_job_status(state: &SharedState, args: &Value) -> AppResult<Value> {
    let job_id = require_str(args, "job_id")?;
    // job_status 是单 job 只读查询 —— 直接经 job_store 查 (jobs.rs 只有 list/cancel/requeue,
    // 无单 job GET handler;这里复用 store::get_job + office::job_record_to_status_json,
    // 不重写业务: get_job + 既有 json 转换函数都是已有 API)。
    let store = state
        .job_store()
        .ok_or_else(|| AppError::ServiceUnavailable("durable job queue unavailable".into()))?;
    let rec = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.get_job(&job_id)
            .map_err(|e| AppError::Internal(e.to_string()))?
    };
    match rec {
        Some(r) => Ok(crate::routes::office::job_record_to_status_json(&r)),
        None => Err(AppError::NotFound(format!("job '{job_id}' not found"))),
    }
}

fn require_str(args: &Value, key: &str) -> AppResult<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::BadRequest(format!("missing required param '{key}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_scope_covers_six_tools() {
        assert_eq!(required_scope("vault_search"), Some("search"));
        assert_eq!(required_scope("job_status"), Some("search"));
        assert_eq!(required_scope("ingest"), Some("ingest"));
        assert_eq!(required_scope("annotate"), Some("ingest"));
        assert_eq!(required_scope("vault_chat"), Some("chat"));
        assert_eq!(required_scope("agent_invoke"), Some("chat"));
        assert_eq!(required_scope("nope"), None);
        assert_eq!(TOOL_NAMES.len(), 6);
    }

    #[test]
    fn list_tools_filters_by_scope() {
        let only_search = list_tools(&["search".to_string()]);
        let names: Vec<&str> = only_search
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"vault_search"));
        assert!(names.contains(&"job_status"));
        assert!(!names.contains(&"vault_chat"));
        assert!(!names.contains(&"ingest"));

        let all = list_tools(&["search".into(), "chat".into(), "ingest".into()]);
        assert_eq!(all.len(), 6);

        let none = list_tools(&[]);
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn every_listed_tool_has_schema() {
        let all = list_tools(&["search".into(), "chat".into(), "ingest".into()]);
        for t in &all {
            assert!(t["name"].is_string());
            assert!(t["description"].is_string());
            assert!(t["inputSchema"]["type"] == "object");
        }
    }
}
