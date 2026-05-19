//! Multi-layer memory — tiered memory architecture for token-efficient context.
//!
//! See `docs/superpowers/plans/2026-05-18-multilayer-memory.md`.
//!
//! Layers:
//!   - L0 raw chunks            — `items.content` + vectors + FTS (unchanged)
//!   - L1 chunk summaries       — `chunk_summaries` (unchanged)
//!   - L2 episodic memory       — `memories(kind='episodic')`, 6h day-window worker
//!   - L3 semantic memory (NEW) — `memories(kind='semantic')`, topic-clustered
//!
//! This module wires L2/L3 into the retrieval/assembly path: [`retrieval`] makes
//! them vector-searchable, [`semantic`] builds L3 from L2, [`assembler`] picks the
//! right tier per query so recall/overview questions answer from compact summaries
//! instead of paying raw-chunk token cost.

pub mod assembler;
pub mod retrieval;
pub mod semantic;

pub use assembler::{
    assemble_context, classify_query_shape, compact_history, AssembledContext, ContextBlock,
    MemoryConfig, QueryShape,
};
pub use retrieval::{search_memories, MemoryHit, MemoryVectorIndex};
pub use semantic::{
    apply_semantic_result, generate_one_semantic_memory, prepare_semantic_cycle, SemanticCluster,
    MAX_TOPICS_PER_CYCLE, MIN_MEMS_PER_TOPIC,
};
