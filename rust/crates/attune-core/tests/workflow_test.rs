//! Workflow runner 集成测试 — 验证 schema → runner 完整链路。

use attune_core::workflow::{parse_workflow_yaml, run_workflow, WorkflowEvent};
use serde_json::json;
use std::collections::BTreeMap;

const SIMPLE_DETERMINISTIC_YAML: &str = r#"
id: test/echo
type: workflow
trigger:
  on: manual
  scope: global
steps:
  - id: noop
    type: deterministic
    operation: echo_input
    input:
      msg: hello
    output: result
"#;

#[test]
fn runner_executes_simple_deterministic_step() {
    let wf = parse_workflow_yaml(SIMPLE_DETERMINISTIC_YAML).expect("parse");
    let event = WorkflowEvent {
        event_type: "manual".into(),
        data: BTreeMap::new(),
    };
    let result = run_workflow(&wf, &event, None).expect("run");
    assert!(result.outputs.contains_key("result"));
    assert_eq!(result.workflow_id, "test/echo");
}

const TWO_STEP_YAML: &str = r#"
id: test/two_step
type: workflow
trigger:
  on: manual
  scope: global
steps:
  - id: first
    type: deterministic
    operation: echo_input
    input:
      x: $event.input_value
    output: first_out

  - id: second
    type: deterministic
    operation: echo_input
    input:
      y: $first.x
    output: second_out
"#;

#[test]
fn runner_resolves_step_ref_chain() {
    let wf = parse_workflow_yaml(TWO_STEP_YAML).expect("parse");
    let mut data = BTreeMap::new();
    data.insert("input_value".into(), json!("foo"));
    let event = WorkflowEvent {
        event_type: "manual".into(),
        data,
    };
    let result = run_workflow(&wf, &event, None).expect("run");
    assert!(result.outputs.contains_key("first_out"));
    assert!(result.outputs.contains_key("second_out"));
}

const FAIL_FAST_YAML: &str = r#"
id: test/fail
type: workflow
trigger:
  on: manual
  scope: global
steps:
  - id: bad_step
    type: deterministic
    operation: nonexistent_op
    input: {}
    output: never
"#;

#[test]
fn runner_fails_fast_on_unknown_op() {
    let wf = parse_workflow_yaml(FAIL_FAST_YAML).expect("parse");
    let event = WorkflowEvent {
        event_type: "manual".into(),
        data: BTreeMap::new(),
    };
    let result = run_workflow(&wf, &event, None);
    assert!(result.is_err(), "unknown op should fail");
}
