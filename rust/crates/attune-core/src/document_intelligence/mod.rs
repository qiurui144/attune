//! Document Intelligence — OSS attune general-purpose document capabilities.
//!
//! Three features (per spec `docs/superpowers/specs/2026-06-06-oss-document-intelligence.md`):
//!   1. document comparison (`compare`)
//!   2. token-thrift deep summary (`deep_summary`) — flagship
//!   3. chapter-by-chapter reading (`chapters`)
//!
//! All share REUSED attune-core primitives (no fork): `chunker` (section split),
//! `context_compress` (token estimate + chunk_hash), `store::chunk_summaries` (cache reuse),
//! `llm`, `vlm`, `cost`, `usage`, `crypto`.
//!
//! ## Retrieval scope — chapter-local only (NOT knowledge-base RAG)
//!
//! These features operate on the SINGLE document passed in. Retrieval is **document-local**:
//! `compare`/`deep_summary`/`chapters` split the doc with `chunker` and (for chapters `ask`)
//! inject *prior chapters' cached summaries* as cross-chapter memory. They do **not** call the
//! `search` engine (`usearch`/`tantivy` over the vault) — there is no knowledge-base-level RAG
//! that pulls in OTHER documents. Knowledge-base RAG across the vault is a v.next item (would
//! wire `search::search_with_context` into chapters `ask` / deep_summary context). Do not read
//! the per-feature doc comments' word "RAG" as vault-wide retrieval — it means doc-local context.
//!
//! Cost discipline (CLAUDE.md §Cost&Trigger Contract): the only tier-3 LLM stages are
//! map/reduce/semantic-verdict/Q&A — those are member-gated at the route layer (not in this
//! module) and the routing DECISION is client-side (see [`model_routing`]).

pub mod chapters; // T-05 — chapter-by-chapter reading (review output-mode contract)
pub mod compare; // T-04 — document comparison (marked output-mode contract)
pub mod deep_summary; // T-02 — flagship token-thrift pipeline
pub mod extractive; // T-03 — local zero-LLM pre-cut
pub mod model_routing; // T-01 — per-stage vetted-model selection
pub mod token_bill; // T-06 — bill struct + savings computation
pub mod vlm_extract; // T-09 — scanned/image source → VLM text extraction
