pub mod chat;
pub mod chunker;
pub mod classifier;
pub mod clusterer;
pub mod crypto;
pub mod embed;
pub mod infer;
pub mod error;
pub mod index;
pub mod llm;
pub mod parser;
pub mod platform;
pub mod queue;
pub mod scanner;
pub mod scanner_patent;
pub mod scanner_webdav;
pub mod search;
pub mod store;
pub mod tag_index;
pub mod taxonomy;
pub mod vault;
pub mod vectors;
pub mod web_search;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
