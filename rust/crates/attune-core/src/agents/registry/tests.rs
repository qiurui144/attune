//! ACP-1 registry tests (spec §9 ACP-1 row).
//!
//! Coverage: golden (consistency: 6 OSS-core all-registered / no empty gate /
//! handoff closed), boundary (empty registry / duplicate id), error (shadow-agent
//! reject), integration (the shipped `agents.registry.toml` loads + validates).
//! S4b: industry agents (law-pro/tech-pro) removed from OSS registry — regression
//! tests verify only oss-core agents remain.

use super::*;

/// Locate the shipped `agents.registry.toml` relative to the crate manifest.
/// (`CARGO_MANIFEST_DIR` = `<ws>/crates/attune-core`; registry lives at `<ws>/`.)
fn shipped_registry_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("agents.registry.toml")
}

fn load_shipped() -> AgentRegistry {
    AgentRegistry::from_path(&shipped_registry_path())
        .expect("shipped agents.registry.toml must load + validate")
}

// ── minimal valid fixture for pure-logic tests ──────────────────────────

fn one_agent_toml() -> &'static str {
    r#"
[[agent]]
id = "fact_extractor"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "借条 OCR → 本息事实"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "law-pro/agent_golden_gate::fact"
route_keywords = ["抽取事实", "借条信息"]
route_priority = 8
[agent.handoff]
consumes = "RawCaseText"
produces = "CaseFacts"
"#
}

// ── integration: the shipped registry ───────────────────────────────────

#[test]
fn shipped_registry_loads_and_validates() {
    let reg = load_shipped();
    assert!(!reg.is_empty(), "shipped registry must not be empty");
}

/// S4b: replaced shipped_registry_has_all_22_agents — OSS ships 6 oss-core agents only.
/// Industry agents (law-pro / tech-pro) live in attune-pro and are not in OSS registry.
#[test]
fn shipped_registry_has_all_6_oss_core_agents() {
    let reg = load_shipped();
    assert_eq!(
        reg.len(),
        6,
        "S4b OSS registry = 6 oss-core agents only (law-pro/tech-pro moved to attune-pro); \
         got {}",
        reg.len()
    );
}

/// S4b regression: OSS registry contains ONLY oss-core agents — no industry plugin agents.
#[test]
fn oss_registry_contains_only_oss_core_agents() {
    let reg = load_shipped();
    for a in reg.agents() {
        assert_eq!(
            a.plugin.as_str(),
            "oss-core",
            "S4b: agent '{}' has plugin='{}' — only oss-core agents allowed in OSS registry; \
             industry agents belong in attune-pro/plugins/<vertical>/",
            a.id,
            a.plugin,
        );
    }
}

/// S4b regression: registry.get() returns None for industry agent ids (graceful degrade, no panic).
#[test]
fn registry_get_missing_industry_agent_returns_none() {
    let reg = load_shipped();
    // These industry agents were removed from OSS in S4b; lookup must degrade gracefully.
    for industry_id in [
        "civil_loan_agent",
        "defamation_extractor",
        "defamation_agent",
        "fact_extractor",
        "code_reviewer",
        "evidence_classifier",
        "divorce_extractor",
        "traffic_accident_agent",
    ] {
        assert!(
            reg.get(industry_id).is_none(),
            "S4b: industry agent '{}' must not be in OSS registry (graceful degrade → None)",
            industry_id,
        );
    }
}

#[test]
fn shipped_registry_no_empty_gate() {
    let reg = load_shipped();
    for a in reg.agents() {
        assert!(
            !a.gate.trim().is_empty(),
            "agent {} has empty gate — every agent must bind a quality gate",
            a.id
        );
    }
}

#[test]
fn shipped_registry_every_agent_declares_handoff() {
    // The autonomous-flow foundation: each agent must declare a typed handoff
    // so the ACP-6 flow executor can chain steps.
    let reg = load_shipped();
    for a in reg.agents() {
        assert!(!a.handoff.consumes.trim().is_empty(), "{} consumes empty", a.id);
        assert!(!a.handoff.produces.trim().is_empty(), "{} produces empty", a.id);
    }
}

#[test]
fn shipped_registry_handoff_type_graph_nonempty() {
    // The set of typed handoff names the flow executor walks.
    let reg = load_shipped();
    let types = reg.handoff_types();
    assert!(
        types.len() >= 2,
        "handoff type graph must have ≥2 distinct types; got {types:?}"
    );
}

/// S4b: updated from 7-agent spot-check to OSS-only agents.
#[test]
fn shipped_registry_known_oss_agents_present() {
    // Spot-check all 6 oss-core agents (S4b: law-pro/tech-pro removed from OSS).
    let reg = load_shipped();
    for id in [
        "document_classifier",    // OSS deterministic
        "memory_consolidation",   // OSS LLM
        "linker",                 // OSS deterministic
        "chat_reliability",       // OSS deterministic
        "self_evolving_skill",    // OSS deterministic
        "skill_evolution_cycle",  // OSS LLM
    ] {
        assert!(reg.contains(id), "S4b: shipped OSS registry missing oss-core agent '{id}'");
    }
}

/// S4b: kept for historical reference — defamation_extractor is now in attune-pro only.
/// Graceful degrade: registry.get returns None, does not panic.
#[test]
fn shipped_registry_defamation_extractor_absent_in_oss() {
    // S4b: defamation_extractor (law-pro, gpt-4o-mini floor) removed from OSS registry.
    // attune-pro/plugins/law-pro/agents.registry.toml holds the authoritative entry.
    let reg = load_shipped();
    assert!(
        reg.get("defamation_extractor").is_none(),
        "S4b: defamation_extractor must not appear in OSS registry",
    );
}

#[test]
fn shipped_registry_oss_agents_are_free() {
    let reg = load_shipped();
    for a in reg.agents().iter().filter(|a| a.plugin == "oss-core") {
        assert_eq!(a.tier, Tier::Free, "OSS agent {} must be free tier", a.id);
    }
}

// ── ACP-5 dedupe: alias (shared binary) ─────────────────────────────────

#[test]
fn alias_field_defaults_to_none() {
    let reg = AgentRegistry::from_toml_str(one_agent_toml()).unwrap();
    let a = reg.get("fact_extractor").unwrap();
    assert!(a.shares_binary.is_none(), "no alias declared → None");
}

#[test]
fn alias_to_existing_binary_validates() {
    // Two agent ids sharing one binary (the divorce dedupe shape).
    let toml = r#"
[[agent]]
id = "agent_divorce"
tier = "paid"
plugin = "law-pro"
kind = "deterministic"
capability_boundary = "离婚确定性路径"
cost_class = "zero"
gate = "g"
[agent.handoff]
consumes = "DivorceFacts"
produces = "DivorceComputation"

[[agent]]
id = "divorce_extractor"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "离婚案情抽取"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "g2"
shares_binary = "agent_divorce"
[agent.handoff]
consumes = "RawCaseText"
produces = "DivorceFacts"
"#;
    let reg = AgentRegistry::from_toml_str(toml).expect("alias to existing agent validates");
    let a = reg.get("divorce_extractor").unwrap();
    assert_eq!(a.shares_binary.as_deref(), Some("agent_divorce"));
}

#[test]
fn alias_to_unregistered_binary_rejected() {
    let toml = r#"
[[agent]]
id = "divorce_extractor"
tier = "paid"
plugin = "law-pro"
kind = "llm-judge"
capability_boundary = "离婚案情抽取"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "g2"
shares_binary = "ghost_binary"
[agent.handoff]
consumes = "RawCaseText"
produces = "DivorceFacts"
"#;
    let err = AgentRegistry::from_toml_str(toml).unwrap_err();
    assert!(err.contains("shares_binary") && err.contains("ghost_binary"), "got: {err}");
}

#[test]
fn alias_to_self_rejected() {
    let toml = r#"
[[agent]]
id = "x"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "b"
cost_class = "zero"
gate = "g"
shares_binary = "x"
[agent.handoff]
consumes = "A"
produces = "B"
"#;
    let err = AgentRegistry::from_toml_str(toml).unwrap_err();
    assert!(err.contains("shares_binary") && err.contains("itself"), "got: {err}");
}

// ── golden / pure-logic: parse + accessors ──────────────────────────────

#[test]
fn parses_single_agent() {
    let reg = AgentRegistry::from_toml_str(one_agent_toml()).unwrap();
    assert_eq!(reg.len(), 1);
    let a = reg.get("fact_extractor").unwrap();
    assert_eq!(a.tier, Tier::Paid);
    assert_eq!(a.kind, Kind::LlmJudge);
    assert_eq!(a.cost_class, CostClass::Cloud);
    assert_eq!(a.handoff.consumes, "RawCaseText");
    assert_eq!(a.handoff.produces, "CaseFacts");
    assert_eq!(a.route_priority, 8);
}

#[test]
fn contains_distinguishes_registered_from_shadow() {
    let reg = AgentRegistry::from_toml_str(one_agent_toml()).unwrap();
    assert!(reg.contains("fact_extractor"));
    assert!(!reg.contains("ghost_agent"), "shadow agent must not be 'contained'");
}

// ── boundary: empty registry ────────────────────────────────────────────

#[test]
fn empty_registry_rejected() {
    let err = AgentRegistry::from_toml_str("").unwrap_err();
    assert!(err.contains("empty"), "empty registry must be rejected: {err}");
}

// ── error: duplicate id ─────────────────────────────────────────────────

#[test]
fn duplicate_id_rejected() {
    let toml = r#"
[[agent]]
id = "dup"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "boundary one"
cost_class = "zero"
gate = "oss/g1"
[agent.handoff]
consumes = "A"
produces = "B"

[[agent]]
id = "dup"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "boundary two"
cost_class = "zero"
gate = "oss/g2"
[agent.handoff]
consumes = "B"
produces = "C"
"#;
    let err = AgentRegistry::from_toml_str(toml).unwrap_err();
    assert!(err.contains("duplicate agent id"), "got: {err}");
}

// ── error: overlapping capability boundary (single-point responsibility) ──

#[test]
fn overlapping_boundary_rejected() {
    let toml = r#"
[[agent]]
id = "a1"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "same job"
cost_class = "zero"
gate = "oss/g1"
[agent.handoff]
consumes = "A"
produces = "B"

[[agent]]
id = "a2"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "same job"
cost_class = "zero"
gate = "oss/g2"
[agent.handoff]
consumes = "B"
produces = "C"
"#;
    let err = AgentRegistry::from_toml_str(toml).unwrap_err();
    assert!(err.contains("overlapping capability_boundary"), "got: {err}");
}

// ── error: empty handoff type ───────────────────────────────────────────

#[test]
fn empty_handoff_type_rejected() {
    let toml = r#"
[[agent]]
id = "a1"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "some job"
cost_class = "zero"
gate = "oss/g1"
[agent.handoff]
consumes = ""
produces = "B"
"#;
    let err = AgentRegistry::from_toml_str(toml).unwrap_err();
    assert!(err.contains("handoff.consumes"), "got: {err}");
}

// ── error: empty gate ───────────────────────────────────────────────────

#[test]
fn empty_gate_rejected() {
    let toml = r#"
[[agent]]
id = "a1"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "some job"
cost_class = "zero"
gate = ""
[agent.handoff]
consumes = "A"
produces = "B"
"#;
    let err = AgentRegistry::from_toml_str(toml).unwrap_err();
    assert!(err.contains("empty gate"), "got: {err}");
}

// ── route conflict detection (warning, not fatal) ───────────────────────

#[test]
fn route_conflicts_detected_when_keyword_shared() {
    let toml = r#"
[[agent]]
id = "a1"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "job one"
cost_class = "zero"
gate = "oss/g1"
route_keywords = ["shared", "uniq1"]
route_priority = 5
[agent.handoff]
consumes = "A"
produces = "B"

[[agent]]
id = "a2"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "job two"
cost_class = "zero"
gate = "oss/g2"
route_keywords = ["shared", "uniq2"]
route_priority = 9
[agent.handoff]
consumes = "B"
produces = "C"
"#;
    let reg = AgentRegistry::from_toml_str(toml).unwrap();
    let conflicts = reg.route_conflicts();
    assert_eq!(conflicts.len(), 1, "exactly one shared keyword");
    assert_eq!(conflicts[0].keyword, "shared");
    assert_eq!(conflicts[0].agent_ids, vec!["a1".to_string(), "a2".to_string()]);
}

#[test]
fn no_route_conflicts_when_keywords_disjoint() {
    let reg = AgentRegistry::from_toml_str(one_agent_toml()).unwrap();
    assert!(reg.route_conflicts().is_empty());
}

// ── directory render (CLI §5.5) ─────────────────────────────────────────

/// S4b: updated from 22-agent multi-plugin view to 6-agent oss-core only.
/// Industry agents (law-pro / tech-pro) removed from OSS registry in S4b.
#[test]
fn render_directory_lists_all_agents_with_columns() {
    let reg = load_shipped();
    let out = reg.render_directory();
    // S4b: 6 oss-core agents only.
    assert!(out.contains("Agent Registry — 6 agents"), "S4b: OSS registry has 6 agents, got:\n{out}");
    // Only oss-core plugin group.
    assert!(out.contains("[oss-core]"), "oss-core group must be present");
    // No industry plugin groups (S4b — moved to attune-pro).
    assert!(!out.contains("[law-pro]"), "S4b: law-pro group must not appear in OSS");
    assert!(!out.contains("[tech-pro]"), "S4b: tech-pro group must not appear in OSS");
    // Representative oss-core agents.
    assert!(out.contains("document_classifier"), "document_classifier must be listed");
    assert!(out.contains("memory_consolidation"), "memory_consolidation must be listed");
    // S4b: industry agents absent.
    assert!(!out.contains("defamation_extractor"), "S4b: defamation_extractor must not appear in OSS");
    // Column headers still rendered.
    assert!(out.contains("handoff:"), "handoff chain rendered");
    assert!(out.contains("gate="), "gate binding rendered");
}

// ── property tests (spec §9: 随机 agent 组合校验) ────────────────────────

use proptest::prelude::*;

/// Build a registry TOML from a list of (id, boundary) pairs. All other fields
/// are constant-valid so only id/boundary uniqueness drives validation.
fn build_toml(specs: &[(String, String)]) -> String {
    let mut s = String::new();
    for (id, boundary) in specs {
        s.push_str(&format!(
            "[[agent]]\nid = \"{id}\"\ntier = \"free\"\nplugin = \"oss-core\"\n\
             kind = \"deterministic\"\ncapability_boundary = \"{boundary}\"\n\
             cost_class = \"zero\"\ngate = \"oss/g\"\n\
             [agent.handoff]\nconsumes = \"In\"\nproduces = \"Out\"\n\n"
        ));
    }
    s
}

proptest! {
    /// Any set of agents with DISTINCT ids AND DISTINCT boundaries validates.
    #[test]
    fn distinct_ids_and_boundaries_always_validate(n in 1usize..12) {
        let specs: Vec<(String, String)> = (0..n)
            .map(|i| (format!("agent_{i}"), format!("boundary number {i}")))
            .collect();
        let reg = AgentRegistry::from_toml_str(&build_toml(&specs)).unwrap();
        prop_assert_eq!(reg.len(), n);
    }

    /// A duplicate id ANYWHERE in the set is always rejected.
    #[test]
    fn duplicate_id_anywhere_rejected(n in 2usize..10, dup_at in 0usize..9) {
        let dup_at = dup_at % n;
        let mut specs: Vec<(String, String)> = (0..n)
            .map(|i| (format!("agent_{i}"), format!("boundary {i}")))
            .collect();
        // Force the element at dup_at to collide with element 0's id.
        if dup_at != 0 {
            specs[dup_at].0 = specs[0].0.clone();
        } else if n >= 2 {
            specs[1].0 = specs[0].0.clone();
        }
        let err = AgentRegistry::from_toml_str(&build_toml(&specs)).unwrap_err();
        prop_assert!(err.contains("duplicate agent id"));
    }

    /// route_conflicts is order-independent: shuffling the agent list yields the
    /// same conflict set (determinism the flow router relies on).
    #[test]
    fn route_conflicts_order_independent(n in 2usize..6) {
        // Every agent shares keyword "k"; ids + boundaries distinct.
        let mk = |order: &[usize]| -> AgentRegistry {
            let mut s = String::new();
            for &i in order {
                s.push_str(&format!(
                    "[[agent]]\nid = \"a{i}\"\ntier = \"free\"\nplugin = \"oss-core\"\n\
                     kind = \"deterministic\"\ncapability_boundary = \"b{i}\"\n\
                     cost_class = \"zero\"\ngate = \"oss/g\"\nroute_keywords = [\"k\"]\n\
                     route_priority = {i}\n[agent.handoff]\nconsumes = \"In\"\nproduces = \"Out\"\n\n"
                ));
            }
            AgentRegistry::from_toml_str(&s).unwrap()
        };
        let forward: Vec<usize> = (0..n).collect();
        let reverse: Vec<usize> = (0..n).rev().collect();
        let cf = mk(&forward).route_conflicts();
        let cr = mk(&reverse).route_conflicts();
        prop_assert_eq!(&cf, &cr);
        // Exactly one conflict on "k", listing all n agents (sorted).
        prop_assert_eq!(cf.len(), 1);
        prop_assert_eq!(cf[0].agent_ids.len(), n);
    }
}

#[test]
fn shipped_registry_route_conflicts_are_intentional() {
    // The defamation case_kind is deliberately split (defamation_agent det-calc
    // + defamation_extractor LLM-extract) per A audit Q5 — they may share
    // keywords. Assert that ANY shipped conflict is resolvable by distinct
    // priority (the operator's intentional ordering), never two agents at the
    // same priority for the same keyword (which would be a real ambiguity).
    let reg = load_shipped();
    for c in reg.route_conflicts() {
        let priorities: Vec<i32> = c
            .agent_ids
            .iter()
            .map(|id| reg.get(id).unwrap().route_priority)
            .collect();
        let distinct: std::collections::BTreeSet<i32> = priorities.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            priorities.len(),
            "keyword {:?} shared by {:?} at non-distinct priorities {:?} — ambiguous route",
            c.keyword,
            c.agent_ids,
            priorities
        );
    }
}
