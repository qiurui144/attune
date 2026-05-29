//! ACP-5 — integration test for the chat-path autonomous-flow wire.
//!
//! Verifies the exact component graph the chat route assembles
//! (`state.agent_flows` → `acp_chat::run_chat_flow` with real provider + cache +
//! usage + entitlement): a message routing to the workspace `legal_defamation`
//! declared multi-step flow runs end-to-end through the GovernedStepRunner, the
//! per-step trace is produced, and the LLM lead step writes telemetry. A
//! non-matching message takes the free-form path (None) — no regression.
//!
//! Spec: docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md §5.3b / §9.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use attune_core::agents::flow::Payload;
use attune_core::agents::registry::AgentSpec;
use attune_core::agents::scheduler::Entitlement;
use attune_core::cache::memory::MemoryLruCache;
use attune_core::cache::CacheBackend;
use attune_core::llm::LlmProvider;
use attune_core::store::Store;
use attune_core::usage::{TokenUsage, UsageAggregator};

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

/// The chat route loads `agents.registry.toml` + `agent_flows.toml` at startup
/// (AppState::new). Loading them here the same way must surface the canonical
/// legal_defamation flow — i.e. the wire has a real flow to route to.
#[test]
fn workspace_flows_present_for_chat_wire() {
    let loaded = attune_core::agents::load_workspace_flows(
        "agents.registry.toml",
        "agent_flows.toml",
    );
    let (flows, _reg) = loaded.expect("workspace flows must load (tests run in the crate dir)");
    assert!(
        flows.get("legal_defamation").is_some(),
        "the legal_defamation flow the chat wire routes to must exist"
    );
}

/// The exact graph the chat handler assembles: a defamation message → declared
/// multi-step flow runs through the GovernedStepRunner; the LLM lead step makes
/// the upstream call and writes a telemetry row.
#[test]
fn defamation_message_runs_flow_through_governed_runner() {
    let (flows, reg) = attune_core::agents::load_workspace_flows(
        "agents.registry.toml",
        "agent_flows.toml",
    )
    .expect("workspace flows");

    let provider = CountingProvider {
        calls: AtomicUsize::new(0),
        model: "qwen2.5:3b".into(),
    };
    let cache: Arc<dyn CacheBackend> = Arc::new(MemoryLruCache::new(512));
    let store = Arc::new(Mutex::new(Store::open_memory().expect("store")));
    let agg = UsageAggregator::new(store.clone(), 100, 1000);

    // Server has no embedded agent binaries → deterministic steps degrade.
    let mut dispatch = |_a: &AgentSpec, _i: &Payload| -> std::result::Result<serde_json::Value, String> {
        Err("no agent binary in this process".to_string())
    };

    // "名誉权" routes to legal_defamation (route_keywords include 名誉权).
    let out = run_chat_flow(
        "我的名誉权被侵害了，对方公开诽谤我",
        &flows,
        &reg,
        &provider,
        Some(cache.as_ref()),
        Some(&agg),
        Entitlement::paid_with_quota(1_000_000),
        &HashSet::new(),
        &mut dispatch,
    )
    .expect("a defamation message must resolve to the declared multi-step flow");

    assert_eq!(out.flow_id, "legal_defamation");
    // The flow has an optional fact_extractor lead, then defamation_extractor
    // (LLM) → defamation_agent (deterministic, degrades). At least one LLM step
    // must have run (upstream call made).
    assert!(
        provider.calls.load(Ordering::SeqCst) >= 1,
        "an LLM step must have made an upstream call"
    );
    assert!(!out.steps.is_empty(), "every step leaves a trace (ACP-5 ④)");
    // Telemetry recorded for the LLM step(s).
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    rt.block_on(agg.flush_now());
    let summary = store.lock().unwrap().usage_summary(0, i64::MAX).expect("summary");
    assert!(summary.events >= 1, "LLM flow step must record telemetry");
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
