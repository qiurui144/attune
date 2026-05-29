//! ACP-7 — Cost-Aware Scheduler (entitlement + cost-class routing).
//!
//! Per spec `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §3 (ACP-7 data flow) + §4 (module boundary) + §7 (quota-exhausted degrade) +
//! §9 (test matrix) + §11 R8 (graceful degrade — scheduler down → default local).
//!
//! ## Why this exists (C audit `2026-05-29-token-cot-context-audit.md`)
//!
//! There was **no cost-aware scheduling and no over-quota degrade**. The product
//! posture (CLAUDE.md Cost & Trigger Contract) is "云端为主，本地为辅" with a
//! free / paid split and a cloud quota — but nothing routed an agent call by
//! entitlement or fell back to local when the cloud quota ran out.
//!
//! This module is pure decision logic: given an agent's declared [`Tier`] +
//! [`CostClass`] (from ACP-1 registry) and the caller's runtime [`Entitlement`]
//! (free / paid + remaining cloud quota), it produces a [`ScheduleDecision`]:
//!
//! - **zero / local** cost-class agents → run locally, no quota touched;
//! - **cloud** cost-class, paid + quota left → cloud;
//! - **cloud**, paid but quota exhausted → degrade to local qwen fallback (if the
//!   agent can run on a weak local model) else a friendly quota-exhausted block;
//! - **paid** agent, free user → entitlement block (must upgrade);
//! - a soft-disabled agent (ACP-3) → blocked-disabled (the flow executor skips it
//!   if optional, else degrades — never cascade-fail, §11 R8).
//!
//! It changes no agent computation (§2.3): scheduling only chooses *where* the
//! call runs, never *what it computes*.

use super::registry::{AgentSpec, CostClass, Tier};

/// The local fallback model used when a paid cloud agent's quota is exhausted but
/// the agent can still run (degraded) on a weak local model. Matches the wizard's
/// local-Ollama default (CLAUDE.md LLM provider strategy).
pub const LOCAL_FALLBACK_MODEL: &str = "qwen2.5:3b";

/// The caller's runtime entitlement — drives ACP-7 routing. Constructed from the
/// account/license layer (paid plan?) plus the live cloud-quota accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entitlement {
    /// Does the user hold a paid plan (may invoke `tier = paid` agents)?
    pub paid: bool,
    /// Remaining cloud quota units (e.g. tokens or calls). `0` = exhausted.
    pub cloud_quota_remaining: u64,
    /// Is a local model available as a fallback (Ollama installed / K3 reachable)?
    pub local_available: bool,
}

impl Entitlement {
    /// A free user with a local model available and no cloud quota.
    pub fn free_local() -> Self {
        Entitlement {
            paid: false,
            cloud_quota_remaining: 0,
            local_available: true,
        }
    }

    /// A paid user with the given cloud quota and a local fallback available.
    pub fn paid_with_quota(cloud_quota_remaining: u64) -> Self {
        Entitlement {
            paid: true,
            cloud_quota_remaining,
            local_available: true,
        }
    }
}

/// Where (and whether) an agent call should run. Produced by [`Scheduler::route`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleDecision {
    /// Run locally — zero/local cost class, or a paid agent degraded to a local
    /// fallback model after quota exhaustion. `model` is `None` for zero-cost
    /// (deterministic) agents that never touch a model.
    Local {
        /// The local model to use (`None` for deterministic / zero-cost agents).
        model: Option<String>,
        /// True when this is a degraded fallback from an exhausted cloud quota.
        degraded_from_cloud: bool,
    },
    /// Run on the cloud (paid agent, quota available).
    Cloud,
    /// Blocked: a paid agent invoked by a free user (must upgrade). Carries a
    /// user-friendly reason. The flow executor treats this like a step failure
    /// (skip if optional, else degrade per policy — never cascade-fail).
    BlockedEntitlement {
        /// Friendly explanation for the UI.
        reason: String,
    },
    /// Blocked: a cloud agent whose quota is exhausted and which cannot fall back
    /// to a local model. Carries a friendly reason.
    BlockedQuotaExhausted {
        /// Friendly explanation for the UI.
        reason: String,
    },
    /// Blocked: the agent was soft-disabled by ACP-3 (weak-model F1 below floor).
    /// The flow executor skips it if optional, else degrades.
    BlockedDisabled {
        /// The disable reason surfaced by the FeedbackController.
        reason: String,
    },
}

impl ScheduleDecision {
    /// Did the call actually get dispatched (locally or to cloud)?
    pub fn is_runnable(&self) -> bool {
        matches!(self, ScheduleDecision::Local { .. } | ScheduleDecision::Cloud)
    }

    /// Did scheduling block the call (entitlement / quota / disabled)?
    pub fn is_blocked(&self) -> bool {
        !self.is_runnable()
    }
}

/// The cost-aware scheduler (§3 ACP-7). Holds the caller's entitlement; routing
/// is a pure function of that plus the agent's declared tier + cost class.
#[derive(Debug, Clone, Copy)]
pub struct Scheduler {
    entitlement: Entitlement,
}

impl Scheduler {
    /// Construct with the caller's runtime entitlement.
    pub fn new(entitlement: Entitlement) -> Self {
        Scheduler { entitlement }
    }

    /// The entitlement this scheduler routes against.
    pub fn entitlement(&self) -> Entitlement {
        self.entitlement
    }

    /// Route one agent call. `disabled_reason` is `Some` when ACP-3 has
    /// soft-disabled this agent (the flow executor passes it through so a disabled
    /// step blocks before any cost is spent). A weak local model can be used as a
    /// degraded fallback only when the agent declares a non-empty
    /// `model_tier_floor` of `qwen3b` (the only tier a 3B local model satisfies)
    /// — pricier-floor agents (gpt-4o-mini / sonnet) cannot degrade to local.
    pub fn route(&self, agent: &AgentSpec, disabled_reason: Option<&str>) -> ScheduleDecision {
        // ACP-3 soft-disable takes precedence — never dispatch a disabled agent.
        if let Some(reason) = disabled_reason {
            return ScheduleDecision::BlockedDisabled {
                reason: reason.to_string(),
            };
        }

        // Entitlement gate: a paid agent requires a paid plan.
        if agent.tier == Tier::Paid && !self.entitlement.paid {
            return ScheduleDecision::BlockedEntitlement {
                reason: format!(
                    "agent {} requires a paid plan (tier=paid)",
                    agent.id
                ),
            };
        }

        match agent.cost_class {
            // Zero/local cost class never touches the cloud quota.
            CostClass::Zero => ScheduleDecision::Local {
                model: None,
                degraded_from_cloud: false,
            },
            CostClass::Local => ScheduleDecision::Local {
                model: model_for_local(agent),
                degraded_from_cloud: false,
            },
            CostClass::Cloud => self.route_cloud(agent),
        }
    }

    /// Cloud-class routing with quota accounting + local degrade chain.
    fn route_cloud(&self, agent: &AgentSpec) -> ScheduleDecision {
        if self.entitlement.cloud_quota_remaining > 0 {
            return ScheduleDecision::Cloud;
        }
        // Quota exhausted → try the local qwen fallback (§7 "本地 qwen 兜底").
        if self.entitlement.local_available && can_degrade_to_local(agent) {
            return ScheduleDecision::Local {
                model: Some(LOCAL_FALLBACK_MODEL.to_string()),
                degraded_from_cloud: true,
            };
        }
        ScheduleDecision::BlockedQuotaExhausted {
            reason: format!(
                "cloud quota exhausted for agent {} and no local fallback (floor={})",
                agent.id,
                if agent.model_tier_floor.is_empty() {
                    "-"
                } else {
                    agent.model_tier_floor.as_str()
                }
            ),
        }
    }
}

/// The local model an `local` cost-class agent should run on. A `qwen3b`-floor
/// agent runs on the local fallback; otherwise it has no explicit model (the
/// caller's default local model applies).
fn model_for_local(agent: &AgentSpec) -> Option<String> {
    if agent.model_tier_floor == "qwen3b" {
        Some(LOCAL_FALLBACK_MODEL.to_string())
    } else {
        None
    }
}

/// Can this cloud agent degrade to a weak *local* model when the quota runs out?
/// Only agents whose floor is the weakest tier (`qwen3b`) — a 3B local model
/// satisfies `qwen3b` but not `gpt-4o-mini` / `sonnet`, so pricier-floor agents
/// (e.g. defamation_extractor, evidence_classifier VLM) cannot degrade and must
/// surface a friendly quota-exhausted block instead of silently downgrading
/// quality (§2.3 — never sacrifice correctness for cost).
fn can_degrade_to_local(agent: &AgentSpec) -> bool {
    agent.model_tier_floor == "qwen3b"
}

#[cfg(test)]
mod tests;
