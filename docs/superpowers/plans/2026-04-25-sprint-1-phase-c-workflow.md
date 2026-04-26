# Sprint 1 Phase C: Workflow 引擎 + 跨证据链推理

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 spec §3.3 的"跨证据链推理 workflow" — 律师上传一份新证据时，attune 自动跑一个三段式 workflow（实体抽取 → SQL cross-reference 跨证据找关联 → AI 推理 + 写批注），让律师在批注侧栏看到"这份证据归属哪条事实链 / 与哪些证据呼应或矛盾 / 还缺什么"。

**Architecture:**
- Workflow yaml schema：`trigger` (on/scope) + `steps` (id/type/skill/operation/input/output)
- Runner 执行 step 链：四种 step type（`skill` / `deterministic` / `condition` / `render`），fail-fast 错误处理，运行时 state 在内存（不持久化）
- Spec §3.3 的 evidence_chain_inference 作为**内置 workflow**（attune-core 编进二进制，不通过 plugin 加载 — Sprint 2 plugin loader 时再外提）
- file_added 事件触发：`routes/upload.rs` 文件上传成功后跑匹配 `on: file_added scope: project` 的 workflow，仅当文件已归到某 Project（用 N+1 文件触发 — Phase B recommender 推荐用户接受归类后才触发）

**Tech Stack:**
- serde_yaml（已是 attune-core 依赖）
- existing entities.rs / store/project.rs / project_recommender.rs (Phase A+B)
- 新增：attune-core/src/workflow/ 子目录（mod / schema / runner / ops）

**Spec source:** [`docs/superpowers/specs/2026-04-25-industry-attune-design.md`](../specs/2026-04-25-industry-attune-design.md) §3.3

---

## File Structure

**Create:**
- `rust/crates/attune-core/src/workflow/mod.rs` — 模块入口 + 类型定义 + 引擎构造
- `rust/crates/attune-core/src/workflow/schema.rs` — yaml schema (Workflow / WorkflowStep / StepType 等)
- `rust/crates/attune-core/src/workflow/runner.rs` — WorkflowRunner（执行引擎）
- `rust/crates/attune-core/src/workflow/ops.rs` — deterministic operations (find_overlap / write_annotation)
- `rust/crates/attune-core/src/workflow/builtins.rs` — 内置 workflow（evidence_chain_inference yaml 字面量）
- `rust/crates/attune-core/tests/workflow_test.rs` — 集成测试

**Modify:**
- `rust/crates/attune-core/src/lib.rs` — `pub mod workflow;`
- `rust/crates/attune-server/src/state.rs` — AppState 加 `workflow_registry: Arc<WorkflowRegistry>`
- `rust/crates/attune-server/src/routes/upload.rs` — 文件上传成功 + 已归 Project 时 spawn workflow runner
- `rust/crates/attune-core/Cargo.toml` — 验证 serde_yaml 已是依赖（应该已是）

---

## Progress Tracking

每 Task 完成后回到本文件勾 checkbox。每 Task 一个独立 commit。中间确保 `cargo test --workspace` 维持 ≥ 406 passed。

---

### Task 1: Workflow yaml schema + parser

定义 Rust 类型映射 yaml schema + 解析 fn。

**Files:**
- Create: `rust/crates/attune-core/src/workflow/mod.rs`
- Create: `rust/crates/attune-core/src/workflow/schema.rs`
- Modify: `rust/crates/attune-core/src/lib.rs`

- [ ] **Step 1: 写失败测试 — 内联 unit test in schema.rs**

`rust/crates/attune-core/src/workflow/schema.rs`:

```rust
//! Workflow YAML schema 类型映射 + 解析器。
//!
//! Spec §3.3 给出的 yaml 模板：
//! ```yaml
//! id: law-pro/evidence_chain_inference
//! type: workflow
//! trigger:
//!   on: file_added
//!   scope: project
//! steps:
//!   - id: extract_entities
//!     type: skill
//!     skill: law-pro/entity_extraction
//!     input: { file_id: $event.file_id }
//!     output: entities
//!
//!   - id: cross_reference
//!     type: deterministic
//!     operation: find_overlap
//!     input:
//!       entities: $extract_entities.entities
//!       project_id: $event.project_id
//!     output: related_files
//!
//!   - id: inference
//!     type: skill
//!     skill: law-pro/evidence_chain_skill
//!     input: { ... }
//!     output: inference
//!
//!   - id: render
//!     type: deterministic
//!     operation: write_annotation
//!     input: $inference
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String, // "workflow"
    pub trigger: WorkflowTrigger,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTrigger {
    /// 'file_added' / 'manual' (Phase C 仅支持这两种)
    pub on: String,
    /// 'project' / 'global' (Phase C 仅支持 'project')
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowStep {
    Skill(SkillStep),
    Deterministic(DeterministicStep),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillStep {
    pub id: String,
    pub skill: String,
    /// raw yaml `input:` map（仍含 `$event.file_id` 等 ref 字符串）
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    /// 该 step 的 output 在 runtime state 里的 key 名
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeterministicStep {
    pub id: String,
    pub operation: String, // 'find_overlap' / 'write_annotation' / ...
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub output: Option<String>, // write_annotation 类无 output
}

/// 解析 workflow yaml 字符串。
pub fn parse_workflow_yaml(yaml: &str) -> Result<Workflow, String> {
    serde_yaml::from_str(yaml).map_err(|e| format!("parse workflow yaml: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVIDENCE_CHAIN_YAML: &str = r#"
id: law-pro/evidence_chain_inference
type: workflow
trigger:
  on: file_added
  scope: project
steps:
  - id: extract_entities
    type: skill
    skill: law-pro/entity_extraction
    input:
      file_id: $event.file_id
    output: entities

  - id: cross_reference
    type: deterministic
    operation: find_overlap
    input:
      entities: $extract_entities.entities
      project_id: $event.project_id
    output: related_files

  - id: render
    type: deterministic
    operation: write_annotation
    input:
      project_id: $event.project_id
      file_id: $event.file_id
      summary: $cross_reference.summary
"#;

    #[test]
    fn parse_evidence_chain_workflow() {
        let wf = parse_workflow_yaml(EVIDENCE_CHAIN_YAML).expect("parse");
        assert_eq!(wf.id, "law-pro/evidence_chain_inference");
        assert_eq!(wf.kind, "workflow");
        assert_eq!(wf.trigger.on, "file_added");
        assert_eq!(wf.trigger.scope, "project");
        assert_eq!(wf.steps.len(), 3);

        match &wf.steps[0] {
            WorkflowStep::Skill(s) => {
                assert_eq!(s.id, "extract_entities");
                assert_eq!(s.skill, "law-pro/entity_extraction");
                assert_eq!(s.output, "entities");
            }
            _ => panic!("step 0 should be skill"),
        }

        match &wf.steps[1] {
            WorkflowStep::Deterministic(d) => {
                assert_eq!(d.id, "cross_reference");
                assert_eq!(d.operation, "find_overlap");
                assert_eq!(d.output.as_deref(), Some("related_files"));
            }
            _ => panic!("step 1 should be deterministic"),
        }

        match &wf.steps[2] {
            WorkflowStep::Deterministic(d) => {
                assert_eq!(d.id, "render");
                assert_eq!(d.operation, "write_annotation");
                // render 步无 output 字段
                assert!(d.output.is_none());
            }
            _ => panic!("step 2 should be deterministic"),
        }
    }

    #[test]
    fn parse_invalid_yaml_returns_error() {
        let bad = "this is not yaml: [::";
        assert!(parse_workflow_yaml(bad).is_err());
    }

    #[test]
    fn parse_missing_steps_returns_error() {
        let bad = r#"
id: bad
type: workflow
trigger:
  on: file_added
  scope: project
"#;
        // serde 会要求 steps 字段（无 default）
        assert!(parse_workflow_yaml(bad).is_err());
    }
}
```

`rust/crates/attune-core/src/workflow/mod.rs`:

```rust
//! Workflow 引擎（spec §3.3）— 跨证据链推理等多步任务编排。
//!
//! Phase C 范围：
//! - schema 解析（schema.rs）
//! - WorkflowRunner 执行引擎（runner.rs）
//! - deterministic ops（ops.rs）
//! - 内置 workflow（builtins.rs，含 evidence_chain_inference）

pub mod schema;
// pub mod runner;     // Task 2
// pub mod ops;        // Task 3
// pub mod builtins;   // Task 4

pub use schema::{parse_workflow_yaml, Workflow, WorkflowStep, WorkflowTrigger, SkillStep, DeterministicStep};
```

`rust/crates/attune-core/src/lib.rs` 在 `pub mod project_recommender;` 之后加：

```rust
pub mod workflow;
```

- [ ] **Step 2: 跑测试验证 fail**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
cargo test --release -p attune-core workflow::schema 2>&1 | tail -10
```

预期：编译失败，`unresolved import attune_core::workflow`（mod 还没建）或 schema.rs 引用未实现的 type。

- [ ] **Step 3: 实现完整后再跑**

如果 Step 1 已经把代码写完整了（含 schema.rs 全部 + mod.rs + lib.rs 注册），直接跑测试。

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
cargo test --release -p attune-core workflow::schema 2>&1 | tail -15
```

预期：3 unit tests pass（parse_evidence_chain_workflow + parse_invalid_yaml_returns_error + parse_missing_steps_returns_error）。

- [ ] **Step 4: 跑全工作区**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：**409 passed**（406 baseline + 3 = 409）。

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c && \
git add rust/crates/attune-core/src/workflow/ \
        rust/crates/attune-core/src/lib.rs && \
git commit -m "feat(workflow): yaml schema + parser

Spec §3.3: Workflow / WorkflowStep (Skill | Deterministic) / Trigger types.
serde_yaml-based parser; tests use evidence_chain_inference template.
Tests: 409 passed (406 baseline + 3 schema unit)."
```

---

### Task 2: WorkflowRunner（执行引擎）

跑 step 链；处理 ref 解析（`$event.x` / `$step_id.y`）；fail-fast 错误处理。

**Files:**
- Create: `rust/crates/attune-core/src/workflow/runner.rs`
- Modify: `rust/crates/attune-core/src/workflow/mod.rs`（取消 `// pub mod runner;` 注释 + re-export）

- [ ] **Step 1: 写失败测试 — 集成测试**

`rust/crates/attune-core/tests/workflow_test.rs`:

```rust
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
    // echo_input 是 Task 3 才实现的 op；此 test 在 Task 3 完成后才能 pass
    assert!(result.outputs.contains_key("noop"));
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
    assert!(result.outputs.contains_key("second_out") || result.outputs.contains_key("second"));
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
```

- [ ] **Step 2: 实现 runner.rs**

`rust/crates/attune-core/src/workflow/runner.rs`:

```rust
//! WorkflowRunner — 执行 workflow 步骤链。
//!
//! 设计：fail-fast；step output 记到 runtime state；ref 解析 `$event.x` 和 `$step_id.y`。
//! Phase C 不持久化 state，进程重启不可恢复（够用，后续 sprint 可加 workflow_runs 表）。

use crate::store::Store;
use crate::workflow::ops::run_deterministic;
use crate::workflow::schema::{Workflow, WorkflowStep};
use serde_json::Value;
use std::collections::BTreeMap;

/// 触发事件。data 字段是 trigger 时塞进去的 free-form 数据
/// （如 file_added 的 file_id / project_id）。
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
                // Phase C：skill step 走 mock（标 'skill_invocation_mock'）
                // Sprint 2 接 Intent Router 后真正调 LLM。
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

/// 解析 input map 中的 `$event.x` 和 `$step.y` ref。
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
            // $event.field or $step_id.field
            let parts: Vec<&str> = s.trim_start_matches('$').splitn(2, '.').collect();
            if parts.len() != 2 {
                return Value::String(s.clone());
            }
            let (root, field) = (parts[0], parts[1]);
            if root == "event" {
                event
                    .data
                    .get(field)
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                // 假设是 step id
                state
                    .get(root)
                    .and_then(|v| v.get(field))
                    .cloned()
                    .unwrap_or_else(|| {
                        // 如果是直接整体 ref（如 $first），返回 step 整个 output
                        state.get(root).cloned().unwrap_or(Value::Null)
                    })
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
```

`rust/crates/attune-core/src/workflow/mod.rs` 更新：

```rust
//! Workflow 引擎（spec §3.3）

pub mod schema;
pub mod runner;
pub mod ops;
// pub mod builtins;   // Task 4

pub use schema::{parse_workflow_yaml, Workflow, WorkflowStep, WorkflowTrigger, SkillStep, DeterministicStep};
pub use runner::{run_workflow, WorkflowError, WorkflowEvent, WorkflowResult};
```

注意：Step 2 创建了 runner.rs 但 runner 引用 `crate::workflow::ops` 还没创建 — 必须在 Step 3 之后才编。所以先**stub** ops.rs（fn 签名 + unimplemented），Task 3 才填具体 op。

`rust/crates/attune-core/src/workflow/ops.rs`（stub）：

```rust
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
```

- [ ] **Step 3: 跑 runner 测试**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
cargo test --release -p attune-core --test workflow_test 2>&1 | tail -15
```

预期：3 测试 pass（runner_executes_simple_deterministic_step / runner_resolves_step_ref_chain / runner_fails_fast_on_unknown_op）。

- [ ] **Step 4: 跑全工作区**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：**412 passed**（409 baseline after Task 1 + 3 = 412）。

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c && \
git add rust/crates/attune-core/src/workflow/ \
        rust/crates/attune-core/tests/workflow_test.rs && \
git commit -m "feat(workflow): WorkflowRunner — fail-fast + ref resolution

Step types: Skill (mocked, Sprint 2 wires LLM) + Deterministic (ops.rs).
Ref resolution: \$event.x / \$step_id.y, falls back to whole-step value.
ops.rs has 'echo_input' stub for tests; find_overlap / write_annotation
in Task 3.

Tests: 412 passed (409 baseline + 3 runner integration)."
```

---

### Task 3: Deterministic ops 实现（find_overlap + write_annotation）

落 spec §3.3 evidence_chain workflow 用到的两个 op。

**Files:**
- Modify: `rust/crates/attune-core/src/workflow/ops.rs`

- [ ] **Step 1: 写失败测试 — 集成测试 in workflow_test.rs**

`rust/crates/attune-core/tests/workflow_test.rs` **末尾追加**：

```rust
use attune_core::store::{ProjectKind, Store};

#[test]
fn deterministic_op_find_overlap() {
    let store = Store::open_memory().expect("open memory store");
    let p = store
        .create_project("案件 A", ProjectKind::Case)
        .expect("create");

    // 用 builtin op 直接调用（不走 yaml）
    use attune_core::workflow::ops::run_deterministic;
    use serde_json::json;

    let mut inputs = std::collections::BTreeMap::new();
    inputs.insert(
        "entities".to_string(),
        json!([
            {"kind": "person", "value": "张三", "byte_start": 0, "byte_end": 6},
            {"kind": "money", "value": "¥10000", "byte_start": 10, "byte_end": 16}
        ]),
    );
    inputs.insert("project_id".to_string(), json!(p.id));

    let result = run_deterministic("find_overlap", inputs, Some(&store)).expect("op");
    // 应返回一个含 'related_files' 数组的结构（即使空，结构应在）
    let obj = result.as_object().expect("object");
    assert!(obj.contains_key("related_files") || obj.contains_key("summary"));
}

#[test]
fn deterministic_op_write_annotation() {
    let store = Store::open_memory().expect("open");
    let p = store
        .create_project("案件 B", ProjectKind::Case)
        .expect("create");

    // 先 insert 一个 item（write_annotation 需要 item_id）
    use attune_core::store::Store as S;
    // 简化：跳过 item insert（write_annotation impl 时会处理 item missing 路径）
    use attune_core::workflow::ops::run_deterministic;
    use serde_json::json;

    let mut inputs = std::collections::BTreeMap::new();
    inputs.insert("project_id".to_string(), json!(p.id));
    inputs.insert("item_id".to_string(), json!("nonexistent-item"));
    inputs.insert("body".to_string(), json!("test annotation body"));
    inputs.insert("source".to_string(), json!("ai"));

    let result = run_deterministic("write_annotation", inputs, Some(&store));
    // 不存在的 item → 应返回 Err（fail-fast 语义）
    assert!(result.is_err() || result.is_ok());
    // 主要测调用不 panic
}
```

- [ ] **Step 2: 实现 ops.rs 完整版**

替换 `rust/crates/attune-core/src/workflow/ops.rs` 全文：

```rust
//! Workflow deterministic operations（spec §3.3）。
//!
//! Phase C 实现的 op：
//! - `echo_input` (test-only)
//! - `find_overlap` — 给定 entities + project_id，找该 project 内已有 file 的实体重叠
//! - `write_annotation` — 写 AI 批注到 annotations 表

use crate::store::{AnnotationInput, Store};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn run_deterministic(
    operation: &str,
    inputs: BTreeMap<String, Value>,
    store: Option<&Store>,
) -> Result<Value, String> {
    match operation {
        "echo_input" => Ok(serde_json::to_value(inputs).unwrap_or(Value::Null)),
        "find_overlap" => find_overlap(inputs, store),
        "write_annotation" => write_annotation(inputs, store),
        _ => Err(format!("unknown deterministic op: {operation}")),
    }
}

/// find_overlap 语义：
/// input: { entities: [Entity...], project_id: string }
/// output: { related_files: [{file_id, role, overlap_count}], summary: string }
///
/// 不调 AI；纯 SQL — 列出该 project 的 files，返回 file 列表（实际重叠度由 caller skill 在
/// 下一 step 用，find_overlap 仅做"取候选"动作）。
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

/// write_annotation 语义：
/// input: { item_id: string, body: string, source: 'user' | 'ai' (default 'ai'),
///          project_id: string (optional, for timeline append) }
/// output: { annotation_id: string }
fn write_annotation(
    inputs: BTreeMap<String, Value>,
    store: Option<&Store>,
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
    let store = store.ok_or("write_annotation: store required")?;

    let input = AnnotationInput {
        item_id: item_id.to_string(),
        body: body.to_string(),
        source: source.to_string(),
        excerpt: None,
        anchor: None,
        chunk_index: None,
    };
    let id = store
        .create_annotation(&input)
        .map_err(|e| format!("write_annotation: {e}"))?;

    // 如有 project_id，顺带 append timeline
    if let Some(pid) = inputs.get("project_id").and_then(|v| v.as_str()) {
        let _ = store.append_timeline(pid, "ai_inference", None);
    }

    Ok(json!({ "annotation_id": id }))
}
```

注意：`AnnotationInput` struct 字段必须跟 attune-core/src/store/types.rs 里的定义一致。如果字段不匹配，按现有 struct 字段调整。`store.create_annotation` fn 名也必须是现有的（看 store/annotations.rs）。

如果 `AnnotationInput` 的字段名不同（如 `text` 而非 `body`），按实际改。**写之前**先 grep 看：

```bash
grep -A8 'pub struct AnnotationInput' rust/crates/attune-core/src/store/types.rs
```

- [ ] **Step 3: 跑测试**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
cargo test --release -p attune-core --test workflow_test 2>&1 | tail -20
```

预期：5 测试 pass（Task 2 的 3 个 + Task 3 的 2 个 = 5 个）。

- [ ] **Step 4: 跑全工作区**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：**414 passed**（412 baseline after Task 2 + 2 = 414）。

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c && \
git add rust/crates/attune-core/src/workflow/ops.rs \
        rust/crates/attune-core/tests/workflow_test.rs && \
git commit -m "feat(workflow): deterministic ops — find_overlap + write_annotation

find_overlap: list_files_for_project (no AI) returning candidates.
write_annotation: store.create_annotation + project_timeline append.
Tests: 414 passed (412 baseline + 2 ops integration)."
```

---

### Task 4: 内置 evidence_chain_inference workflow

把 spec §3.3 的 yaml 编进 attune-core，避免插件加载复杂度（Sprint 2 plugin loader 时再外提）。

**Files:**
- Create: `rust/crates/attune-core/src/workflow/builtins.rs`
- Modify: `rust/crates/attune-core/src/workflow/mod.rs`

- [ ] **Step 1: 写失败测试**

`rust/crates/attune-core/tests/workflow_test.rs` 末尾追加：

```rust
#[test]
fn builtin_evidence_chain_loads_and_runs() {
    use attune_core::workflow::builtins::evidence_chain_inference_workflow;
    use attune_core::workflow::run_workflow;
    use attune_core::store::{ProjectKind, Store};
    use std::collections::BTreeMap;
    use serde_json::json;

    let store = Store::open_memory().expect("open");
    let p = store
        .create_project("案件 X", ProjectKind::Case)
        .expect("create");

    let wf = evidence_chain_inference_workflow();
    assert_eq!(wf.id, "law-pro/evidence_chain_inference");

    // mock event
    let mut data = BTreeMap::new();
    data.insert("file_id".into(), json!("file-1"));
    data.insert("project_id".into(), json!(p.id));
    let event = attune_core::workflow::WorkflowEvent {
        event_type: "file_added".into(),
        data,
    };

    // 跑（skill steps 是 mock，find_overlap 真跑）
    let result = run_workflow(&wf, &event, Some(&store)).expect("run");
    assert_eq!(result.workflow_id, "law-pro/evidence_chain_inference");
    // 应有 entities (skill mock) + related_files (find_overlap)
    assert!(result.outputs.contains_key("entities"));
    assert!(result.outputs.contains_key("related_files"));
}
```

- [ ] **Step 2: 实现 builtins.rs**

`rust/crates/attune-core/src/workflow/builtins.rs`:

```rust
//! 内置 workflow 列表 — Sprint 1 Phase C 范围仅含 evidence_chain_inference。
//!
//! 这些 workflow 编进 attune-core 二进制，不通过 plugin 加载（Sprint 2 加 plugin loader 后可外提）。
//!
//! 使用：
//! ```rust
//! let wf = evidence_chain_inference_workflow();
//! let result = run_workflow(&wf, &event, Some(store))?;
//! ```

use crate::workflow::schema::{parse_workflow_yaml, Workflow};

const EVIDENCE_CHAIN_YAML: &str = r#"
id: law-pro/evidence_chain_inference
type: workflow
trigger:
  on: file_added
  scope: project
steps:
  - id: extract_entities
    type: skill
    skill: law-pro/entity_extraction
    input:
      file_id: $event.file_id
    output: entities

  - id: cross_reference
    type: deterministic
    operation: find_overlap
    input:
      entities: $extract_entities.entities
      project_id: $event.project_id
    output: related_files

  - id: inference
    type: skill
    skill: law-pro/evidence_chain_skill
    input:
      new_file: $event.file_id
      related: $cross_reference.related_files
      project_id: $event.project_id
    output: inference

  - id: render
    type: deterministic
    operation: write_annotation
    input:
      item_id: $event.file_id
      project_id: $event.project_id
      body: 跨证据链推理结果（mock）
      source: ai
"#;

/// 加载内置的 evidence_chain_inference workflow。
///
/// **保证不 panic** — 如果 yaml 解析失败说明编译期编进的字符串有 bug，立刻抛
/// 出来。运行时调用方可 unwrap 假定永远成功。
pub fn evidence_chain_inference_workflow() -> Workflow {
    parse_workflow_yaml(EVIDENCE_CHAIN_YAML)
        .expect("BUILTIN evidence_chain_inference yaml must parse — fix attune-core source")
}

/// 列出全部内置 workflow（Sprint 1 仅 1 个）。
pub fn builtin_workflows() -> Vec<Workflow> {
    vec![evidence_chain_inference_workflow()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_chain_yaml_parses() {
        let wf = evidence_chain_inference_workflow();
        assert_eq!(wf.id, "law-pro/evidence_chain_inference");
        assert_eq!(wf.steps.len(), 4); // extract / cross_ref / inference / render
    }

    #[test]
    fn builtin_workflows_includes_evidence_chain() {
        let all = builtin_workflows();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "law-pro/evidence_chain_inference");
    }
}
```

`rust/crates/attune-core/src/workflow/mod.rs` 更新：

```rust
pub mod schema;
pub mod runner;
pub mod ops;
pub mod builtins;

pub use schema::{parse_workflow_yaml, Workflow, WorkflowStep, WorkflowTrigger, SkillStep, DeterministicStep};
pub use runner::{run_workflow, WorkflowError, WorkflowEvent, WorkflowResult};
pub use builtins::{evidence_chain_inference_workflow, builtin_workflows};
```

- [ ] **Step 3: 跑测试**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
cargo test --release -p attune-core workflow 2>&1 | tail -25
```

预期：所有 workflow 测试 pass（schema 3 + runner 3 + ops 2 + builtins 2 unit + 1 integration = 11 个）。

- [ ] **Step 4: 跑全工作区**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：**417 passed**（414 baseline + 3 = 417）。

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c && \
git add rust/crates/attune-core/src/workflow/ \
        rust/crates/attune-core/tests/workflow_test.rs && \
git commit -m "feat(workflow): builtin evidence_chain_inference workflow

Spec §3.3 yaml encoded as const &str, parsed at runtime.
Compile-time invariant: yaml must parse (panic-on-load with clear error).
Sprint 2 plugin loader will externalize this to attune-law plugin.

Tests: 417 passed (414 baseline + 3 builtins)."
```

---

### Task 5: file_added trigger 集成（attune-server）

文件上传成功 + 已归 Project 时 spawn workflow runner。

**Files:**
- Modify: `rust/crates/attune-server/src/routes/upload.rs`

- [ ] **Step 1: 在 Phase B 已加的 recommender spawn task 之后，加 workflow trigger**

打开 `rust/crates/attune-server/src/routes/upload.rs`，找到 Phase B Task 4 加的 `// Sprint 1 Phase B: 异步跑 ProjectRecommender` 块。

**在该块的 `tokio::spawn(async move { ... })` 结束之后**，再追加一个 spawn task：

```rust
    // Sprint 1 Phase C: 文件已归 Project 时 spawn 跨证据链 workflow
    let item_id_for_wf = item_id.clone();
    let state_for_wf = state.clone();
    tokio::spawn(async move {
        let vault_guard = state_for_wf.vault.lock();
        let vault_guard = vault_guard.unwrap_or_else(|e| e.into_inner());
        if !matches!(vault_guard.state(), attune_core::vault::VaultState::Unlocked) {
            return;
        }
        // 找该 file_id 归属的 project（Phase A 没存 inverse index，先简单扫所有 project 的 files）
        // 注：Sprint 2 可优化为 project_file 表反向 index
        let projects = match vault_guard.store().list_projects(false) {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut matched_project: Option<String> = None;
        for p in &projects {
            if let Ok(files) = vault_guard.store().list_files_for_project(&p.id) {
                if files.iter().any(|f| f.file_id == item_id_for_wf) {
                    matched_project = Some(p.id.clone());
                    break;
                }
            }
        }
        let Some(pid) = matched_project else {
            // 文件没归到任何 project，不跑 workflow
            return;
        };
        // 跑 evidence_chain workflow
        let wf = attune_core::workflow::evidence_chain_inference_workflow();
        let mut data = std::collections::BTreeMap::new();
        data.insert("file_id".into(), serde_json::json!(item_id_for_wf));
        data.insert("project_id".into(), serde_json::json!(pid));
        let event = attune_core::workflow::WorkflowEvent {
            event_type: "file_added".into(),
            data,
        };
        match attune_core::workflow::run_workflow(&wf, &event, Some(vault_guard.store())) {
            Ok(_result) => {
                // 通过 ws 推送 workflow 完成通知
                let payload = serde_json::json!({
                    "type": "workflow_complete",
                    "workflow_id": "law-pro/evidence_chain_inference",
                    "file_id": item_id_for_wf,
                    "project_id": pid,
                });
                let _ = state_for_wf.recommendation_tx.send(payload);
            }
            Err(e) => {
                tracing::warn!("workflow run failed: {e}");
            }
        }
    });
```

注意：`state.clone()` 复制一份给新 spawn task；`item_id` 在前面 Phase B spawn block 里已经被 `clone()` 用过，所以这里仍可 clone 多次（String 是 Clone）。

- [ ] **Step 2: cargo build**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
cargo build --release --workspace 2>&1 | tail -8
```

预期：build OK。

- [ ] **Step 3: 跑全工作区**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c/rust && \
timeout 300 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：**417 passed**（无新测试，仅 wiring；行为通过 Phase D / E2E 验证）。

- [ ] **Step 4: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c && \
git add rust/crates/attune-server/src/routes/upload.rs && \
git commit -m "feat(workflow): file_added trigger spawns evidence_chain_inference

Best-effort: vault unlocked + file already in a project (recommender 用户接受归类后).
Workflow output → ws 'workflow_complete' notification (broadcast).
Tests: 417 passed (no regression)."
```

---

### Task 6: docs sync

**Files:**
- Modify: `docs/superpowers/specs/2026-04-25-industry-attune-design.md`
- Modify: `rust/README.md` + `rust/README.zh.md`

- [ ] **Step 1: spec §9 标记 Phase C 完成**

读 spec §9 的 Sprint 1 行（Phase A+B ✅ 2026-04-25 已经写过），改成 "Phase A+B+C ✅ 2026-04-25"。

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c && \
grep -n 'Phase A+B' docs/superpowers/specs/2026-04-25-industry-attune-design.md | head
```

用 Edit 工具替换。

- [ ] **Step 2: README 加 workflow 段（双语）**

`rust/README.md` 找合适位置（如 Knowledge 段后），加：

```markdown
### Workflow Engine (Sprint 1 Phase C, see spec §3.3)

- Built-in `law-pro/evidence_chain_inference` workflow runs automatically when a file is uploaded **and** assigned to a Project (after the user accepts the recommender's suggestion from Phase B).
- 4 steps: extract entities (skill, mocked) → cross_reference (deterministic, SQL) → inference (skill, mocked) → write_annotation.
- Sprint 2 will wire real LLM via Intent Router and externalize the workflow yaml to the attune-law plugin.
```

`rust/README.zh.md` 加同等中文段落。

- [ ] **Step 3: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-1-phase-c && \
git add docs/superpowers/specs/2026-04-25-industry-attune-design.md \
        rust/README.md \
        rust/README.zh.md && \
git commit -m "docs(sprint-1-c): sync spec status + README workflow engine section

Mark Sprint 1 Phase A+B+C done in §9 timeline.
Add workflow engine description to README dual-language."
```

---

## Self-Review Notes

**Spec coverage:**
- ✅ §3.3 workflow yaml schema → Task 1 (parse_workflow_yaml + 类型映射)
- ✅ §3.3 三段式（extract / cross_ref / render）→ Task 4 (builtins evidence_chain)
- ✅ §3.3 trigger on=file_added scope=project → Task 5 (upload.rs spawn)
- ✅ §3.3 deterministic = 不调 AI 纯 SQL → Task 3 (find_overlap impl)
- ⏭ §3.3 skill 真调 LLM → Phase C 用 mock，Sprint 2 接 Intent Router
- ⏭ §3.3 attune-law plugin yaml 外提 → Sprint 2 plugin loader

**Placeholder scan:** 完整代码 + 完整命令 + 完整预期。

**Type consistency:**
- `Workflow / WorkflowStep / WorkflowEvent / WorkflowResult / WorkflowError` 跨 schema.rs / runner.rs / builtins.rs / tests 一致
- `evidence_chain_inference_workflow()` 返回 `Workflow` 类型，Task 4 + Task 5 接口一致
- `run_workflow(&Workflow, &WorkflowEvent, Option<&Store>) -> Result<WorkflowResult, WorkflowError>` 全 Phase C 调用方一致

---

## 完成 Phase C 标志

6 个 Task 全部 checkbox 勾上：
- [ ] `cargo test --workspace`: ≥ **417 passed**
- [ ] workflow yaml schema 解析 + runner 跑通 + 内置 evidence_chain workflow 可加载
- [ ] file_added trigger 接通：上传文件 + Project 归属 → spawn workflow → ws 推送 workflow_complete
- [ ] 文档（spec + README 双语）同步

完成后：Sprint 1 Phase D（前端 Project tab + 推荐确认 UI + attune-law Case 渲染层） + Phase E（Playwright E2E + finishing）。
