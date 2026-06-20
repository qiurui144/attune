//! Skills orchestration runtime (spec §3.1 / §4.1) — turns attune from a *knowledge tool* into
//! a *deliverable generator*. A declarative YAML **skill** chains existing agents as primitives
//! (`retrieve → agent → render → export → download`) and returns a downloadable artifact + a
//! token bill, under the cost contract (CLAUDE.md §成本契约): 💰 multi-step, **user-triggered**,
//! with a pre-run estimate and a per-skill token hard cap.
//!
//! This module **extends** the `workflow` engine (spec B.3) rather than the SkillClaw `skills/`
//! query-expander or `skill_evolution` (those are not orchestration). It is named
//! `skill_runtime` to avoid that namespace collision.
//!
//! Layout:
//! - [`schema`] — the skill YAML contract (superset of `workflow::Workflow`) + the cost-trigger guard.
//! - [`runner`] — the [`runner::SkillRunner`]-equivalent `run_skill` step executor + [`runner::RagResolver`].
//! - [`compare_to_table`] — the OSS `compare_to_table` agent (用户例 3; §4.5 LLM hardening).
//! - [`render`] — map a step output into the export `Artifact` IR (zero-cost).
//! - [`cost`] — zero-LLM estimate + the [`cost::MAX_TOTAL_TOKENS`] hard cap.
//! - [`registry`] — built-in OSS skills + pro plugin registration extension point.

pub mod compare_to_table;
pub mod cost;
pub mod doc_render;
pub mod reference_generate;
pub mod registry;
pub mod render;
pub mod research_synthesis;
pub mod runner;
pub mod schema;

pub use compare_to_table::{compare_to_table, ParamComparison, ParamRow};
pub use cost::{estimate, SkillEstimate, StepEstimate, MAX_TOTAL_TOKENS};
pub use doc_render::{sections_to_document, writing_to_document, UNVERIFIED_MARKER};
pub use reference_generate::{reference_generate, ReferenceDoc};
pub use registry::{RegisteredSkill, SkillRegistry};
pub use render::comparison_to_table;
pub use research_synthesis::{parse_structure, research_synthesis};
pub use runner::{
    run_skill, validate_inputs, MapResolver, RagResolver, SkillError, SkillRunResult,
};
pub use schema::{
    parse_skill_yaml, validate_skill, CostTier, InputSpec, InputType, RenderAs, Skill, SkillStep,
    SkillTrigger,
};
