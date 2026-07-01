//! Skill cost estimation + the skill-level token hard cap (spec §8 / R3).
//!
//! Estimation is **🆓 zero-LLM** (spec §8 `/estimate`): it derives a token budget from the
//! input size × per-step heuristics, so the UI can show a `~预估 NK tok · $0.00X · ~Ns` chip
//! *before* the user confirms a paid run. The hard cap (`MAX_TOTAL_TOKENS`, default 50K)
//! prevents orchestration cost blow-up (spec R3): the runner aborts mid-skill if the running
//! bill exceeds it.

use crate::skill_runtime::schema::{Skill, SkillStep};
use serde::Serialize;

/// Skill-level token hard cap (spec §8 上限护栏 / R3). A skill whose running bill exceeds this
/// aborts with `partial-failure` rather than running away with cost.
pub const MAX_TOTAL_TOKENS: u32 = 50_000;

/// Per-LLM-step output-token allowance used by the static estimate (extraction/synthesis emit a
/// bounded structured object, not a long essay).
const EST_OUTPUT_TOKENS_PER_LLM_STEP: u32 = 800;

/// A per-step line in the estimate breakdown (spec §5.2 `steps: [{id, tier}]`).
#[derive(Debug, Clone, Serialize)]
pub struct StepEstimate {
    pub id: String,
    /// `free` (rag/render/export) or `llm_multi_step` (agent/synthesize).
    pub tier: String,
    pub est_tokens: u32,
}

/// The static estimate returned by `POST /skills/{id}/estimate` (spec §5.2).
#[derive(Debug, Clone, Serialize)]
pub struct SkillEstimate {
    pub est_tokens: u32,
    pub est_usd: f64,
    pub est_seconds: u32,
    pub steps: Vec<StepEstimate>,
    /// True if the static estimate already exceeds [`MAX_TOTAL_TOKENS`] — the UI should warn
    /// the user to narrow the input before running.
    pub over_cap: bool,
}

/// Estimate a skill's cost given the **total input character count** of its retrieved material.
///
/// `input_chars` is the size of the text the LLM steps will see (the RAG bundle). Each LLM step
/// is charged `~input_tokens + EST_OUTPUT_TOKENS_PER_LLM_STEP`; free steps are charged 0.
/// `model` is the logical model used for USD pricing.
pub fn estimate(skill: &Skill, input_chars: usize, model: &str) -> SkillEstimate {
    // ~1 token per 3 chars (CJK-aware rough heuristic; matches context_compress::estimate_tokens
    // order of magnitude without importing the heavier tokenizer for a pre-run estimate).
    let input_tokens = (input_chars / 3).min(u32::MAX as usize) as u32;

    let mut steps = Vec::with_capacity(skill.steps.len());
    let mut total: u32 = 0;
    for step in &skill.steps {
        let (tier, est) = if step.is_llm() {
            let est = input_tokens.saturating_add(EST_OUTPUT_TOKENS_PER_LLM_STEP);
            ("llm_multi_step", est)
        } else {
            ("free", 0)
        };
        total = total.saturating_add(est);
        steps.push(StepEstimate {
            id: step_id(step).to_string(),
            tier: tier.to_string(),
            est_tokens: est,
        });
    }

    // USD via existing per-model pricing; treat est tokens as half-in/half-out for a rough USD.
    let est_usd = crate::cost::estimate_cost_usd((total / 2) as usize, (total / 2) as usize, model)
        .unwrap_or(0.0);

    // ~1s per LLM step + a flat 1s for render/export.
    let llm_steps = skill.steps.iter().filter(|s| s.is_llm()).count() as u32;
    let est_seconds = llm_steps.saturating_add(1);

    SkillEstimate {
        est_tokens: total,
        est_usd,
        est_seconds,
        steps,
        over_cap: total > MAX_TOTAL_TOKENS,
    }
}

fn step_id(step: &SkillStep) -> &str {
    step.id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_runtime::schema::parse_skill_yaml;

    const YAML: &str = r#"
id: compare-to-table
type: skill
cost_tier: llm_multi_step
trigger: { on: manual, scope: project }
steps:
  - type: rag
    id: load
    input: {}
    output: docs
  - type: agent
    id: extract
    agent: document_intelligence.compare_to_table
    input: {}
    output: diff
  - type: render
    id: build
    as_kind: table
    input: {}
    output: artifact
  - type: export
    id: out
    input: {}
    output: file
"#;

    #[test]
    fn free_steps_cost_zero_llm_steps_charged() {
        let skill = parse_skill_yaml(YAML).unwrap();
        let est = estimate(&skill, 3000, "gpt-4o-mini"); // 3000 chars → ~1000 input tokens
                                                         // rag/render/export are free; only `extract` (agent) is charged.
        let charged: Vec<_> = est.steps.iter().filter(|s| s.est_tokens > 0).collect();
        assert_eq!(charged.len(), 1);
        assert_eq!(charged[0].id, "extract");
        assert_eq!(charged[0].tier, "llm_multi_step");
        assert!(est.est_tokens >= 1000, "input + output tokens");
        assert!(!est.over_cap);
        assert_eq!(est.est_seconds, 2, "1 llm step + 1 flat");
    }

    #[test]
    fn huge_input_trips_over_cap() {
        let skill = parse_skill_yaml(YAML).unwrap();
        // 1e6 chars → ~333K input tokens > 50K cap.
        let est = estimate(&skill, 1_000_000, "gpt-4o-mini");
        assert!(est.over_cap, "1e6-char input must exceed the 50K cap");
    }

    #[test]
    fn estimate_is_zero_llm_deterministic() {
        // Two estimates of the same skill+input must be identical (no randomness / no LLM call).
        let skill = parse_skill_yaml(YAML).unwrap();
        let a = estimate(&skill, 5000, "gpt-4o-mini");
        let b = estimate(&skill, 5000, "gpt-4o-mini");
        assert_eq!(a.est_tokens, b.est_tokens);
    }
}
