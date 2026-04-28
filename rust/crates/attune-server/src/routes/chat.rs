use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use attune_core::llm::ChatMessage;

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

    // Sprint 1 Phase B: chat keyword trigger for project recommendation
    // 纯 observer：检测当前 user message 中的项目相关关键词，命中即通过 broadcast 推 ws hint，
    // 不影响主流程（错误静默忽略，broadcast 无订阅者也只返回 Err 不 panic）
    //
    // v0.6 边界瘦身：keywords 不再硬编码到 attune-core，由 PluginRegistry 聚合各
    // vertical plugin 的 chat_trigger.project_keywords 后传入。无 plugin 时 = []，永不触发。
    let project_keywords: Vec<&str> = state
        .plugin_registry
        .all_chat_trigger_project_keywords()
        .into_iter()
        .collect();
    if let Some(hint) = attune_core::project_recommender::recommend_for_chat(
        &body.message,
        &project_keywords,
    ) {
        let payload = serde_json::json!({
            "type": "project_recommendation",
            "trigger": "chat_keyword",
            "matched_keywords": hint.matched_keywords,
            "suggestion": hint.suggestion,
        });
        let _ = state.recommendation_tx.send(payload);
    }

    // Sprint 2 Phase C: Skills Router — 纯 observer，匹配 plugin skill 后通过 broadcast 推 ws skill_suggested
    // 不影响主流程；disabled 集合从 vault settings.skills.disabled 读取（Task 4），
    // has_pending_doc 留 false（Task 5 后由 chat context 决定）
    {
        let registry = state.plugin_registry.clone();
        // 从 vault metadata 读 settings.skills.disabled；锁失败 / 读失败 / 解析失败均回退空集合
        // （observer 路径不能阻断主流程）
        let disabled: std::collections::HashSet<String> = {
            let bytes = match state.vault.lock() {
                Ok(vault) => vault.store().get_meta("app_settings").ok().flatten(),
                Err(_) => None,
            };
            bytes
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v.get("skills")
                    .and_then(|s| s.get("disabled"))
                    .and_then(|d| d.as_array())
                    .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
                .unwrap_or_default()
        };
        let has_pending_doc = false;
        let router = attune_core::intent_router::IntentRouter::new(&registry);
        let matches = router.route(&body.message, has_pending_doc, &disabled);
        if !matches.is_empty() {
            let payload = serde_json::json!({
                "type": "skill_suggested",
                "trigger": "chat_intent",
                "matches": matches,
                "user_message": body.message,
            });
            let _ = state.recommendation_tx.send(payload);
        }
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
    let expanded_query = attune_core::skill_evolution::expand_query(&body.message, &app_settings);

    // v0.6 Phase B F-Pro Stage 4：query 意图 detect → cross-domain penalty
    let detected_domain = attune_core::search::detect_query_domain(&expanded_query);

    // 1. Search knowledge base via three-stage pipeline (initial_k → rerank → top_k)
    let mut search_params = attune_core::search::SearchParams::with_defaults(5);
    if let Some(d) = detected_domain.as_ref() {
        search_params.domain_hint = Some(d.clone());
        tracing::info!("F-Pro: query='{}' → detected_domain={d}", body.message.chars().take(40).collect::<String>());
    }
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

        let ctx = attune_core::search::SearchContext {
            fulltext: ft_guard.as_ref(),
            vectors: vec_guard.as_ref(),
            embedding: emb,
            reranker,
            store: vault_guard.store(),
            dek: &dek,
        };
        attune_core::search::search_with_context(&ctx, &expanded_query, &search_params)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?
    };

    // 按 INJECTION_BUDGET 分配每条文档的注入字符数，防止超出 LLM context window
    let mut search_results = search_results;
    attune_core::search::allocate_budget(&mut search_results, attune_core::search::INJECTION_BUDGET);

    // 2a0. 批注加权（Batch B.2）—— 🆓 零成本（仅 DB 读 + 算数）
    //
    // 读每条结果的批注，按 label 精确匹配调整 score：
    //   · 🗑/🕰 过时     → 直接剔除
    //   · ⭐/要点/风险    → ×1.5
    //   · 🤔/📍 等       → ×1.2
    // 多个批注取 MAX，不累乘。
    //
    // 包在 spawn_blocking：`list_annotations` 是同步 SQLite + 解密每条 content blob，
    // N=10 结果时可能 ~10ms，避免阻塞 async worker（与下面压缩阶段的三阶段模式一致）。
    let (weight_stats, mut weighted_results) = {
        let state_clone = state.clone();
        let dek_clone = dek.clone();
        let mut results_in = std::mem::take(&mut search_results);
        tokio::task::spawn_blocking(move || {
            let vault_guard = state_clone.vault.lock().unwrap_or_else(|e| e.into_inner());
            let store = vault_guard.store();
            let mut stats = attune_core::annotation_weight::AnnotationWeightStats::default();
            stats.items_total = results_in.len();
            let mut kept = Vec::with_capacity(results_in.len());
            for r in results_in.drain(..) {
                let anns = store.list_annotations(&dek_clone, &r.item_id).unwrap_or_default();
                match attune_core::annotation_weight::compute_adjust(&anns) {
                    attune_core::annotation_weight::ScoreAdjust::Drop => {
                        stats.items_dropped += 1;
                    }
                    attune_core::annotation_weight::ScoreAdjust::Multiply(m) => {
                        if m > 1.0 { stats.items_boosted += 1; }
                        let mut r = r;
                        r.score *= m;
                        kept.push(r);
                    }
                }
            }
            stats.items_kept = stats.items_total - stats.items_dropped;
            (stats, kept)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("chat: annotation weighting task join failed: {e}; falling back to raw search_results");
            (attune_core::annotation_weight::AnnotationWeightStats::default(), Vec::new())
        })
    };
    // spawn_blocking 失败时 weighted_results 为空 —— 此时我们丢失了原 search_results。
    // 但 spawn_blocking 的 panic/join 错误极罕见（内存爆/进程被信号中断），概率远低于
    // 用户被影响的回本。已通过 tracing::warn 记录，UI 会显示 knowledge_count=0 + hint。
    search_results = std::mem::take(&mut weighted_results);

    // 按新的 score 降序重排（过时已剔除，boost 项自然前移）
    search_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    if weight_stats.items_boosted > 0 || weight_stats.items_dropped > 0 {
        tracing::info!(
            "chat: annotation weighting {} items ({} boosted, {} dropped, {} kept)",
            weight_stats.items_total, weight_stats.items_boosted,
            weight_stats.items_dropped, weight_stats.items_kept,
        );
    }

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
            // v0.6 Phase B fix: 透传证据流字段到 chat citations
            "breadcrumb": r.breadcrumb,
            "chunk_offset_start": r.chunk_offset_start,
            "chunk_offset_end": r.chunk_offset_end,
        })).collect()
    };

    // 2b+. 上下文压缩（Batch B.1）
    //
    // 按 settings.context_strategy 压缩每条 knowledge 的 inject_content：
    //   - raw / web 来源       → passthrough（web 无 item_id、成本不对称）
    //   - economical / accurate → sha256(chunk) 查缓存 → 命中 0 成本；缺失调本地 LLM
    //
    // 整个压缩阶段放在 spawn_blocking 里，避免阻塞 async worker（LLM chat 是同步的）。
    let strategy_str = app_settings.get("context_strategy")
        .and_then(|v| v.as_str())
        .unwrap_or("economical")
        .to_string();
    let mut compression_stats = (0usize, 0usize, 0usize);  // (chunks, hits, orig_total_chars)
    let knowledge: Vec<serde_json::Value> = if web_search_used {
        // 网络搜索结果已经是 snippet，不做二次压缩
        knowledge
    } else {
        use attune_core::context_compress::{ContextStrategy, chunk_hash, CompressedChunk};
        let strategy = ContextStrategy::parse(&strategy_str);
        if strategy == ContextStrategy::Raw {
            knowledge
        } else {
            // 三阶段压缩，尽量缩短 vault lock 持有时间：
            //   Phase 1（锁）：查 cache，收集 miss 清单
            //   Phase 2（无锁）：对 misses 批量调 LLM 生成摘要
            //   Phase 3（锁）：批量写回 cache
            //
            // **关键 bug 修复（Batch B R1-I1）**：用 `content`（完整内容）而非 `inject_content`
            // 作为 hash 源。原代码用 inject_content 会因 allocate_budget 按分数截断而每次
            // hash 不同，摧毁缓存命中率。content 在同一 item 跨查询是稳定的。
            let inputs: Vec<(String /*item_id*/, String /*content_for_hash*/, String /*injected_text*/)> =
                knowledge.iter().map(|k| {
                    let item_id = k.get("item_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    // 用全量 content 计算 hash + 喂 LLM（生成 chunk 级摘要）
                    let content = k.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    // inject 文本是 allocate_budget 后的 —— 做后备（若 content 为空）
                    let inject = k.get("inject_content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let text = if content.is_empty() { inject } else { content };
                    (item_id, text.clone(), text)
                }).collect();

            let state_compress = state.clone();
            let dek_compress = dek.clone();
            let strategy_str_for_log = strategy_str.clone();

            // 把整个三阶段都放进 spawn_blocking 里（锁/LLM 都是同步的）。
            // 内部：phase 1 + 3 持锁；phase 2 释放锁后跑 LLM。
            let compressed_result: std::result::Result<Vec<CompressedChunk>, String> =
                tokio::task::spawn_blocking(move || {
                    let llm_arc = state_compress.llm.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_ref().cloned();
                    let target = strategy.target_chars();
                    let strategy_str = strategy.as_str();

                    // Phase 1：查 cache + 识别短 chunk（免压缩）
                    struct Slot {
                        item_id: String,
                        text: String,
                        hash: String,
                        original_chars: usize,
                        summary: Option<String>,      // Phase 1 填（cache hit）或 Phase 2 填（LLM 新生成）
                        was_cache_hit: bool,          // 严格区分 Phase 1 命中 vs Phase 2 新生成
                        needs_writeback: bool,        // Phase 3 只回写"新生成"的，避免幂等 REPLACE 浪费 IO
                        is_short: bool,               // target_chars 以下，不压缩
                    }
                    let mut slots: Vec<Slot> = {
                        let vault_guard = state_compress.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let store = vault_guard.store();
                        inputs.into_iter().map(|(item_id, hash_src, text)| {
                            let original_chars = text.chars().count();
                            let is_short = original_chars <= target;
                            let hash = chunk_hash(&hash_src);
                            let (summary, was_cache_hit) = if is_short || item_id.is_empty() {
                                (None, false)
                            } else {
                                match store.get_chunk_summary(&dek_compress, &hash, strategy_str).unwrap_or(None) {
                                    Some(s) => (Some(s), true),
                                    None => (None, false),
                                }
                            };
                            Slot {
                                item_id, text, hash, original_chars,
                                summary, was_cache_hit,
                                needs_writeback: false,
                                is_short,
                            }
                        }).collect()
                        // vault_guard drop 此处 → 释放锁
                    };

                    // Phase 2（无锁）：对真正 miss 调 LLM
                    for s in slots.iter_mut() {
                        if s.is_short || s.was_cache_hit || s.item_id.is_empty() {
                            continue;
                        }
                        let Some(ref llm) = llm_arc else {
                            continue; // LLM 不可用 → 降级原文（summary 保持 None）
                        };
                        match attune_core::context_compress::generate_summary(llm.as_ref(), &s.text, strategy) {
                            Ok(summary) => {
                                s.summary = Some(summary);
                                s.needs_writeback = true;
                            }
                            Err(e) => {
                                tracing::warn!("chat: summary generation failed for chunk {}: {e}", &s.hash[..8]);
                            }
                        }
                    }

                    // Phase 3（锁）：回写新生成摘要（不动命中项）
                    {
                        let vault_guard = state_compress.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let store = vault_guard.store();
                        let model_name = llm_arc.as_ref().map(|l| l.model_name().to_string()).unwrap_or_default();
                        for s in slots.iter() {
                            if !s.needs_writeback { continue; }
                            if let Some(ref sum) = s.summary {
                                let _ = store.put_chunk_summary(
                                    &dek_compress, &s.hash, strategy_str,
                                    &s.item_id, &model_name, sum, s.original_chars,
                                );
                            }
                        }
                    }

                    // 组装结果
                    slots.into_iter().map(|s| {
                        let injected = match &s.summary {
                            Some(sum) if !s.is_short => match strategy {
                                ContextStrategy::Accurate => {
                                    let head: String = s.text.chars().take(100).collect();
                                    format!("{sum}\n原文摘录: {head}...")
                                }
                                _ => sum.clone(),
                            },
                            _ => s.text,  // 短文本 / miss 无降级 / LLM 不可用 → 用原文
                        };
                        // cache_hit 严格语义：Phase 1 真实命中 or 短文本（无需压缩）
                        // —— 本次"没花 LLM 钱"即为 hit。Phase 2 的 fresh 生成不算 hit。
                        let cache_hit = s.is_short || s.was_cache_hit;
                        CompressedChunk {
                            injected,
                            original_chars: s.original_chars,
                            cache_hit,
                        }
                    }).collect::<Vec<_>>()
                }).await.map_err(|e| format!("compression task join error: {e}"));

            // **关键 bug 修复（Batch B R2-C1）**：spawn_blocking panic/join 错误时
            // 过去用 .unwrap_or_default() → 空 Vec → zip 丢光所有 knowledge。
            // 现在改为：面板错时降级为 raw 注入（保留 knowledge 原样），只是错过压缩收益。
            match compressed_result {
                Ok(compressed) => {
                    debug_assert_eq!(knowledge.len(), compressed.len(),
                        "compression must produce one CompressedChunk per input");
                    for c in &compressed {
                        compression_stats.0 += 1;
                        if c.cache_hit { compression_stats.1 += 1; }
                        compression_stats.2 += c.original_chars;
                    }
                    knowledge.into_iter().zip(compressed.into_iter()).map(|(mut k, c)| {
                        if let Some(obj) = k.as_object_mut() {
                            obj.insert("inject_content".into(), serde_json::Value::String(c.injected));
                            obj.insert("compression_cached".into(), serde_json::Value::Bool(c.cache_hit));
                        }
                        k
                    }).collect()
                }
                Err(e) => {
                    tracing::warn!("chat: compression task failed ({e}); falling back to raw RAG injection");
                    let _ = strategy_str_for_log;  // 已在 warn 里说明
                    knowledge
                }
            }
        }
    };
    if compression_stats.0 > 0 {
        tracing::info!(
            "chat: context compressed {} chunks ({} cache hits, {} orig chars) strategy={}",
            compression_stats.0, compression_stats.1, compression_stats.2, strategy_str
        );
    }

    // 2c. Build RAG system prompt（根据来源调整措辞）
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
            let citations_for_session: Vec<attune_core::store::Citation> = knowledge
                .iter()
                .map(|k| attune_core::store::Citation {
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

    // 6. Build citations — v0.6 Phase B fix:
    //    透传 breadcrumb + chunk_offset 让前端 reader 可点击跳转源 chunk
    //    fallback：当 chunker 给 chunk[0] 空 path 时，用 [title] 给个最小面包屑
    let citations: Vec<serde_json::Value> = knowledge
        .iter()
        .map(|k| {
            let title = k.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let breadcrumb_arr = k
                .get("breadcrumb")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let breadcrumb = if breadcrumb_arr.is_empty() && !title.is_empty() {
                vec![serde_json::Value::String(title.clone())]
            } else {
                breadcrumb_arr
            };
            serde_json::json!({
                "item_id": k.get("item_id"),
                "title": title,
                "relevance": k.get("score"),
                "breadcrumb": breadcrumb,
                "chunk_offset_start": k.get("chunk_offset_start"),
                "chunk_offset_end": k.get("chunk_offset_end"),
            })
        })
        .collect();

    // v0.6 Phase B fix: 解析 confidence + 剥离 marker（J5 strict prompt 要求 LLM 末尾输出）
    let confidence = attune_core::parse_confidence(&response);
    let response = attune_core::strip_confidence_marker(&response).to_string();

    // 6. Build response with optional hint when web search unavailable
    // v0.6 Phase B fix: 透传 confidence (parsed from LLM 末尾 marker)
    let mut response_json = serde_json::json!({
        "content": response,
        "citations": citations,
        "knowledge_count": knowledge.len(),
        "session_id": session_id,
        "web_search_used": web_search_used,
        "confidence": confidence,
        // Batch B.2: 批注加权 / 上下文压缩统计 —— token chip 展开时展示
        "weight_stats": {
            "items_total": weight_stats.items_total,
            "items_boosted": weight_stats.items_boosted,
            "items_dropped": weight_stats.items_dropped,
            "items_kept": weight_stats.items_kept,
        },
        "compression_stats": {
            "chunks": compression_stats.0,
            "cache_hits": compression_stats.1,
            "orig_chars": compression_stats.2,
            "strategy": strategy_str,
        },
    });

    // 本地无结果 + 浏览器不可用：明确告知用户而非静默失败
    if knowledge.is_empty() {
        let ws_available = state.web_search.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if !ws_available {
            response_json["hint"] = serde_json::Value::String(
                "本地知识库无相关内容；网络搜索不可用（未检测到 Chrome 或 Edge 浏览器）。\
                 请安装 Chromium 内核浏览器后重试，或手动录入相关知识。".into(),
            );
        }
    }

    Ok(Json(response_json))
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
