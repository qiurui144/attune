use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use attune_core::vault::VaultState;

use crate::state::SharedState;

#[derive(Deserialize)]
pub struct WsAuth {
    pub token: Option<String>,
}

/// GET /ws/scan-progress?token=<bearer>
/// 每 2 秒推送一次队列进度 JSON，vault locked 时推送 locked 状态后持续等待。
///
/// OSS-S16 fix: token 通过 query string 传，因为 RFC 6455 subprotocol 不允许含 `:` 字符
/// (token 格式 `id:ts:hash` 含 colon)。Middleware 已 bypass 此 endpoint，由本 handler
/// 自查 token 校验。校验失败立即关闭连接。
pub async fn scan_progress(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    Query(auth): Query<WsAuth>,
) -> Response {
    // 当 require_auth 启用时，必须有 token
    if state.require_auth {
        let token = match auth.token {
            Some(t) if !t.is_empty() => t,
            _ => {
                return (StatusCode::UNAUTHORIZED, "missing token query param").into_response();
            }
        };
        let verify = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            vault.verify_session(&token)
        };
        if verify.is_err() {
            return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
    }
    ws.on_upgrade(|socket| handle_scan_progress(socket, state))
}

async fn handle_scan_progress(mut socket: WebSocket, state: SharedState) {
    let interval = std::time::Duration::from_secs(2);
    let mut rx = state.recommendation_tx.subscribe();

    loop {
        // 1. 推 progress（原有）
        let payload = {
            let vault_guard = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let vault_state = vault_guard.state();
            if !matches!(vault_state, VaultState::Unlocked) {
                serde_json::json!({
                    "type": "progress",
                    "vault_state": "locked",
                    "pending_embeddings": 0,
                    "pending_classify": 0,
                    "bound_dirs": 0,
                })
            } else {
                let pending_embed = vault_guard
                    .store()
                    .pending_count_by_type("embed")
                    .unwrap_or(0);
                let pending_classify = vault_guard
                    .store()
                    .pending_count_by_type("classify")
                    .unwrap_or(0);
                let bound_dirs = vault_guard
                    .store()
                    .list_bound_directories()
                    .map(|v| v.len())
                    .unwrap_or(0);
                serde_json::json!({
                    "type": "progress",
                    "vault_state": "unlocked",
                    "pending_embeddings": pending_embed,
                    "pending_classify": pending_classify,
                    "bound_dirs": bound_dirs,
                })
            }
        };
        if socket.send(Message::Text(payload.to_string().into())).await.is_err() {
            break;
        }

        // 2. 非阻塞拉所有积压的 recommendation 一并推
        while let Ok(rec_payload) = rx.try_recv() {
            if socket
                .send(Message::Text(rec_payload.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }

        tokio::time::sleep(interval).await;
    }
}
