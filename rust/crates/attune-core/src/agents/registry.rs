//! ACP-1 — Agent Registry (central agent directory).
//!
//! Per spec `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §3 (ACP-1) + §5.1 (registry contract) + §5.3b (typed handoff = autonomous-flow
//! foundation) + §7 (compile/startup validation) + §9 (test matrix).
//!
//! ## Why this exists (A audit `2026-05-29-agent-inventory-audit.md`)
//!
//! There was **no central agent directory**. `intent_router.rs` routed by keyword
//! against each loaded plugin's `chat_trigger` with no notion of agent identity,
//! tier, capability boundary, cost class, quality gate, or — critically for the
//! ACP-6 autonomous flow executor — the **typed handoff contract** (`consumes` /
//! `produces`) that lets work flow between agents without manual stitching.
//!
//! This module is the SSOT declaration (`rust/agents.registry.toml`, human-authored
//! + auditable) plus the load + **startup validation** (§7):
//!   - every agent id unique (no duplicate ids),
//!   - every capability boundary unique (single-point responsibility — no overlap),
//!   - route keyword conflicts surfaced (same keyword on two agents → warning,
//!     resolved by route_priority), and
//!   - handoff type names are non-empty (the type-graph the flow executor walks).
//!
//! It does NOT change any agent's computation (per §2.3 — registry only *describes*).
//! A shadow agent (an id invoked but absent from the registry) is rejected via
//! [`AgentRegistry::contains`] at the call site (§7 "shadow agent → fail-fast").

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Entitlement tier for an agent. Mirrors the plugin-level `pricing.tier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Built into OSS attune-core — no entitlement check.
    Free,
    /// Requires a paid plugin entitlement.
    Paid,
}

/// The computational nature of an agent — drives whether it touches the LLM
/// path (and therefore whether ACP-4 cost governance + ACP-3 telemetry apply).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Pure-function / arithmetic agent — zero LLM, runs locally.
    Deterministic,
    /// LLM-as-judge / extractor — hits an LLM, subject to telemetry + cost cap.
    LlmJudge,
    /// Rule-engine agent (deterministic with an externalized rule table).
    Rule,
    /// Vision-language-model capability (image → structured).
    Vlm,
}

/// What buys the work — drives ACP-7 scheduling (zero/local can run in the
/// background; cloud must be user-triggered per the Cost & Trigger Contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    /// CPU-only, millisecond — runs freely.
    Zero,
    /// Local GPU/NPU, seconds — runs at build time / background, pausable.
    Local,
    /// Cloud LLM token, seconds-to-minutes — must be user-triggered.
    Cloud,
}

/// Typed handoff contract — the **autonomous-flow foundation** (§5.3b). A flow
/// DAG step `A → B` is legal iff `A.produces == B.consumes` (the flow executor
/// in ACP-6 walks this type graph). Empty type names are rejected at load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    /// Typed input this agent accepts from an upstream step (or the user intent).
    pub consumes: String,
    /// Typed output this agent emits for a downstream step.
    pub produces: String,
}

/// One agent declaration. Single-point responsibility: `capability_boundary`
/// must be unique across the whole registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Unique agent id (matches `plugin.yaml` agents[].id / OSS module name).
    pub id: String,
    /// Entitlement tier.
    pub tier: Tier,
    /// Owning plugin (`oss-core` for built-in agents).
    pub plugin: String,
    /// Computational nature.
    pub kind: Kind,
    /// Single-point responsibility — unique across the registry.
    pub capability_boundary: String,
    /// Minimum model tier floor for LLM agents (`""` / `none` for deterministic).
    #[serde(default)]
    pub model_tier_floor: String,
    /// What buys the work.
    pub cost_class: CostClass,
    /// Bound quality gate id (`<plugin>/<harness>::<case>` or `oss/<harness>`).
    pub gate: String,
    /// Chat-trigger keywords (router entry points). May be empty for
    /// pipeline/background agents that are never user-routed.
    #[serde(default)]
    pub route_keywords: Vec<String>,
    /// Router priority — higher wins when keywords collide.
    #[serde(default)]
    pub route_priority: i32,
    /// Typed handoff contract (autonomous-flow foundation).
    pub handoff: Handoff,
}

/// The whole registry — deserialized from `agents.registry.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRegistry {
    #[serde(rename = "agent", default)]
    agents: Vec<AgentSpec>,
}

/// A route-keyword collision: the same keyword is claimed by two or more agents.
/// Surfaced (not fatal) per §7 "route_keywords 冲突 → priority + 编译期冲突告警".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteConflict {
    /// The shared keyword.
    pub keyword: String,
    /// Agent ids that all claim it (sorted, ≥ 2).
    pub agent_ids: Vec<String>,
}

impl AgentRegistry {
    /// Parse + validate from a TOML string. Validation (§7) runs eagerly so a
    /// malformed registry cannot reach production: duplicate id, duplicate
    /// capability boundary, empty handoff type, or empty gate all fail.
    pub fn from_toml_str(s: &str) -> Result<Self, String> {
        let reg: AgentRegistry =
            toml::from_str(s).map_err(|e| format!("registry parse error: {e}"))?;
        reg.validate()?;
        Ok(reg)
    }

    /// Load from a file path (used by startup + CLI).
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read registry {}: {e}", path.display()))?;
        Self::from_toml_str(&s)
    }

    /// All agents, registry order.
    pub fn agents(&self) -> &[AgentSpec] {
        &self.agents
    }

    /// Number of registered agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// True when the registry holds no agents.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Look up an agent by id.
    pub fn get(&self, id: &str) -> Option<&AgentSpec> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Shadow-agent guard (§7): is this id a registered agent? Call-site rejects
    /// any invocation of an unregistered (shadow) agent.
    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// Validate the registry (§7, fail-fast). Errors:
    ///   - duplicate agent id,
    ///   - duplicate capability boundary (single-point responsibility),
    ///   - empty gate binding,
    ///   - empty handoff `consumes` / `produces` type name.
    ///
    /// Route conflicts are NOT errors (resolved by priority) — see
    /// [`AgentRegistry::route_conflicts`].
    pub fn validate(&self) -> Result<(), String> {
        if self.agents.is_empty() {
            return Err("registry is empty: at least one agent required".to_string());
        }
        let mut seen_ids: BTreeMap<&str, ()> = BTreeMap::new();
        let mut seen_boundaries: BTreeMap<&str, &str> = BTreeMap::new();
        for a in &self.agents {
            if a.id.trim().is_empty() {
                return Err("agent with empty id".to_string());
            }
            if seen_ids.insert(a.id.as_str(), ()).is_some() {
                return Err(format!("duplicate agent id: {}", a.id));
            }
            let boundary = a.capability_boundary.trim();
            if boundary.is_empty() {
                return Err(format!("agent {} has empty capability_boundary", a.id));
            }
            if let Some(prev) = seen_boundaries.insert(boundary, a.id.as_str()) {
                return Err(format!(
                    "overlapping capability_boundary {boundary:?} on agents {prev} and {}",
                    a.id
                ));
            }
            if a.gate.trim().is_empty() {
                return Err(format!("agent {} has empty gate binding", a.id));
            }
            if a.handoff.consumes.trim().is_empty() {
                return Err(format!("agent {} has empty handoff.consumes type", a.id));
            }
            if a.handoff.produces.trim().is_empty() {
                return Err(format!("agent {} has empty handoff.produces type", a.id));
            }
        }
        Ok(())
    }

    /// Compute route-keyword conflicts (§7 warning, not fatal). Two agents that
    /// claim the same keyword are reported so the operator can confirm the
    /// priority ordering is intentional. Determinism: keyword + ids sorted.
    pub fn route_conflicts(&self) -> Vec<RouteConflict> {
        let mut by_keyword: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for a in &self.agents {
            for kw in &a.route_keywords {
                by_keyword.entry(kw.as_str()).or_default().push(a.id.as_str());
            }
        }
        by_keyword
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(kw, mut ids)| {
                ids.sort_unstable();
                RouteConflict {
                    keyword: kw.to_string(),
                    agent_ids: ids.into_iter().map(String::from).collect(),
                }
            })
            .collect()
    }

    /// Render the directory view for `attune agent registry` (§5.5). One row per
    /// agent: id / tier / kind / cost / plugin / model-floor / gate / boundary.
    /// Grouped by plugin, agents sorted by id for a stable dashboard.
    pub fn render_directory(&self) -> String {
        let mut by_plugin: BTreeMap<&str, Vec<&AgentSpec>> = BTreeMap::new();
        for a in &self.agents {
            by_plugin.entry(a.plugin.as_str()).or_default().push(a);
        }
        let mut out = String::new();
        out.push_str(&format!("Agent Registry — {} agents\n", self.agents.len()));
        out.push_str(&"=".repeat(60));
        out.push('\n');
        for (plugin, mut agents) in by_plugin {
            agents.sort_by(|x, y| x.id.cmp(&y.id));
            out.push_str(&format!("\n[{plugin}] ({} agents)\n", agents.len()));
            for a in agents {
                let tier = match a.tier {
                    Tier::Free => "free",
                    Tier::Paid => "paid",
                };
                let kind = match a.kind {
                    Kind::Deterministic => "deterministic",
                    Kind::LlmJudge => "llm-judge",
                    Kind::Rule => "rule",
                    Kind::Vlm => "vlm",
                };
                let cost = match a.cost_class {
                    CostClass::Zero => "zero",
                    CostClass::Local => "local",
                    CostClass::Cloud => "cloud",
                };
                let floor = if a.model_tier_floor.is_empty() {
                    "-"
                } else {
                    a.model_tier_floor.as_str()
                };
                out.push_str(&format!(
                    "  {id:<24} {tier:<4} {kind:<13} {cost:<5} floor={floor:<11} gate={gate}\n      ↳ {boundary}\n        handoff: {consumes} → {produces}\n",
                    id = a.id,
                    gate = a.gate,
                    boundary = a.capability_boundary,
                    consumes = a.handoff.consumes,
                    produces = a.handoff.produces,
                ));
            }
        }
        // Surface any route-keyword conflicts (resolved by priority — informational).
        let conflicts = self.route_conflicts();
        if !conflicts.is_empty() {
            out.push_str("\nRoute-keyword conflicts (resolved by priority):\n");
            for c in conflicts {
                out.push_str(&format!("  {:?} → {:?}\n", c.keyword, c.agent_ids));
            }
        }
        out
    }

    /// The set of distinct handoff type names referenced by any agent. The flow
    /// executor (ACP-6) uses this to validate that a declared flow's steps form
    /// a typed chain (`upstream.produces == downstream.consumes`).
    pub fn handoff_types(&self) -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        for a in &self.agents {
            set.insert(a.handoff.consumes.clone());
            set.insert(a.handoff.produces.clone());
        }
        set
    }
}

#[cfg(test)]
mod tests;
