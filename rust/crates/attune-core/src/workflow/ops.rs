//! Workflow deterministic operations（spec §3.3）。
//!
//! Phase C 实现的 op：
//! - `echo_input` (test-only)
//! - `find_overlap` — 列出该 project 已归档文件作为重叠候选（不调 AI；Sprint 2 接 LLM
//!   做真正的实体重叠分析）
//! - `write_annotation` — 写 AI 批注 + 在 project timeline 留痕
//!
//! ### dek 缺位说明（Phase C 妥协）
//!
//! `Store::create_annotation` 签名要求 `&Key32`（vault DEK，用于加密 content）。
//! 但当前 `run_deterministic` 只接 `Option<&Store>`，没有 dek 通道 —— Sprint 2 把
//! workflow 接到真实运行时（Intent Router + vault context）后会扩展签名。
//!
//! 在此之前，`write_annotation` 只走 timeline append（不需要 dek）+ 返回一个 stub
//! annotation_id。这让 evidence_chain workflow 端到端能跑通，但批注表本身不会有新行。
//! Sprint 2 接入 vault context 时，把 stub 改成真调 `create_annotation(dek, ..)` 即可。

use crate::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

pub fn run_deterministic(
    operation: &str,
    inputs: BTreeMap<String, Value>,
    store: Option<&Store>,
) -> Result<Value, String> {
    match operation {
        "echo_input" => {
            // 测试用：把 input 原样返回
            Ok(serde_json::to_value(inputs).unwrap_or(Value::Null))
        }
        "find_overlap" => find_overlap(inputs, store),
        "write_annotation" => write_annotation(inputs, store),
        _ => Err(format!("unknown deterministic op: {operation}")),
    }
}

/// `find_overlap`：列出 project 已归档文件作为候选。
///
/// 输入：`{ project_id: string, entities?: [...] }`（entities 暂保留，Sprint 2 用）
/// 输出：`{ related_files: [{file_id, role, added_at}], summary: string }`
///
/// 不调 AI；纯 SQL。下游 skill step 拿到 related_files 后再做语义重叠判断。
fn find_overlap(inputs: BTreeMap<String, Value>, store: Option<&Store>) -> Result<Value, String> {
    let project_id = inputs
        .get("project_id")
        .and_then(|v| v.as_str())
        .ok_or("find_overlap: missing project_id")?;
    let store = store.ok_or("find_overlap: store required")?;

    let files = store
        .list_files_for_project(project_id)
        .map_err(|e| format!("find_overlap: {e}"))?;

    let related: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({
                "file_id": f.file_id,
                "role": f.role,
                "added_at": f.added_at,
            })
        })
        .collect();

    Ok(json!({
        "related_files": related,
        "summary": format!("Project 中共有 {} 份已归档文件", related.len()),
    }))
}

/// `write_annotation`：当前 Phase C 实现 = timeline 留痕 + stub annotation_id。
///
/// 输入：`{ item_id, body, source ('user'|'ai', default 'ai'), project_id (optional) }`
/// 输出：`{ annotation_id: string, persisted: bool }`
///
/// `persisted=false` 时调用方知道该批注还没真正写到 annotations 表 —— Sprint 2 接 vault
/// dek 后改为真调 `Store::create_annotation` 并设 `persisted=true`。
fn write_annotation(
    inputs: BTreeMap<String, Value>,
    store: Option<&Store>,
) -> Result<Value, String> {
    let item_id = inputs
        .get("item_id")
        .and_then(|v| v.as_str())
        .ok_or("write_annotation: missing item_id")?;
    let _body = inputs
        .get("body")
        .and_then(|v| v.as_str())
        .ok_or("write_annotation: missing body")?;
    let source = inputs
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("ai");
    if !matches!(source, "user" | "ai") {
        return Err(format!(
            "write_annotation: source must be 'user' or 'ai', got '{source}'"
        ));
    }
    let store = store.ok_or("write_annotation: store required")?;

    // Phase C: stub id；Sprint 2 接 dek 后改为 store.create_annotation 真返回 id。
    let annotation_id = format!("stub-{}", Uuid::new_v4().simple());

    // timeline append 不需要 dek，可以真做。失败不致命（project_id 缺失或不存在均忽略）。
    if let Some(pid) = inputs.get("project_id").and_then(|v| v.as_str()) {
        let _ = store.append_timeline(pid, "ai_inference", None);
    }

    Ok(json!({
        "annotation_id": annotation_id,
        "item_id": item_id,
        "source": source,
        "persisted": false,
    }))
}
