//! Tests for ACP-5 autonomous flow — DAG parsing, typed-handoff validation,
//! cycle detection, flow routing, and (Task 2) the flow executor + 4 guarantees.

use super::*;
use crate::agents::registry::AgentRegistry;

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
// agents.registry.toml (the legal_defamation 3-step flow must type-connect) ──

fn shipped_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(name)
}

#[test]
fn shipped_flows_validate_against_shipped_registry() {
    let reg = AgentRegistry::from_path(&shipped_path("agents.registry.toml"))
        .expect("shipped registry loads");
    let flows =
        FlowSet::from_path(&shipped_path("agent_flows.toml")).expect("shipped flows parse");
    flows
        .validate_against(&reg)
        .expect("shipped flows must type-connect against the shipped registry");
    // The canonical legal_defamation 3-step flow must be present.
    let f = flows.get("legal_defamation").expect("legal_defamation present");
    assert_eq!(f.steps, ["fact_extractor", "defamation_extractor", "defamation_agent"]);
}
