//! # attune-core
//!
//! attune 私有 AI 知识伙伴 — 核心库。
//!
//! 提供 vault (Argon2id + AES-256-GCM 加密存储) / store (SQLite + FTS5 全文索引) /
//! vectors (usearch HNSW + f16) / chunker / parser / embed / chat (含 PII redactor +
//! evidence chain) / classifier / clusterer / web search 等 ~50 个模块。
//!
//! ## 主要 facade
//!
//! - [`vault::Vault`] — 加密存储入口, unlock 后暴露 [`store::Store`]
//! - [`search`] — RRF 混合搜索 (vector + FTS) + 两阶段层级 + 注入预算
//! - [`chunker`] — 滑动窗口 + 章节切割, code fence balanced
//! - [`chat::ChatEngine`] — RAG 对话引擎, 含 PII redact + cite extraction
//! - [`embed::EmbeddingProvider`] — Ollama / ONNX 抽象 trait
//! - [`classifier::Classifier`] + [`taxonomy::Taxonomy`] — 自动分类
//! - [`async_fs`] — spawn_blocking 包装 fs ops, async handler 友好
//!
//! ## 加密策略
//!
//! - 主密码 → Argon2id 派生 KEK → 解 DEK (随机 32B)
//! - 字段级 AES-256-GCM (items.content / tags / embed_queue.chunk_text)
//! - title/url/created_at TEXT 明文 (per privacy trade-off, list 性能优先)
//!
//! ## 三档隐私 (per Phase A.5)
//!
//! - L0 🔒 chunk 永不出网 (强制本地 LLM)
//! - L1 默认 12 PII 类脱敏 → 云端 LLM
//! - L3 LLM 语义脱敏 (v0.7, K3 一体机)

pub mod ai_annotator;
pub mod annotation_weight;
// async_fs: D3 review 引入 — async-safe fs helpers (spawn_blocking 包装).
// 新代码默认走 async_fs::*, 防止 future async handler 误调用 sync std::fs.
pub mod async_fs;
// chat 模块整体 pub(crate) — ChatEngine 只能内部构造（依赖 Vault/Store internal types）。
// 外部消费者（attune-server route）通过本 crate re-export 拿到 Citation / ChatResponse /
// parse_confidence / strip_confidence_marker 这些公开 API（per reviewer I3）。
pub(crate) mod chat;
pub use chat::{parse_confidence, strip_confidence_marker, Citation, ChatEngine, ChatResponse};
pub mod chunker;
pub mod context_compress;
pub mod plugin_hub;
pub mod plugin_loader;
pub mod plugin_registry;
pub mod capability_dispatch;
pub mod skills;
pub mod agents;
pub mod mcp_client;
pub mod case_metadata;
pub mod plugin_encryption;
pub mod device_binding;
pub mod accounts_client;
pub mod ui_runtime;
pub mod agent_runner;
pub mod license;
pub mod license_cache;
pub mod member_session;
pub mod cloud_client;
pub mod plugin_sync;
pub mod plugin_sig;
pub mod classifier;
pub mod clusterer;
pub mod crypto;
pub mod embed;
pub mod entities;
pub mod infer;
pub mod error;
pub mod index;
pub mod intent_router;
pub mod llm;
pub mod ocr;  // v0.6.0-rc.3: pub for ai_stack status API
pub mod asr;
pub mod parser;
pub mod pii;
pub mod platform;
pub mod memory_consolidation;
pub mod project_recommender;
pub mod queue;
pub mod reindex;
pub mod resource_governor;
pub mod scanner;
pub mod scanner_patent;
pub mod scanner_webdav;
pub mod search;
pub mod store;
pub mod tag_index;
pub mod taxonomy;
pub mod vault;
pub mod vectors;
pub mod skill_evolution;

// v0.7 sprint feature modules
pub mod cost;
pub mod tools;
pub mod demo;
pub mod query_rewrite;
pub mod entity_graph;
pub mod skill_eval;
pub mod report;
pub mod reader;
pub mod web_search;
pub mod web_search_browser;
pub(crate) mod web_search_engines;
pub mod workflow;
pub mod capture;
pub mod sync;
pub mod vlm;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
