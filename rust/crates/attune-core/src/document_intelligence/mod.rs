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

pub mod model_routing;

// B1 sibling modules (T-03 / T-06) and later batches register here on merge:
//   pub mod extractive;   // T-03
//   pub mod token_bill;   // T-06
//   pub mod deep_summary;  // T-02
//   pub mod compare;       // T-04
//   pub mod chapters;      // T-05
