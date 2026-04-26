//! Workflow deterministic operations（Phase C：先 stub，Task 3 填实现）。

use crate::store::Store;
use serde_json::Value;
use std::collections::BTreeMap;

pub fn run_deterministic(
    operation: &str,
    inputs: BTreeMap<String, Value>,
    _store: Option<&Store>,
) -> Result<Value, String> {
    match operation {
        "echo_input" => {
            // 测试用：把 input 原样返回
            Ok(serde_json::to_value(inputs).unwrap_or(Value::Null))
        }
        // Task 3 加：find_overlap / write_annotation
        _ => Err(format!("unknown deterministic op: {operation}")),
    }
}
