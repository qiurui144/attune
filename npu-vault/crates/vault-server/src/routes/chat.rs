use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use vault_core::llm::ChatMessage;

use crate::state::SharedState;

type ApiError = (StatusCode, Json<serde_json::Value>);

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub history: Vec<HistoryMessage>,
    pub session_id: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// POST /api/v1/chat -- RAG 对话（非流式）
/// 消息最大字节数（与 MAX_SEQ_LEN 对齐，防止 LLM 请求体过大）
const MAX_MESSAGE_LEN: usize = 32_768;
/// 历史消息最大条数（超限则截断至最近 N 条）
const MAX_HISTORY_DEPTH: usize = 20;

pub async fn chat(
    State(state): State<SharedState>,
    Json(mut body): Json<ChatRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Input validation — 在所有状态检查之前优先拒绝无效输入
    if body.message.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "message cannot be empty"}))));
    }
    if body.message.len() > MAX_MESSAGE_LEN {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": format!("message too long (max {MAX_MESSAGE_LEN} bytes)")
        }))));
    }
    // 白名单校验 history role：防止客户端注入 system 消息绕过 RAG 指令
    const ALLOWED_ROLES: &[&str] = &["user", "assistant"];
    for h in &body.history {
        if !ALLOWED_ROLES.contains(&h.role.as_str()) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("invalid role '{}': must be 'user' or 'assistant'", h.role)
                })),
            ));
        }
    }
    // 静默截断历史深度：保留最近 N 条
    if body.history.len() > MAX_HISTORY_DEPTH {
        let drop = body.history.len() - MAX_HISTORY_DEPTH;
        body.history.drain(..drop);
    }

    // Check LLM availability
    let llm = state.llm.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "llm lock poisoned"}))))?
        .as_ref().cloned();
    let llm = match llm {
        Some(l) => l,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "AI 后端不可用",
                    "hint": "请安装 Ollama 并下载 chat 模型: ollama pull qwen2.5:3b"
                })),
            ))
        }
    };

    let dek = {
        let vault = state.vault.lock()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
        vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?
    };

    // 1. Search knowledge base via three-stage pipeline (initial_k → rerank → top_k)
    let search_params = vault_core::search::SearchParams::with_defaults(5);
    let reranker = state.reranker.lock().map_err(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "reranker lock"})))
    })?.clone();
    let emb = state.embedding.lock().map_err(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "emb lock"})))
    })?.clone();

    let search_results = {
        let ft_guard = state.fulltext.lock().map_err(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "ft lock"})))
        })?;
        let vec_guard = state.vectors.lock().map_err(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vec lock"})))
        })?;
        let vault_guard = state.vault.lock().map_err(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock"})))
        })?;

        let ctx = vault_core::search::SearchContext {
            fulltext: ft_guard.as_ref(),
            vectors: vec_guard.as_ref(),
            embedding: emb,
            reranker,
            store: vault_guard.store(),
            dek: &dek,
        };
        vault_core::search::search_with_context(&ctx, &body.message, &search_params)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?
    };

    // 按 INJECTION_BUDGET 分配每条文档的注入字符数，防止超出 LLM context window
    let mut search_results = search_results;
    vault_core::search::allocate_budget(&mut search_results, vault_core::search::INJECTION_BUDGET);

    let knowledge: Vec<serde_json::Value> = search_results
        .iter()
        .map(|r| serde_json::json!({
            "item_id": r.item_id,
            "title": r.title,
            // inject_content 是预算截断后的内容；content 保留全文用于 citations
            "inject_content": r.inject_content,
            "content": r.content,
            "score": r.score,
            "source_type": r.source_type,
        }))
        .collect();

    // 2. Build RAG system prompt
    let mut system_prompt = String::from(
        "你是用户的个人知识助手。以下是从用户本地知识库中检索到的相关文档。\n\
         请基于这些知识回答用户的问题。如果引用了某个文档，请标注 [文档标题]。\n\
         如果知识库中没有相关信息，正常回答即可，不要编造引用。\n\n",
    );

    if !knowledge.is_empty() {
        system_prompt.push_str("=== 知识库相关文档 ===\n\n");
        for (i, k) in knowledge.iter().enumerate() {
            let title = k.get("title").and_then(|v| v.as_str()).unwrap_or("?");
            // 优先使用 inject_content（预算截断版），回退到 content（全文）
            let content = k.get("inject_content").and_then(|v| v.as_str())
                .or_else(|| k.get("content").and_then(|v| v.as_str()))
                .unwrap_or("");
            let score = k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            system_prompt.push_str(&format!(
                "[{}] 《{}》(相关度: {:.0}%)\n{}\n\n",
                i + 1,
                title,
                score.max(0.0) * 100.0,
                content
            ));
        }
        system_prompt.push_str("=== 知识库结束 ===\n");
    }

    // 3. Build messages with history
    let mut messages: Vec<ChatMessage> = vec![ChatMessage::system(&system_prompt)];
    for h in &body.history {
        messages.push(ChatMessage {
            role: h.role.clone(),
            content: h.content.clone(),
        });
    }
    messages.push(ChatMessage::user(&body.message));

    // 4. Call LLM (blocking via spawn_blocking)
    let response = tokio::task::spawn_blocking(move || llm.chat_with_history(&messages))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    // 5. Persist to conversation session
    let session_id = {
        let vault = state.vault.lock()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
        let title: String = body.message.chars().take(50).collect();
        // 取已有或新建 session；create_conversation 失败时跳过消息持久化（不插入孤悬消息）
        let sid_opt: Option<String> = match &body.session_id {
            Some(id) => {
                // 验证 session 存在；不存在则自动创建（保证 append_message 外键约束成功）
                match vault.store().get_conversation_by_id(&dek, id) {
                    Ok(Some(_)) => Some(id.clone()),
                    _ => {
                        tracing::warn!("session_id {id} not found, creating new session");
                        vault.store().create_conversation(&dek, &title)
                            .map_err(|e| tracing::warn!("create_conversation failed: {e}"))
                            .ok()
                    }
                }
            }
            None => vault.store().create_conversation(&dek, &title)
                .map_err(|e| tracing::warn!("create_conversation failed: {e}"))
                .ok(),
        };
        if let Some(sid) = sid_opt.as_ref() {
            // 构造引用列表
            let citations_for_session: Vec<vault_core::store::Citation> = knowledge
                .iter()
                .map(|k| vault_core::store::Citation {
                    item_id: k.get("item_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    title: k.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    relevance: k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                })
                .collect();
            // 使用事务原子写入 user+assistant 一对：任一失败则两条均不写入
            if let Err(e) = vault.store().append_conversation_turn(
                &dek, sid, &body.message, &response, &citations_for_session,
            ) {
                tracing::warn!("failed to persist conversation turn to session {sid}: {e}");
            }
        }
        sid_opt
    };

    // 6. Build citations
    let citations: Vec<serde_json::Value> = knowledge
        .iter()
        .map(|k| {
            serde_json::json!({
                "item_id": k.get("item_id"),
                "title": k.get("title"),
                "relevance": k.get("score"),
            })
        })
        .collect();

    // session_id 为 null 表示会话持久化失败（不影响本次 AI 响应）
    Ok(Json(serde_json::json!({
        "content": response,
        "citations": citations,
        "knowledge_count": knowledge.len(),
        "session_id": session_id,
    })))
}

/// GET /api/v1/chat/history -- 已废弃，返回与 /chat/sessions 一致的格式
/// @deprecated 请使用 GET /api/v1/chat/sessions?limit=50&offset=0
pub async fn chat_history(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let vault = state.vault.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "vault lock"})),
        )
    })?;
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let sessions = vault.store().list_conversations(&dek, 50, 0).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    // 返回与 /chat/sessions 相同的 key 结构，保持 API 一致性
    Ok(Json(serde_json::json!({"sessions": sessions, "total": sessions.len()})))
}
