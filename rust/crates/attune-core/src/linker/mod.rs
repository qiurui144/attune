//! Internal Knowledge-Base Linker (agent).
//!
//! Activates the previously-dead [`crate::entity_graph`] by feeding it from the
//! persisted `item_entities` inverted index this module owns. Computes 3 link
//! kinds at ingest time, **all strictly 🆓/⚡ cost-tier** (no LLM call):
//!
//! - [`LinkKind::SharedEntity`] — two items co-mention the same person / money /
//!   date / organization (extracted by [`crate::entities::extract_entities`]).
//! - [`LinkKind::SemanticNear`] — two items have chunk vectors above a cosine
//!   threshold (re-uses already-produced embeddings via
//!   [`crate::vectors::VectorIndex`]; zero new embedding calls).
//! - [`LinkKind::ExplicitRef`] — one item's content literally references another
//!   item by URL match, `[[wiki link]]`, or verbatim title substring of ≥ N
//!   characters.
//!
//! ## Design contract (matches spec
//! `docs/superpowers/specs/2026-05-19-internal-knowledge-linking-design.md`)
//!
//! - **No LLM call on this path.** [`compute_links_for_item`] does not accept an
//!   `LlmProvider`; any link explanation is a separate, user-triggered route.
//! - **No verdict, no overwrite.** Only emits `item_links` rows; never touches
//!   item content / tags / annotations.
//! - **Idempotent rebuild.** `relink_item` clears this item's existing links
//!   before inserting the freshly computed set (same "清旧 → 加新" philosophy
//!   as [`crate::reindex::reindex_item`]).
//! - **Symmetric storage with canonical ordering.** Symmetric kinds store one
//!   row with `item_a < item_b` lexicographic; directed kinds (`ExplicitRef`)
//!   set `directed = 1` with `(a = citing, b = cited)`.
//! - **Caps + DF filter mandatory.** Out-degree cap (top-K by weight per item)
//!   and stop-entity filter (entity in > 30 % of items) are not tuning knobs —
//!   without them, ubiquitous entities (the user's own name) make the graph
//!   near-complete.
//!
//! ## Cost contract reminder
//!
//! Per `CLAUDE.md` § "成本感知与触发契约": the linker runs in the build /
//! ⚡ tier. It must never call into LLMs — link kinds are *mechanical*
//! identifiers (`shared_entity` / `semantic_near` / `explicit_ref`), exactly
//! like `law-pro::evidence_chain` emits mechanical `Corroborates` / `SameParty`
//! with no verdict.

pub mod agent;

pub use agent::{
    compute_links_for_item, purge_links_for_item, ComputedLink, LinkKind,
    LinkThresholds, LinkerStats,
};
