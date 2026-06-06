//! Document Intelligence — OSS attune general-purpose document capabilities.
//!
//! Three features (per spec `docs/superpowers/specs/2026-06-06-oss-document-intelligence.md`):
//!   1. document comparison (`compare`)
//!   2. token-thrift deep summary (`deep_summary`) — flagship
//!   3. chapter-by-chapter reading (`chapters`)
//!
//! All share REUSED attune-core primitives (no fork): `chunker`, `context_compress`,
//! `context_budget`, `search`, `store::chunk_summaries`, `llm`, `vlm`, `member_session`,
//! `cost`, `usage`.
//!
//! Cost discipline (CLAUDE.md §Cost&Trigger Contract): the only tier-3 LLM stages are
//! map/reduce/semantic-verdict/Q&A — those are member-gated at the route layer and the
//! routing DECISION is client-side (see [`model_routing`]).

pub mod deep_summary; // T-02 — flagship token-thrift pipeline
pub mod extractive; // T-03 — local zero-LLM pre-cut
pub mod model_routing; // T-01 — per-stage vetted-model selection
pub mod token_bill; // T-06 — bill struct + savings computation
pub mod vlm_extract; // T-09 — scanned/image source → VLM text extraction

// Later batches register here on merge:
//   pub mod compare;       // T-04
//   pub mod chapters;      // T-05
