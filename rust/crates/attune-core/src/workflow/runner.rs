//! WorkflowRunner — 执行 workflow 步骤链。
//!
//! 设计：fail-fast；step output 记到 runtime state；ref 解析 `$event.x` 和 `$step_id.y`。
//! Phase C 不持久化 state（进程重启不可恢复）。

use crate::crypto::Key32;
use crate::store::Store;
use crate::workflow::ops::run_deterministic;
use crate::workflow::schema::{Workflow, WorkflowStep};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct WorkflowEvent {
    pub event_type: String,
    pub data: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct WorkflowResult {
    pub workflow_id: String,
    pub outputs: BTreeMap<String, Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("step {step_id} failed: {cause}")]
    StepFailed { step_id: String, cause: String },
    #[error("unknown ref: {0}")]
    UnknownRef(String),
    #[error("unknown operation: {0}")]
    UnknownOp(String),
    #[error("missing required input: {0}")]
    MissingInput(String),
}

pub fn run_workflow(
    wf: &Workflow,
    event: &WorkflowEvent,
    store: Option<&Store>,
    dek: Option<&Key32>,
) -> Result<WorkflowResult, WorkflowError> {
    let mut state: BTreeMap<String, Value> = BTreeMap::new();

    for step in &wf.steps {
        match step {
            WorkflowStep::Skill(s) => {
                // Phase C: skill step 走 mock。Sprint 2 接 Intent Router 后真正调 LLM。
                let resolved = resolve_inputs(&s.input, &state, event);
                let output_value = serde_json::json!({
                    "skill": s.skill,
                    "resolved_input": resolved,
                    "mock": true,
                });
                state.insert(s.output.clone(), output_value);
            }
            WorkflowStep::Deterministic(d) => {
                let resolved = resolve_inputs(&d.input, &state, event);
                let output_value =
                    run_deterministic(&d.operation, resolved, store, dek).map_err(|e| {
                        WorkflowError::StepFailed {
                            step_id: d.id.clone(),
                            cause: e,
                        }
                    })?;
                if let Some(out_key) = &d.output {
                    state.insert(out_key.clone(), output_value);
                }
            }
        }
    }

    Ok(WorkflowResult {
        workflow_id: wf.id.clone(),
        outputs: state,
    })
}

fn resolve_inputs(
    input: &BTreeMap<String, serde_yaml::Value>,
    state: &BTreeMap<String, Value>,
    event: &WorkflowEvent,
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (k, v) in input {
        let resolved = resolve_value(v, state, event);
        out.insert(k.clone(), resolved);
    }
    out
}

fn resolve_value(
    v: &serde_yaml::Value,
    state: &BTreeMap<String, Value>,
    event: &WorkflowEvent,
) -> Value {
    match v {
        serde_yaml::Value::String(s) if s.starts_with('$') => {
            let parts: Vec<&str> = s.trim_start_matches('$').splitn(2, '.').collect();
            if parts.len() != 2 {
                return Value::String(s.clone());
            }
            let (root, field) = (parts[0], parts[1]);
            if root == "event" {
                event.data.get(field).cloned().unwrap_or(Value::Null)
            } else {
                state
                    .get(root)
                    .and_then(|v| v.get(field))
                    .cloned()
                    .unwrap_or_else(|| state.get(root).cloned().unwrap_or(Value::Null))
            }
        }
        serde_yaml::Value::String(s) => Value::String(s.clone()),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }
        serde_yaml::Value::Bool(b) => Value::Bool(*b),
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Sequence(seq) => {
            Value::Array(seq.iter().map(|v| resolve_value(v, state, event)).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                if let Some(key) = k.as_str() {
                    obj.insert(key.to_string(), resolve_value(v, state, event));
                }
            }
            Value::Object(obj)
        }
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests covering F-13-WORKFLOW (runner.rs step execution + ref resolution).
    //!
    //! Strategy: focus on the runner control flow + `$event.x` / `$step_id.y` ref
    //! resolution. Skill steps go through mock path (no LLM). Deterministic steps
    //! exercise echo_input op (no Store / DEK needed). Real Store-backed ops are
    //! covered by integration tests (`tests/workflow_test.rs`).
    use super::*;
    use crate::workflow::schema::{
        DeterministicStep, SkillStep, Workflow, WorkflowStep, WorkflowTrigger,
    };

    fn empty_event() -> WorkflowEvent {
        WorkflowEvent {
            event_type: "test".into(),
            data: BTreeMap::new(),
        }
    }

    fn workflow_with_steps(id: &str, steps: Vec<WorkflowStep>) -> Workflow {
        Workflow {
            id: id.into(),
            kind: "workflow".into(),
            trigger: WorkflowTrigger {
                on: "manual".into(),
                scope: "global".into(),
            },
            steps,
        }
    }

    fn det_step(id: &str, op: &str, output: Option<&str>) -> WorkflowStep {
        WorkflowStep::Deterministic(DeterministicStep {
            id: id.into(),
            operation: op.into(),
            input: BTreeMap::new(),
            output: output.map(String::from),
        })
    }

    // ── Empty workflow ──────────────────────────────────────────────────────

    #[test]
    fn empty_workflow_returns_empty_outputs() {
        let wf = workflow_with_steps("wf/empty", vec![]);
        let result = run_workflow(&wf, &empty_event(), None, None).expect("ok");
        assert_eq!(result.workflow_id, "wf/empty");
        assert!(result.outputs.is_empty());
    }

    // ── Skill step (mock) ───────────────────────────────────────────────────
    // covers: F-13-WORKFLOW skill mock path until Sprint 2 LLM hookup

    #[test]
    fn skill_step_writes_mock_output_to_state() {
        let skill = WorkflowStep::Skill(SkillStep {
            id: "s1".into(),
            skill: "examples/extract".into(),
            input: BTreeMap::new(),
            output: "extracted".into(),
        });
        let wf = workflow_with_steps("wf/skill", vec![skill]);
        let result = run_workflow(&wf, &empty_event(), None, None).expect("ok");

        let extracted = result.outputs.get("extracted").expect("output key present");
        assert_eq!(extracted["skill"], serde_json::json!("examples/extract"));
        assert_eq!(extracted["mock"], serde_json::json!(true));
    }

    // ── Deterministic step ──────────────────────────────────────────────────

    #[test]
    fn deterministic_echo_input_passes_resolved_to_output() {
        // input { foo: $event.bar } + event.bar = "hello" → state.echo_out.foo = "hello"
        let mut input: BTreeMap<String, serde_yaml::Value> = BTreeMap::new();
        input.insert("foo".into(), serde_yaml::Value::String("$event.bar".into()));

        let step = WorkflowStep::Deterministic(DeterministicStep {
            id: "s1".into(),
            operation: "echo_input".into(),
            input,
            output: Some("echo_out".into()),
        });
        let wf = workflow_with_steps("wf/echo", vec![step]);

        let mut event = empty_event();
        event.data.insert("bar".into(), serde_json::json!("hello"));

        let result = run_workflow(&wf, &event, None, None).expect("ok");
        let out = result.outputs.get("echo_out").expect("output present");
        assert_eq!(out["foo"], serde_json::json!("hello"));
    }

    #[test]
    fn deterministic_step_without_output_key_does_not_pollute_state() {
        // output: None → step runs but writes nothing to state
        let step = det_step("s1", "echo_input", None);
        let wf = workflow_with_steps("wf/no_output", vec![step]);
        let result = run_workflow(&wf, &empty_event(), None, None).expect("ok");
        assert!(result.outputs.is_empty(), "step without output key should not pollute state");
    }

    #[test]
    fn deterministic_step_failure_propagates_with_step_id() {
        // unknown op → error must carry step id so users can locate the failing step
        let step = det_step("my_failing_step", "no_such_op", Some("never_set"));
        let wf = workflow_with_steps("wf/fail", vec![step]);
        let err = run_workflow(&wf, &empty_event(), None, None).unwrap_err();
        match err {
            WorkflowError::StepFailed { step_id, cause } => {
                assert_eq!(step_id, "my_failing_step");
                assert!(cause.contains("unknown deterministic op"), "got cause: {cause}");
            }
            _ => panic!("expected StepFailed, got {err:?}"),
        }
    }

    // ── Ref resolution ──────────────────────────────────────────────────────
    // covers: F-13-WORKFLOW $event / $step ref grammar

    #[test]
    fn event_ref_resolves_to_event_data() {
        let state = BTreeMap::new();
        let mut event = empty_event();
        event.data.insert("file_id".into(), serde_json::json!("f_123"));

        let val = resolve_value(
            &serde_yaml::Value::String("$event.file_id".into()),
            &state,
            &event,
        );
        assert_eq!(val, serde_json::json!("f_123"));

        // Missing event field → Null (not panic, not error — "graceful")
        let null_val = resolve_value(
            &serde_yaml::Value::String("$event.missing".into()),
            &state,
            &event,
        );
        assert_eq!(null_val, serde_json::Value::Null);
    }

    #[test]
    fn step_ref_resolves_to_state_field() {
        let mut state = BTreeMap::new();
        state.insert(
            "prev_step".into(),
            serde_json::json!({"summary": "5 files matched"}),
        );

        let val = resolve_value(
            &serde_yaml::Value::String("$prev_step.summary".into()),
            &state,
            &empty_event(),
        );
        assert_eq!(val, serde_json::json!("5 files matched"));
    }

    #[test]
    fn step_ref_falls_back_to_full_value_when_field_absent() {
        // $prev_step.nonexistent → falls back to the whole prev_step value (not Null)
        // This is documented runner.rs:107-111 behavior — preserve forward compat.
        let mut state = BTreeMap::new();
        state.insert(
            "prev_step".into(),
            serde_json::json!({"summary": "x", "count": 3}),
        );

        let val = resolve_value(
            &serde_yaml::Value::String("$prev_step.notafield".into()),
            &state,
            &empty_event(),
        );
        // Falls back to full state[prev_step]
        assert_eq!(val["summary"], serde_json::json!("x"));
        assert_eq!(val["count"], serde_json::json!(3));
    }

    #[test]
    fn malformed_ref_without_dot_treated_as_literal_string() {
        // $foo (no dot) → stays as literal "$foo" since splitn(2, '.') returns 1 part
        let val = resolve_value(
            &serde_yaml::Value::String("$foo".into()),
            &BTreeMap::new(),
            &empty_event(),
        );
        assert_eq!(val, serde_json::json!("$foo"));
    }

    #[test]
    fn non_ref_string_passes_through() {
        let val = resolve_value(
            &serde_yaml::Value::String("plain text".into()),
            &BTreeMap::new(),
            &empty_event(),
        );
        assert_eq!(val, serde_json::json!("plain text"));
    }

    #[test]
    fn primitives_resolve_correctly() {
        // numbers, bools, nulls — all pass through resolve_value
        let int_val = resolve_value(
            &serde_yaml::Value::Number(serde_yaml::Number::from(42)),
            &BTreeMap::new(),
            &empty_event(),
        );
        assert_eq!(int_val, serde_json::json!(42));

        let bool_val = resolve_value(
            &serde_yaml::Value::Bool(true),
            &BTreeMap::new(),
            &empty_event(),
        );
        assert_eq!(bool_val, serde_json::json!(true));

        let null_val = resolve_value(
            &serde_yaml::Value::Null,
            &BTreeMap::new(),
            &empty_event(),
        );
        assert_eq!(null_val, serde_json::Value::Null);
    }

    #[test]
    fn nested_mapping_resolves_with_refs() {
        // input: { outer: { ref: $event.x, lit: "hi" } }
        let mut event = empty_event();
        event.data.insert("x".into(), serde_json::json!("dynamic"));

        let mut inner = serde_yaml::Mapping::new();
        inner.insert(
            serde_yaml::Value::String("ref".into()),
            serde_yaml::Value::String("$event.x".into()),
        );
        inner.insert(
            serde_yaml::Value::String("lit".into()),
            serde_yaml::Value::String("hi".into()),
        );

        let val = resolve_value(
            &serde_yaml::Value::Mapping(inner),
            &BTreeMap::new(),
            &event,
        );
        assert_eq!(val["ref"], serde_json::json!("dynamic"));
        assert_eq!(val["lit"], serde_json::json!("hi"));
    }
}
