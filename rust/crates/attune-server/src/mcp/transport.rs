//! G1 — MCP transport (HTTP + SSE)。
//!
//! - `POST /mcp` — JSON-RPC 2.0 over HTTP (主路径)。`Authorization: Bearer <scoped-token>`。
//! - `GET /mcp/sse` — SSE 流 (MCP HTTP+SSE transport 的 server→client 通道;headless 24h
//!   常驻时 agent host 用它接 server 事件)。当前发 endpoint 事件 + keep-alive。
//!
//! token 校验:Bearer → `vault.verify_scoped_token` → [`crate::mcp::CallCtx`]。
//! 失败 → JSON-RPC unauthorized error (HTTP 200, error 在 body —— JSON-RPC 惯例)。
//!
//! 这两条路由 **不走** 普通 `bearer_auth_guard`(那校验的是 vault session token);MCP 用
//! scoped token,独立校验。lib.rs 在 vault_guard / bearer_auth_guard 之外单独挂 `/mcp*`。

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures_util::stream::{self, Stream};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;

use crate::mcp::{self, gate, CallCtx};
use crate::state::SharedState;

/// 从 `Authorization: Bearer <token>` 提取 token。
fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.to_string())
}

/// 校验 scoped token → CallCtx。返回 Err((reason, code)) 供上层转 JSON-RPC error。
fn authenticate(state: &SharedState, headers: &HeaderMap) -> Result<CallCtx, (String, i64)> {
    let token = extract_bearer(headers).ok_or_else(|| {
        (
            "missing bearer scoped-token".to_string(),
            gate::ERR_UNAUTHORIZED,
        )
    })?;
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    match vault.verify_scoped_token(&token) {
        Ok(meta) => Ok(CallCtx {
            agent_source: meta.label,
            scopes: meta.scopes,
        }),
        Err(e) => Err((
            format!("scoped-token rejected: {e}"),
            gate::ERR_UNAUTHORIZED,
        )),
    }
}

/// POST /mcp — JSON-RPC 2.0 over HTTP。
pub async fn mcp_http(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    let id = req.get("id").cloned().unwrap_or(Value::Null);

    // 认证 (scoped token)。失败 → JSON-RPC error (HTTP 200, error in body)。
    let ctx = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err((msg, code)) => {
            return (
                StatusCode::OK,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": code, "message": msg }
                })),
            );
        }
    };

    let resp = mcp::handle_request(&state, &ctx, &req).await;
    (StatusCode::OK, Json(resp))
}

/// GET /mcp/sse — MCP HTTP+SSE transport 的 server→client 流。
///
/// 认证同样走 scoped token (query 或 header)。当前发一个 `endpoint` 事件 (告知 client POST
/// 回哪) + keep-alive 心跳。完整双向工具调用经 POST /mcp;SSE 是 server push 通道占位
/// (24h headless 常驻保活)。
pub async fn mcp_sse(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)> {
    // SSE 需先认证 —— 未授权直接 401 (SSE 无 JSON-RPC error body 语义)。
    authenticate(&state, &headers).map_err(|(msg, _)| (StatusCode::UNAUTHORIZED, msg))?;

    // 单 endpoint 事件 + keep-alive。client 收到 endpoint 后 POST /mcp 走 JSON-RPC。
    let endpoint_event = Event::default()
        .event("endpoint")
        .data(json!({ "uri": "/mcp" }).to_string());
    let s = stream::once(async move { Ok(endpoint_event) });

    Ok(Sse::new(s).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn extract_bearer_parses_header() {
        let mut h = HeaderMap::new();
        h.insert("authorization", HeaderValue::from_static("Bearer abc.def"));
        assert_eq!(extract_bearer(&h).as_deref(), Some("abc.def"));

        let empty = HeaderMap::new();
        assert!(extract_bearer(&empty).is_none());

        let mut wrong = HeaderMap::new();
        wrong.insert("authorization", HeaderValue::from_static("Basic xxx"));
        assert!(extract_bearer(&wrong).is_none());
    }

    #[test]
    fn authenticate_rejects_missing_and_bad_token() {
        let state = crate::test_support::unlocked_state();
        let empty = HeaderMap::new();
        let (_, code) = authenticate(&state, &empty).unwrap_err();
        assert_eq!(code, gate::ERR_UNAUTHORIZED);

        let mut bad = HeaderMap::new();
        bad.insert(
            "authorization",
            HeaderValue::from_static("Bearer not.a.real.token"),
        );
        assert!(authenticate(&state, &bad).is_err());
    }

    #[test]
    fn authenticate_accepts_valid_scoped_token() {
        let state = crate::test_support::unlocked_state();
        let (token, _meta) = {
            let vault = state.vault.lock().unwrap();
            vault
                .issue_scoped_token("hermes", &["search".into(), "chat".into()], None)
                .unwrap()
        };
        let mut h = HeaderMap::new();
        h.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let ctx = authenticate(&state, &h).unwrap();
        assert_eq!(ctx.agent_source, "hermes");
        assert_eq!(ctx.scopes, vec!["search", "chat"]);
    }
}
