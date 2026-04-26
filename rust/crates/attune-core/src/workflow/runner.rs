//! WorkflowRunner — 执行 workflow 步骤链。
//!
//! 设计：fail-fast；step output 记到 runtime state；ref 解析 `$event.x` 和 `$step_id.y`。
//! Phase C 不持久化 state（进程重启不可恢复）。

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
                let output_value = run_deterministic(&d.operation, resolved, store)
                    .map_err(|e| WorkflowError::StepFailed {
                        step_id: d.id.clone(),
                        cause: e,
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
