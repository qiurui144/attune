//! ACP-5 — chat-path autonomous-flow wiring (production assembly).
//!
//! Spec: `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §5.3b ③ (autonomous flow: `resolve_flow` → `GovernedStepRunner` → `run_flow`).
//!
//! This is the production装配点 that connects a real chat request to the flow
//! engine. The chat handler (`routes/chat.rs`) calls [`run_chat_flow`] BEFORE the
//! free-form RAG path; when the user message resolves to a **declared multi-step
//! flow** (e.g. `legal_defamation`), the flow runs end-to-end through the
//! [`GovernedStepRunner`] — each step scheduled (ACP-7) + cost-governed (ACP-4) +
//! telemetered (ACP-3) + threaded along the typed-handoff DAG (ACP-5 ④). The flow
//! outcome (status + per-step trace + final payload) is attached to the chat
//! response as an `acp_flow` block.
//!
//! Backward compatibility (spec §10, never regress chat):
//! - A message that resolves to a **single agent** (synthesized 1-step flow) or to
//!   **nothing** returns `None` — the chat handler then takes the unchanged
//!   free-form RAG path. Only a *declared multi-step composition* is the more
//!   specific intent worth running as an autonomous flow.
//! - The free-form RAG / grounding / citations / cost path is untouched; the flow
//!   block is purely additive.
//!
//! Graceful degradation (spec §7 / §11 R8, never cascade-fail):
//! - The flow executor already degrades (skip optional / partial / abort) around a
//!   failed/blocked/unregistered step. In an OSS install the law-pro agent binaries
//!   are absent, so the deterministic-dispatch closure returns an error → the flow
//!   degrades to a partial result rather than cascading. The chat answer is still
//!   produced by the normal RAG path; the `acp_flow` block reports the partial.
//! - A quota-exhausted cloud step degrades per the ACP-7 scheduler (local fallback
//!   for a `qwen3b`-floor agent, else `BlockedQuotaExhausted` → no silent
//!   quality-drop; the step is recorded blocked in the trace).

use serde::Serialize;

use attune_core::agents::flow::{
    resolve_flow, run_flow, FlowResult, FlowSet, FlowStatus, Payload,
};
use attune_core::agents::flow_runner::{DeterministicDispatch, GovernedStepRunner};
use attune_core::agents::registry::AgentRegistry;
use attune_core::agents::scheduler::{Entitlement, Scheduler};
use attune_core::cache::CacheBackend;
use attune_core::llm::{LlmCallOptions, LlmProvider};
use attune_core::usage::UsageAggregator;

/// One step's audit entry, shaped for the chat response (`acp_flow.steps[]`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatFlowStep {
    /// The step's agent id.
    pub agent_id: String,
    /// Did the runner actually execute this step (false = skipped/degraded)?
    pub ran: bool,
    /// Was the schedule a degraded local fallback (cloud quota exhausted)?
    pub degraded: bool,
    /// Scheduling decision / skip reason / failure detail (for the UI + audit).
    pub note: String,
}

/// The chat-facing flow outcome attached to the chat response as `acp_flow`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatFlowOutcome {
    /// The declared flow id that ran (e.g. `legal_defamation`).
    pub flow_id: String,
    /// Terminal status string: `complete` | `partial` | `aborted` | `degraded`.
    pub status: String,
    /// Per-step audit trace (ACP-5 guarantee ④ — every step leaves a trace).
    pub steps: Vec<ChatFlowStep>,
    /// The handoff type name of the final (possibly partial) payload.
    pub final_type: String,
    /// The final (possibly partial) structured payload.
    pub final_value: serde_json::Value,
}

fn status_str(s: &FlowStatus) -> &'static str {
    match s {
        FlowStatus::Complete => "complete",
        FlowStatus::Partial => "partial",
        FlowStatus::Aborted => "aborted",
        FlowStatus::Degraded => "degraded",
    }
}

impl ChatFlowOutcome {
    fn from_result(flow_id: &str, result: &FlowResult) -> Self {
        ChatFlowOutcome {
            flow_id: flow_id.to_string(),
            status: status_str(result.status()).to_string(),
            steps: result
                .trace()
                .iter()
                .map(|t| ChatFlowStep {
                    agent_id: t.agent_id.clone(),
                    ran: t.ran,
                    degraded: t.degraded,
                    note: t.note.clone(),
                })
                .collect(),
            final_type: result.payload().type_name().to_string(),
            final_value: result.payload().value().clone(),
        }
    }
}

/// Resolve a chat message to a flow and, when it is a **declared multi-step flow**,
/// run it end-to-end through the production [`GovernedStepRunner`].
///
/// Returns `Some(outcome)` when a declared multi-step flow ran (the chat handler
/// attaches it as `acp_flow`), or `None` when the message resolved to a single
/// agent / nothing (chat takes the unchanged free-form path — no regression).
///
/// Parameters mirror the chat handler's already-available state:
/// - `provider` — the live chat LLM (used for `llm-judge` steps via `governed_chat`).
/// - `cache` / `usage` — ACP-4 cache + ACP-3 telemetry (both optional / graceful).
/// - `entitlement` — ACP-7 routing (paid + remaining cloud quota + local fallback).
/// - `disabled` — agent ids ACP-3 has soft-disabled (skip/degrade, never dispatch).
/// - `deterministic` — dispatch for deterministic/rule/vlm steps. Production passes
///   a closure routing to the agent binary; in OSS (no binaries) it returns an
///   error → the flow degrades gracefully. Parameterized so tests inject a working
///   dispatch to exercise the full DAG.
#[allow(clippy::too_many_arguments)]
pub fn run_chat_flow(
    message: &str,
    flows: &FlowSet,
    registry: &AgentRegistry,
    provider: &dyn LlmProvider,
    cache: Option<&dyn CacheBackend>,
    usage: Option<&UsageAggregator>,
    entitlement: Entitlement,
    disabled: &std::collections::HashSet<String>,
    deterministic: &mut DeterministicDispatch<'_>,
) -> Option<ChatFlowOutcome> {
    let resolved = resolve_flow(message, flows, registry)?;
    // Backward compatibility: a single-agent (synthesized) or single-step flow is
    // NOT run as an autonomous flow — chat keeps its unchanged free-form path. Only
    // a declared multi-step composition is the more-specific intent worth running.
    if resolved.synthesized || resolved.flow.steps.len() < 2 {
        return None;
    }

    let scheduler = Scheduler::new(entitlement);
    // Free-form-equivalent options: no output cap (never truncate an agent's
    // structured output); per-agent CoT budget is threaded by the runner.
    let base_opts = LlmCallOptions::default();
    let mut runner =
        GovernedStepRunner::new(provider, cache, usage, base_opts, None, deterministic);

    // The flow's first step consumes the registered `consumes` type of step[0];
    // seed the autonomous flow with the raw user message under that type so the
    // first agent receives well-typed input (the executor threads outputs forward).
    let seed_type = registry
        .get(&resolved.flow.steps[0])
        .map(|a| a.handoff.consumes.clone())
        .unwrap_or_else(|| "RawCaseText".to_string());
    let seed = Payload::new(&seed_type, serde_json::json!({ "text": message }));

    let result = run_flow(
        &resolved.flow,
        registry,
        &scheduler,
        seed,
        disabled,
        &mut runner,
    );
    Some(ChatFlowOutcome::from_result(&resolved.id, &result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::agents::registry::AgentRegistry;
    use attune_core::llm::MockLlmProvider;
    use attune_core::store::Store;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    /// The legal_defamation-shaped registry: an LLM extractor (cloud) leading a
    /// deterministic damages agent (zero-cost), plus a routing keyword.
    fn defamation_registry() -> AgentRegistry {
        let toml = r#"
[[agent]]
id = "extractor"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "LLM 抽取事实"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "g"
route_keywords = ["名誉"]
route_priority = 9
[agent.handoff]
consumes = "RawCaseText"
produces = "Facts"

[[agent]]
id = "damages"
tier = "paid"
plugin = "law-pro"
kind = "deterministic"
capability_boundary = "确定性损害计算"
cost_class = "zero"
gate = "g2"
route_keywords = []
route_priority = 0
[agent.handoff]
consumes = "Facts"
produces = "Award"
"#;
        AgentRegistry::from_toml_str(toml).expect("registry")
    }

    fn defamation_flow() -> FlowSet {
        FlowSet::from_toml_str(
            r#"
[[flow]]
id = "legal_defamation"
route_keywords = ["名誉"]
route_priority = 9
steps = ["extractor", "damages"]
"#,
        )
        .expect("flows")
    }

    /// A single-agent registry — resolves to a synthesized 1-step flow.
    fn single_agent_registry() -> AgentRegistry {
        let toml = r#"
[[agent]]
id = "solo"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "单步"
cost_class = "zero"
gate = "g"
route_keywords = ["solo"]
route_priority = 5
[agent.handoff]
consumes = "X"
produces = "Y"
"#;
        AgentRegistry::from_toml_str(toml).expect("registry")
    }

    // ① Declared multi-step flow hit → really runs through GovernedStepRunner,
    //    completes, and the per-step trace records BOTH steps as having run.
    #[test]
    fn declared_flow_runs_end_to_end_with_trace() {
        let reg = defamation_registry();
        let flows = defamation_flow();
        flows.validate_against(&reg).unwrap();
        // Mock LLM returns a JSON object the next step consumes as `Facts`.
        let provider = MockLlmProvider::new("qwen2.5:3b");
        provider.push_response(r#"{"victim":"A"}"#);
        let mut dispatch =
            |_a: &attune_core::agents::registry::AgentSpec, _i: &Payload| {
                Ok(serde_json::json!({"award": 5000}))
            };
        let out = run_chat_flow(
            "名誉权纠纷",
            &flows,
            &reg,
            &provider,
            None,
            None,
            Entitlement::paid_with_quota(1000),
            &HashSet::new(),
            &mut dispatch,
        )
        .expect("declared multi-step flow must run");
        assert_eq!(out.flow_id, "legal_defamation");
        assert_eq!(out.status, "complete");
        assert_eq!(out.steps.len(), 2);
        assert!(out.steps.iter().all(|s| s.ran), "both steps must run: {out:?}");
        assert_eq!(out.final_type, "Award");
    }

    // ①-telemetry: a usage aggregator records the LLM extractor step.
    #[test]
    fn declared_flow_records_telemetry_for_llm_step() {
        let reg = defamation_registry();
        let flows = defamation_flow();
        let provider = MockLlmProvider::new("qwen2.5:3b");
        provider.push_response(r#"{"victim":"A"}"#);
        let store = Arc::new(Mutex::new(Store::open_memory().expect("memory store")));
        let usage = UsageAggregator::new(store, 50, 1000);
        let mut dispatch =
            |_a: &attune_core::agents::registry::AgentSpec, _i: &Payload| {
                Ok(serde_json::json!({"award": 5000}))
            };
        let out = run_chat_flow(
            "名誉权纠纷",
            &flows,
            &reg,
            &provider,
            None,
            Some(&usage),
            Entitlement::paid_with_quota(1000),
            &HashSet::new(),
            &mut dispatch,
        )
        .expect("flow must run");
        assert_eq!(out.status, "complete");
        let events = usage.recent(16);
        assert!(
            events.iter().any(|e| e.agent_id.as_deref() == Some("extractor")),
            "extractor LLM call must record telemetry; got {events:?}"
        );
    }

    // ② No regression: a single-agent (synthesized) resolution returns None so the
    //    chat handler keeps the unchanged free-form RAG path.
    #[test]
    fn single_agent_returns_none_for_freeform_fallback() {
        let reg = single_agent_registry();
        let flows = FlowSet::from_toml_str("").unwrap(); // no declared flows
        let provider = MockLlmProvider::new("qwen2.5:3b");
        let mut dispatch =
            |_a: &attune_core::agents::registry::AgentSpec, _i: &Payload| Ok(serde_json::json!({}));
        let out = run_chat_flow(
            "solo please",
            &flows,
            &reg,
            &provider,
            None,
            None,
            Entitlement::free_local(),
            &HashSet::new(),
            &mut dispatch,
        );
        assert!(out.is_none(), "single-agent intent must fall back to free-form chat");
    }

    // ② No regression: a non-matching message returns None (free-form chat path).
    #[test]
    fn no_match_returns_none() {
        let reg = defamation_registry();
        let flows = defamation_flow();
        let provider = MockLlmProvider::new("qwen2.5:3b");
        let mut dispatch =
            |_a: &attune_core::agents::registry::AgentSpec, _i: &Payload| Ok(serde_json::json!({}));
        let out = run_chat_flow(
            "今天天气怎么样",
            &flows,
            &reg,
            &provider,
            None,
            None,
            Entitlement::paid_with_quota(1000),
            &HashSet::new(),
            &mut dispatch,
        );
        assert!(out.is_none(), "irrelevant message must not trigger a flow");
    }

    // ③ Quota exhausted (free user, no paid plan) → the paid extractor is blocked
    //    by entitlement; the flow degrades (partial) rather than silently
    //    dropping quality. The trace records the block reason.
    #[test]
    fn quota_block_degrades_not_silent() {
        let reg = defamation_registry();
        let flows = defamation_flow();
        let provider = MockLlmProvider::new("qwen2.5:3b");
        let mut dispatch =
            |_a: &attune_core::agents::registry::AgentSpec, _i: &Payload| {
                Ok(serde_json::json!({"award": 5000}))
            };
        // Free user: paid extractor (tier=paid) is blocked by entitlement.
        let out = run_chat_flow(
            "名誉权纠纷",
            &flows,
            &reg,
            &provider,
            None,
            None,
            Entitlement::free_local(),
            &HashSet::new(),
            &mut dispatch,
        )
        .expect("flow resolves even if a step blocks");
        // The blocked non-optional first step degrades the flow (not silent).
        assert_ne!(out.status, "complete", "a blocked paid step must not complete silently");
        assert!(
            out.steps.iter().any(|s| !s.ran && s.note.to_lowercase().contains("paid")),
            "the block reason must be recorded in the trace: {out:?}"
        );
    }

    // ④ Graceful degrade, never panic: in an OSS-style install the deterministic
    //    dispatch errors (no agent binary). A flow whose LLM lead succeeds but
    //    whose deterministic step errors degrades to partial — no panic, no cascade.
    #[test]
    fn deterministic_dispatch_error_degrades_no_panic() {
        let reg = defamation_registry();
        let flows = defamation_flow();
        let provider = MockLlmProvider::new("qwen2.5:3b");
        provider.push_response(r#"{"victim":"A"}"#);
        let mut dispatch =
            |_a: &attune_core::agents::registry::AgentSpec, _i: &Payload| {
                Err("no agent binary in OSS install".to_string())
            };
        let out = run_chat_flow(
            "名誉权纠纷",
            &flows,
            &reg,
            &provider,
            None,
            None,
            Entitlement::paid_with_quota(1000),
            &HashSet::new(),
            &mut dispatch,
        )
        .expect("flow must run");
        assert_ne!(out.status, "complete");
        // extractor (LLM) ran; damages (deterministic) failed → recorded, not panicked.
        assert!(out.steps.iter().any(|s| s.agent_id == "extractor" && s.ran));
        assert!(out.steps.iter().any(|s| s.agent_id == "damages" && !s.ran));
    }
}
