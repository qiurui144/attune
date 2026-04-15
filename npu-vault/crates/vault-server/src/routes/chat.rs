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
/// 历史消息单条 content 最大字节数（防止绕过 message 限制的大负载攻击）
const MAX_HISTORY_CONTENT_LEN: usize = 8_192;
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
        if h.content.len() > MAX_HISTORY_CONTENT_LEN {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("history message content too long (max {MAX_HISTORY_CONTENT_LEN} bytes)")
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

    // 1a. 读取 app_settings（用于查询扩展 + web_search 配置）
    let app_settings: serde_json::Value = {
        let vault = state.vault.lock()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock"}))))?;
        vault.store().get_meta("app_settings")
            .ok()
            .flatten()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    };

    // 1b. 用 learned_expansions 自动扩展查询词（语义扩展，透明无感）
    let expanded_query = vault_core::skill_evolution::expand_query(&body.message, &app_settings);

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
        vault_core::search::search_with_context(&ctx, &expanded_query, &search_params)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?
    };

    // 按 INJECTION_BUDGET 分配每条文档的注入字符数，防止超出 LLM context window
    let mut search_results = search_results;
    vault_core::search::allocate_budget(&mut search_results, vault_core::search::INJECTION_BUDGET);

    // 2a. 本地无结果时记录失败信号（后台技能进化的驱动数据），非阻塞
    if search_results.is_empty() {
        let signal_state = state.clone();
        let signal_query = body.message.clone();
        tokio::spawn(async move {
            let vault = signal_state.vault.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = vault.store().record_skill_signal(&signal_query, 0, false) {
                tracing::debug!("record_skill_signal failed (non-fatal): {e}");
            }
        });
    }

    // 2b. 若本地无结果，尝试网络搜索 fallback
    let web_search_used;
    let knowledge: Vec<serde_json::Value> = if search_results.is_empty() {
        let ws = state.web_search.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(ws_provider) = ws {
            let query = body.message.clone();
            let web_results = tokio::task::spawn_blocking(move || {
                ws_provider.search(&query, 3)
            })
            .await
            .unwrap_or(Ok(vec![]))
            .unwrap_or_default();

            if !web_results.is_empty() {
                web_search_used = true;
                web_results.into_iter().map(|r| serde_json::json!({
                    "item_id": format!("web:{}", r.url),
                    "title": r.title,
                    "inject_content": r.snippet,
                    "content": r.snippet,
                    "score": 0.55,
                    "source_type": "web",
                    "url": r.url,
                })).collect()
            } else {
                web_search_used = false;
                vec![]
            }
        } else {
            web_search_used = false;
            vec![]
        }
    } else {
        web_search_used = false;
        search_results.iter().map(|r| serde_json::json!({
            "item_id": r.item_id,
            "title": r.title,
            "inject_content": r.inject_content,
            "content": r.content,
            "score": r.score,
            "source_type": r.source_type,
        })).collect()
    };

    // 2b. Build RAG system prompt（根据来源调整措辞）
    let mut system_prompt = if web_search_used {
        "你是用户的个人知识助手。本地知识库暂无相关内容，以下来自实时网络搜索。\n\
         请基于这些搜索结果回答用户的问题，并在回答末尾标注「来源：[URL]」。\n\
         如果搜索结果不够可靠，请明确说明并补充你自己的判断。\n\n".to_string()
    } else {
        "你是用户的个人知识助手。以下是从用户本地知识库中检索到的相关文档。\n\
         请基于这些知识回答用户的问题。如果引用了某个文档，请标注 [文档标题]。\n\
         如果知识库中没有相关信息，正常回答即可，不要编造引用。\n\n".to_string()
    };

    if !knowledge.is_empty() {
        let section_label = if web_search_used {
            "=== 网络搜索结果 ==="
        } else {
            "=== 知识库相关文档 ==="
        };
        system_prompt.push_str(section_label);
        system_prompt.push_str("\n\n");
        for (i, k) in knowledge.iter().enumerate() {
            let title = k.get("title").and_then(|v| v.as_str()).unwrap_or("?");
            let content = k.get("inject_content").and_then(|v| v.as_str())
                .or_else(|| k.get("content").and_then(|v| v.as_str()))
                .unwrap_or("");
            if web_search_used {
                let url = k.get("url").and_then(|v| v.as_str()).unwrap_or("");
                system_prompt.push_str(&format!("[{}] 《{}》\nURL: {}\n{}\n\n", i + 1, title, url, content));
            } else {
                let score = k.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                system_prompt.push_str(&format!("[{}] 《{}》(相关度: {:.0}%)\n{}\n\n", i + 1, title, score.max(0.0) * 100.0, content));
            }
        }
        system_prompt.push_str("=== 参考内容结束 ===\n");
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
        "web_search_used": web_search_used,
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
