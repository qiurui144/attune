//! ACP-3 FeedbackController tests (§3 + §5.2 + §2.3 red line + §11 R2/R7).
//!
//! Test matrix (spec §9 ACP-3 row): fail-rate → correct TuningAction; 0-call /
//! all-fail boundary; deterministic agent never tuned (§2.3 red line); R2
//! auto-escalate default OFF; R7 disable needs consecutive N + min sample.

use super::*;
use crate::agent_telemetry::AgentModelHealth;
use crate::agents::registry::{AgentRegistry, AgentSpec, CostClass, Handoff, Kind, Tier};

/// A two-agent registry: one LLM-judge (`judge`, floor `qwen3b`) and one
/// deterministic (`calc`, no floor). Used across the decision tests.
fn test_registry() -> AgentRegistry {
    let toml = r#"
[[agent]]
id = "judge"
tier = "free"
plugin = "oss-core"
kind = "llm-judge"
capability_boundary = "judge things"
model_tier_floor = "qwen3b"
cost_class = "cloud"
gate = "oss/judge"
handoff = { consumes = "A", produces = "B" }

[[agent]]
id = "calc"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "calc things"
model_tier_floor = ""
cost_class = "zero"
gate = "oss/calc"
handoff = { consumes = "C", produces = "D" }
"#;
    AgentRegistry::from_toml_str(toml).expect("valid registry")
}

fn health(agent: &str, model: &str, total: u64, failures: u64) -> AgentModelHealth {
    AgentModelHealth::new(agent.to_string(), model.to_string(), total, failures)
}

// ── §2.3 RED LINE: deterministic agent is NEVER tuned ─────────────────────────

#[test]
fn deterministic_agent_never_tuned_even_at_100pct_fail() {
    let reg = test_registry();
    // auto_escalate ON, low thresholds — the most aggressive config possible.
    let cfg = FeedbackConfig {
        auto_escalate: true,
        min_samples: 1,
        consecutive_periods: 1,
    };
    let ctrl = FeedbackController::new(cfg);
    // calc is deterministic; even a 100% failure rate must yield NoOp.
    let rows = vec![health("calc", "qwen3b", 50, 50)];
    let actions = ctrl.decide(&reg, &rows);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].action, TuningAction::NoOp);
    assert!(actions[0].is_red_line_protected());
}

#[test]
fn shadow_agent_absent_from_registry_is_noop_not_tuned() {
    let reg = test_registry();
    let cfg = FeedbackConfig::aggressive_for_test();
    let ctrl = FeedbackController::new(cfg);
    // "ghost" is not in the registry → cannot be classified → must NoOp (never
    // tune something we don't understand).
    let rows = vec![health("ghost", "qwen3b", 50, 50)];
    let actions = ctrl.decide(&reg, &rows);
    assert_eq!(actions[0].action, TuningAction::NoOp);
}

// ── fail-rate → action band ───────────────────────────────────────────────────

#[test]
fn below_threshold_is_noop() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig::aggressive_for_test());
    // 30% is NOT strictly above 30% → NoOp (boundary inclusive on the safe side).
    let rows = vec![health("judge", "qwen3b", 10, 3)];
    let actions = ctrl.decide(&reg, &rows);
    assert_eq!(actions[0].action, TuningAction::NoOp);
}

#[test]
fn above_threshold_with_headroom_escalates_when_enabled() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig {
        auto_escalate: true,
        min_samples: 5,
        consecutive_periods: 1,
    });
    // judge on qwen3b at 40% fail, plenty of samples → escalate to next tier.
    let rows = vec![health("judge", "qwen3b", 20, 8)];
    let actions = ctrl.decide(&reg, &rows);
    assert_eq!(
        actions[0].action,
        TuningAction::EscalateModelTier {
            from: "qwen3b".to_string(),
            to: "flash".to_string(),
        }
    );
}

#[test]
fn top_tier_failing_disables_with_alert_not_escalate() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig {
        auto_escalate: true,
        min_samples: 5,
        consecutive_periods: 1,
    });
    // Already on the top tier (sonnet) and still failing → no higher tier to
    // escalate to → DisableWithAlert (soft, human review).
    let rows = vec![health("judge", "sonnet", 20, 12)];
    let actions = ctrl.decide(&reg, &rows);
    match &actions[0].action {
        TuningAction::DisableWithAlert { agent_id, .. } => assert_eq!(agent_id, "judge"),
        other => panic!("expected DisableWithAlert, got {other:?}"),
    }
}

// ── R2: auto-escalate default OFF (cost guardrail) ────────────────────────────

#[test]
fn r2_default_config_does_not_auto_escalate() {
    let cfg = FeedbackConfig::default();
    assert!(!cfg.auto_escalate, "auto_escalate must default to OFF (R2)");
}

#[test]
fn r2_auto_escalate_off_yields_recommendation_not_applied_escalation() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig {
        auto_escalate: false,
        min_samples: 5,
        consecutive_periods: 1,
    });
    let rows = vec![health("judge", "qwen3b", 20, 8)];
    let actions = ctrl.decide(&reg, &rows);
    // The action the controller *would* take is still surfaced (for dry-run),
    // but `applied` must be false because auto_escalate is OFF.
    assert_eq!(
        actions[0].action,
        TuningAction::EscalateModelTier {
            from: "qwen3b".to_string(),
            to: "flash".to_string(),
        }
    );
    assert!(!actions[0].applied, "escalation must NOT be auto-applied (R2)");
    assert!(actions[0].is_recommendation());
}

#[test]
fn r2_auto_escalate_on_marks_escalation_applied() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig {
        auto_escalate: true,
        min_samples: 5,
        consecutive_periods: 1,
    });
    let rows = vec![health("judge", "qwen3b", 20, 8)];
    let actions = ctrl.decide(&reg, &rows);
    assert!(actions[0].applied, "escalation applied when auto_escalate ON");
}

// ── R7: disable needs minimum sample + consecutive periods (misfire guard) ────

#[test]
fn r7_single_high_fail_below_min_samples_does_not_disable() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig {
        auto_escalate: true,
        min_samples: 20,
        consecutive_periods: 1,
    });
    // sonnet failing at 100% but only 3 calls (< min_samples 20) → too little
    // evidence → NoOp, never disable on a single noisy spike (R7).
    let rows = vec![health("judge", "sonnet", 3, 3)];
    let actions = ctrl.decide(&reg, &rows);
    assert_eq!(actions[0].action, TuningAction::NoOp);
}

#[test]
fn r7_disable_requires_consecutive_periods() {
    let reg = test_registry();
    let cfg = FeedbackConfig {
        auto_escalate: true,
        min_samples: 5,
        consecutive_periods: 3,
    };
    let mut ctrl = FeedbackController::new(cfg);
    let rows = vec![health("judge", "sonnet", 20, 14)];

    // Period 1 + 2: breach observed but consecutive count not reached → NoOp.
    let a1 = ctrl.observe(&reg, &rows);
    assert_eq!(a1[0].action, TuningAction::NoOp, "period 1 must not disable");
    let a2 = ctrl.observe(&reg, &rows);
    assert_eq!(a2[0].action, TuningAction::NoOp, "period 2 must not disable");

    // Period 3: third consecutive breach → DisableWithAlert.
    let a3 = ctrl.observe(&reg, &rows);
    assert!(
        matches!(a3[0].action, TuningAction::DisableWithAlert { .. }),
        "period 3 (consecutive=3) must disable, got {:?}",
        a3[0].action
    );
}

#[test]
fn r7_consecutive_streak_resets_on_recovery() {
    let reg = test_registry();
    let cfg = FeedbackConfig {
        auto_escalate: true,
        min_samples: 5,
        consecutive_periods: 2,
    };
    let mut ctrl = FeedbackController::new(cfg);
    let breaching = vec![health("judge", "sonnet", 20, 14)];
    let healthy = vec![health("judge", "sonnet", 20, 2)];

    // breach, then recover (resets streak), then breach again → still only 1
    // consecutive breach → must NOT disable.
    let _ = ctrl.observe(&reg, &breaching);
    let recovered = ctrl.observe(&reg, &healthy);
    assert_eq!(recovered[0].action, TuningAction::NoOp);
    let after_reset = ctrl.observe(&reg, &breaching);
    assert_eq!(
        after_reset[0].action,
        TuningAction::NoOp,
        "streak reset by recovery → single breach must not disable"
    );
}

// ── disable is SOFT (degrade, human review) not a hard delete ─────────────────

#[test]
fn disable_action_is_soft_and_carries_reason() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig {
        auto_escalate: true,
        min_samples: 5,
        consecutive_periods: 1,
    });
    let rows = vec![health("judge", "sonnet", 20, 18)];
    let actions = ctrl.decide(&reg, &rows);
    match &actions[0].action {
        TuningAction::DisableWithAlert { reason, .. } => {
            assert!(!reason.is_empty(), "disable must carry a human-readable reason");
        }
        other => panic!("expected DisableWithAlert, got {other:?}"),
    }
    // Soft: requires human review, never auto-deletes the agent.
    assert!(actions[0].needs_human_review());
    assert!(!actions[0].applied, "disable is never silently auto-applied");
}

// ── 0-call boundary (§9) ──────────────────────────────────────────────────────

#[test]
fn zero_calls_is_noop() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig::aggressive_for_test());
    let rows = vec![health("judge", "qwen3b", 0, 0)];
    let actions = ctrl.decide(&reg, &rows);
    assert_eq!(actions[0].action, TuningAction::NoOp);
}

#[test]
fn empty_health_yields_no_actions() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig::aggressive_for_test());
    let actions = ctrl.decide(&reg, &[]);
    assert!(actions.is_empty());
}

// ── idempotency (§9 "tuning action 幂等") ─────────────────────────────────────

#[test]
fn decide_is_pure_idempotent_no_streak_mutation() {
    let reg = test_registry();
    let ctrl = FeedbackController::new(FeedbackConfig {
        auto_escalate: true,
        min_samples: 5,
        consecutive_periods: 1,
    });
    let rows = vec![health("judge", "qwen3b", 20, 8)];
    let a = ctrl.decide(&reg, &rows);
    let b = ctrl.decide(&reg, &rows);
    assert_eq!(a, b, "decide() must be a pure function of (registry, rows)");
}

// ── model tier ladder ─────────────────────────────────────────────────────────

#[test]
fn next_tier_ladder_is_ordered_and_capped() {
    assert_eq!(next_model_tier("qwen3b"), Some("flash"));
    assert_eq!(next_model_tier("flash"), Some("gpt-4o-mini"));
    assert_eq!(next_model_tier("gpt-4o-mini"), Some("sonnet"));
    assert_eq!(next_model_tier("sonnet"), None, "top tier has no higher tier");
    assert_eq!(next_model_tier("unknown-model"), None);
}

// ── helper used by several tests ──────────────────────────────────────────────

impl FeedbackConfig {
    fn aggressive_for_test() -> Self {
        FeedbackConfig {
            auto_escalate: true,
            min_samples: 1,
            consecutive_periods: 1,
        }
    }
}

// ── Task 3: FeedbackSource trait + multi-source aggregation ───────────────────

/// A stub feedback source for testing aggregation independent of skill_evolution.
struct StubSource {
    name: &'static str,
    signals: Vec<FeedbackSignal>,
}

impl FeedbackSource for StubSource {
    fn source_name(&self) -> &str {
        self.name
    }
    fn collect_signals(&self) -> Vec<FeedbackSignal> {
        self.signals.clone()
    }
}

#[test]
fn controller_aggregates_multiple_feedback_sources() {
    let ctrl = FeedbackController::new(FeedbackConfig::default());
    let s1 = StubSource {
        name: "telemetry",
        signals: vec![FeedbackSignal::new(
            "telemetry",
            "judge",
            SignalSeverity::Warn,
            "fail-rate 40%",
        )],
    };
    let s2 = StubSource {
        name: "skill_evolution",
        signals: vec![FeedbackSignal::new(
            "skill_evolution",
            "self_evolving_skill",
            SignalSeverity::Info,
            "12 unprocessed search-miss signals",
        )],
    };
    let agg = ctrl.aggregate_signals(&[&s1, &s2]);
    assert_eq!(agg.len(), 2);
    // both source names are represented
    let sources: std::collections::BTreeSet<&str> =
        agg.iter().map(|s| s.source.as_str()).collect();
    assert!(sources.contains("telemetry"));
    assert!(sources.contains("skill_evolution"));
}

#[test]
fn skill_evolution_source_reports_pending_signal_count() {
    let store = crate::store::Store::open_memory().unwrap();
    // below EVOLVE_THRESHOLD → Info, not actionable yet
    store.record_skill_signal("专利检索", 0, false).unwrap();
    store.record_skill_signal("合同纠纷", 0, false).unwrap();
    let src = SkillEvolutionFeedback::new(&store);
    let signals = src.collect_signals();
    assert_eq!(src.source_name(), "skill_evolution");
    assert_eq!(signals.len(), 1, "one rollup signal describing pending count");
    assert!(signals[0].detail.contains('2'), "detail mentions the count");
    assert_eq!(signals[0].severity, SignalSeverity::Info);
}

#[test]
fn skill_evolution_source_warns_when_threshold_reached() {
    let store = crate::store::Store::open_memory().unwrap();
    for i in 0..crate::skill_evolution::EVOLVE_THRESHOLD {
        store
            .record_skill_signal(&format!("miss query {i}"), 0, false)
            .unwrap();
    }
    let src = SkillEvolutionFeedback::new(&store);
    let signals = src.collect_signals();
    assert_eq!(signals.len(), 1);
    assert_eq!(
        signals[0].severity,
        SignalSeverity::Warn,
        "threshold reached → an evolution cycle is due (actionable)"
    );
}

#[test]
fn skill_evolution_source_silent_when_no_signals() {
    let store = crate::store::Store::open_memory().unwrap();
    let src = SkillEvolutionFeedback::new(&store);
    assert!(
        src.collect_signals().is_empty(),
        "no search-miss signals → nothing to report"
    );
}

// silence unused-import warnings for types only referenced in some builds
#[allow(unused)]
fn _type_anchors(_: AgentSpec, _: Tier, _: Kind, _: CostClass, _: Handoff) {}
