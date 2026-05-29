//! ACP-7 scheduler tests (spec §9 ACP-7 row): cost-class routing, quota=0
//! degrade, entitlement block, weak-model disable.

use super::*;
use crate::agents::registry::{AgentSpec, CostClass, Handoff, Kind, Tier};

fn agent(id: &str, tier: Tier, cost: CostClass, floor: &str) -> AgentSpec {
    AgentSpec {
        id: id.to_string(),
        tier,
        plugin: "law-pro".to_string(),
        kind: if floor.is_empty() { Kind::Deterministic } else { Kind::LlmJudge },
        capability_boundary: format!("boundary-{id}"),
        model_tier_floor: floor.to_string(),
        cost_class: cost,
        gate: "g".to_string(),
        route_keywords: vec![],
        route_priority: 0,
        handoff: Handoff {
            consumes: "In".to_string(),
            produces: "Out".to_string(),
        },
    }
}

// ── cost-class routing ────────────────────────────────────────────────────

#[test]
fn zero_cost_deterministic_runs_local_no_model() {
    let sched = Scheduler::new(Entitlement::paid_with_quota(100));
    let a = agent("civil_loan", Tier::Paid, CostClass::Zero, "");
    let d = sched.route(&a, None);
    assert_eq!(d, ScheduleDecision::Local { model: None, degraded_from_cloud: false });
    assert!(d.is_runnable());
}

#[test]
fn local_cost_qwen_floor_runs_local_with_fallback_model() {
    let sched = Scheduler::new(Entitlement::free_local());
    let a = agent("self_evolving", Tier::Free, CostClass::Local, "qwen3b");
    let d = sched.route(&a, None);
    assert_eq!(
        d,
        ScheduleDecision::Local {
            model: Some(LOCAL_FALLBACK_MODEL.to_string()),
            degraded_from_cloud: false
        }
    );
}

#[test]
fn cloud_paid_with_quota_routes_cloud() {
    let sched = Scheduler::new(Entitlement::paid_with_quota(50));
    let a = agent("fact_extractor", Tier::Paid, CostClass::Cloud, "qwen3b");
    assert_eq!(sched.route(&a, None), ScheduleDecision::Cloud);
}

// ── quota=0 degrade ───────────────────────────────────────────────────────

#[test]
fn cloud_quota_exhausted_qwen_floor_degrades_to_local() {
    let sched = Scheduler::new(Entitlement::paid_with_quota(0));
    let a = agent("fact_extractor", Tier::Paid, CostClass::Cloud, "qwen3b");
    let d = sched.route(&a, None);
    assert_eq!(
        d,
        ScheduleDecision::Local {
            model: Some(LOCAL_FALLBACK_MODEL.to_string()),
            degraded_from_cloud: true
        }
    );
}

#[test]
fn cloud_quota_exhausted_high_floor_cannot_degrade_blocks() {
    // gpt-4o-mini floor: a 3B local model cannot satisfy it → block (never
    // silently downgrade quality, §2.3).
    let sched = Scheduler::new(Entitlement::paid_with_quota(0));
    let a = agent("defamation_extractor", Tier::Paid, CostClass::Cloud, "gpt-4o-mini");
    let d = sched.route(&a, None);
    assert!(matches!(d, ScheduleDecision::BlockedQuotaExhausted { .. }));
    assert!(d.is_blocked());
}

#[test]
fn cloud_quota_exhausted_no_local_available_blocks() {
    let ent = Entitlement {
        paid: true,
        cloud_quota_remaining: 0,
        local_available: false,
    };
    let sched = Scheduler::new(ent);
    let a = agent("fact_extractor", Tier::Paid, CostClass::Cloud, "qwen3b");
    assert!(matches!(sched.route(&a, None), ScheduleDecision::BlockedQuotaExhausted { .. }));
}

// ── entitlement block ─────────────────────────────────────────────────────

#[test]
fn paid_agent_free_user_blocked_entitlement() {
    let sched = Scheduler::new(Entitlement::free_local());
    let a = agent("fact_extractor", Tier::Paid, CostClass::Cloud, "qwen3b");
    let d = sched.route(&a, None);
    assert!(matches!(d, ScheduleDecision::BlockedEntitlement { .. }));
    assert!(d.is_blocked());
}

#[test]
fn free_agent_free_user_runs() {
    let sched = Scheduler::new(Entitlement::free_local());
    let a = agent("linker", Tier::Free, CostClass::Zero, "");
    assert!(sched.route(&a, None).is_runnable());
}

// ── weak-model disable (ACP-3) ────────────────────────────────────────────

#[test]
fn disabled_agent_blocks_before_any_cost() {
    let sched = Scheduler::new(Entitlement::paid_with_quota(100));
    let a = agent("defamation_extractor", Tier::Paid, CostClass::Cloud, "gpt-4o-mini");
    let d = sched.route(&a, Some("weak-model F1 below floor"));
    assert!(matches!(d, ScheduleDecision::BlockedDisabled { .. }));
    assert!(d.is_blocked());
}

#[test]
fn disabled_takes_precedence_over_entitlement() {
    // Even a free user hitting a paid agent: disabled wins (we never even reach
    // the entitlement check — the agent is off).
    let sched = Scheduler::new(Entitlement::free_local());
    let a = agent("fact_extractor", Tier::Paid, CostClass::Cloud, "qwen3b");
    let d = sched.route(&a, Some("disabled"));
    assert!(matches!(d, ScheduleDecision::BlockedDisabled { .. }));
}

// ── entitlement constructors ──────────────────────────────────────────────

#[test]
fn free_local_constructor() {
    let e = Entitlement::free_local();
    assert!(!e.paid);
    assert_eq!(e.cloud_quota_remaining, 0);
    assert!(e.local_available);
}

#[test]
fn paid_constructor() {
    let e = Entitlement::paid_with_quota(42);
    assert!(e.paid);
    assert_eq!(e.cloud_quota_remaining, 42);
}
