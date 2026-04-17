use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use attune_core::vault::VaultState;

use crate::state::SharedState;

/// GET /ws/scan-progress
/// 每 2 秒推送一次队列进度 JSON，vault locked 时推送 locked 状态后持续等待。
pub async fn scan_progress(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
) -> Response {
    ws.on_upgrade(|socket| handle_scan_progress(socket, state))
}

async fn handle_scan_progress(mut socket: WebSocket, state: SharedState) {
    let interval = std::time::Duration::from_secs(2);

    loop {
        let payload = {
            let vault_guard = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let vault_state = vault_guard.state();

            if !matches!(vault_state, VaultState::Unlocked) {
                serde_json::json!({
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
                    "vault_state": "unlocked",
                    "pending_embeddings": pending_embed,
                    "pending_classify": pending_classify,
                    "bound_dirs": bound_dirs,
                })
            }
        };

        let text = payload.to_string();
        if socket.send(Message::Text(text.into())).await.is_err() {
            break;
        }

        tokio::time::sleep(interval).await;
    }
}
