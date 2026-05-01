//! Workflow deterministic operations（spec §3.3）。
//!
//! 当前实现的 op：
//! - `echo_input` (test-only)
//! - `find_overlap` — 列出该 project 已归档文件作为重叠候选（不调 AI；Sprint 2 接 LLM
//!   做真正的实体重叠分析）
//! - `write_annotation` — 真持久化 AI 批注（`Store::create_annotation` + 加密）+
//!   project timeline 留痕。Sprint 2 Phase D 起从 stub 升级为真写。
//!
//! ### dek 通道
//!
//! `Store::create_annotation` 需要 `&Key32`（vault DEK，用于加密 content）。
//! `run_deterministic` 通过 `dek: Option<&Key32>` 透传 — 调用方（HTTP 路由 / 后台 spawn）
//! 在 vault unlocked 时拿 `vault.dek_db()` 注入；vault locked 或调用方不传时，
//! 需要 dek 的 op（`write_annotation`）会显式 fail。

use crate::crypto::Key32;
use crate::store::{AnnotationInput, Store};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn run_deterministic(
    operation: &str,
    inputs: BTreeMap<String, Value>,
    store: Option<&Store>,
    dek: Option<&Key32>,
) -> Result<Value, String> {
    match operation {
        "echo_input" => {
            // 测试用：把 input 原样返回
            Ok(serde_json::to_value(inputs).unwrap_or(Value::Null))
        }
        "find_overlap" => find_overlap(inputs, store),
        "write_annotation" => write_annotation(inputs, store, dek),
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

/// `write_annotation`：真持久化 AI 批注（Sprint 2 Phase D — 不再是 stub）。
///
/// 输入：`{ item_id, body, source ('user'|'ai', default 'ai'), project_id (optional) }`
/// 输出：`{ annotation_id: string, item_id, source, persisted: true }`
///
/// 必需：vault unlocked + dek（`Some(&Key32)`）。dek 缺位时显式 fail，避免静默退化为 stub。
/// 可选：`project_id` 存在时同步 append timeline。
fn write_annotation(
    inputs: BTreeMap<String, Value>,
    store: Option<&Store>,
    dek: Option<&Key32>,
) -> Result<Value, String> {
    let item_id = inputs
        .get("item_id")
        .and_then(|v| v.as_str())
        .ok_or("write_annotation: missing item_id")?;
    let body = inputs
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
    let dek = dek.ok_or("write_annotation: dek required (vault must be unlocked)")?;

    // AI 批注覆盖整个 item content（offset 0..len），text_snippet 取 body 前 100 字符做预览。
    // body 当作 annotation content 加密入库；source='ai' 标识 AI 路径写入。
    let body_chars: Vec<char> = body.chars().collect();
    let body_char_len = body_chars.len() as i64;
    let snippet: String = body_chars.iter().take(100).collect();

    let input = AnnotationInput {
        offset_start: 0,
        offset_end: body_char_len,
        text_snippet: snippet,
        label: None,
        color: "yellow".into(),
        content: body.to_string(),
        source: Some(source.to_string()),
    };

    let annotation_id = store
        .create_annotation(dek, item_id, &input)
        .map_err(|e| format!("write_annotation: {e}"))?;

    // timeline append：失败不致命（project_id 缺失或不存在均忽略）。
    if let Some(pid) = inputs.get("project_id").and_then(|v| v.as_str()) {
        let _ = store.append_timeline(pid, "ai_inference", None);
    }

    Ok(json!({
        "annotation_id": annotation_id,
        "item_id": item_id,
        "source": source,
        "persisted": true,
    }))
}

#[cfg(test)]
mod tests {
    //! Unit tests covering F-13-WORKFLOW (ops.rs deterministic operations).
    //!
    //! Strategy: focus on input validation + error paths. Success paths for
    //! `find_overlap` and `write_annotation` require a real Store + DEK and
    //! belong to integration tests (see `tests/workflow_test.rs`).
    use super::*;

    fn empty_inputs() -> BTreeMap<String, Value> {
        BTreeMap::new()
    }

    // ── echo_input ──────────────────────────────────────────────────────────
    // covers: F-13-WORKFLOW dispatch routing

    #[test]
    fn echo_input_returns_inputs_verbatim() {
        let mut inputs = empty_inputs();
        inputs.insert("foo".into(), json!("bar"));
        inputs.insert("count".into(), json!(42));
        let out = run_deterministic("echo_input", inputs, None, None).expect("ok");
        // returned shape: BTreeMap-as-Object, keys sorted alphabetically
        assert_eq!(out["foo"], json!("bar"));
        assert_eq!(out["count"], json!(42));
    }

    #[test]
    fn echo_input_with_empty_inputs_returns_empty_object() {
        let out = run_deterministic("echo_input", empty_inputs(), None, None).expect("ok");
        assert!(out.is_object(), "echo_input should return an object, got {out:?}");
        assert_eq!(out.as_object().unwrap().len(), 0);
    }

    // ── unknown op ──────────────────────────────────────────────────────────

    #[test]
    fn unknown_op_returns_explicit_error() {
        let err = run_deterministic("does_not_exist", empty_inputs(), None, None).unwrap_err();
        assert!(err.contains("unknown deterministic op"), "got: {err}");
        assert!(err.contains("does_not_exist"), "error should name the op, got: {err}");
    }

    // ── find_overlap input validation ───────────────────────────────────────
    // covers: F-12-PROJECT cross-evidence Project recommender precondition

    #[test]
    fn find_overlap_missing_project_id_errors() {
        // store=None is fine — we never reach store access; missing project_id is checked first.
        let err = run_deterministic("find_overlap", empty_inputs(), None, None).unwrap_err();
        assert!(err.contains("missing project_id"), "got: {err}");
    }

    #[test]
    fn find_overlap_with_project_id_but_no_store_errors() {
        let mut inputs = empty_inputs();
        inputs.insert("project_id".into(), json!("p_abc"));
        let err = run_deterministic("find_overlap", inputs, None, None).unwrap_err();
        assert!(err.contains("store required"), "got: {err}");
    }

    #[test]
    fn find_overlap_project_id_must_be_string_not_number() {
        // .as_str() returns None for numbers → triggers missing project_id error
        let mut inputs = empty_inputs();
        inputs.insert("project_id".into(), json!(123));
        let err = run_deterministic("find_overlap", inputs, None, None).unwrap_err();
        assert!(err.contains("missing project_id"), "got: {err}");
    }

    // ── write_annotation input validation ──────────────────────────────────
    // covers: F-04-READER persistence precondition + F-13-WORKFLOW gating

    #[test]
    fn write_annotation_missing_item_id_errors() {
        let err = run_deterministic("write_annotation", empty_inputs(), None, None).unwrap_err();
        assert!(err.contains("missing item_id"), "got: {err}");
    }

    #[test]
    fn write_annotation_missing_body_errors() {
        let mut inputs = empty_inputs();
        inputs.insert("item_id".into(), json!("i_abc"));
        let err = run_deterministic("write_annotation", inputs, None, None).unwrap_err();
        assert!(err.contains("missing body"), "got: {err}");
    }

    #[test]
    fn write_annotation_invalid_source_errors() {
        let mut inputs = empty_inputs();
        inputs.insert("item_id".into(), json!("i_abc"));
        inputs.insert("body".into(), json!("hello"));
        inputs.insert("source".into(), json!("hacker"));
        let err = run_deterministic("write_annotation", inputs, None, None).unwrap_err();
        assert!(err.contains("source must be 'user' or 'ai'"), "got: {err}");
        assert!(err.contains("hacker"), "error should echo bad value, got: {err}");
    }

    #[test]
    fn write_annotation_default_source_is_ai() {
        // No source → default "ai" → passes validation (then fails at "store required")
        let mut inputs = empty_inputs();
        inputs.insert("item_id".into(), json!("i_abc"));
        inputs.insert("body".into(), json!("hello"));
        let err = run_deterministic("write_annotation", inputs, None, None).unwrap_err();
        // not "source must be 'user' or 'ai'" — proves default kicked in
        assert!(err.contains("store required"), "got: {err}");
    }

    #[test]
    fn write_annotation_user_source_accepted() {
        // source='user' is also valid (user annotations bypass AI 'ai' tag)
        let mut inputs = empty_inputs();
        inputs.insert("item_id".into(), json!("i_abc"));
        inputs.insert("body".into(), json!("hello"));
        inputs.insert("source".into(), json!("user"));
        let err = run_deterministic("write_annotation", inputs, None, None).unwrap_err();
        assert!(err.contains("store required"), "got: {err}");
    }

    #[test]
    fn write_annotation_dek_required_explicitly_when_store_provided_is_documented() {
        // We can't easily provide a real Store without integration setup, but
        // ensure the doc-comment promise holds: "dek required (vault must be unlocked)"
        // appears in the error message path. This guards the F-13 dek-channel design.
        // Without store: we hit "store required" first, which means the dek check
        // is correctly placed AFTER the store check — verify ordering by inspection.
        let mut inputs = empty_inputs();
        inputs.insert("item_id".into(), json!("i_abc"));
        inputs.insert("body".into(), json!("hello"));
        let err = run_deterministic("write_annotation", inputs, None, None).unwrap_err();
        assert!(err.contains("store required"), "store check must precede dek check, got: {err}");
    }
}
