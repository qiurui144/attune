//! ACP-5 — Autonomous Flow (插件间工程协作 / declarative flow DAG + executor).
//!
//! Per spec `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §5.3b (autonomous flow: three-layer abstraction + typed handoff + agent_flows.toml
//! DAG + flow executor + 4 guarantees) + §7 (graceful degrade) + §9 (test matrix) +
//! §11 R8 (control-plane single-point protection).
//!
//! ## Why this exists (user 2026-05-29 twice-reiterated: "确保插件的自主流转能力 /
//! agents 之间是工程协作的关系")
//!
//! ACP-1 [`super::registry`] made each agent's typed handoff (`consumes` /
//! `produces`) a first-class citizen. This module **consumes** that to implement
//! **autonomous flow**: work flows between agents along a declared DAG without
//! manual per-step stitching — but the flow chain itself is **human-authored**
//! (declarative TOML), NOT agent-spawns-agent (that is deferred to v2.x per §2.2).
//!
//! ### Three-layer abstraction (§5.3b)
//!
//! ```text
//! user intent ──► [Intent Router] ──► flow_id ──► [Flow Executor runs the DAG]
//!                  keyword/priority match           typed-handoff chain, step by step
//!                                                    each step: ACP-7 schedule + ACP-4
//!                                                    cost-governance + ACP-3 telemetry
//! ```
//!
//! ### The 4 guarantees (§5.3b)
//!
//! 1. **Type-safe handoff** — `A → B` legal iff `A.produces == B.consumes`,
//!    validated at load against the registry ([`FlowSet::validate_against`]). A
//!    mismatch is rejected before any execution (no runtime mis-wiring).
//! 2. **Per-step governable** — every hop goes through ACP-7 schedule + ACP-4
//!    cost + ACP-3 telemetry (the executor [`run_flow`] wires the [`StepRunner`]).
//! 3. **Graceful degrade** — a disabled agent (ACP-3) / quota exhaustion (ACP-7) /
//!    a step failure follows the `degrade` policy: skip an optional step or return
//!    a partial result — **never cascade-fail** (§11 R8).
//! 4. **Auditable** — the flow DAG is declarative TOML and every step leaves a
//!    telemetry trace (replayable, diagnosable).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::registry::AgentRegistry;

/// How a flow degrades when a step cannot complete (spec §5.3b ②).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnStepFail {
    /// Return the partial payload accumulated so far (default — never hard-fail).
    #[default]
    Partial,
    /// Abort the whole flow with an error.
    Abort,
    /// Route to a declared fallback agent for this step.
    FallbackAgent,
}

/// The degrade policy for a flow (spec §5.3b ②).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Degrade {
    /// Steps that may be skipped on failure / disable / quota exhaustion without
    /// failing the flow (their output is simply omitted).
    #[serde(default)]
    pub optional: Vec<String>,
    /// What to do when a *non-optional* step fails.
    #[serde(default)]
    pub on_step_fail: OnStepFail,
    /// Fallback agent id used when `on_step_fail == FallbackAgent`.
    #[serde(default)]
    pub fallback_agent: Option<String>,
}

/// One declarative flow DAG (spec §5.3b ②, `agent_flows.toml`). `steps` is an
/// ordered chain of agent ids; the typed handoff between consecutive steps is
/// validated against the registry at load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowDef {
    /// Unique flow id (router matches to this).
    pub id: String,
    /// Router entry keywords (the flow is selected when these match the intent).
    #[serde(default)]
    pub route_keywords: Vec<String>,
    /// Router priority — higher wins on keyword collision.
    #[serde(default)]
    pub route_priority: i32,
    /// Ordered chain of agent ids. A single-agent flow has `steps = ["x"]`
    /// (the 1-step flow that gives backward compatibility with single-agent
    /// routing, §10).
    pub steps: Vec<String>,
    /// Degrade policy (graceful-degrade guarantee).
    #[serde(default)]
    pub degrade: Degrade,
}

impl FlowDef {
    /// Is this step declared optional (skippable on failure)?
    pub fn is_optional(&self, step: &str) -> bool {
        self.degrade.optional.iter().any(|s| s == step)
    }
}

/// The whole flow catalogue — deserialized from `agent_flows.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FlowSet {
    #[serde(rename = "flow", default)]
    flows: Vec<FlowDef>,
}

impl FlowSet {
    /// Parse from a TOML string. Structural validation (non-empty id, non-empty
    /// steps, no duplicate flow id, no self-cycle in a step chain) runs eagerly;
    /// **typed-handoff** validation needs the registry and runs in
    /// [`FlowSet::validate_against`].
    pub fn from_toml_str(s: &str) -> Result<Self, String> {
        let set: FlowSet = toml::from_str(s).map_err(|e| format!("flows parse error: {e}"))?;
        set.validate_structure()?;
        Ok(set)
    }

    /// Load from a file path (startup + CLI).
    pub fn from_path(path: &std::path::Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read flows {}: {e}", path.display()))?;
        Self::from_toml_str(&s)
    }

    /// All flows, declaration order.
    pub fn flows(&self) -> &[FlowDef] {
        &self.flows
    }

    /// Number of flows.
    pub fn len(&self) -> usize {
        self.flows.len()
    }

    /// True when the set holds no flows.
    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    /// Look up a flow by id.
    pub fn get(&self, id: &str) -> Option<&FlowDef> {
        self.flows.iter().find(|f| f.id == id)
    }

    /// Structural validation (registry-independent): non-empty id + steps, unique
    /// flow id, no repeated agent id within a single step chain (a within-flow
    /// cycle — `[a, b, a]` would re-enter `a`), and `fallback_agent` present when
    /// the policy is `FallbackAgent`.
    fn validate_structure(&self) -> Result<(), String> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for f in &self.flows {
            if f.id.trim().is_empty() {
                return Err("flow with empty id".to_string());
            }
            if !seen.insert(f.id.as_str()) {
                return Err(format!("duplicate flow id: {}", f.id));
            }
            if f.steps.is_empty() {
                return Err(format!("flow {} has no steps", f.id));
            }
            let mut step_seen: BTreeSet<&str> = BTreeSet::new();
            for step in &f.steps {
                if step.trim().is_empty() {
                    return Err(format!("flow {} has an empty step id", f.id));
                }
                if !step_seen.insert(step.as_str()) {
                    return Err(format!(
                        "flow {} has a cyclic / repeated step {step:?} (a step may appear once)",
                        f.id
                    ));
                }
            }
            // optional steps must reference real steps in the chain.
            for opt in &f.degrade.optional {
                if !f.steps.iter().any(|s| s == opt) {
                    return Err(format!(
                        "flow {} marks {opt:?} optional but it is not a step",
                        f.id
                    ));
                }
            }
            if f.degrade.on_step_fail == OnStepFail::FallbackAgent
                && f.degrade.fallback_agent.as_deref().unwrap_or("").trim().is_empty()
            {
                return Err(format!(
                    "flow {} uses on_step_fail=fallback_agent but declares no fallback_agent",
                    f.id
                ));
            }
        }
        Ok(())
    }

    /// **Typed-handoff validation against the registry** (guarantee ① — spec
    /// §5.3b). For every flow and every consecutive step pair `A → B`:
    ///   - both ids must be registered agents (no shadow agent),
    ///   - `A.produces == B.consumes` (the typed handoff must connect),
    ///   - any declared `fallback_agent` must also be registered.
    ///
    /// This is the load-time gate that makes mis-wiring impossible to reach
    /// execution.
    pub fn validate_against(&self, registry: &AgentRegistry) -> Result<(), String> {
        for f in &self.flows {
            for step in &f.steps {
                if registry.get(step).is_none() {
                    return Err(format!(
                        "flow {} references unregistered (shadow) agent {step:?}",
                        f.id
                    ));
                }
            }
            for pair in f.steps.windows(2) {
                let (a_id, b_id) = (&pair[0], &pair[1]);
                // Both are registered (checked above); unwrap is safe.
                let a = registry.get(a_id).unwrap();
                let b = registry.get(b_id).unwrap();
                if a.handoff.produces != b.handoff.consumes {
                    return Err(format!(
                        "flow {}: handoff type mismatch {a_id} produces {:?} but {b_id} consumes \
                         {:?}",
                        f.id, a.handoff.produces, b.handoff.consumes
                    ));
                }
            }
            if let Some(fb) = &f.degrade.fallback_agent {
                if registry.get(fb).is_none() {
                    return Err(format!(
                        "flow {} declares fallback_agent {fb:?} which is not registered",
                        f.id
                    ));
                }
            }
        }
        Ok(())
    }

    /// Route an intent to a flow id (spec §5.3b — router extended to flow routing).
    /// Returns the highest-priority flow whose `route_keywords` the message
    /// contains. A single-agent flow (`steps=[x]`) is routed exactly like a
    /// multi-step flow, giving backward compatibility (§10). Ties broken by flow
    /// id for determinism.
    pub fn route(&self, message: &str) -> Option<&FlowDef> {
        self.flows
            .iter()
            .filter(|f| f.route_keywords.iter().any(|k| message.contains(k.as_str())))
            .max_by(|x, y| {
                x.route_priority
                    .cmp(&y.route_priority)
                    .then_with(|| y.id.cmp(&x.id))
            })
    }

    /// Render the flow catalogue for `attune agent flow list` (§5.5 / Task 6):
    /// each flow + its step chain + the typed-handoff type at each hop (validated
    /// against the registry so the printed chain reflects the real type graph).
    pub fn render_list(&self, registry: &AgentRegistry) -> String {
        let mut out = String::new();
        out.push_str(&format!("Agent Flows — {} flow(s)\n", self.flows.len()));
        out.push_str(&"=".repeat(60));
        out.push('\n');
        for f in &self.flows {
            out.push_str(&format!(
                "\n[{}] priority={} keywords={:?}\n",
                f.id, f.route_priority, f.route_keywords
            ));
            // Render the typed chain: agent --Type--> agent --Type--> ...
            let mut chain = String::from("  ");
            for (i, step) in f.steps.iter().enumerate() {
                if i > 0 {
                    // The handoff type between step i-1 and step i.
                    let ty = registry
                        .get(&f.steps[i - 1])
                        .map(|a| a.handoff.produces.as_str())
                        .unwrap_or("?");
                    chain.push_str(&format!(" --{ty}--> "));
                }
                let opt = if f.is_optional(step) { "?" } else { "" };
                chain.push_str(&format!("{step}{opt}"));
            }
            out.push_str(&chain);
            out.push('\n');
            out.push_str(&format!(
                "    degrade: optional={:?} on_step_fail={:?}\n",
                f.degrade.optional, f.degrade.on_step_fail
            ));
        }
        out
    }
}

/// Build a quick lookup of `flow_id → priority` (used by routing diagnostics).
pub fn flow_priorities(set: &FlowSet) -> BTreeMap<String, i32> {
    set.flows.iter().map(|f| (f.id.clone(), f.route_priority)).collect()
}

#[cfg(test)]
mod tests;
