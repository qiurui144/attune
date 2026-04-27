pub mod ai_annotator;
pub mod annotation_weight;
// chat 模块整体 pub(crate) — ChatEngine 只能内部构造（依赖 Vault/Store internal types）。
// 外部消费者（attune-server route）通过本 crate re-export 拿到 Citation / ChatResponse /
// parse_confidence / strip_confidence_marker 这些公开 API（per reviewer I3）。
pub(crate) mod chat;
pub use chat::{parse_confidence, strip_confidence_marker, Citation, ChatEngine, ChatResponse};
pub mod chunker;
pub mod context_compress;
pub mod plugin_loader;
pub mod plugin_registry;
pub(crate) mod plugin_sig;
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
pub mod platform;
pub mod memory_consolidation;
pub mod project_recommender;
pub(crate) mod queue;
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
pub mod web_search;
pub mod web_search_browser;
pub(crate) mod web_search_engines;
pub mod workflow;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
