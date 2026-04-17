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
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Citation {
    pub item_id: String,
    pub title: String,
    pub relevance: f32,
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
    pub fn chat(
        &self,
        user_message: &str,
        history: &[ChatMessage],
        dek: &Key32,
    ) -> Result<ChatResponse> {
        // 1. 搜索本地知识库
        let local_knowledge = self.search_for_context(user_message, dek, 5)?;

        // 2. 若本地无结果，尝试网络搜索 fallback
        let (knowledge, web_search_used) = if local_knowledge.is_empty() {
            if let Some(ws) = &self.web_search {
                match ws.search(user_message, 3) {
                    Ok(web_results) if !web_results.is_empty() => {
                        let synthetic: Vec<SearchResult> = web_results.into_iter().map(|r| SearchResult {
                            item_id: format!("web:{}", r.url),
                            score: 0.55,
                            title: r.title,
                            content: r.snippet.clone(),
                            source_type: "web".into(),
                            inject_content: Some(r.snippet),
                        }).collect();
                        (synthetic, true)
                    }
                    // 网络搜索失败或无结果时降级：继续用空知识库（不报错）
                    Ok(_) | Err(_) => (local_knowledge, false),
                }
            } else {
                (local_knowledge, false)
            }
        } else {
            (local_knowledge, false)
        };

        // 3. 构建 system prompt
        let system = build_rag_system_prompt(&knowledge, web_search_used);

        // 4. 组装完整消息列表
        let mut messages = Vec::new();
        messages.push(ChatMessage::system(&system));
        messages.extend_from_slice(history);
        messages.push(ChatMessage::user(user_message));

        // 5. 调用 LLM
        let response = self.llm.chat_with_history(&messages)?;

        // 6. 提取引用
        let citations: Vec<Citation> = knowledge.iter().map(|k| Citation {
            item_id: k.item_id.clone(),
            title: k.title.clone(),
            relevance: k.score,
        }).collect();

        let knowledge_count = knowledge.len();

        // 7. 自动保存对话到知识库
        self.auto_save_conversation(user_message, &response, dek)?;

        Ok(ChatResponse { content: response, citations, knowledge_count, web_search_used })
    }

    fn search_for_context(&self, query: &str, dek: &Key32, top_k: usize) -> Result<Vec<SearchResult>> {
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
        let params = crate::search::SearchParams::with_defaults(top_k);
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

fn build_rag_system_prompt(knowledge: &[SearchResult], from_web: bool) -> String {
    if knowledge.is_empty() {
        return "你是用户的个人知识助手。知识库中暂无与此问题相关的文档，网络搜索也未返回结果。\
                请凭借自身知识正常回答，不要编造引用。".into();
    }

    let (section_label, intro) = if from_web {
        (
            "=== 网络搜索结果（本地知识库无结果，自动补充）===",
            "你是用户的个人知识助手。本地知识库暂无相关内容，以下来自实时网络搜索。\n\
             请基于这些搜索结果回答用户的问题，并在回答末尾标注「来源：[URL]」。\n\
             如果搜索结果不够可靠，请明确说明并补充你自己的判断。\n\n",
        )
    } else {
        (
            "=== 知识库相关文档 ===",
            "你是用户的个人知识助手。以下是从用户本地知识库中检索到的相关文档。\n\
             请基于这些知识回答用户的问题。如果引用了某个文档，请标注 [文档标题]。\n\
             如果知识库中没有相关信息，正常回答即可，不要编造引用。\n\n",
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
        }];
        let prompt = build_rag_system_prompt(&results, true);
        assert!(prompt.contains("网络搜索"));
        assert!(prompt.contains("Example Article"));
        assert!(!prompt.contains("相关度"));
    }

    #[test]
    fn citation_serializable() {
        let c = Citation { item_id: "a".into(), title: "T".into(), relevance: 0.9 };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("relevance"));
    }

    #[test]
    fn chat_engine_with_empty_indices() {
        // ChatEngine with no fulltext/vector indices should still work
        let mock = Arc::new(MockLlmProvider::new("test"));
        mock.push_response("LLM回答".into());

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
