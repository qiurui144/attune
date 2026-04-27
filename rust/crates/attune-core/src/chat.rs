// npu-vault/crates/vault-core/src/chat.rs

use crate::crypto::Key32;
use crate::error::Result;
use crate::index::FulltextIndex;
use crate::llm::{ChatMessage, LlmProvider};
use crate::search::{allocate_budget, SearchResult, INJECTION_BUDGET};
use crate::store::Store;
use crate::vectors::VectorIndex;
use crate::web_search::WebSearchProvider;
use std::sync::{Arc, Mutex};

/// RAG 对话引擎
pub struct ChatEngine {
    llm: Arc<dyn LlmProvider>,
    store: Arc<Mutex<Store>>,
    fulltext: Arc<Mutex<Option<FulltextIndex>>>,
    vectors: Arc<Mutex<Option<VectorIndex>>>,
    embedding: Arc<Mutex<Option<Arc<dyn crate::embed::EmbeddingProvider>>>>,
    reranker: Arc<Mutex<Option<Arc<dyn crate::infer::RerankProvider>>>>,
    /// 可选网络搜索提供者：本地知识库无结果时作为 fallback
    web_search: Option<Arc<dyn WebSearchProvider>>,
}

/// 对话响应
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatResponse {
    pub content: String,
    pub citations: Vec<Citation>,
    pub knowledge_count: usize,
    /// 本次回答是否使用了网络搜索补充
    pub web_search_used: bool,
    // ── J5 (W2, 2026-04-27)：置信度 + 二次检索追踪 ──────────────────────────
    /// LLM 自评置信度 1-5；缺失时 fallback 到 3（中性）。
    /// per Self-RAG 论文 (arXiv:2310.11511) 的 token-level confidence 简化版。
    pub confidence: u8,
    /// 第一次 LLM 置信度 < 3 时触发了一次降阈值二次检索。
    /// per CRAG 论文 (arXiv:2401.15884) §3.2 三分类门控的 ambiguous 分支。
    pub secondary_retrieval_used: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Citation {
    pub item_id: String,
    pub title: String,
    pub relevance: f32,
    // ── B1 (W2, 2026-04-27)：deep-link 数据 ───────────────────────────────
    /// 字符级 offset 到源 item content（含 start，不含 end）。
    /// web 搜索结果为 None（无源 item 可定位）。
    /// **Known limitation (W3 batch A v1)**：当前 offset 是 sidecar 内累计 char，
    /// 不严格对齐原文 char index — 适合顶层导航不适合精确 Reader 高亮（W5+ 修）。
    /// per reviewer S2：skip_serializing_if 让 None 不出现在 JSON，前端不必处理 null。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_offset_start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_offset_end: Option<usize>,
    /// 来自 J1 chunker 面包屑路径，例如 ["公司手册", "第三章 福利", "3.2 假期"]。
    /// 无章节切分的源（plain notes）为空 Vec。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb: Vec<String>,
}

impl ChatEngine {
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        store: Arc<Mutex<Store>>,
        fulltext: Arc<Mutex<Option<FulltextIndex>>>,
        vectors: Arc<Mutex<Option<VectorIndex>>>,
        embedding: Arc<Mutex<Option<Arc<dyn crate::embed::EmbeddingProvider>>>>,
        reranker: Arc<Mutex<Option<Arc<dyn crate::infer::RerankProvider>>>>,
    ) -> Self {
        Self { llm, store, fulltext, vectors, embedding, reranker, web_search: None }
    }

    /// 设置网络搜索提供者（链式调用）
    pub fn with_web_search(mut self, ws: Arc<dyn WebSearchProvider>) -> Self {
        self.web_search = Some(ws);
        self
    }

    /// RAG 对话：搜索知识库 -> (可选) 网络搜索 fallback -> 构建 prompt -> 调用 LLM
    ///
    /// J5 (W2, per CRAG arXiv:2401.15884 + Self-RAG arXiv:2310.11511)：
    /// - prompt 强约束 + 末尾置信度自评（build_rag_system_prompt）
    /// - confidence < 3 时触发**一次**降阈值二次检索（min_score 0.65 → 0.55）
    /// - 硬上限一次重试，避免无限循环
    pub fn chat(
        &self,
        user_message: &str,
        history: &[ChatMessage],
        dek: &Key32,
    ) -> Result<ChatResponse> {
        // 1. 搜索本地知识库（默认阈值 0.65）
        let local_knowledge = self.search_for_context(user_message, dek, 5, None)?;

        // 2. 若本地无结果，尝试网络搜索 fallback（C1 W3 batch A：先查本地缓存）
        let (mut knowledge, web_search_used) = if local_knowledge.is_empty() {
            if let Some(ws) = &self.web_search {
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                // C1: 先查 web_search_cache（避免重复网络调用）
                let cached = self
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.get_web_search_cached(dek, user_message, now_secs).ok().flatten());

                let web_results = match cached {
                    Some(hits) => {
                        log::info!("C1: web_search cache HIT (saved network call)");
                        Ok(hits)
                    }
                    None => match ws.search(user_message, 3) {
                        Ok(fresh) => {
                            // 写入缓存（默认 30 天 TTL）— per reviewer I6：空结果也缓存，
                            // 否则偏门 query 反复浪费网络。靠 TTL 自然过期。per reviewer I1：
                            // 错误显式 log 而非吞掉，便于排查 SQLite 满 / DEK 错。
                            if let Ok(s) = self.store.lock() {
                                if let Err(e) = s.put_web_search_cached(
                                    dek,
                                    user_message,
                                    &fresh,
                                    crate::store::DEFAULT_WEB_SEARCH_TTL_SECS,
                                    now_secs,
                                ) {
                                    log::warn!(
                                        "C1: write cache failed (next call will refetch): {e}"
                                    );
                                }
                            }
                            Ok(fresh)
                        }
                        Err(e) => Err(e),
                    },
                };

                match web_results {
                    Ok(web) if !web.is_empty() => {
                        let synthetic: Vec<SearchResult> = web.into_iter().map(|r| SearchResult {
                            item_id: format!("web:{}", r.url),
                            score: 0.55,
                            title: r.title,
                            content: r.snippet.clone(),
                            source_type: "web".into(),
                            inject_content: Some(r.snippet),
                            breadcrumb: Vec::new(),         // F2: web 无源 item 路径
                            chunk_offset_start: None,
                            chunk_offset_end: None,
                        }).collect();
                        (synthetic, true)
                    }
                    _ => (local_knowledge, false),
                }
            } else {
                (local_knowledge, false)
            }
        } else {
            (local_knowledge, false)
        };

        // 3. 第一轮 LLM（per N3：直接拿 run_llm_once 返回的 confidence，不二次 parse）
        let (raw_response_1, confidence_1) =
            self.run_llm_once(user_message, history, &knowledge, web_search_used)?;

        // 4. J5.c：置信度 < 3 → 降阈值二次检索（per CRAG §3.2 ambiguous 分支）。
        // per reviewer S3：web 路径也允许 fallback 到本地 broader 召回，不无脑跳过 —
        // 否则用户拿到模糊答案没救济。本地 broader 即使 confidence_1 路径来自 web，
        // 重检拿到的本地结果若更相关，下次 LLM 调用 web_search_used 设回 false。
        let (final_response, final_confidence, secondary_used) = if confidence_1 < 3 {
            log::info!(
                "J5: confidence {} < 3, triggering secondary retrieval with lowered threshold (broader local recall)",
                confidence_1
            );
            // 阈值 0.65 → 0.55 扩大本地召回（始终走本地，不重跑 web）
            let pre_count = knowledge.len();
            let was_empty = pre_count == 0;
            match self.search_for_context(user_message, dek, 5, Some(0.55)) {
                Ok(broader) if broader.len() > knowledge.len() => {
                    // F1 (W3 batch A) 可观测性：区分"fallback 召回更多"vs"同样空"
                    log::info!(
                        "J5 F1: secondary retrieval result — local_was_empty={}, pre_count={}, broader_count={}",
                        was_empty, pre_count, broader.len()
                    );
                    knowledge = broader;
                    // 二次 LLM 调用：因为 broader 是本地结果，web_search_used 强制 false
                    match self.run_llm_once(user_message, history, &knowledge, false) {
                        Ok((response_2, confidence_2)) => (response_2, confidence_2, true),
                        Err(e) => {
                            // 二次 LLM 失败 → 保留第一次响应；secondary_used 仍 true 表示尝试过
                            log::warn!("J5: secondary LLM call failed: {e}, keeping first response");
                            (raw_response_1, confidence_1, true)
                        }
                    }
                }
                Ok(broader) => {
                    // F1: broader 没召回更多 → no-op 路径，记录但不重跑 LLM
                    log::info!(
                        "J5 F1: secondary retrieval no-op — local_was_empty={}, pre_count={}, broader_count={} (no improvement)",
                        was_empty, pre_count, broader.len()
                    );
                    (raw_response_1, confidence_1, false)
                }
                Err(e) => {
                    log::warn!("J5 F1: secondary retrieval search failed: {e}");
                    (raw_response_1, confidence_1, false)
                }
            }
        } else {
            (raw_response_1, confidence_1, false)
        };

        // 5. 剥离 confidence 标记后给用户
        let display_response = strip_confidence_marker(&final_response);

        // 6. 提取引用 — F2 (W3 batch A) 已透传 SearchResult.breadcrumb / offset 真值
        // per spec docs/superpowers/specs/2026-04-27-w3-batch-a-design.md §4
        // 关闭了 W2 batch 1 的 placeholder 状态。老 vault / web 无 sidecar 时优雅降级为空。
        let citations: Vec<Citation> = knowledge.iter().map(|k| Citation {
            item_id: k.item_id.clone(),
            title: k.title.clone(),
            relevance: k.score,
            chunk_offset_start: k.chunk_offset_start,
            chunk_offset_end: k.chunk_offset_end,
            breadcrumb: k.breadcrumb.clone(),
        }).collect();

        let knowledge_count = knowledge.len();

        // 7. 自动保存对话到知识库（保存剥离 confidence 后的版本）
        self.auto_save_conversation(user_message, &display_response, dek)?;

        Ok(ChatResponse {
            content: display_response,
            citations,
            knowledge_count,
            web_search_used,
            confidence: final_confidence,
            secondary_retrieval_used: secondary_used,
        })
    }

    /// 单次 LLM 调用，封装 prompt 构建 + 调用。返回 (raw response, confidence)。
    fn run_llm_once(
        &self,
        user_message: &str,
        history: &[ChatMessage],
        knowledge: &[SearchResult],
        web_search_used: bool,
    ) -> Result<(String, u8)> {
        let system = build_rag_system_prompt(knowledge, web_search_used);
        let mut messages = Vec::new();
        messages.push(ChatMessage::system(&system));
        messages.extend_from_slice(history);
        messages.push(ChatMessage::user(user_message));
        let response = self.llm.chat_with_history(&messages)?;
        let conf = parse_confidence(&response);
        Ok((response, conf))
    }

    fn search_for_context(
        &self,
        query: &str,
        dek: &Key32,
        top_k: usize,
        min_score_override: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        let ft_guard = self.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        let vec_guard = self.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let emb_guard = self.embedding.lock().unwrap_or_else(|e| e.into_inner());
        let reranker_guard = self.reranker.lock().unwrap_or_else(|e| e.into_inner());
        let store_guard = self.store.lock().unwrap_or_else(|e| e.into_inner());

        let ctx = crate::search::SearchContext {
            fulltext: ft_guard.as_ref(),
            vectors: vec_guard.as_ref(),
            embedding: emb_guard.clone(),
            reranker: reranker_guard.clone(),
            store: &store_guard,
            dek,
        };
        // chat 路径默认走 RAG 阈值（J3 0.65）；override 用于 J5.c 二次检索（0.55）
        let mut params = crate::search::SearchParams::with_defaults_for_rag(top_k);
        if let Some(threshold) = min_score_override {
            params.min_score = Some(threshold);
        }
        let mut results = crate::search::search_with_context(&ctx, query, &params)?;
        allocate_budget(&mut results, INJECTION_BUDGET);
        Ok(results)
    }

    fn auto_save_conversation(&self, user_msg: &str, assistant_msg: &str, dek: &Key32) -> Result<()> {
        let content = format!("用户: {}\n\n助手: {}", user_msg, assistant_msg);
        let title = user_msg.chars().take(50).collect::<String>();
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = store.insert_item(dek, &title, &content, None, "ai_chat", None, None);
        Ok(())
    }
}

/// J5.a 强约束 system prompt（per spec §J5.a + 吴师兄 §4 + Self-RAG 置信度 token 概念）。
/// 历史温和版本"如果有信息回答"已替换 — 那种允许 LLM 自由发挥的措辞会让模型补一句
/// "可能/大概/建议咨询客服"，与产品级精确答案需求冲突。
fn build_rag_system_prompt(knowledge: &[SearchResult], from_web: bool) -> String {
    // 即使 knowledge 为空也保留置信度自评要求，让前端能区分"无知识"vs"有但模糊"
    if knowledge.is_empty() {
        return "你是用户的个人知识助手。知识库中暂无与此问题相关的文档，网络搜索也未返回结果。\n\
                请凭借自身知识正常回答，不要编造引用。\n\
                回答末尾必须输出【置信度: N/5】（5=完全确定，1=高度不确定）。".into();
    }

    let (section_label, intro) = if from_web {
        (
            "=== 网络搜索结果（本地知识库无结果，自动补充）===",
            "你是用户的个人知识助手。本地知识库暂无相关内容，以下来自实时网络搜索。\n\n\
             【硬性规则】\n\
             1. 只用搜索结果中的信息，不要补充推理\n\
             2. 搜索结果不足以回答 → 回复\"网络搜索结果不足以确定答案\"\n\
             3. 禁用模糊措辞：\"可能\" \"大概\" \"建议咨询\" \"或许\" \"应该\"\n\
             4. 引用必带来源：「来源：[URL]」\n\
             5. 回答末尾必须输出【置信度: N/5】（5=完全确定，1=高度不确定）\n\n",
        )
    } else {
        (
            "=== 知识库相关文档 ===",
            "你是用户的个人知识助手。请严格基于以下文档回答用户问题。\n\n\
             【硬性规则】\n\
             1. 只用文档中的信息，不要补充推理\n\
             2. 文档无明确答案 → 回复\"知识库中暂无相关信息\"\n\
             3. 禁用模糊措辞：\"可能\" \"大概\" \"建议咨询\" \"或许\" \"应该\"\n\
             4. 引用必带来源：[文档标题 > 路径]（路径来自文档面包屑）\n\
             5. 回答末尾必须输出【置信度: N/5】（5=完全确定，1=高度不确定）\n\n",
        )
    };

    let mut prompt = intro.to_string();
    prompt.push_str(section_label);
    prompt.push_str("\n\n");
    for (i, item) in knowledge.iter().enumerate() {
        let content = item.inject_content.as_deref().unwrap_or(&item.content);
        if from_web {
            prompt.push_str(&format!(
                "[{}] 《{}》\nURL: {}\n{}\n\n",
                i + 1, item.title, item.item_id.trim_start_matches("web:"), content
            ));
        } else {
            prompt.push_str(&format!(
                "[{}] 《{}》(来源: {}, 相关度: {:.0}%)\n{}\n\n",
                i + 1, item.title, item.source_type,
                item.score * 100.0,
                content
            ));
        }
    }
    prompt.push_str("=== 参考内容结束 ===\n");
    prompt
}

// ── J5.b 置信度解析（per spec §J5.b + Self-RAG arXiv:2310.11511）─────────────
//
// LLM 末尾按 prompt 要求输出 【置信度: N/5】（中文）或 [Confidence: N/5]（英文 fallback）。
// 我们解析末尾片段，避免 N/5 在中段误识别（如 "5/5 stars"）。
//
// 设计取舍：用宽松正则 + 末尾 200 字搜索，而不是严格全文 strict parse —
// 本地小模型（qwen2.5:3b）格式遵循率不稳定，宽松匹配让 90%+ 的输出能解出置信度。

/// 从 LLM 响应中解析置信度（1-5）。缺失或非法时返回 3（中性默认值）。
///
/// 全文匹配（per reviewer S4：避免末尾 byte offset 与 UTF-8 字符边界冲突），
/// 取最后一个 marker（LLM 偶尔会在草稿中提到示例数字，最终结论在末尾）。
pub fn parse_confidence(response: &str) -> u8 {
    // 中文格式：【置信度: N/5】（容忍中英文冒号 + 可选空格）
    let zh_re = regex::Regex::new(r"【置信度[:：]\s*([1-5])\s*/\s*5】").ok();
    if let Some(re) = &zh_re {
        if let Some(cap) = re.captures_iter(response).last() {
            if let Some(n_str) = cap.get(1) {
                if let Ok(n) = n_str.as_str().parse::<u8>() {
                    return n;
                }
            }
        }
    }
    // 英文 fallback：[Confidence: N/5] 或 (Confidence: N/5)
    let en_re = regex::Regex::new(r"(?i)[\[\(]\s*confidence[:：]\s*([1-5])\s*/\s*5\s*[\]\)]").ok();
    if let Some(re) = &en_re {
        if let Some(cap) = re.captures_iter(response).last() {
            if let Some(n_str) = cap.get(1) {
                if let Ok(n) = n_str.as_str().parse::<u8>() {
                    return n;
                }
            }
        }
    }
    3 // 中性默认
}

/// 把 confidence marker 从用户最终看到的响应中剥离。
///
/// **WARNING**：删除 marker 之后到末尾的**所有**内容（防 LLM 在 marker 后续写无关
/// 描述）。当前 prompt 明确要求 "回答末尾必须输出【置信度: N/5】"，所以 marker 后
/// 续写视为噪音。如果未来 telemetry 显示 LLM 频繁在 marker 后输出有用信息（如附加
/// 章节标题 / 参考链接），需改为只删 marker 那一行而非到末尾。per reviewer I4。
///
/// 用 (?s) flag 让 `.` 匹配换行符（marker 后可能有换行+续文）。
pub fn strip_confidence_marker(response: &str) -> String {
    // per reviewer P1 #2：与 parse_confidence 对称，取**最后一个** marker 的位置开始删；
    // 如果 LLM 输出"草稿提到【置信度: 2/5】... 最终【置信度: 5/5】"，应保留草稿、
    // 仅从最终 marker 开始截。
    let zh_marker = regex::Regex::new(r"【置信度[:：]\s*[1-5]\s*/\s*5】").ok();
    if let Some(re) = &zh_marker {
        if let Some(m) = re.find_iter(response).last() {
            return response[..m.start()].trim_end().to_string();
        }
    }
    let en_marker = regex::Regex::new(r"(?i)[\[\(]\s*confidence[:：]\s*[1-5]\s*/\s*5\s*[\]\)]").ok();
    if let Some(re) = &en_marker {
        if let Some(m) = re.find_iter(response).last() {
            return response[..m.start()].trim_end().to_string();
        }
    }
    response.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;
    use crate::llm::MockLlmProvider;

    #[test]
    fn build_rag_prompt_empty_knowledge() {
        let prompt = build_rag_system_prompt(&[], false);
        assert!(prompt.contains("暂无"));
    }

    #[test]
    fn build_rag_prompt_with_knowledge() {
        let results = vec![SearchResult {
            item_id: "id1".into(),
            score: 0.85,
            title: "合同A".into(),
            content: "合同内容...".into(),
            source_type: "file".into(),
            inject_content: Some("合同内容...".into()),
            ..Default::default()
        }];
        let prompt = build_rag_system_prompt(&results, false);
        assert!(prompt.contains("合同A"));
        assert!(prompt.contains("85%"));
        assert!(prompt.contains("知识库"));
    }

    #[test]
    fn build_rag_prompt_from_web_uses_web_label() {
        let results = vec![SearchResult {
            item_id: "web:https://example.com".into(),
            score: 0.55,
            title: "Example Article".into(),
            content: "Some web content.".into(),
            source_type: "web".into(),
            inject_content: Some("Some web content.".into()),
            ..Default::default()
        }];
        let prompt = build_rag_system_prompt(&results, true);
        assert!(prompt.contains("网络搜索"));
        assert!(prompt.contains("Example Article"));
        assert!(!prompt.contains("相关度"));
    }

    #[test]
    fn citation_serializable() {
        let c = Citation {
            item_id: "a".into(),
            title: "T".into(),
            relevance: 0.9,
            chunk_offset_start: None,
            chunk_offset_end: None,
            breadcrumb: vec![],
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("relevance"));
    }

    // ── J5 + B1 tests（per spec §J5 §B1）──────────────────────────────

    #[test]
    fn strict_prompt_contains_anti_fabrication_rules() {
        let results = vec![SearchResult {
            item_id: "id1".into(),
            score: 0.85,
            title: "合同A".into(),
            content: "条款".into(),
            source_type: "file".into(),
            inject_content: Some("条款".into()),
            ..Default::default()
        }];
        let prompt = build_rag_system_prompt(&results, false);
        // 强约束的关键 marker
        assert!(prompt.contains("禁用模糊措辞"), "prompt 必须含禁用模糊措辞规则");
        assert!(prompt.contains("可能"), "应明确列出禁用词 '可能'");
        assert!(prompt.contains("【置信度: N/5】"), "必须要求置信度自评");
    }

    #[test]
    fn strict_prompt_web_path_also_has_confidence() {
        let results = vec![SearchResult {
            item_id: "web:https://x.com".into(),
            score: 0.55,
            title: "T".into(),
            content: "C".into(),
            source_type: "web".into(),
            inject_content: Some("C".into()),
            ..Default::default()
        }];
        let prompt = build_rag_system_prompt(&results, true);
        assert!(prompt.contains("【置信度: N/5】"), "web 路径也必须置信度");
        assert!(prompt.contains("禁用模糊措辞"), "web 路径也禁用模糊");
    }

    #[test]
    fn parse_confidence_zh_format() {
        assert_eq!(parse_confidence("回答内容。\n\n【置信度: 4/5】"), 4);
        assert_eq!(parse_confidence("内容【置信度：5/5】"), 5);
        // 全角冒号也支持
        assert_eq!(parse_confidence("【置信度： 1 / 5】"), 1);
    }

    #[test]
    fn parse_confidence_en_fallback() {
        assert_eq!(parse_confidence("answer text\n\n[Confidence: 3/5]"), 3);
        assert_eq!(parse_confidence("(confidence: 2/5)"), 2);
        // 大小写不敏感
        assert_eq!(parse_confidence("[CONFIDENCE: 4/5]"), 4);
    }

    #[test]
    fn parse_confidence_missing_defaults_to_3() {
        assert_eq!(parse_confidence("just an answer with no marker"), 3);
        assert_eq!(parse_confidence(""), 3);
    }

    #[test]
    fn parse_confidence_ignores_irrelevant_n_over_5_in_middle() {
        // "5/5 stars" 不应被识别成置信度
        assert_eq!(parse_confidence("This product has 5/5 stars rating."), 3);
    }

    #[test]
    fn strip_confidence_marker_removes_zh_tail() {
        let stripped = strip_confidence_marker("正确答案是 42。\n\n【置信度: 5/5】");
        assert_eq!(stripped, "正确答案是 42。");
    }

    #[test]
    fn strip_confidence_marker_removes_en_tail_and_after() {
        // marker 后续写也一并清除（防 LLM 在 marker 后再补一段）
        let stripped = strip_confidence_marker("Answer.\n\n[Confidence: 4/5]\n\nExtra rambling.");
        assert_eq!(stripped, "Answer.");
    }

    #[test]
    fn strip_confidence_marker_no_op_when_absent() {
        let original = "An answer without any marker.";
        assert_eq!(strip_confidence_marker(original), original);
    }

    #[test]
    fn citation_with_b1_fields_serializes() {
        let c = Citation {
            item_id: "src".into(),
            title: "Doc".into(),
            relevance: 0.8,
            chunk_offset_start: Some(100),
            chunk_offset_end: Some(250),
            breadcrumb: vec!["书".into(), "第一章".into()],
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"chunk_offset_start\":100"));
        assert!(json.contains("\"chunk_offset_end\":250"));
        assert!(json.contains("breadcrumb"));
        assert!(json.contains("第一章"));
    }

    #[test]
    fn chat_response_includes_confidence_and_secondary_flag() {
        // 仅结构 sanity check — 实际流程在集成测试覆盖
        let r = ChatResponse {
            content: "x".into(),
            citations: vec![],
            knowledge_count: 0,
            web_search_used: false,
            confidence: 4,
            secondary_retrieval_used: false,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"confidence\":4"));
        assert!(json.contains("\"secondary_retrieval_used\":false"));
    }

    #[test]
    fn chat_engine_with_empty_indices() {
        // ChatEngine with no fulltext/vector indices should still work
        let mock = Arc::new(MockLlmProvider::new("test"));
        mock.push_response("LLM回答");

        let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
        let fulltext: Arc<Mutex<Option<FulltextIndex>>> = Arc::new(Mutex::new(None));
        let vectors: Arc<Mutex<Option<VectorIndex>>> = Arc::new(Mutex::new(None));
        let embedding: Arc<Mutex<Option<Arc<dyn crate::embed::EmbeddingProvider>>>> =
            Arc::new(Mutex::new(None));

        let reranker: Arc<Mutex<Option<Arc<dyn crate::infer::RerankProvider>>>> =
            Arc::new(Mutex::new(None));
        let engine = ChatEngine::new(mock, store, fulltext, vectors, embedding, reranker);
        let dek = crypto::Key32::generate();
        let resp = engine.chat("你好", &[], &dek).unwrap();

        assert_eq!(resp.content, "LLM回答");
        assert_eq!(resp.knowledge_count, 0);
        assert!(!resp.web_search_used);
        assert!(resp.citations.is_empty());
    }
}
