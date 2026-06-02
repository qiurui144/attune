//! Tests for ACP-5 autonomous flow — DAG parsing, typed-handoff validation,
//! cycle detection, flow routing, and (Task 2) the flow executor + 4 guarantees.

use super::*;
use crate::agents::registry::AgentRegistry;
use crate::agents::scheduler::{Entitlement, ScheduleDecision, Scheduler};
use std::collections::HashSet;

/// A minimal registry that connects fact → defamation → citation, plus a
/// dangling agent whose handoff types do not connect.
fn test_registry() -> AgentRegistry {
    let toml = r#"
[[agent]]
id = "fact_extractor"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "raw → facts"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "law-pro/g::fact"
[agent.handoff]
consumes = "RawCaseText"
produces = "CaseFacts"

[[agent]]
id = "defamation_judge"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "facts → verdict"
model_tier_floor = "gpt-4o-mini"
cost_class = "cloud"
gate = "law-pro/g::defamation"
[agent.handoff]
consumes = "CaseFacts"
produces = "DefamationVerdict"

[[agent]]
id = "citation_linker"
tier = "paid"
plugin = "law-pro"
kind = "deterministic"
capability_boundary = "verdict → cited"
cost_class = "zero"
gate = "law-pro/g::cite"
[agent.handoff]
consumes = "DefamationVerdict"
produces = "CitedVerdict"

[[agent]]
id = "unrelated_agent"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "x → y (does not connect)"
cost_class = "zero"
gate = "oss/g"
[agent.handoff]
consumes = "Apples"
produces = "Oranges"
"#;
    AgentRegistry::from_toml_str(toml).expect("registry parses")
}

// ── Task 1: DAG parsing ───────────────────────────────────────────────────

#[test]
fn parse_single_flow_with_steps() {
    let toml = r#"
[[flow]]
id = "legal_defamation"
route_keywords = ["名誉", "诽谤"]
route_priority = 9
steps = ["fact_extractor", "defamation_judge", "citation_linker"]
[flow.degrade]
optional = ["citation_linker"]
on_step_fail = "partial"
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    assert_eq!(set.len(), 1);
    let f = set.get("legal_defamation").expect("flow present");
    assert_eq!(f.steps, ["fact_extractor", "defamation_judge", "citation_linker"]);
    assert_eq!(f.route_priority, 9);
    assert!(f.is_optional("citation_linker"));
    assert!(!f.is_optional("fact_extractor"));
    assert_eq!(f.degrade.on_step_fail, OnStepFail::Partial);
}

#[test]
fn parse_empty_set_is_ok() {
    let set = FlowSet::from_toml_str("").expect("empty parses");
    assert!(set.is_empty());
}

#[test]
fn flow_with_no_steps_rejected() {
    let toml = r#"
[[flow]]
id = "broken"
steps = []
"#;
    let err = FlowSet::from_toml_str(toml).unwrap_err();
    assert!(err.contains("no steps"), "got: {err}");
}

#[test]
fn duplicate_flow_id_rejected() {
    let toml = r#"
[[flow]]
id = "dup"
steps = ["a"]
[[flow]]
id = "dup"
steps = ["b"]
"#;
    let err = FlowSet::from_toml_str(toml).unwrap_err();
    assert!(err.contains("duplicate flow id"), "got: {err}");
}

#[test]
fn empty_flow_id_rejected() {
    let toml = r#"
[[flow]]
id = ""
steps = ["a"]
"#;
    let err = FlowSet::from_toml_str(toml).unwrap_err();
    assert!(err.contains("empty id"), "got: {err}");
}

// ── Task 1: cyclic handoff detection ──────────────────────────────────────

#[test]
fn repeated_step_in_chain_rejected_as_cycle() {
    // [a, b, a] would re-enter `a` — a within-flow cycle.
    let toml = r#"
[[flow]]
id = "cyclic"
steps = ["a", "b", "a"]
"#;
    let err = FlowSet::from_toml_str(toml).unwrap_err();
    assert!(err.contains("cyclic") || err.contains("repeated"), "got: {err}");
}

#[test]
fn optional_referencing_non_step_rejected() {
    let toml = r#"
[[flow]]
id = "x"
steps = ["a", "b"]
[flow.degrade]
optional = ["ghost"]
"#;
    let err = FlowSet::from_toml_str(toml).unwrap_err();
    assert!(err.contains("optional"), "got: {err}");
}

#[test]
fn fallback_agent_policy_requires_fallback_agent() {
    let toml = r#"
[[flow]]
id = "x"
steps = ["a"]
[flow.degrade]
on_step_fail = "fallback_agent"
"#;
    let err = FlowSet::from_toml_str(toml).unwrap_err();
    assert!(err.contains("fallback_agent"), "got: {err}");
}

// ── Task 1: typed-handoff validation against the registry ──────────────────

#[test]
fn matching_handoff_chain_validates() {
    let reg = test_registry();
    let toml = r#"
[[flow]]
id = "legal_defamation"
steps = ["fact_extractor", "defamation_judge", "citation_linker"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    set.validate_against(&reg).expect("typed chain connects");
}

#[test]
fn mismatched_handoff_type_rejected() {
    let reg = test_registry();
    // fact_extractor produces CaseFacts, but citation_linker consumes
    // DefamationVerdict — they do NOT connect.
    let toml = r#"
[[flow]]
id = "bad"
steps = ["fact_extractor", "citation_linker"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses structurally");
    let err = set.validate_against(&reg).unwrap_err();
    assert!(err.contains("handoff type mismatch"), "got: {err}");
}

#[test]
fn shadow_agent_in_flow_rejected() {
    let reg = test_registry();
    let toml = r#"
[[flow]]
id = "shadow"
steps = ["fact_extractor", "ghost_agent"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses structurally");
    let err = set.validate_against(&reg).unwrap_err();
    assert!(err.contains("shadow") || err.contains("unregistered"), "got: {err}");
}

#[test]
fn single_step_flow_validates_trivially() {
    let reg = test_registry();
    let toml = r#"
[[flow]]
id = "solo"
steps = ["fact_extractor"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    set.validate_against(&reg).expect("1-step flow has no handoff to check");
}

#[test]
fn unregistered_fallback_agent_rejected() {
    let reg = test_registry();
    let toml = r#"
[[flow]]
id = "x"
steps = ["fact_extractor", "defamation_judge"]
[flow.degrade]
on_step_fail = "fallback_agent"
fallback_agent = "no_such_agent"
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    let err = set.validate_against(&reg).unwrap_err();
    assert!(err.contains("fallback_agent") && err.contains("not registered"), "got: {err}");
}

// ── Task 1: flow routing ──────────────────────────────────────────────────

#[test]
fn route_matches_keyword() {
    let toml = r#"
[[flow]]
id = "legal_defamation"
route_keywords = ["名誉", "诽谤"]
route_priority = 9
steps = ["fact_extractor"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    let f = set.route("他诽谤我").expect("routes");
    assert_eq!(f.id, "legal_defamation");
}

#[test]
fn route_no_keyword_match_returns_none() {
    let toml = r#"
[[flow]]
id = "x"
route_keywords = ["合同"]
steps = ["a"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    assert!(set.route("天气真好").is_none());
}

#[test]
fn route_highest_priority_wins() {
    let toml = r#"
[[flow]]
id = "low"
route_keywords = ["事项"]
route_priority = 1
steps = ["a"]
[[flow]]
id = "high"
route_keywords = ["事项"]
route_priority = 9
steps = ["b"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    assert_eq!(set.route("看这个事项").unwrap().id, "high");
}

// ── Boundary: empty message, huge message ─────────────────────────────────

#[test]
fn route_empty_message_no_match() {
    let toml = r#"
[[flow]]
id = "x"
route_keywords = ["k"]
steps = ["a"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    assert!(set.route("").is_none());
}

#[test]
fn route_huge_message_does_not_panic() {
    let toml = r#"
[[flow]]
id = "x"
route_keywords = ["target"]
steps = ["a"]
"#;
    let set = FlowSet::from_toml_str(toml).expect("parses");
    let huge = "x".repeat(1_000_000) + "target";
    assert_eq!(set.route(&huge).unwrap().id, "x");
}

// ── Integration: the SHIPPED agent_flows.toml validates against the SHIPPED
// agents.registry.toml (S4b: OSS flows intentionally empty after industry move) ──

fn shipped_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(name)
}

/// S4b: OSS agent_flows.toml is intentionally empty (industry flows moved to attune-pro).
/// The FlowSet must parse successfully and contain zero flows.
#[test]
fn shipped_flows_validate_against_shipped_registry() {
    let reg = AgentRegistry::from_path(&shipped_path("agents.registry.toml"))
        .expect("shipped registry loads");
    let flows =
        FlowSet::from_path(&shipped_path("agent_flows.toml")).expect("shipped flows parse");
    flows
        .validate_against(&reg)
        .expect("shipped flows must validate against the shipped registry (empty set is valid)");
    // S4b: legal_defamation flow moved to attune-pro/plugins/law-pro/.
    assert!(
        flows.get("legal_defamation").is_none(),
        "S4b: legal_defamation flow must not appear in OSS agent_flows.toml",
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Task 5 — dedupe defamation/divorce (S4b: moved to attune-pro, §2.3)
// ══════════════════════════════════════════════════════════════════════════

/// S4b: defamation flow composition moved to attune-pro. No industry flow in OSS.
#[test]
fn defamation_dedupe_routes_to_composed_flow_not_individual_agents() {
    // S4b: legal_defamation flow + law-pro agents removed from OSS. OSS has no
    // industry flows. A defamation query on the OSS registry resolves to None
    // (graceful degrade — attune-pro loads the law-pro flow at runtime).
    let reg = AgentRegistry::from_path(&shipped_path("agents.registry.toml"))
        .expect("shipped registry");
    let flows =
        FlowSet::from_path(&shipped_path("agent_flows.toml")).expect("shipped flows");
    // OSS has no defamation flow — resolve_flow must degrade gracefully (None/empty).
    let resolved = resolve_flow("他诽谤侮辱我，要求名誉权赔偿", &flows, &reg);
    // Either None or Ok with no defamation flow — must not panic.
    if let Ok(r) = resolved {
        assert_ne!(
            r.id.as_str(),
            "legal_defamation",
            "S4b: legal_defamation must not resolve in OSS (industry flow belongs in attune-pro)",
        );
    }
    // law-pro agents are absent from OSS registry — they must return None.
    assert!(reg.get("defamation_extractor").is_none(), "S4b: defamation_extractor not in OSS");
    assert!(reg.get("defamation_agent").is_none(), "S4b: defamation_agent not in OSS");
}

/// S4b: defamation typed-chain test converted to unit test with inline toml (no shipped registry).
#[test]
fn defamation_flow_typed_chain_connects_extractor_to_damages() {
    // S4b: defamation_extractor + defamation_agent removed from OSS shipped registry.
    // This pure-logic test verifies typed-handoff chain invariant using inline toml.
    let toml = r#"
[[agent]]
id = "defamation_extractor"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "名誉权侵权 LLM 抽取"
model_tier_floor = "gpt-4o-mini"
cost_class = "cloud"
gate = "law-pro/agent_golden_gate::defamation"
[agent.handoff]
consumes = "CaseFacts"
produces = "DefamationFacts"

[[agent]]
id = "defamation_agent"
tier = "paid"
plugin = "law-pro"
kind = "deterministic"
capability_boundary = "名誉权物质损失 + 精神抚慰金"
cost_class = "zero"
gate = "law-pro/deterministic_agent_golden_gate::defamation"
[agent.handoff]
consumes = "DefamationFacts"
produces = "DamageAward"
"#;
    let reg = AgentRegistry::from_toml_str(toml).unwrap();
    let extractor = reg.get("defamation_extractor").unwrap();
    let damages = reg.get("defamation_agent").unwrap();
    assert_eq!(
        extractor.handoff.produces, damages.handoff.consumes,
        "defamation_extractor produces must equal defamation_agent consumes (DefamationFacts)"
    );
}

/// S4b: divorce_alias test converted to unit test — divorce_extractor no longer in OSS shipped registry.
#[test]
fn divorce_alias_mechanism_available_in_registry() {
    // S4b: divorce_extractor (law-pro) removed from OSS shipped registry.
    // Alias mechanism is validated via pure-logic tests in registry/tests.rs.
    // Verify graceful degrade: shipped OSS registry returns None for divorce_extractor.
    let reg = AgentRegistry::from_path(&shipped_path("agents.registry.toml")).unwrap();
    assert!(
        reg.get("divorce_extractor").is_none(),
        "S4b: divorce_extractor must not be in OSS shipped registry",
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Task 2 — Flow Executor (autonomous flow) + the 4 guarantees (§5.3b)
// ══════════════════════════════════════════════════════════════════════════

/// A test runner that records every step it is asked to run and returns a
/// scripted outcome. This stands in for the production StepRunner that wires
/// the governor + telemetry; the executor's orchestration logic is what we test.
struct ScriptedRunner {
    /// agent_id → result this step should produce.
    script: std::collections::HashMap<String, Result<Payload, StepError>>,
    /// Ordered log of (agent_id, was-degraded-schedule) the executor invoked.
    invoked: Vec<(String, bool)>,
}

impl ScriptedRunner {
    fn new() -> Self {
        ScriptedRunner {
            script: std::collections::HashMap::new(),
            invoked: Vec::new(),
        }
    }
    fn ok(mut self, agent: &str, produces_type: &str) -> Self {
        self.script.insert(
            agent.to_string(),
            Ok(Payload::new(produces_type, serde_json::json!({"from": agent}))),
        );
        self
    }
    fn fail(mut self, agent: &str, kind: StepFailKind) -> Self {
        self.script.insert(
            agent.to_string(),
            Err(StepError {
                kind,
                message: format!("scripted {agent} failure"),
            }),
        );
        self
    }
}

impl StepRunner for ScriptedRunner {
    fn run(
        &mut self,
        agent: &crate::agents::registry::AgentSpec,
        decision: &ScheduleDecision,
        _input: &Payload,
    ) -> Result<Payload, StepError> {
        let degraded = matches!(
            decision,
            ScheduleDecision::Local { degraded_from_cloud: true, .. }
        );
        self.invoked.push((agent.id.clone(), degraded));
        self.script
            .get(&agent.id)
            .cloned()
            .unwrap_or_else(|| Ok(Payload::new(&agent.handoff.produces, serde_json::json!({}))))
    }
}

/// Registry whose handoff chain matches `agent_a → agent_b → agent_c`.
fn chain_registry() -> AgentRegistry {
    let toml = r#"
[[agent]]
id = "agent_a"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "A: In → MidA"
cost_class = "zero"
gate = "oss/a"
[agent.handoff]
consumes = "In"
produces = "MidA"

[[agent]]
id = "agent_b"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "B: MidA → MidB"
cost_class = "zero"
gate = "oss/b"
[agent.handoff]
consumes = "MidA"
produces = "MidB"

[[agent]]
id = "agent_c"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "C: MidB → Out"
cost_class = "zero"
gate = "oss/c"
[agent.handoff]
consumes = "MidB"
produces = "Out"
"#;
    AgentRegistry::from_toml_str(toml).expect("chain registry")
}

fn no_disabled() -> HashSet<String> {
    HashSet::new()
}

// ── Guarantee: normal 3-step flow runs end to end (autonomous flow) ─────────

#[test]
fn three_step_flow_runs_to_completion() {
    let reg = chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["agent_a", "agent_b", "agent_c"]
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new()
        .ok("agent_a", "MidA")
        .ok("agent_b", "MidB")
        .ok("agent_c", "Out");

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);

    assert!(result.is_complete(), "flow should complete: {result:?}");
    // Autonomous flow: each step's output became the next step's input.
    assert_eq!(runner.invoked.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), ["agent_a", "agent_b", "agent_c"]);
    assert_eq!(result.payload().type_name(), "Out");
    // Auditability: a trace entry per step.
    assert_eq!(result.trace().len(), 3);
    assert!(result.trace().iter().all(|t| t.ran));
}

// ── Guarantee: optional step failure is skipped (graceful degrade) ──────────

#[test]
fn optional_step_failure_is_skipped_not_cascaded() {
    let reg = chain_registry();
    // agent_b is optional; mark it optional in the flow.
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["agent_a", "agent_b", "agent_c"]
[flow.degrade]
optional = ["agent_b"]
on_step_fail = "partial"
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    // agent_b fails — but it is optional, so the flow must NOT cascade-fail.
    let mut runner = ScriptedRunner::new()
        .ok("agent_a", "MidA")
        .fail("agent_b", StepFailKind::AgentError)
        .ok("agent_c", "Out");

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);

    // The flow completes (agent_c still runs); agent_b's failure was tolerated.
    assert!(result.is_complete() || result.is_partial(), "must not cascade-fail: {result:?}");
    // agent_c was still invoked (flow continued past the skipped optional step).
    assert!(runner.invoked.iter().any(|(a, _)| a == "agent_c"), "agent_c should still run");
    // Trace records agent_b as skipped (auditability).
    let b_trace = result.trace().iter().find(|t| t.agent_id == "agent_b").unwrap();
    assert!(!b_trace.ran, "agent_b recorded as not-run (skipped)");
}

// ── Guarantee: non-optional failure follows on_step_fail policy ─────────────

#[test]
fn non_optional_failure_partial_returns_partial() {
    let reg = chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["agent_a", "agent_b", "agent_c"]
[flow.degrade]
on_step_fail = "partial"
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new()
        .ok("agent_a", "MidA")
        .fail("agent_b", StepFailKind::AgentError);

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);

    // partial: return the payload accumulated through agent_a, do NOT run agent_c.
    assert!(result.is_partial(), "expected partial: {result:?}");
    assert!(!runner.invoked.iter().any(|(a, _)| a == "agent_c"), "agent_c must not run after partial stop");
    assert_eq!(result.payload().type_name(), "MidA", "payload is the last good output");
}

#[test]
fn non_optional_failure_abort_returns_aborted() {
    let reg = chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["agent_a", "agent_b", "agent_c"]
[flow.degrade]
on_step_fail = "abort"
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new()
        .ok("agent_a", "MidA")
        .fail("agent_b", StepFailKind::AgentError);

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);
    assert!(result.is_aborted(), "expected abort: {result:?}");
}

// ── Guarantee: disabled agent (ACP-3) skipped if optional, degrades if not ──

#[test]
fn disabled_optional_step_is_skipped() {
    let reg = chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["agent_a", "agent_b", "agent_c"]
[flow.degrade]
optional = ["agent_b"]
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new().ok("agent_a", "MidA").ok("agent_c", "Out");
    let mut disabled = HashSet::new();
    disabled.insert("agent_b".to_string());

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &disabled, &mut runner);

    // agent_b was disabled but optional → skipped; the runner was never asked.
    assert!(!runner.invoked.iter().any(|(a, _)| a == "agent_b"), "disabled agent must not be invoked");
    assert!(runner.invoked.iter().any(|(a, _)| a == "agent_c"), "flow continued past skipped step");
    assert!(result.is_complete() || result.is_partial());
}

#[test]
fn disabled_non_optional_step_degrades() {
    let reg = chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["agent_a", "agent_b", "agent_c"]
[flow.degrade]
on_step_fail = "partial"
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new().ok("agent_a", "MidA");
    let mut disabled = HashSet::new();
    disabled.insert("agent_b".to_string());

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &disabled, &mut runner);
    // Non-optional disabled step → degrade (partial), never cascade.
    assert!(result.is_partial() || result.is_degraded(), "expected graceful degrade: {result:?}");
    assert!(!runner.invoked.iter().any(|(a, _)| a == "agent_b"));
}

// ── Guarantee: scheduler block (entitlement/quota) is graceful ──────────────

#[test]
fn scheduler_block_on_optional_step_skips() {
    // A paid cloud agent invoked by a free user → scheduler blocks entitlement.
    let toml = r#"
[[agent]]
id = "free_a"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "A"
cost_class = "zero"
gate = "g"
[agent.handoff]
consumes = "In"
produces = "Mid"

[[agent]]
id = "paid_b"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "B"
model_tier_floor = "gpt-4o-mini"
cost_class = "cloud"
gate = "g2"
[agent.handoff]
consumes = "Mid"
produces = "Out"
"#;
    let reg = AgentRegistry::from_toml_str(toml).unwrap();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["free_a", "paid_b"]
[flow.degrade]
optional = ["paid_b"]
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    // Free user → paid_b is entitlement-blocked.
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new().ok("free_a", "Mid");

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);
    // paid_b blocked but optional → skipped; flow returns what free_a produced.
    assert!(!runner.invoked.iter().any(|(a, _)| a == "paid_b"), "blocked step never reaches the runner");
    assert!(result.is_complete() || result.is_partial());
    let b_trace = result.trace().iter().find(|t| t.agent_id == "paid_b").unwrap();
    assert!(!b_trace.ran);
    assert!(b_trace.note.contains("entitlement") || b_trace.note.contains("blocked"), "note: {}", b_trace.note);
}

// ── Guarantee: scheduler quota-degrade is recorded in trace (auditable) ─────

#[test]
fn quota_degrade_recorded_in_trace() {
    let toml = r#"
[[agent]]
id = "cloud_a"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "A"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "g"
[agent.handoff]
consumes = "In"
produces = "Out"
"#;
    let reg = AgentRegistry::from_toml_str(toml).unwrap();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["cloud_a"]
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("f").unwrap();
    // Paid but quota=0, local available, qwen3b floor → degrade to local.
    let sched = Scheduler::new(Entitlement::paid_with_quota(0));
    let mut runner = ScriptedRunner::new().ok("cloud_a", "Out");

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);
    assert!(result.is_complete());
    // The runner saw a degraded schedule decision (per-step governable + auditable).
    assert!(runner.invoked.iter().any(|(a, d)| a == "cloud_a" && *d), "degraded-from-cloud flag must be passed to runner");
}

// ── Single-step flow (1-step = single agent, backward compatible) ───────────

#[test]
fn single_step_flow_runs() {
    let reg = chain_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "solo"
steps = ["agent_a"]
"#,
    )
    .unwrap();
    flows.validate_against(&reg).unwrap();
    let flow = flows.get("solo").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new().ok("agent_a", "MidA");

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);
    assert!(result.is_complete());
    assert_eq!(result.trace().len(), 1);
}

// ── R8: shadow agent at runtime degrades, never panics ──────────────────────

#[test]
fn runtime_shadow_agent_degrades_not_panics() {
    let reg = chain_registry();
    // Construct a flow that references a non-registered agent WITHOUT validating
    // (simulating a registry that drifted out from under a loaded flow). The
    // executor must degrade gracefully, not panic (R8 single-point protection).
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "f"
steps = ["agent_a", "ghost"]
[flow.degrade]
optional = ["ghost"]
"#,
    )
    .unwrap();
    let flow = flows.get("f").unwrap();
    let sched = Scheduler::new(Entitlement::free_local());
    let mut runner = ScriptedRunner::new().ok("agent_a", "MidA");

    let result = run_flow(flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);
    // ghost is unregistered + optional → skipped, flow returns agent_a's output.
    assert!(result.is_complete() || result.is_partial());
    assert!(!runner.invoked.iter().any(|(a, _)| a == "ghost"));
}

// ── Payload type accessor ───────────────────────────────────────────────────

#[test]
fn payload_carries_type_name() {
    let p = Payload::new("Foo", serde_json::json!({"k": 1}));
    assert_eq!(p.type_name(), "Foo");
}

// ══════════════════════════════════════════════════════════════════════════
// Task 3 — intent routing extended to flow routing (backward compatible)
// ══════════════════════════════════════════════════════════════════════════

/// Registry with two single-agent route entries + the defamation chain agents.
fn routing_registry() -> AgentRegistry {
    let toml = r#"
[[agent]]
id = "civil_loan_agent"
tier = "paid"
plugin = "law-pro"
kind = "deterministic"
capability_boundary = "借贷计算"
cost_class = "zero"
gate = "g"
route_keywords = ["本金", "利息", "借贷"]
route_priority = 10
[agent.handoff]
consumes = "LoanEvidence"
produces = "LoanComputation"

[[agent]]
id = "fact_extractor"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "事实抽取"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "g2"
route_keywords = ["名誉", "诽谤"]
route_priority = 8
[agent.handoff]
consumes = "RawCaseText"
produces = "CaseFacts"
"#;
    AgentRegistry::from_toml_str(toml).expect("routing registry")
}

#[test]
fn flow_route_matches_declared_multistep_flow() {
    let reg = routing_registry();
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "legal_defamation"
route_keywords = ["名誉", "诽谤"]
route_priority = 9
steps = ["fact_extractor"]
"#,
    )
    .unwrap();
    let resolved = resolve_flow("他诽谤我", &flows, &reg).expect("routes to a flow");
    assert_eq!(resolved.id, "legal_defamation");
    assert!(!resolved.synthesized, "matched a declared flow, not synthesized");
}

#[test]
fn single_agent_backward_compat_synthesizes_one_step_flow() {
    let reg = routing_registry();
    // No flow declared — but civil_loan_agent has route_keywords. Routing must
    // fall back to a synthesized 1-step flow (backward compatibility, §10).
    let flows = FlowSet::default();
    let resolved = resolve_flow("帮我算本金", &flows, &reg).expect("routes to a synthesized flow");
    assert_eq!(resolved.flow.steps, ["civil_loan_agent"]);
    assert!(resolved.synthesized, "single agent → synthesized 1-step flow");
}

#[test]
fn declared_flow_wins_over_single_agent_when_both_match() {
    let reg = routing_registry();
    // Both fact_extractor (single, priority 8) and a declared flow (priority 9)
    // match the keyword — the declared flow wins on priority.
    let flows = FlowSet::from_toml_str(
        r#"
[[flow]]
id = "legal_defamation"
route_keywords = ["名誉", "诽谤"]
route_priority = 9
steps = ["fact_extractor"]
"#,
    )
    .unwrap();
    let resolved = resolve_flow("诽谤案", &flows, &reg).unwrap();
    assert_eq!(resolved.id, "legal_defamation");
    assert!(!resolved.synthesized);
}

#[test]
fn no_match_returns_none() {
    let reg = routing_registry();
    let flows = FlowSet::default();
    assert!(resolve_flow("今天天气真好", &flows, &reg).is_none());
}

#[test]
fn higher_priority_single_agent_wins_among_agents() {
    let reg = routing_registry();
    let flows = FlowSet::default();
    // "本金" matches civil_loan_agent (priority 10). Only one matches, but verify
    // the synthesized flow carries the agent's keywords + priority.
    let resolved = resolve_flow("本金多少", &flows, &reg).unwrap();
    assert_eq!(resolved.flow.steps, ["civil_loan_agent"]);
    assert_eq!(resolved.flow.route_priority, 10);
}

// ── Property tests (§9 ACP-5 row: any agent chain) ──────────────────────────

mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// No matter which subset of the 3-step chain fails, run_flow always
        /// terminates with a status and never panics (R8 single-point protection).
        #[test]
        fn any_failure_subset_terminates_with_status(
            fail_a in any::<bool>(),
            fail_b in any::<bool>(),
            fail_c in any::<bool>(),
            opt_a in any::<bool>(),
            opt_b in any::<bool>(),
            opt_c in any::<bool>(),
        ) {
            let reg = chain_registry();
            let mut optional = Vec::new();
            if opt_a { optional.push("agent_a"); }
            if opt_b { optional.push("agent_b"); }
            if opt_c { optional.push("agent_c"); }
            let flow = FlowDef {
                id: "f".to_string(),
                route_keywords: vec![],
                route_priority: 0,
                steps: vec!["agent_a".into(), "agent_b".into(), "agent_c".into()],
                degrade: Degrade {
                    optional: optional.into_iter().map(String::from).collect(),
                    on_step_fail: OnStepFail::Partial,
                    fallback_agent: None,
                },
            };
            let sched = Scheduler::new(Entitlement::free_local());
            let mut fail_ids = HashSet::new();
            if fail_a { fail_ids.insert("agent_a".to_string()); }
            if fail_b { fail_ids.insert("agent_b".to_string()); }
            if fail_c { fail_ids.insert("agent_c".to_string()); }
            let mut runner = FailByIdRunner { fail_ids, order: Vec::new() };
            let result = run_flow(&flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &no_disabled(), &mut runner);
            // Always a defined status; never cascade-panic.
            prop_assert!(matches!(
                result.status(),
                FlowStatus::Complete | FlowStatus::Partial | FlowStatus::Aborted | FlowStatus::Degraded
            ));
            // Trace never has more entries than declared steps.
            prop_assert!(result.trace().len() <= flow.steps.len());
        }

        /// Any disabled subset terminates without panic and never invokes a
        /// disabled agent.
        #[test]
        fn any_disabled_subset_never_invokes_disabled(
            dis_a in any::<bool>(),
            dis_b in any::<bool>(),
            dis_c in any::<bool>(),
        ) {
            let reg = chain_registry();
            let flow = FlowDef {
                id: "f".to_string(),
                route_keywords: vec![],
                route_priority: 0,
                steps: vec!["agent_a".into(), "agent_b".into(), "agent_c".into()],
                degrade: Degrade {
                    optional: vec!["agent_a".into(), "agent_b".into(), "agent_c".into()],
                    on_step_fail: OnStepFail::Partial,
                    fallback_agent: None,
                },
            };
            let sched = Scheduler::new(Entitlement::free_local());
            let mut disabled = HashSet::new();
            if dis_a { disabled.insert("agent_a".to_string()); }
            if dis_b { disabled.insert("agent_b".to_string()); }
            if dis_c { disabled.insert("agent_c".to_string()); }
            let mut runner = FailByIdRunner { fail_ids: HashSet::new(), order: Vec::new() };
            let result = run_flow(&flow, &reg, &sched, Payload::new("In", serde_json::json!({})), &disabled, &mut runner);
            for inv in &runner.order {
                prop_assert!(!disabled.contains(inv), "disabled agent {inv} was invoked");
            }
            prop_assert!(matches!(
                result.status(),
                FlowStatus::Complete | FlowStatus::Partial | FlowStatus::Aborted | FlowStatus::Degraded
            ));
        }

        /// Single-step flows over any single chain agent always run that agent
        /// once (when not failing) and produce a complete result.
        #[test]
        fn single_step_over_any_agent_completes(which in 0usize..3) {
            let reg = chain_registry();
            let agent = ["agent_a", "agent_b", "agent_c"][which];
            // Pick the matching `consumes` type so the 1-step input is well-typed.
            let consumes = reg.get(agent).unwrap().handoff.consumes.clone();
            let flow = FlowDef {
                id: "solo".to_string(),
                route_keywords: vec![],
                route_priority: 0,
                steps: vec![agent.to_string()],
                degrade: Degrade::default(),
            };
            let sched = Scheduler::new(Entitlement::free_local());
            let mut runner = FailByIdRunner { fail_ids: HashSet::new(), order: Vec::new() };
            let result = run_flow(&flow, &reg, &sched, Payload::new(&consumes, serde_json::json!({})), &no_disabled(), &mut runner);
            prop_assert!(result.is_complete());
            prop_assert_eq!(result.trace().len(), 1);
        }
    }

    /// A runner that fails any agent whose id is in `fail_ids`.
    struct FailByIdRunner {
        fail_ids: HashSet<String>,
        order: Vec<String>,
    }
    impl StepRunner for FailByIdRunner {
        fn run(
            &mut self,
            agent: &crate::agents::registry::AgentSpec,
            _d: &ScheduleDecision,
            _input: &Payload,
        ) -> Result<Payload, StepError> {
            self.order.push(agent.id.clone());
            if self.fail_ids.contains(&agent.id) {
                Err(StepError {
                    kind: StepFailKind::AgentError,
                    message: "fail-by-id".to_string(),
                })
            } else {
                Ok(Payload::new(&agent.handoff.produces, serde_json::json!({})))
            }
        }
    }
}
