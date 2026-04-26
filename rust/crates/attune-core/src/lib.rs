pub mod ai_annotator;
pub mod annotation_weight;
pub(crate) mod chat;
pub mod chunker;
pub mod context_compress;
pub(crate) mod plugin_loader;
pub(crate) mod plugin_sig;
pub mod classifier;
pub mod clusterer;
pub mod crypto;
pub mod embed;
pub mod entities;
pub mod infer;
pub mod error;
pub mod index;
pub mod llm;
pub(crate) mod ocr;
pub mod parser;
pub mod platform;
pub mod project_recommender;
pub(crate) mod queue;
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

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
