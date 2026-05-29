//! ACP-3 — FeedbackController: the monitor → fine-tune closed loop (§3 + §5.2).
//!
//! Spec: `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §3 (ACP-3 Self-Monitoring & Fine-Tuning Loop) + §5.2 (`TuningAction`) +
//! §2.3 (red line) + §6 (`FeedbackSource` extension point) + §11 (R2/R7).
//!
//! ## What this closes (B audit `2026-05-29-quality-gating-telemetry-audit.md`)
//!
//! ACP-3 telemetry (`agent_telemetry.rs`) records per-(agent × model) failure
//! rate, and the `attune agent health` dashboard renders it — but the **loop was
//! open**: a high failure rate produced an observation, never an action. This
//! module is the closed half: failure-rate → [`TuningAction`].
//!
//! ## Hard red line (§2.3 — non-negotiable)
//!
//! The controller **never** touches an agent's correctness: it only ever
//! escalates the model tier, injects few-shot examples, or soft-disables an
//! LLM-judge agent (with a human-review alert). A **deterministic** agent — whose
//! correctness is a pure function, per the reliability framework — is *never*
//! tuned, no matter how its (mislabelled) telemetry looks. [`FeedbackController::decide`]
//! classifies every row against the registry and returns [`TuningAction::NoOp`]
//! for any non-LLM-judge agent.
//!
//! ## Cost & misfire guards (§11 R2 / R7)
//!
//! - **R2 (cost):** auto-escalating the model tier silently pushes traffic onto
//!   more expensive cloud models. So [`FeedbackConfig::auto_escalate`] defaults to
//!   **OFF**: the controller still *recommends* the escalation (surfaced by
//!   `attune agent tune --dry-run` for human review) but marks it `applied =
//!   false`. The user must opt in.
//! - **R7 (misfire):** a single noisy spike must not disable a working agent.
//!   `DisableWithAlert` requires both a **minimum sample size** ([`FeedbackConfig::min_samples`])
//!   *and* a **consecutive-breach streak** ([`FeedbackConfig::consecutive_periods`]).
//!   Disable is **soft** — it flags the agent for human review and never deletes
//!   it; recovery in any period resets the streak.

use serde::{Deserialize, Serialize};

use crate::agent_telemetry::{AgentModelHealth, FAILURE_RATE_ALERT_THRESHOLD};
use crate::agents::registry::{AgentRegistry, Kind};

/// The model-tier ladder (lowest → highest), per `agents.registry.toml` header:
/// `qwen3b < flash < gpt-4o-mini < sonnet`. Escalation walks one rung up.
const MODEL_TIER_LADDER: [&str; 4] = ["qwen3b", "flash", "gpt-4o-mini", "sonnet"];

/// Streak map key for one (agent × model). NUL separator avoids collisions
/// between ids/models that share a substring.
fn streak_key(agent_id: &str, model: &str) -> String {
    format!("{agent_id}\u{0}{model}")
}

/// The next-higher model tier above `current`, or `None` if `current` is already
/// the top tier (or is not a recognised tier — escalation only happens between
/// known rungs).
pub fn next_model_tier(current: &str) -> Option<&'static str> {
    let idx = MODEL_TIER_LADDER.iter().position(|&t| t == current)?;
    MODEL_TIER_LADDER.get(idx + 1).copied()
}

/// A fine-tuning action the controller may take for one (agent × model). Per
/// spec §5.2 the controller only ever adjusts model tier / few-shot / disable —
/// **never** correctness (§2.3 red line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TuningAction {
    /// Move the (agent × model) up one rung on the model-tier ladder because the
    /// failure rate exceeded the alert threshold and a higher tier exists.
    EscalateModelTier {
        /// Current (failing) tier.
        from: String,
        /// Recommended higher tier.
        to: String,
    },
    /// Inject additional few-shot examples into the agent's prompt (weak-model
    /// fallback, per §4.5-C) — used when no higher tier is reachable but the
    /// agent might still recover with more in-context examples.
    InjectFewShot {
        /// Target agent id.
        agent_id: String,
        /// Number of examples to add.
        examples: u8,
    },
    /// Soft-disable the agent and raise a human-review alert (weak-model F1 below
    /// floor on the top tier, sustained). Never a hard delete (§2.3 / R7).
    DisableWithAlert {
        /// Target agent id.
        agent_id: String,
        /// Human-readable reason for the alert.
        reason: String,
    },
    /// No action — the agent is healthy, deterministic (red-line protected),
    /// below the alert threshold, or lacks sufficient evidence (R7).
    NoOp,
}

/// Why a [`TuningAction`] resolved the way it did — the audit trail for one
/// (agent × model) decision. `applied` distinguishes an *auto-applied* action
/// (only escalation, and only when `auto_escalate` is ON) from a *recommendation*
/// surfaced for human review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TuningDecision {
    /// The agent this decision concerns.
    pub agent_id: String,
    /// The model the agent was running on.
    pub model: String,
    /// The chosen action.
    pub action: TuningAction,
    /// Whether the controller auto-applied the action. Escalations are applied
    /// only when [`FeedbackConfig::auto_escalate`] is ON (R2). Disable and
    /// few-shot are surfaced for human review and never auto-applied here.
    pub applied: bool,
    /// True when this row is a deterministic (or shadow) agent that the red line
    /// (§2.3) forbids tuning — recorded for transparency in the dashboard.
    pub red_line_protected: bool,
}

impl TuningDecision {
    fn noop(agent_id: &str, model: &str, red_line_protected: bool) -> Self {
        TuningDecision {
            agent_id: agent_id.to_string(),
            model: model.to_string(),
            action: TuningAction::NoOp,
            applied: false,
            red_line_protected,
        }
    }

    /// True when this decision was forced to [`TuningAction::NoOp`] by the §2.3
    /// red line (deterministic / unknown agent).
    pub fn is_red_line_protected(&self) -> bool {
        self.red_line_protected
    }

    /// True when the controller chose a real action but did **not** auto-apply
    /// it (R2 dry-run / human-review path).
    pub fn is_recommendation(&self) -> bool {
        !matches!(self.action, TuningAction::NoOp) && !self.applied
    }

    /// True when the action requires a human in the loop before taking effect
    /// (soft disable). Per R7 disable is never silently auto-applied.
    pub fn needs_human_review(&self) -> bool {
        matches!(self.action, TuningAction::DisableWithAlert { .. })
    }
}

/// FeedbackController tuning policy. Defaults are the safe production posture:
/// auto-escalate OFF (R2), a real minimum sample size and a multi-period
/// consecutive-breach requirement before any disable (R7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackConfig {
    /// R2: when OFF (default), escalations are recommendations only — never
    /// auto-applied. The user must explicitly opt in (`acp.auto_escalate = true`).
    pub auto_escalate: bool,
    /// R7: minimum number of calls before any tuning action fires — guards
    /// against acting on a tiny, noisy sample.
    pub min_samples: u64,
    /// R7: number of consecutive breaching observations required before a
    /// soft-disable fires. `1` = act on the first sustained breach.
    pub consecutive_periods: u32,
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        FeedbackConfig {
            // R2: never push spend onto pricier cloud tiers without opt-in.
            auto_escalate: false,
            // R7: at least 20 calls of evidence before acting.
            min_samples: 20,
            // R7: require 3 consecutive breaching periods before disabling.
            consecutive_periods: 3,
        }
    }
}

/// The monitor → fine-tune controller (§3 ACP-3). Pure decision logic over
/// telemetry roll-ups plus a small amount of streak state for the R7
/// consecutive-period guard.
#[derive(Debug, Clone)]
pub struct FeedbackController {
    cfg: FeedbackConfig,
    /// Per-(agent × model) consecutive breach counter (R7). Key = `"agent\u{0}model"`.
    streaks: std::collections::HashMap<String, u32>,
}

impl FeedbackController {
    /// Construct with a tuning policy.
    pub fn new(cfg: FeedbackConfig) -> Self {
        FeedbackController {
            cfg,
            streaks: std::collections::HashMap::new(),
        }
    }

    /// The active config.
    pub fn config(&self) -> &FeedbackConfig {
        &self.cfg
    }

    /// Decide a [`TuningDecision`] for every telemetry row — a **pure** function
    /// of `(registry, rows)` (no streak mutation; idempotent). This is what
    /// `attune agent tune --dry-run` calls. The consecutive-period guard treats
    /// `consecutive_periods` as already-satisfied for the disable branch (a
    /// stateless single-shot view); use [`FeedbackController::observe`] for the
    /// stateful streak-tracking loop.
    pub fn decide(&self, reg: &AgentRegistry, rows: &[AgentModelHealth]) -> Vec<TuningDecision> {
        rows.iter()
            .map(|row| self.decide_one(reg, row, /* breach_streak_met = */ true))
            .collect()
    }

    /// Observe one telemetry period: update the per-(agent × model) consecutive
    /// breach streak, then decide. Unlike [`FeedbackController::decide`] this is
    /// **stateful** — a [`TuningAction::DisableWithAlert`] only fires once the
    /// streak reaches `consecutive_periods` (R7), and any non-breaching period
    /// resets the streak.
    pub fn observe(&mut self, reg: &AgentRegistry, rows: &[AgentModelHealth]) -> Vec<TuningDecision> {
        rows.iter()
            .map(|row| {
                let key = streak_key(&row.agent_id, &row.model);
                let breaching = self.is_breaching(reg, row);
                let streak_met = if breaching {
                    let n = self.streaks.entry(key).or_insert(0);
                    *n += 1;
                    *n >= self.cfg.consecutive_periods
                } else {
                    // Recovery resets the streak (R7).
                    self.streaks.remove(&key);
                    false
                };
                self.decide_one(reg, row, streak_met)
            })
            .collect()
    }

    /// Is this row a *tunable* breach: an LLM-judge agent, with enough samples,
    /// strictly above the alert threshold? Deterministic / unknown agents and
    /// thin samples are never breaches.
    fn is_breaching(&self, reg: &AgentRegistry, row: &AgentModelHealth) -> bool {
        if row.total_calls < self.cfg.min_samples {
            return false;
        }
        match reg.get(&row.agent_id).map(|a| a.kind) {
            Some(Kind::LlmJudge) => row.failure_rate > FAILURE_RATE_ALERT_THRESHOLD,
            // Deterministic / Rule / Vlm / shadow → never tuned by ACP-3 (§2.3).
            _ => false,
        }
    }

    /// Decide one row. `breach_streak_met` is whether the R7 consecutive-period
    /// requirement is satisfied for the disable branch.
    fn decide_one(
        &self,
        reg: &AgentRegistry,
        row: &AgentModelHealth,
        breach_streak_met: bool,
    ) -> TuningDecision {
        // §2.3 red line: only LLM-judge agents are ever tuned. Deterministic /
        // Rule / Vlm / shadow agents are red-line protected → NoOp.
        let kind = reg.get(&row.agent_id).map(|a| a.kind);
        let is_llm_judge = matches!(kind, Some(Kind::LlmJudge));
        if !is_llm_judge {
            // red_line_protected only when the agent exists but is non-LLM (a
            // shadow agent is "unknown", not "protected" — but both NoOp).
            let protected = kind.is_some();
            return TuningDecision::noop(&row.agent_id, &row.model, protected || kind.is_none());
        }

        // Below threshold or too few samples → NoOp (R7 evidence floor).
        if row.total_calls < self.cfg.min_samples
            || row.failure_rate <= FAILURE_RATE_ALERT_THRESHOLD
        {
            return TuningDecision::noop(&row.agent_id, &row.model, false);
        }

        // Breaching LLM-judge with enough evidence. Prefer escalation if a higher
        // model tier exists; otherwise soft-disable (no headroom left).
        match next_model_tier(&row.model) {
            Some(higher) => {
                // R2: only auto-apply when the user opted in.
                let applied = self.cfg.auto_escalate;
                TuningDecision {
                    agent_id: row.agent_id.clone(),
                    model: row.model.clone(),
                    action: TuningAction::EscalateModelTier {
                        from: row.model.clone(),
                        to: higher.to_string(),
                    },
                    applied,
                    red_line_protected: false,
                }
            }
            None => {
                // Top tier and still failing. R7: only disable once the
                // consecutive-period streak is met; otherwise hold (NoOp).
                if breach_streak_met {
                    TuningDecision {
                        agent_id: row.agent_id.clone(),
                        model: row.model.clone(),
                        action: TuningAction::DisableWithAlert {
                            agent_id: row.agent_id.clone(),
                            reason: format!(
                                "failure rate {:.1}% on top tier {} over {} calls (≥ {} consecutive periods) — soft-disabled pending human review",
                                row.failure_rate * 100.0,
                                row.model,
                                row.total_calls,
                                self.cfg.consecutive_periods,
                            ),
                        },
                        // Disable is never silently auto-applied: it always needs
                        // human review (R7 soft-disable).
                        applied: false,
                        red_line_protected: false,
                    }
                } else {
                    TuningDecision::noop(&row.agent_id, &row.model, false)
                }
            }
        }
    }

    /// Aggregate signals from multiple [`FeedbackSource`] channels (§6 extension
    /// point). The telemetry fail-rate is one channel; `skill_evolution` is
    /// another — the controller is source-agnostic and simply collects them.
    pub fn aggregate_signals(&self, sources: &[&dyn FeedbackSource]) -> Vec<FeedbackSignal> {
        sources
            .iter()
            .flat_map(|s| s.collect_signals())
            .collect()
    }
}

/// Severity of a [`FeedbackSignal`] — how actionable it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalSeverity {
    /// Informational — observed, not yet actionable.
    Info,
    /// A threshold was crossed — an action is due.
    Warn,
    /// Sustained / severe — escalate to human attention.
    Critical,
}

/// One feedback signal from a [`FeedbackSource`] (§6). The controller aggregates
/// signals across many sources into a single quality view. This is intentionally
/// generic: telemetry, skill-evolution, future RAG-miss or annotation channels
/// all emit the same shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackSignal {
    /// Originating channel name (`"telemetry"` / `"skill_evolution"` / ...).
    pub source: String,
    /// The subject — an agent id, a skill, or a free-form scope.
    pub subject: String,
    /// How actionable this signal is.
    pub severity: SignalSeverity,
    /// Human-readable detail.
    pub detail: String,
}

impl FeedbackSignal {
    /// Construct a signal.
    pub fn new(
        source: impl Into<String>,
        subject: impl Into<String>,
        severity: SignalSeverity,
        detail: impl Into<String>,
    ) -> Self {
        FeedbackSignal {
            source: source.into(),
            subject: subject.into(),
            severity,
            detail: detail.into(),
        }
    }
}

/// A pluggable feedback channel (§6 extension point). The first implementation is
/// [`SkillEvolutionFeedback`]; telemetry fail-rate is consumed directly by
/// [`FeedbackController::decide`]. New channels (RAG-miss, annotation drift, ...)
/// implement this trait and are passed to [`FeedbackController::aggregate_signals`].
pub trait FeedbackSource {
    /// Stable channel name (used to group / attribute signals).
    fn source_name(&self) -> &str;
    /// Collect the current signals from this channel (may be empty).
    fn collect_signals(&self) -> Vec<FeedbackSignal>;
}

/// `skill_evolution` as a [`FeedbackSource`] (§6). The self-evolving-skill agent
/// learns search-expansion terms from `search_miss` signals; surfacing its
/// pending-signal backlog as a feedback channel lets the controller see "an
/// evolution cycle is due" alongside agent fail-rates. This does **not** change
/// skill_evolution's behaviour — it keeps learning expansions; it merely *also*
/// reports its backlog.
pub struct SkillEvolutionFeedback<'a> {
    store: &'a crate::store::Store,
}

impl<'a> SkillEvolutionFeedback<'a> {
    /// Wrap a store as a skill-evolution feedback channel.
    pub fn new(store: &'a crate::store::Store) -> Self {
        SkillEvolutionFeedback { store }
    }
}

impl FeedbackSource for SkillEvolutionFeedback<'_> {
    fn source_name(&self) -> &str {
        "skill_evolution"
    }

    fn collect_signals(&self) -> Vec<FeedbackSignal> {
        // Best-effort read; a failed count must not poison aggregation.
        let pending = self.store.count_unprocessed_signals().unwrap_or(0);
        if pending == 0 {
            return Vec::new();
        }
        let severity = if pending >= crate::skill_evolution::EVOLVE_THRESHOLD {
            SignalSeverity::Warn
        } else {
            SignalSeverity::Info
        };
        vec![FeedbackSignal::new(
            "skill_evolution",
            "self_evolving_skill",
            severity,
            format!(
                "{pending} unprocessed search-miss signal(s) pending (threshold {})",
                crate::skill_evolution::EVOLVE_THRESHOLD
            ),
        )]
    }
}

/// Render the `attune agent tune --dry-run` view (§5.5 / Task 4): for each
/// telemetry row, the [`TuningAction`] the controller chose and whether it was
/// applied. `auto_escalate` reflects the effective config so the banner can warn
/// the operator that escalations are recommendations only when it is OFF (R2).
pub fn render_tune(decisions: &[TuningDecision], auto_escalate: bool) -> String {
    let mut out = String::new();
    out.push_str("Agent Tuning Plan (ACP-3 FeedbackController, dry-run)\n");
    out.push_str(&"=".repeat(60));
    out.push('\n');
    if !auto_escalate {
        out.push_str(
            "  auto-escalate is OFF (default) — escalations below are recommendations\n  \
             only and were NOT applied. Set `acp.auto_escalate = true` to enable (R2).\n\n",
        );
    }
    let actionable: Vec<&TuningDecision> = decisions
        .iter()
        .filter(|d| !matches!(d.action, TuningAction::NoOp) || d.red_line_protected)
        .collect();
    if actionable.is_empty() {
        out.push_str("  No tuning actions — all agents healthy (no rows above the 30% alert threshold).\n");
        return out;
    }
    for d in &actionable {
        let (verb, detail) = match &d.action {
            TuningAction::EscalateModelTier { from, to } => {
                ("escalate", format!("{from} → {to}"))
            }
            TuningAction::InjectFewShot { examples, .. } => {
                ("inject-few-shot", format!("+{examples} examples"))
            }
            TuningAction::DisableWithAlert { reason, .. } => {
                ("disable+alert", reason.clone())
            }
            TuningAction::NoOp => {
                if d.red_line_protected {
                    ("noop", "red-line protected (deterministic) — never tuned".to_string())
                } else {
                    ("noop", String::new())
                }
            }
        };
        let status = if d.needs_human_review() {
            "[needs human review]"
        } else if d.applied {
            "[applied]"
        } else if d.is_recommendation() {
            "[recommendation — not applied]"
        } else {
            ""
        };
        out.push_str(&format!(
            "  {:<24} {:<14} {:<16} {} {}\n",
            d.agent_id, d.model, verb, detail, status
        ));
    }
    out
}

#[cfg(test)]
mod tests;
