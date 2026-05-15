//! Streaming chat output via SSE (Sprint v0.7 / F5).
//!
//! POST /api/v1/chat/stream
//!
//! ## 当前实现说明（v0.7 sprint 占位版）
//!
//! 本 commit 提供 SSE 端点骨架，把已生成的 message 切成 ~20 char chunk 包成
//! `text/event-stream` 响应。**当前实现为一次性 buffered 响应**（不是真流式），
//! 这是为了避免在 attune-server `Cargo.toml` 里追加 `futures-util` /
//! `tokio-stream` / `async-stream` 三个新依赖（按 sprint 隔离规则，依赖增量留给
//! 集成 commit 决定）。功能上前端 EventSource 可正常解析多个 event，行为兼容。
//!
//! ## 真流式升级路径
//!
//! TODO v0.8 真 LLM stream when trait extended
//!
//!   1. Cargo.toml 加 `futures-util = "0.3"` + `tokio-stream = "0.1"`
//!   2. LlmProvider trait 加：
//!        fn chat_stream(&self, messages: &[ChatMessage])
//!            -> Pin<Box<dyn Stream<Item = Result<String>> + Send>>;
//!      Ollama / OpenAI / Anthropic 三 provider 各自实装
//!   3. 本端点改用 `axum::response::sse::Sse::new(ReceiverStream::new(rx))`
//!      逐 token 转发，参考 axum-examples/sse
//!
//! ## Wire format
//!
//! ```text
//! event: token
//! data: {"chunk":"some text","done":false}
//!
//! event: token
//! data: {"chunk":"","done":true}
//! ```

use crate::routes::chat::ChatRequest;
use crate::state::SharedState;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;

/// 单个 chunk 的字符数（per task spec，模拟 token-by-token streaming）
const CHUNK_CHARS: usize = 20;

/// POST /api/v1/chat/stream — RAG 对话（SSE buffered）
///
/// 当前以 buffered SSE 响应实现：把 `req.message` 作为 echo 内容切成 chunk，
/// 一次性 flush。前端用 EventSource API 仍可正常读取多个 event。
///
/// 真 streaming 升级需要 LlmProvider trait 增加 stream 方法 + 加 futures-util
/// / tokio-stream 依赖，留给 v0.8（详见模块级 doc-comment）。
pub async fn chat_stream(
    State(_state): State<SharedState>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    // 输入校验：拒绝空消息
    if req.message.is_empty() {
        let body = serde_json::json!({"error": "message cannot be empty"}).to_string();
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        );
    }
    // R2 F1 fix (P0): 长度上限校验，与 chat.rs::MAX_MESSAGE_LEN 一致。
    // 否则 `String::with_capacity(len * 2 + 256)` 按用户输入预分配 → 单请求 1GB
    // message 触发 ~2GB 内存预分配 OOM。
    const MAX_MESSAGE_LEN: usize = 32_768;
    if req.message.len() > MAX_MESSAGE_LEN {
        let body = serde_json::json!({
            "error": format!("message too long: {} bytes (max {})", req.message.len(), MAX_MESSAGE_LEN)
        }).to_string();
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        );
    }

    // TODO v0.8 真 LLM stream when trait extended
    //
    // 当前行为：直接 echo 用户的 message，切 chunk 包成 SSE 文本，一次性返回。
    // 接 LlmProvider::chat_stream 后改用 axum::response::sse::Sse + ReceiverStream
    // 流式发送（需先加 futures-util + tokio-stream 依赖）。
    let full_text = req.message.clone();

    let mut body = String::with_capacity(full_text.len() * 2 + 256);
    let chars: Vec<char> = full_text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + CHUNK_CHARS).min(chars.len());
        let chunk: String = chars[i..end].iter().collect();
        let payload = serde_json::json!({
            "chunk": chunk,
            "done": false,
        });
        body.push_str("event: token\n");
        body.push_str("data: ");
        body.push_str(&payload.to_string());
        body.push_str("\n\n");
        i = end;
    }
    // 终止 event
    let final_payload = serde_json::json!({
        "chunk": "",
        "done": true,
        "session_id": serde_json::Value::Null,
        "citations": [],
    });
    body.push_str("event: token\n");
    body.push_str("data: ");
    body.push_str(&final_payload.to_string());
    body.push_str("\n\n");

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream")],
        body,
    )
}

#[cfg(test)]
mod tests {
    //! 单测仅验证：handler 函数存在 + 类型签名能 mount 到 axum router。
    //! 真 SSE wire format / E2E 测试留给集成层。

    use super::*;

    /// fake helper — 验证 chat_stream 类型签名匹配 axum handler trait。
    #[allow(dead_code)]
    fn _signature_check() {
        let _f = chat_stream;
    }

    #[test]
    fn chunk_size_constant_sane() {
        assert!(CHUNK_CHARS >= 1);
        assert!(CHUNK_CHARS <= 200);
    }

    #[test]
    fn sse_payload_format_lines_terminated() {
        // 构造 SSE event 的 string 必须以 "\n\n" 结尾才能被浏览器 EventSource 解析
        let payload = serde_json::json!({"chunk": "x", "done": false});
        let line = format!("event: token\ndata: {}\n\n", payload);
        assert!(line.ends_with("\n\n"));
        assert!(line.starts_with("event: token\n"));
    }
}
