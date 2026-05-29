//! ACP-5 production runner tests — the GovernedStepRunner bridges the flow
//! executor to the live governor (cache/cap/telemetry) for LLM agents and to a
//! dispatch closure for deterministic agents. Integration test (§9 ACP-5 row):
//! the legal_defamation-shaped chain runs end-to-end through the runner.

use super::*;
use crate::agents::flow::{run_flow, FlowSet, Payload};
use crate::agents::registry::AgentRegistry;
use crate::agents::scheduler::{Entitlement, Scheduler};
use crate::llm::{LlmCallOptions, MockLlmProvider};
use crate::store::Store;
use crate::usage::UsageAggregator;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// A registry with one LLM extractor → one deterministic damages agent (the
/// defamation dedupe shape: extractor leads, det calc follows).
fn defamation_chain_registry() -> AgentRegistry {
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
[agent.handoff]
consumes = "Facts"
produces = "Award"
"#;
    AgentRegistry::from_toml_str(toml).expect("registry")
}

#[test]
fn governed_runner_runs_llm_then_deterministic_end_to_end() {
    let reg = defamation_chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["extractor", "damages"]
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();

    // Mock LLM returns a JSON Facts payload for the extractor step.
    let provider = MockLlmProvider::new("qwen2.5:3b");
    provider.push_response(r#"{"victim":"A","statement":"defamatory"}"#);
    let store = Arc::new(Mutex::new(Store::open_memory().expect("memory store")));
    let usage = UsageAggregator::new(store, 50, 1000);

    // Deterministic closure computes the damages from the extracted Facts. It is
    // the agent's own computation (the runner never reimplements it).
    let mut dispatch = |_agent: &crate::agents::registry::AgentSpec, input: &Payload| {
        // The damages step receives the Facts payload the extractor produced.
        assert_eq!(input.type_name(), "Facts");
        Ok(serde_json::json!({"material": 5000, "emotional": 3000}))
    };

    let mut runner = GovernedStepRunner::new(
        &provider,
        None,
        Some(&usage),
        LlmCallOptions::default(),
        None,
        &mut dispatch,
    );

    let sched = Scheduler::new(Entitlement::paid_with_quota(1000));
    let result = run_flow(
        flow,
        &reg,
        &sched,
        Payload::new("RawCaseText", serde_json::json!({"text": "他诽谤我"})),
        &HashSet::new(),
        &mut runner,
    );

    assert!(result.is_complete(), "flow should complete: {result:?}");
    // Autonomous flow: extractor's Facts flowed into damages, final type = Award.
    assert_eq!(result.payload().type_name(), "Award");
    assert_eq!(result.payload().value()["material"], 5000);
    // ACP-3: the LLM step recorded a usage event tagged with the agent id.
    let events = usage.recent(16);
    assert!(
        events.iter().any(|e| e.agent_id.as_deref() == Some("extractor")),
        "extractor LLM call must record telemetry; got {events:?}"
    );
}

#[test]
fn governed_runner_llm_failure_surfaces_step_error() {
    let reg = defamation_chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["extractor", "damages"]
[flow.degrade]
on_step_fail = "partial"
"#,
    )
    .unwrap();
    let flow = flows.get("f").unwrap();

    // Mock with NO pushed response → the extractor LLM call errors. The flow must
    // degrade (partial), never cascade-panic.
    let provider = MockLlmProvider::new("qwen2.5:3b");
    let mut dispatch = |_a: &crate::agents::registry::AgentSpec, _i: &Payload| {
        Ok(serde_json::json!({}))
    };
    let mut runner = GovernedStepRunner::new(
        &provider,
        None,
        None,
        LlmCallOptions::default(),
        None,
        &mut dispatch,
    );
    let sched = Scheduler::new(Entitlement::paid_with_quota(1000));
    let result = run_flow(
        flow,
        &reg,
        &sched,
        Payload::new("RawCaseText", serde_json::json!({})),
        &HashSet::new(),
        &mut runner,
    );
    // extractor (non-optional) failed under partial → Partial, damages not run.
    assert!(result.is_partial(), "expected graceful partial: {result:?}");
}

#[test]
fn governed_runner_deterministic_dispatch_error_surfaces() {
    let reg = defamation_chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["damages"]
"#,
    )
    .unwrap();
    let flow = flows.get("f").unwrap();
    let provider = MockLlmProvider::new("qwen2.5:3b");
    let mut dispatch = |_a: &crate::agents::registry::AgentSpec, _i: &Payload| {
        Err("binary crashed".to_string())
    };
    let mut runner = GovernedStepRunner::new(
        &provider,
        None,
        None,
        LlmCallOptions::default(),
        None,
        &mut dispatch,
    );
    let sched = Scheduler::new(Entitlement::paid_with_quota(1000));
    let result = run_flow(
        flow,
        &reg,
        &sched,
        Payload::new("Facts", serde_json::json!({})),
        &HashSet::new(),
        &mut runner,
    );
    // Single non-optional deterministic step failed → partial (default), no panic.
    assert!(result.is_partial() || !result.is_complete());
}
