//! ACP-5 — integration test for the chat-path autonomous-flow wire.
//!
//! S4b (2026-06-03): OSS agent_flows.toml is intentionally empty — industry flows
//! (legal_defamation etc.) moved to attune-pro/plugins/law-pro/. Tests now verify
//! the graceful-degrade path: empty FlowSet → run_chat_flow returns None for all
//! messages (chat handler falls back to free-form RAG). The CountingProvider
//! is retained to verify no spurious LLM calls are made on the empty-flows path.
//!
//! Spec: docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md §5.3b / §9.
//! S4b: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-1.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use attune_core::agents::flow::Payload;
use attune_core::agents::registry::AgentSpec;
use attune_core::agents::scheduler::Entitlement;
use attune_core::llm::LlmProvider;
use attune_core::usage::TokenUsage;

use attune_server::acp_chat::run_chat_flow;

/// Counts upstream calls so we can prove the LLM lead step actually ran.
struct CountingProvider {
    calls: AtomicUsize,
    model: String,
}
impl LlmProvider for CountingProvider {
    fn chat(&self, _s: &str, _u: &str) -> attune_core::error::Result<(String, TokenUsage)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((
            r#"{"victim":"A","statement":"defamatory","severity":"high"}"#.to_string(),
            TokenUsage {
                tokens_in: 64,
                tokens_out: 24,
                cached_in: 0,
                model: self.model.clone(),
                provider: "ollama".into(),
            },
        ))
    }
    fn is_available(&self) -> bool {
        true
    }
    fn model_name(&self) -> &str {
        &self.model
    }
    fn is_local(&self) -> bool {
        true
    }
}

/// S4b: The chat route loads `agents.registry.toml` + `agent_flows.toml` at
/// startup (AppState::new). In OSS, agent_flows.toml is intentionally empty —
/// industry flows (legal_defamation) live in attune-pro. Loading must succeed
/// (not Err/panic) and return an empty FlowSet; the OSS registry has 6 agents.
#[test]
fn workspace_flows_present_for_chat_wire() {
    let loaded = attune_core::agents::load_workspace_flows(
        "agents.registry.toml",
        "agent_flows.toml",
    );
    let (flows, reg) = loaded.expect("workspace flows must load (tests run in the crate dir)");
    // S4b: OSS registry still has oss-core agents.
    assert!(!reg.is_empty(), "S4b: OSS registry must have oss-core agents");
    // S4b: legal_defamation moved to attune-pro — OSS flow set is empty.
    assert!(
        flows.get("legal_defamation").is_none(),
        "S4b: legal_defamation must not be in OSS flow set (moved to attune-pro)"
    );
    assert!(
        flows.is_empty(),
        "S4b: OSS agent_flows.toml is intentionally empty (industry flows in attune-pro)"
    );
}

/// S4b: legal_defamation flow moved to attune-pro — OSS has no declared flows.
/// A defamation message must fall back to free-form RAG (None), not attempt to
/// route to a flow that no longer exists in the OSS flow set. This verifies the
/// graceful-degrade path: empty FlowSet → run_chat_flow returns None (§7/§11 R8).
#[test]
fn defamation_message_falls_back_to_freeform_in_oss() {
    let (flows, reg) = attune_core::agents::load_workspace_flows(
        "agents.registry.toml",
        "agent_flows.toml",
    )
    .expect("workspace flows");

    // S4b: OSS flow set must be empty.
    assert!(flows.is_empty(), "S4b: OSS has no declared flows");

    let provider = CountingProvider {
        calls: AtomicUsize::new(0),
        model: "qwen2.5:3b".into(),
    };
    let mut dispatch = |_a: &AgentSpec, _i: &Payload| -> std::result::Result<serde_json::Value, String> {
        Ok(serde_json::json!({}))
    };

    // "名誉权" cannot route to legal_defamation (absent in OSS) → must fall back to None.
    let out = run_chat_flow(
        "我的名誉权被侵害了，对方公开诽谤我",
        &flows,
        &reg,
        &provider,
        None,
        None,
        Entitlement::paid_with_quota(1_000_000),
        &HashSet::new(),
        &mut dispatch,
    );

    // S4b graceful degrade: no flow matched → None (chat handler uses free-form RAG path).
    assert!(
        out.is_none(),
        "S4b: defamation message must fall back to free-form RAG in OSS (no industry flows)"
    );
    // No LLM calls made — there was no flow step to execute.
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        0,
        "S4b: no LLM call when no flow matched"
    );
}

/// No regression: an irrelevant message resolves to no flow (None) so the chat
/// handler keeps the unchanged free-form RAG path.
#[test]
fn irrelevant_message_falls_back_to_freeform() {
    let (flows, reg) = attune_core::agents::load_workspace_flows(
        "agents.registry.toml",
        "agent_flows.toml",
    )
    .expect("workspace flows");
    let provider = CountingProvider {
        calls: AtomicUsize::new(0),
        model: "qwen2.5:3b".into(),
    };
    let mut dispatch =
        |_a: &AgentSpec, _i: &Payload| -> std::result::Result<serde_json::Value, String> {
            Ok(serde_json::json!({}))
        };
    let out = run_chat_flow(
        "请帮我总结一下今天的天气",
        &flows,
        &reg,
        &provider,
        None,
        None,
        Entitlement::free_local(),
        &HashSet::new(),
        &mut dispatch,
    );
    assert!(out.is_none(), "non-agent chat must not trigger a flow");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0, "no LLM call for plain chat");
}
