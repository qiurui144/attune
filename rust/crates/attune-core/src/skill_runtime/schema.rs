//! Skill declaration schema (spec §3.3 / §5.3) — the YAML contract for a declarative
//! multi-step skill, a **superset** of [`crate::workflow::Workflow`].
//!
//! A skill is the orchestration layer that turns attune from a *knowledge tool* into a
//! *deliverable generator*: `retrieve → agent → synthesize → render → export → download`.
//! It is **tier-💰 (multi-step LLM)** and therefore **always user-triggered** — the parser
//! rejects any `cost_tier: llm_multi_step` skill that declares a non-`manual` trigger so a
//! paid skill can never fire as a background job (CLAUDE.md §成本契约 / spec §7 + R3).
//!
//! Backward compatibility (spec §10): the new [`SkillStepKind`] variants are an *additive*
//! `#[serde(tag = "type")]` enum; an old `workflow.yaml` (only `skill`/`deterministic` steps)
//! still parses through [`crate::workflow::parse_workflow_yaml`]. This module owns the
//! richer `skill` top-level kind with typed `inputs` + `cost_tier` + the rag/agent/
//! synthesize/render/export step types.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A declarative skill: a typed, validated, cost-aware step graph over existing agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Stable skill id (kebab); used in the run/estimate REST path.
    pub id: String,
    /// Must be `"skill"` (the new top-level kind; `"workflow"` stays on the legacy engine).
    #[serde(rename = "type")]
    pub kind: String,
    /// Semantic version of the skill declaration.
    #[serde(default = "default_version")]
    pub version: String,
    /// Human title (UI card).
    #[serde(default)]
    pub title: String,
    /// One-line description (UI card).
    #[serde(default)]
    pub description: String,
    /// Cost tier — drives the trigger guard + estimate UI.
    #[serde(default)]
    pub cost_tier: CostTier,
    /// When/where it may fire. A `LlmMultiStep` skill MUST be `manual` (validated).
    pub trigger: SkillTrigger,
    /// Typed user inputs, validated before any step runs (spec §7 `input-invalid`).
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    /// The ordered step chain.
    pub steps: Vec<SkillStep>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

/// Cost tier of a skill (spec §3.3 / §8). Default is the cheapest so an unspecified skill is
/// never silently treated as paid; real skills declare `llm_multi_step` explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostTier {
    /// Pure CPU, no LLM (rare for a skill; allowed for deterministic-only skills).
    #[default]
    Free,
    /// One local-accelerator step (embedding/classify); not a paid cloud call.
    Local,
    /// A paid, multi-step LLM skill — the common case. MUST be user-triggered.
    LlmMultiStep,
}

impl CostTier {
    /// Parse a tier from a lowercase string.
    pub fn as_str(self) -> &'static str {
        match self {
            CostTier::Free => "free",
            CostTier::Local => "local",
            CostTier::LlmMultiStep => "llm_multi_step",
        }
    }
}

/// Trigger declaration. `on` is `manual` | `file_added`; `scope` is `project` | `global`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTrigger {
    pub on: String,
    pub scope: String,
}

/// A typed user input. Validated against the run payload before any step executes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: InputType,
    #[serde(default)]
    pub required: bool,
}

/// The value types a skill input may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    /// A KB item id (string).
    ItemId,
    /// A free-form string.
    String,
    /// A list of strings (e.g. the two entities to compare).
    StringList,
}

/// One step of a skill. `#[serde(tag = "type")]` — additive over the legacy workflow enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillStep {
    /// Retrieve KB material (decrypt item ids / RAG query) → text bundle.
    Rag(RagStep),
    /// Invoke an existing agent by capability id (compare / extract / draft / ...).
    Agent(AgentStep),
    /// Combine prior outputs into prose (writing::synthesize). (Declared; reserved for
    /// document skills — `compare_to_table` does not use it.)
    Synthesize(SynthesizeStep),
    /// Build the typed [`crate::export::Artifact`] IR from prior outputs.
    Render(RenderStep),
    /// Terminal: render the Artifact to a downloadable file → artifact_id.
    Export(ExportStep),
}

impl SkillStep {
    /// The step id (for error attribution + the estimate breakdown).
    pub fn id(&self) -> &str {
        match self {
            SkillStep::Rag(s) => &s.id,
            SkillStep::Agent(s) => &s.id,
            SkillStep::Synthesize(s) => &s.id,
            SkillStep::Render(s) => &s.id,
            SkillStep::Export(s) => &s.id,
        }
    }

    /// Whether this step is a paid LLM step (drives the estimate tier breakdown).
    pub fn is_llm(&self) -> bool {
        matches!(self, SkillStep::Agent(_) | SkillStep::Synthesize(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagStep {
    pub id: String,
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStep {
    pub id: String,
    /// Capability id of the agent to invoke, e.g. `document_intelligence.compare_to_table`.
    pub agent: String,
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesizeStep {
    pub id: String,
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderStep {
    pub id: String,
    /// `table` | `document`.
    pub as_kind: RenderAs,
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderAs {
    Table,
    Document,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportStep {
    pub id: String,
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    pub output: String,
}

/// Parse + validate a skill YAML string (mirrors `parse_workflow_yaml`, spec §5.3).
///
/// Validation beyond serde shape:
/// - `kind == "skill"`,
/// - at least one step,
/// - a terminal `export` step exists (a skill must produce a downloadable artifact),
/// - **cost guard**: a `LlmMultiStep` skill must declare `trigger.on == "manual"` (spec §7
///   — a paid skill can never fire as a background job).
pub fn parse_skill_yaml(yaml: &str) -> Result<Skill, String> {
    let skill: Skill =
        serde_yaml::from_str(yaml).map_err(|e| format!("parse skill yaml: {e}"))?;
    validate_skill(&skill)?;
    Ok(skill)
}

/// Validate an already-deserialized [`Skill`] (shared by the parser + the registry).
pub fn validate_skill(skill: &Skill) -> Result<(), String> {
    if skill.kind != "skill" {
        return Err(format!("skill `type` must be \"skill\", got {:?}", skill.kind));
    }
    if skill.steps.is_empty() {
        return Err("skill has no steps".to_string());
    }
    let has_export = skill
        .steps
        .iter()
        .any(|s| matches!(s, SkillStep::Export(_)));
    if !has_export {
        return Err("skill has no terminal `export` step (must produce a downloadable artifact)".to_string());
    }
    // Cost contract guard (spec §7 / R3): a paid multi-step skill can never be a background job.
    if skill.cost_tier == CostTier::LlmMultiStep && skill.trigger.on != "manual" {
        return Err(format!(
            "cost_tier=llm_multi_step skill must be trigger.on=manual (got {:?}) — \
             a paid skill must be user-triggered, never a background job",
            skill.trigger.on
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPARE_SKILL_YAML: &str = r#"
id: compare-to-table
type: skill
version: "1.0.0"
title: 设备参数比对成表
description: 从文档中抽取两个设备的参数并生成可下载的差异表格
cost_tier: llm_multi_step
trigger: { on: manual, scope: project }
inputs:
  - { name: doc, type: item_id, required: true }
  - { name: entities, type: string_list, required: true }
steps:
  - type: rag
    id: load
    input: { item_ids: ["${doc}"] }
    output: docs
  - type: agent
    id: extract
    agent: document_intelligence.compare_to_table
    input: { text: "${docs}", entity_a: "${entities.0}", entity_b: "${entities.1}" }
    output: diff
  - type: render
    id: build
    as_kind: table
    input: { from: "${diff}", title: "设备参数比对" }
    output: artifact
  - type: export
    id: out
    input: { artifact: "${artifact}", format: xlsx }
    output: file
"#;

    #[test]
    fn parse_compare_skill() {
        let skill = parse_skill_yaml(COMPARE_SKILL_YAML).expect("parse");
        assert_eq!(skill.id, "compare-to-table");
        assert_eq!(skill.kind, "skill");
        assert_eq!(skill.cost_tier, CostTier::LlmMultiStep);
        assert_eq!(skill.inputs.len(), 2);
        assert_eq!(skill.steps.len(), 4);
        assert!(matches!(&skill.steps[0], SkillStep::Rag(_)));
        assert!(matches!(&skill.steps[1], SkillStep::Agent(a) if a.agent == "document_intelligence.compare_to_table"));
        assert!(matches!(&skill.steps[3], SkillStep::Export(_)));
    }

    #[test]
    fn reject_non_skill_kind() {
        let bad = COMPARE_SKILL_YAML.replace("type: skill", "type: workflow");
        // The first `type: skill` is the top-level kind; replacing it makes kind=workflow.
        let err = parse_skill_yaml(&bad).unwrap_err();
        assert!(err.contains("must be \"skill\"") || err.contains("parse skill yaml"), "got: {err}");
    }

    #[test]
    fn reject_missing_export_step() {
        let yaml = r#"
id: no-export
type: skill
cost_tier: llm_multi_step
trigger: { on: manual, scope: project }
steps:
  - type: rag
    id: load
    input: {}
    output: docs
"#;
        let err = parse_skill_yaml(yaml).unwrap_err();
        assert!(err.contains("no terminal `export`"), "got: {err}");
    }

    #[test]
    fn reject_paid_skill_with_background_trigger() {
        // The §7 cost-contract guard: a paid skill firing on file_added would be a background
        // 💰 偷跑 — must be rejected at parse time.
        let yaml = COMPARE_SKILL_YAML.replace("on: manual", "on: file_added");
        let err = parse_skill_yaml(&yaml).unwrap_err();
        assert!(err.contains("must be trigger.on=manual"), "got: {err}");
    }

    #[test]
    fn empty_steps_rejected() {
        let yaml = r#"
id: empty
type: skill
trigger: { on: manual, scope: project }
steps: []
"#;
        let err = parse_skill_yaml(yaml).unwrap_err();
        assert!(err.contains("no steps") || err.contains("no terminal"), "got: {err}");
    }

    #[test]
    fn malformed_yaml_errors() {
        assert!(parse_skill_yaml("not: [valid: yaml::").is_err());
    }
}
