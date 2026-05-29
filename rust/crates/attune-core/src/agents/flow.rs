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

/// The flow an intent resolved to — either a declared multi-step flow or a
/// **synthesized 1-step flow** from a single matching agent (backward
/// compatibility with single-agent routing, spec §5.3b / §10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFlow {
    /// The flow id (the declared id, or the agent id for a synthesized flow).
    pub id: String,
    /// The (declared or synthesized) flow definition to execute.
    pub flow: FlowDef,
    /// True when this flow was synthesized from a single agent (not declared in
    /// `agent_flows.toml`).
    pub synthesized: bool,
}

/// Resolve a user intent to a flow (spec §5.3b — intent routing extended to flow
/// routing, backward compatible). Resolution order:
///
///   1. **Declared flows** — the highest-priority `agent_flows.toml` flow whose
///      `route_keywords` the message matches.
///   2. **Single-agent fallback** — if no declared flow matches, the
///      highest-priority *agent* (from the registry's `route_keywords`) whose
///      keywords match, wrapped as a synthesized 1-step flow (`steps=[agent]`).
///
/// A declared flow always wins over a single agent when both match (a declared
/// multi-step composition is the more specific intent). Ties broken by id for
/// determinism. Returns `None` when nothing matches.
pub fn resolve_flow(
    message: &str,
    flows: &FlowSet,
    registry: &AgentRegistry,
) -> Option<ResolvedFlow> {
    // 1. Declared flow.
    if let Some(f) = flows.route(message) {
        return Some(ResolvedFlow {
            id: f.id.clone(),
            flow: f.clone(),
            synthesized: false,
        });
    }
    // 2. Single-agent fallback → synthesized 1-step flow.
    let best = registry
        .agents()
        .iter()
        .filter(|a| a.route_keywords.iter().any(|k| message.contains(k.as_str())))
        .max_by(|x, y| {
            x.route_priority
                .cmp(&y.route_priority)
                .then_with(|| y.id.cmp(&x.id))
        })?;
    Some(ResolvedFlow {
        id: best.id.clone(),
        flow: FlowDef {
            id: best.id.clone(),
            route_keywords: best.route_keywords.clone(),
            route_priority: best.route_priority,
            steps: vec![best.id.clone()],
            degrade: Degrade::default(),
        },
        synthesized: true,
    })
}

// ══════════════════════════════════════════════════════════════════════════
// Flow Executor (autonomous flow engine, spec §5.3b ③)
// ══════════════════════════════════════════════════════════════════════════

use std::collections::HashSet;

use super::registry::AgentSpec;
use super::scheduler::{ScheduleDecision, Scheduler};

/// A typed work item flowing between agents. Carries the handoff `type_name`
/// (so we can verify / record the type at each hop) plus an opaque JSON value
/// (the agent's structured output). The flow executor threads one step's
/// `Payload` into the next step's input — this is the "autonomous flow":
/// `payload = step(payload)` along the declared DAG (spec §5.3b ③).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    type_name: String,
    value: serde_json::Value,
}

impl Payload {
    /// Construct a payload with a handoff type name + structured value.
    pub fn new(type_name: &str, value: serde_json::Value) -> Self {
        Payload {
            type_name: type_name.to_string(),
            value,
        }
    }

    /// The handoff type name (matches a registry `consumes` / `produces`).
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    /// The structured value.
    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }
}

/// Why a single flow step failed inside the [`StepRunner`] (distinct from a
/// scheduler block, which the executor handles before ever calling the runner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepFailKind {
    /// The agent's computation returned an error.
    AgentError,
    /// The agent's output failed the typed-handoff check at runtime.
    HandoffMismatch,
}

/// A failure surfaced by a [`StepRunner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepError {
    /// The failure category.
    pub kind: StepFailKind,
    /// Human-readable detail (telemetry / trace).
    pub message: String,
}

/// Runs the actual computation for one already-scheduled step. The flow executor
/// owns orchestration (registry lookup, disabled check, scheduling, payload
/// threading, degrade policy, trace); the runner owns the agent invocation +
/// ACP-4 cost governance + ACP-3 telemetry for that one call. This split keeps
/// the executor's control-flow logic pure-testable while production wires the
/// real governor / agent dispatch.
pub trait StepRunner {
    /// Run `agent` (already scheduled to `decision`) on `input`, returning its
    /// typed output payload or a [`StepError`].
    fn run(
        &mut self,
        agent: &AgentSpec,
        decision: &ScheduleDecision,
        input: &Payload,
    ) -> Result<Payload, StepError>;
}

/// One audit entry per flow step (auditability guarantee, spec §5.3b ④).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepTrace {
    /// The step's agent id.
    pub agent_id: String,
    /// Did the runner actually execute this step? (`false` = skipped: disabled /
    /// scheduler-blocked / unregistered / optional-failure).
    pub ran: bool,
    /// Was the schedule decision a degraded local fallback (quota exhausted)?
    pub degraded: bool,
    /// Free-text note (scheduling decision / skip reason / failure detail).
    pub note: String,
}

/// The terminal disposition of a flow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowStatus {
    /// Every (non-skipped) step ran and the final payload is the last step's.
    Complete,
    /// A non-optional step failed/blocked under `on_step_fail = partial` — the
    /// payload is the last good output, the flow did not run further (no cascade).
    Partial,
    /// A non-optional step failed under `on_step_fail = abort`.
    Aborted,
    /// A non-optional step was disabled/blocked and could not proceed; the flow
    /// degraded to the last good payload (graceful, never cascade).
    Degraded,
}

/// The result of running a flow: status + the (possibly partial) final payload +
/// a per-step audit trace.
#[derive(Debug, Clone)]
pub struct FlowResult {
    status: FlowStatus,
    payload: Payload,
    trace: Vec<StepTrace>,
}

impl FlowResult {
    /// The terminal status.
    pub fn status(&self) -> &FlowStatus {
        &self.status
    }
    /// The final (possibly partial) payload.
    pub fn payload(&self) -> &Payload {
        &self.payload
    }
    /// The per-step audit trace.
    pub fn trace(&self) -> &[StepTrace] {
        &self.trace
    }
    /// Did the flow complete fully?
    pub fn is_complete(&self) -> bool {
        self.status == FlowStatus::Complete
    }
    /// Did the flow stop early with a partial result (graceful)?
    pub fn is_partial(&self) -> bool {
        self.status == FlowStatus::Partial
    }
    /// Was the flow aborted by an `abort` policy?
    pub fn is_aborted(&self) -> bool {
        self.status == FlowStatus::Aborted
    }
    /// Did the flow degrade around a disabled/blocked non-optional step?
    pub fn is_degraded(&self) -> bool {
        self.status == FlowStatus::Degraded
    }
}

/// **The autonomous-flow engine** (spec §5.3b ③). Walks a declared flow DAG,
/// threading each step's typed output into the next step's input. Each step:
///
///   1. **ACP-1** — look the agent up in the registry. An unregistered (shadow)
///      step degrades like a failure (R8: never panic).
///   2. **ACP-3** — if the agent is disabled (`disabled` set), skip it (optional)
///      or degrade (non-optional) — never dispatch a disabled agent.
///   3. **ACP-7** — schedule the call (`scheduler.route`). A scheduler block
///      (entitlement / quota) is treated like step unavailability: skip if
///      optional, else degrade per policy.
///   4. **ACP-4 + ACP-3** — hand the scheduled step to the [`StepRunner`] (which
///      wires cost governance + telemetry); thread its output forward.
///
/// The 4 guarantees hold: ① typed handoff was validated at load
/// ([`FlowSet::validate_against`]); ② every hop is scheduled + run through the
/// governable runner; ③ any unavailability degrades (skip optional / partial /
/// abort) and **never cascades** (§11 R8); ④ every step leaves a [`StepTrace`].
///
/// `disabled` is the set of agent ids ACP-3's FeedbackController has soft-disabled
/// (the caller computes it from `FeedbackController::decide`).
pub fn run_flow(
    flow: &FlowDef,
    registry: &AgentRegistry,
    scheduler: &Scheduler,
    input: Payload,
    disabled: &HashSet<String>,
    runner: &mut dyn StepRunner,
) -> FlowResult {
    let mut payload = input;
    let mut trace: Vec<StepTrace> = Vec::with_capacity(flow.steps.len());

    for step in &flow.steps {
        let optional = flow.is_optional(step);

        // ① ACP-1 registry lookup — a shadow step degrades, never panics (R8).
        let Some(agent) = registry.get(step) else {
            trace.push(StepTrace {
                agent_id: step.clone(),
                ran: false,
                degraded: false,
                note: "unregistered (shadow) agent — skipped".to_string(),
            });
            if optional {
                continue;
            }
            return finish_non_optional(flow, payload, trace, step, "unregistered agent");
        };

        // ② ACP-3 disabled gate.
        let disabled_reason = if disabled.contains(step) {
            Some("soft-disabled by ACP-3 (weak-model F1 below floor)")
        } else {
            None
        };

        // ③ ACP-7 schedule.
        let decision = scheduler.route(agent, disabled_reason);
        if decision.is_blocked() {
            let note = blocked_note(&decision);
            trace.push(StepTrace {
                agent_id: step.clone(),
                ran: false,
                degraded: false,
                note: note.clone(),
            });
            if optional {
                continue;
            }
            return finish_non_optional(flow, payload, trace, step, &note);
        }

        // ④ ACP-4 + ACP-3 — run the scheduled step through the governable runner.
        let degraded = matches!(
            decision,
            ScheduleDecision::Local { degraded_from_cloud: true, .. }
        );
        match runner.run(agent, &decision, &payload) {
            Ok(out) => {
                trace.push(StepTrace {
                    agent_id: step.clone(),
                    ran: true,
                    degraded,
                    note: schedule_note(&decision),
                });
                payload = out; // autonomous flow: upstream output → downstream input
            }
            Err(e) => {
                trace.push(StepTrace {
                    agent_id: step.clone(),
                    ran: false,
                    degraded,
                    note: format!("step failed: {} ({:?})", e.message, e.kind),
                });
                if optional {
                    continue; // graceful: skip optional step failure, no cascade
                }
                return finish_non_optional(flow, payload, trace, step, &e.message);
            }
        }
    }

    FlowResult {
        status: FlowStatus::Complete,
        payload,
        trace,
    }
}

/// Resolve a non-optional step that could not proceed (failure / block /
/// unavailability) per the flow's `on_step_fail` policy. Never cascade-fails:
/// `partial` returns the last good payload, `abort` marks aborted, and
/// `fallback_agent`/disabled paths degrade. The trailing skipped steps are NOT
/// recorded as runs (the flow stopped here).
fn finish_non_optional(
    flow: &FlowDef,
    payload: Payload,
    trace: Vec<StepTrace>,
    _step: &str,
    _reason: &str,
) -> FlowResult {
    let status = match flow.degrade.on_step_fail {
        OnStepFail::Abort => FlowStatus::Aborted,
        // `fallback_agent` without a wired alternate runner degrades like partial
        // here (the executor has no second runner to dispatch to); the policy is
        // honored structurally — a real fallback runner would be threaded by the
        // caller. Either way: never cascade.
        OnStepFail::Partial | OnStepFail::FallbackAgent => FlowStatus::Partial,
    };
    // A disabled/blocked non-optional step that the scheduler refused is a
    // graceful degrade rather than an agent failure; the distinction is in the
    // trace note. We keep the status as Partial/Aborted from the policy, but if
    // the policy is partial AND the last trace entry was a *block* (not a run
    // failure), surface Degraded to make the disposition explicit.
    let last_was_block = trace
        .last()
        .map(|t| !t.ran && (t.note.contains("blocked") || t.note.contains("disabled") || t.note.contains("quota") || t.note.contains("entitlement")))
        .unwrap_or(false);
    let status = if status == FlowStatus::Partial && last_was_block {
        FlowStatus::Degraded
    } else {
        status
    };
    FlowResult {
        status,
        payload,
        trace,
    }
}

/// A short note describing a runnable schedule decision (trace).
fn schedule_note(decision: &ScheduleDecision) -> String {
    match decision {
        ScheduleDecision::Cloud => "scheduled: cloud".to_string(),
        ScheduleDecision::Local {
            degraded_from_cloud: true,
            ..
        } => "scheduled: local (degraded from cloud — quota exhausted)".to_string(),
        ScheduleDecision::Local { model, .. } => {
            format!("scheduled: local (model={})", model.as_deref().unwrap_or("default"))
        }
        other => format!("scheduled: {other:?}"),
    }
}

/// A short note describing a blocked schedule decision (trace).
fn blocked_note(decision: &ScheduleDecision) -> String {
    match decision {
        ScheduleDecision::BlockedEntitlement { reason } => {
            format!("blocked (entitlement): {reason}")
        }
        ScheduleDecision::BlockedQuotaExhausted { reason } => {
            format!("blocked (quota): {reason}")
        }
        ScheduleDecision::BlockedDisabled { reason } => {
            format!("blocked (disabled): {reason}")
        }
        runnable => format!("unexpected runnable in block path: {runnable:?}"),
    }
}

#[cfg(test)]
mod tests;
