//! ACP-5 — production [`StepRunner`] bridging the flow executor to the live
//! ACP-4 cost governor + ACP-3 telemetry.
//!
//! Per spec `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §5.3b ③ (each step: scheduler + governor.governed_call + telemetry.record).
//!
//! The flow executor ([`super::flow::run_flow`]) owns orchestration over the
//! abstract [`StepRunner`] trait; this is the concrete runner production wires:
//!
//! - **LLM-judge agents** (`Kind::LlmJudge`) → [`crate::governor::governed_chat`],
//!   which folds in the A1 cache + output cap + CoT budget + usage telemetry
//!   (ACP-4 + ACP-3). The runner builds the chat messages from the incoming
//!   [`Payload`] and tags the usage record with the agent id.
//! - **deterministic / rule / vlm agents** → delegated to a caller-supplied
//!   dispatch closure (the agent's own binary computes; §2.3 — the runner never
//!   reimplements an agent's computation, it only routes).
//!
//! Graceful degrade (§11 R8): a governor error becomes a [`StepError`] (the
//! executor then skips an optional step / returns partial — never cascades). The
//! governor itself already degrades around a missing cache / failed telemetry.

use super::flow::{Payload, StepError, StepFailKind, StepRunner};
use super::registry::{AgentSpec, Kind};
use super::scheduler::ScheduleDecision;
use crate::cache::CacheBackend;
use crate::llm::{ChatMessage, LlmCallOptions, LlmProvider};
use crate::usage::UsageAggregator;

/// Computes a deterministic / rule / vlm agent's output. Production passes a
/// closure that dispatches to the agent's binary (or in-process `Agent` impl);
/// the runner only routes — it never computes (§2.3). Returns the typed output
/// JSON value for the agent's declared `produces` type.
pub type DeterministicDispatch<'a> =
    dyn FnMut(&AgentSpec, &Payload) -> Result<serde_json::Value, String> + 'a;

/// Production [`StepRunner`]: routes each step to the cost governor (LLM agents)
/// or the deterministic dispatch closure, wiring ACP-4 + ACP-3 for the LLM path.
pub struct GovernedStepRunner<'a> {
    provider: &'a dyn LlmProvider,
    cache: Option<&'a dyn CacheBackend>,
    usage: Option<&'a UsageAggregator>,
    base_opts: LlmCallOptions,
    ttl_secs: Option<u32>,
    deterministic: &'a mut DeterministicDispatch<'a>,
}

impl<'a> GovernedStepRunner<'a> {
    /// Construct with the LLM provider, optional cache + usage aggregator (ACP-4),
    /// the base call options (output cap / CoT budget are threaded per agent), the
    /// cache TTL, and the deterministic-dispatch closure.
    pub fn new(
        provider: &'a dyn LlmProvider,
        cache: Option<&'a dyn CacheBackend>,
        usage: Option<&'a UsageAggregator>,
        base_opts: LlmCallOptions,
        ttl_secs: Option<u32>,
        deterministic: &'a mut DeterministicDispatch<'a>,
    ) -> Self {
        GovernedStepRunner {
            provider,
            cache,
            usage,
            base_opts,
            ttl_secs,
            deterministic,
        }
    }

    /// Build the chat messages for an LLM step from the incoming payload. The
    /// system message names the agent's single-point boundary (so the prompt is
    /// scoped); the user message carries the upstream typed payload as JSON.
    fn build_messages(agent: &AgentSpec, input: &Payload) -> Vec<ChatMessage> {
        vec![
            ChatMessage::system(&format!(
                "You are the {} agent. Capability: {}. Input type: {}. Output type: {}.",
                agent.id, agent.capability_boundary, input.type_name(), agent.handoff.produces
            )),
            ChatMessage::user(&input.value().to_string()),
        ]
    }
}

impl StepRunner for GovernedStepRunner<'_> {
    fn run(
        &mut self,
        agent: &AgentSpec,
        _decision: &ScheduleDecision,
        input: &Payload,
    ) -> Result<Payload, StepError> {
        match agent.kind {
            Kind::LlmJudge => {
                let messages = Self::build_messages(agent, input);
                // ACP-4: cost governance (cache + cap + CoT budget) + ACP-3
                // telemetry are all inside governed_chat.
                match crate::governor::governed_chat(
                    self.provider,
                    &messages,
                    &self.base_opts,
                    self.cache,
                    self.usage,
                    Some(&agent.id),
                    self.ttl_secs,
                ) {
                    Ok(resp) => {
                        // The governed text is the agent's structured output; wrap
                        // it under the agent's declared `produces` handoff type so
                        // the next step's typed input is well-formed.
                        let value = serde_json::from_str::<serde_json::Value>(&resp.text)
                            .unwrap_or(serde_json::Value::String(resp.text));
                        Ok(Payload::new(&agent.handoff.produces, value))
                    }
                    Err(e) => Err(StepError {
                        kind: StepFailKind::AgentError,
                        message: format!("governed LLM call failed for {}: {e}", agent.id),
                    }),
                }
            }
            // Deterministic / rule / vlm: route to the caller's dispatch closure.
            // The runner never computes the agent's result (§2.3).
            Kind::Deterministic | Kind::Rule | Kind::Vlm => {
                match (self.deterministic)(agent, input) {
                    Ok(value) => Ok(Payload::new(&agent.handoff.produces, value)),
                    Err(msg) => Err(StepError {
                        kind: StepFailKind::AgentError,
                        message: format!("deterministic dispatch failed for {}: {msg}", agent.id),
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
