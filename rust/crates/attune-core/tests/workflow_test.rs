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

// ---------------------------------------------------------------------------
// Task 3: deterministic ops 集成测试（find_overlap）
// ---------------------------------------------------------------------------

use attune_core::store::{ProjectKind, Store};
use attune_core::workflow::ops::run_deterministic;

#[test]
fn deterministic_op_find_overlap_lists_project_files() {
    let store = Store::open_memory().expect("open memory store");
    let p = store
        .create_project("案件 A", ProjectKind::Case)
        .expect("create project");
    store
        .add_file_to_project(&p.id, "file-001", "evidence")
        .expect("add file 1");
    store
        .add_file_to_project(&p.id, "file-002", "pleading")
        .expect("add file 2");

    let mut inputs = BTreeMap::new();
    inputs.insert("project_id".to_string(), json!(p.id));

    let result = run_deterministic("find_overlap", inputs, Some(&store)).expect("op succeeds");
    let obj = result.as_object().expect("object");
    assert!(obj.contains_key("related_files"));
    assert!(obj.contains_key("summary"));

    let related = obj["related_files"].as_array().expect("array");
    assert_eq!(related.len(), 2);
    let roles: Vec<&str> = related
        .iter()
        .map(|v| v["role"].as_str().expect("role str"))
        .collect();
    assert!(roles.contains(&"evidence"));
    assert!(roles.contains(&"pleading"));

    let summary = obj["summary"].as_str().expect("summary str");
    assert!(summary.contains("2"), "summary should mention count: {summary}");
}

#[test]
fn deterministic_op_find_overlap_missing_project_id() {
    let store = Store::open_memory().expect("open");
    let inputs = BTreeMap::new(); // 空：缺 project_id
    let result = run_deterministic("find_overlap", inputs, Some(&store));
    let err = result.expect_err("must fail without project_id");
    assert!(
        err.contains("project_id"),
        "error should mention project_id: {err}"
    );
}
