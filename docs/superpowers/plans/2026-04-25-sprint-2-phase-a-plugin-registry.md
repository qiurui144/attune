# Sprint 2 Phase A: PluginRegistry + Workflow Type Loading

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 让 attune-core 能从文件系统加载 attune-pro 风格的 plugin 目录（含 workflow.yaml），通过 PluginRegistry 索引按 trigger / type 查询，使 file_added 事件能匹配并跑 plugin 注册的 workflow（不再 hardcode）。

**Architecture:**
- 扩展 `plugin_loader.rs::PluginManifest`：加 `type: workflow` 支持（仅 manifest 元数据，workflow 实际内容在 plugin dir 下的 `workflows/*.yaml`）
- 新增 `attune-core/src/plugin_registry.rs`：HashMap<plugin_id, LoadedPlugin> + by_type + by_trigger 查询
- 新增 `LoadedWorkflow` 类型 — 包装现有 `workflow::schema::Workflow` + `plugin_id`
- attune-server `state.rs` 加 `plugin_registry: Arc<PluginRegistry>`，启动时扫描 `~/.local/share/attune/plugins/`
- `routes/upload.rs` file_added trigger 改为：从 registry 取所有 trigger.on=file_added 的 workflow，逐个跑（best-effort spawn）

**Tech Stack:**
- existing serde_yaml + glob crate（如未在 Cargo.toml 加 `glob = "0.3"`）
- existing workflow schema/runner/ops（attune-core/src/workflow/）

**Spec source:** [`docs/superpowers/specs/2026-04-25-industry-attune-design.md`](../specs/2026-04-25-industry-attune-design.md) §3.3 §6.5

---

## File Structure

**Modify:**
- `rust/crates/attune-core/src/plugin_loader.rs` — PluginManifest 加 `type` 通用化（不再仅 annotation_angle）
- `rust/crates/attune-core/src/lib.rs` — `pub(crate) mod plugin_loader;` → `pub mod plugin_loader;` 暴露给 attune-server；加 `pub mod plugin_registry;`
- `rust/crates/attune-server/src/state.rs` — AppState 加 `plugin_registry: Arc<PluginRegistry>` + `AppState::new` 启动时 load
- `rust/crates/attune-server/src/routes/upload.rs` — file_added trigger 基于 registry
- `rust/crates/attune-core/Cargo.toml` — 加 `glob = "0.3"`（扫目录）

**Create:**
- `rust/crates/attune-core/src/plugin_registry.rs` — PluginRegistry struct
- `rust/crates/attune-core/tests/plugin_registry_test.rs` — 集成测试（用 tempfile 创建 mock plugin 目录）

---

## Progress Tracking

每 Task 完成后回到本文件勾 checkbox。每 Task 一个独立 commit。中间确保 `cargo test --workspace` 维持 ≥ 414 passed。

---

### Task 1: 扩展 plugin_loader.rs 支持 workflow 类型

把 PluginManifest 改为通用 manifest，所有 type（annotation_angle / workflow / skill）共用基础字段；类型专属字段用 `#[serde(default)]` 容错。

**Files:**
- Modify: `rust/crates/attune-core/src/plugin_loader.rs`

- [ ] **Step 1: 写失败测试 — 在 plugin_loader.rs 内联 mod tests 加**

打开 `rust/crates/attune-core/src/plugin_loader.rs`，找到 `#[cfg(test)] mod tests` 块末尾追加：

```rust
    #[test]
    fn parses_workflow_type_manifest() {
        let yaml = r#"
id: law-pro/evidence_chain
name: 跨证据链推理
type: workflow
version: "1.0.0"
author: attune-pro
description: 律师上传新证据时跨证据链联想（行业层）
"#;
        let manifest: PluginManifest = serde_yaml::from_str(yaml).expect("parse workflow manifest");
        assert_eq!(manifest.id, "law-pro/evidence_chain");
        assert_eq!(manifest.plugin_type, "workflow");
        assert_eq!(manifest.version, "1.0.0");
    }

    #[test]
    fn parses_skill_type_manifest() {
        let yaml = r#"
id: law-pro/contract_review
name: 合同风险审查
type: skill
version: "0.1.0"
"#;
        let manifest: PluginManifest = serde_yaml::from_str(yaml).expect("parse skill manifest");
        assert_eq!(manifest.plugin_type, "skill");
    }
```

- [ ] **Step 2: 跑测试，验证 fail/pass**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin/rust && \
cargo test --release -p attune-core plugin_loader 2>&1 | tail -15
```

预期：parses_workflow_type_manifest / parses_skill_type_manifest **应直接 pass** — 因为现有 PluginManifest 用了 `#[serde(rename = "type")] pub plugin_type: String`，已经能容纳任意 type 字符串。如果 fail，看具体错误。

如果 pass，跳到 Step 3。

- [ ] **Step 3: 全工作区测试 + commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin/rust && \
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：**416 passed**（414 baseline + 2 = 416）。

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin && \
git add rust/crates/attune-core/src/plugin_loader.rs && \
git commit -m "test(plugin): verify PluginManifest accepts workflow/skill types

Existing #[serde(rename = \"type\")] pub plugin_type: String
already accepts any string — adding tests to lock invariant
before Sprint 2 PluginRegistry uses them.

Tests: 416 passed (414 baseline + 2 type-tag)."
```

---

### Task 2: 创建 PluginRegistry

HashMap<plugin_id, LoadedPlugin> + 按 type / trigger 查询；扫目录加载。

**Files:**
- Create: `rust/crates/attune-core/src/plugin_registry.rs`
- Modify: `rust/crates/attune-core/src/lib.rs`
- Modify: `rust/crates/attune-core/Cargo.toml`（加 glob 依赖）

- [ ] **Step 1: Cargo.toml 加 glob**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin && \
grep -E '^glob ' rust/crates/attune-core/Cargo.toml || echo 'glob not direct dep'
```

如果不是依赖，在 `[dependencies]` 末尾追加：

```toml
glob = "0.3"
```

- [ ] **Step 2: 创建 plugin_registry.rs**

`rust/crates/attune-core/src/plugin_registry.rs`:

```rust
//! PluginRegistry — attune-core 加载 + 索引所有外部 plugin（attune-pro / 用户 / 社区）。
//!
//! ## 目录约定
//!
//! ```text
//! ~/.local/share/attune/plugins/
//! ├── law-pro/
//! │   ├── plugin.yaml          # type: industry / 名称 / 版本
//! │   ├── workflows/
//! │   │   └── evidence_chain_inference.yaml
//! │   └── capabilities/
//! │       └── contract_review/
//! │           ├── plugin.yaml  # type: skill
//! │           └── prompt.md
//! └── user-custom/
//!     └── ...
//! ```
//!
//! 启动时 `PluginRegistry::scan(plugins_root)` 扫所有子目录加载。
//! attune-pro .attunepkg 解压到 `~/.local/share/attune/plugins/<plugin_id>/`。

use crate::error::{Result, VaultError};
use crate::plugin_loader::{LoadedPlugin, PluginManifest};
use crate::workflow::{parse_workflow_yaml, Workflow};
use std::collections::HashMap;
use std::path::Path;

/// 包装一个 plugin dir 加载出的 workflow（含 plugin_id 关联）
#[derive(Debug, Clone)]
pub struct LoadedWorkflow {
    pub plugin_id: String,
    pub workflow: Workflow,
}

#[derive(Debug, Default, Clone)]
pub struct PluginRegistry {
    plugins: HashMap<String, LoadedPlugin>,
    workflows: Vec<LoadedWorkflow>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plugins(&self) -> impl Iterator<Item = &LoadedPlugin> {
        self.plugins.values()
    }

    pub fn get_plugin(&self, id: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(id)
    }

    pub fn workflows(&self) -> &[LoadedWorkflow] {
        &self.workflows
    }

    /// 按 trigger.on 过滤 workflow
    pub fn workflows_by_trigger(&self, on: &str) -> Vec<&LoadedWorkflow> {
        self.workflows
            .iter()
            .filter(|w| w.workflow.trigger.on == on)
            .collect()
    }

    /// 按 plugin_type 过滤已加载 plugin
    pub fn plugins_by_type<'a>(&'a self, ptype: &'a str) -> impl Iterator<Item = &'a LoadedPlugin> + 'a {
        self.plugins.values().filter(move |p| p.manifest.plugin_type == ptype)
    }

    /// 扫描 plugins_root 下每个一级子目录作为一个 plugin。
    /// 每个 plugin dir 必须有 `plugin.yaml`；可选 `workflows/*.yaml` 和 `capabilities/*/plugin.yaml`（嵌套 skill）。
    ///
    /// **best-effort 加载** — 单个 plugin 失败不影响其他。返回错误数量供 caller 决定是否告警。
    pub fn scan(plugins_root: &Path) -> Result<(Self, Vec<String>)> {
        let mut reg = Self::new();
        let mut errors: Vec<String> = Vec::new();

        if !plugins_root.exists() {
            // 没装 plugin 目录 — 空 registry
            return Ok((reg, errors));
        }

        let entries = std::fs::read_dir(plugins_root).map_err(VaultError::Io)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // top-level plugin
            let plugin_yaml = path.join("plugin.yaml");
            if plugin_yaml.exists() {
                match LoadedPlugin::from_dir(&path) {
                    Ok(p) => {
                        let pid = p.manifest.id.clone();
                        reg.plugins.insert(pid.clone(), p);
                        // 扫该 plugin 下的 workflows/
                        let wf_dir = path.join("workflows");
                        if wf_dir.is_dir() {
                            if let Ok(wfs) = std::fs::read_dir(&wf_dir) {
                                for wf_entry in wfs.flatten() {
                                    let wfp = wf_entry.path();
                                    if wfp.extension().and_then(|s| s.to_str()) == Some("yaml") {
                                        match std::fs::read_to_string(&wfp) {
                                            Ok(yaml) => match parse_workflow_yaml(&yaml) {
                                                Ok(workflow) => reg.workflows.push(LoadedWorkflow {
                                                    plugin_id: pid.clone(),
                                                    workflow,
                                                }),
                                                Err(e) => errors.push(format!("{}: workflow yaml parse: {}", wfp.display(), e)),
                                            },
                                            Err(e) => errors.push(format!("{}: read: {}", wfp.display(), e)),
                                        }
                                    }
                                }
                            }
                        }
                        // 扫该 plugin 下的 capabilities/<id>/plugin.yaml（嵌套 skill）
                        let caps_dir = path.join("capabilities");
                        if caps_dir.is_dir() {
                            if let Ok(caps) = std::fs::read_dir(&caps_dir) {
                                for cap_entry in caps.flatten() {
                                    let cap_path = cap_entry.path();
                                    if cap_path.is_dir() && cap_path.join("plugin.yaml").exists() {
                                        match LoadedPlugin::from_dir(&cap_path) {
                                            Ok(cap_plugin) => {
                                                reg.plugins.insert(cap_plugin.manifest.id.clone(), cap_plugin);
                                            }
                                            Err(e) => errors.push(format!("{}: capability load: {}", cap_path.display(), e)),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => errors.push(format!("{}: plugin load: {}", path.display(), e)),
                }
            }
        }

        Ok((reg, errors))
    }

    /// 默认 plugin 目录：`~/.local/share/attune/plugins/`（Linux/macOS）/ `%APPDATA%\attune\plugins\`（Windows）
    pub fn default_plugins_dir() -> Result<std::path::PathBuf> {
        let data = dirs::data_local_dir()
            .ok_or_else(|| VaultError::InvalidInput("cannot resolve user data dir".into()))?;
        Ok(data.join("attune").join("plugins"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_plugin_dir(root: &Path, plugin_id: &str, plugin_yaml: &str) -> std::path::PathBuf {
        let dir = root.join(plugin_id);
        fs::create_dir_all(&dir).expect("mkdir plugin");
        fs::write(dir.join("plugin.yaml"), plugin_yaml).expect("write plugin.yaml");
        dir
    }

    #[test]
    fn scan_empty_root_returns_empty_registry() {
        let tmp = TempDir::new().expect("tmp");
        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert_eq!(reg.plugins().count(), 0);
        assert_eq!(reg.workflows().len(), 0);
        assert!(errs.is_empty());
    }

    #[test]
    fn scan_loads_single_plugin() {
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "test-plugin",
            r#"
id: test-plugin
name: 测试插件
type: industry
version: "1.0.0"
"#,
        );
        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert_eq!(reg.plugins().count(), 1);
        assert!(reg.get_plugin("test-plugin").is_some());
        assert!(errs.is_empty());
    }

    #[test]
    fn scan_loads_workflow_subdir() {
        let tmp = TempDir::new().expect("tmp");
        let pdir = write_plugin_dir(
            tmp.path(),
            "wf-plugin",
            r#"
id: wf-plugin
name: 含 Workflow 的插件
type: industry
version: "1.0.0"
"#,
        );
        let wf_dir = pdir.join("workflows");
        fs::create_dir_all(&wf_dir).expect("mkdir workflows");
        fs::write(
            wf_dir.join("test_wf.yaml"),
            r#"
id: wf-plugin/test
type: workflow
trigger:
  on: file_added
  scope: project
steps:
  - id: noop
    type: deterministic
    operation: echo_input
    input:
      x: hello
    output: y
"#,
        )
        .expect("write workflow");

        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert_eq!(reg.plugins().count(), 1);
        assert_eq!(reg.workflows().len(), 1);
        assert_eq!(errs.len(), 0);
        let by_trigger = reg.workflows_by_trigger("file_added");
        assert_eq!(by_trigger.len(), 1);
        assert_eq!(by_trigger[0].plugin_id, "wf-plugin");
        assert_eq!(by_trigger[0].workflow.id, "wf-plugin/test");
    }

    #[test]
    fn scan_corrupt_workflow_yaml_records_error_but_keeps_others() {
        let tmp = TempDir::new().expect("tmp");
        let pdir = write_plugin_dir(
            tmp.path(),
            "mixed",
            r#"
id: mixed
name: Mixed
type: industry
version: "1.0.0"
"#,
        );
        let wf_dir = pdir.join("workflows");
        fs::create_dir_all(&wf_dir).expect("mkdir");
        fs::write(wf_dir.join("good.yaml"), r#"
id: mixed/good
type: workflow
trigger:
  on: manual
  scope: global
steps:
  - id: a
    type: deterministic
    operation: echo_input
    input: {}
    output: result
"#).expect("write good");
        fs::write(wf_dir.join("broken.yaml"), "this is not yaml: [::").expect("write broken");

        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert_eq!(reg.workflows().len(), 1);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("broken.yaml"));
    }
}
```

- [ ] **Step 3: lib.rs 暴露 plugin_loader + 注册 plugin_registry**

打开 `rust/crates/attune-core/src/lib.rs`：

```rust
// 改前：
pub(crate) mod plugin_loader;

// 改后：
pub mod plugin_loader;
pub mod plugin_registry;
```

按字母序合适位置。

- [ ] **Step 4: 跑测试 + commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin/rust && \
cargo test --release -p attune-core plugin_registry 2>&1 | tail -15
```

预期：4 unit tests pass。

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin/rust && \
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：**420 passed**（416 baseline + 4 = 420）。

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin && \
git add rust/crates/attune-core/Cargo.toml \
        rust/crates/attune-core/src/plugin_registry.rs \
        rust/crates/attune-core/src/lib.rs && \
git commit -m "feat(plugin): PluginRegistry — scan + index plugins with workflows

Loads ~/.local/share/attune/plugins/<plugin_id>/ subdirs:
- plugin.yaml (top-level + capabilities/<cap_id>/)
- workflows/*.yaml (each parsed via attune-core workflow schema)

Best-effort: corrupt yaml → error list, others keep loading.
Querying: get_plugin / plugins_by_type / workflows_by_trigger / default_plugins_dir.

Tests: 420 passed (416 baseline + 4 registry)."
```

---

### Task 3: attune-server startup 扫描 plugin 目录

AppState 加 `plugin_registry`，启动时调 `PluginRegistry::scan`。

**Files:**
- Modify: `rust/crates/attune-server/src/state.rs`

- [ ] **Step 1: state.rs 加字段 + 启动扫**

打开 `rust/crates/attune-server/src/state.rs`，找 `pub struct AppState` 块末尾追加：

```rust
    /// Sprint 2: 启动时加载的 plugins（attune-pro / 用户 / 社区）
    pub plugin_registry: std::sync::Arc<attune_core::plugin_registry::PluginRegistry>,
```

`AppState::new()` 构造内（before `Self { ... }`）加：

```rust
let plugin_registry = match attune_core::plugin_registry::PluginRegistry::default_plugins_dir() {
    Ok(dir) => match attune_core::plugin_registry::PluginRegistry::scan(&dir) {
        Ok((reg, errs)) => {
            tracing::info!(
                "loaded {} plugins, {} workflows from {}",
                reg.plugins().count(),
                reg.workflows().len(),
                dir.display()
            );
            for e in &errs {
                tracing::warn!("plugin load error: {}", e);
            }
            std::sync::Arc::new(reg)
        }
        Err(e) => {
            tracing::warn!("plugin scan failed: {}", e);
            std::sync::Arc::new(attune_core::plugin_registry::PluginRegistry::new())
        }
    },
    Err(e) => {
        tracing::warn!("cannot resolve plugin dir: {}", e);
        std::sync::Arc::new(attune_core::plugin_registry::PluginRegistry::new())
    }
};
```

`Self { ... }` 列表加 `plugin_registry,`。

- [ ] **Step 2: cargo build + 测试**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin/rust && \
cargo build --release --workspace 2>&1 | tail -5
echo '---'
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：build OK + **420 passed**（无新测试，仅 wiring）。

如有测试退化（mock AppState 漏字段）：补 `plugin_registry: Arc::new(PluginRegistry::new())` 到 mock state 构造。

- [ ] **Step 3: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin && \
git add rust/crates/attune-server/src/state.rs && \
git commit -m "feat(plugin): AppState scans plugin dir at startup

Calls PluginRegistry::scan(default_plugins_dir).
Empty / missing dir → empty registry (no error).
Per-plugin failures logged via tracing, others keep loading.

Tests: 420 passed (no regression)."
```

---

### Task 4: file_added trigger 基于 registry

`routes/upload.rs` 文件上传成功 + 已归 Project 时，遍历 registry 中 trigger.on=file_added 的所有 workflow，逐个 spawn 跑。

**Files:**
- Modify: `rust/crates/attune-server/src/routes/upload.rs`

- [ ] **Step 1: 在 upload.rs 文件上传成功响应前加 trigger 块**

打开 `rust/crates/attune-server/src/routes/upload.rs`，找 Phase B Task 4 加的 ProjectRecommender spawn 块。在它**之后**追加（在 `Ok(Json(...))` 之前）：

```rust
    // Sprint 2 Phase A: file_added trigger — 基于 registry 匹配 workflow
    let item_id_for_wf = item_id.clone();
    let state_for_wf = state.clone();
    tokio::spawn(async move {
        let vault_guard = state_for_wf.vault.lock();
        let vault_guard = vault_guard.unwrap_or_else(|e| e.into_inner());
        if !matches!(vault_guard.state(), attune_core::vault::VaultState::Unlocked) {
            return;
        }
        // 找该 file_id 归属的 project
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
            return;
        };
        // 跑所有匹配的 workflow
        let registry = state_for_wf.plugin_registry.clone();
        let matched_wfs = registry.workflows_by_trigger("file_added");
        if matched_wfs.is_empty() {
            return; // 没注册任何 file_added workflow（attune-pro 还没加 / 没装）
        }
        for lwf in matched_wfs {
            let mut data = std::collections::BTreeMap::new();
            data.insert("file_id".into(), serde_json::json!(item_id_for_wf));
            data.insert("project_id".into(), serde_json::json!(pid));
            let event = attune_core::workflow::WorkflowEvent {
                event_type: "file_added".into(),
                data,
            };
            match attune_core::workflow::run_workflow(&lwf.workflow, &event, Some(vault_guard.store())) {
                Ok(_result) => {
                    let payload = serde_json::json!({
                        "type": "workflow_complete",
                        "workflow_id": lwf.workflow.id,
                        "plugin_id": lwf.plugin_id,
                        "file_id": item_id_for_wf,
                        "project_id": pid,
                    });
                    let _ = state_for_wf.recommendation_tx.send(payload);
                }
                Err(e) => {
                    tracing::warn!(
                        "workflow {} (plugin {}) failed: {}",
                        lwf.workflow.id, lwf.plugin_id, e
                    );
                }
            }
        }
    });
```

注：`workflows_by_trigger` 返回 `Vec<&LoadedWorkflow>`，跨 await 不安全。需要在 spawn 内同步处理（看 plan 上面代码写法是 ok 的，因为 `run_workflow` 是同步的，await 在 tokio task 边界外）。如果 borrow checker 不爽，clone `LoadedWorkflow` 内的 `Workflow`（已 derive Clone）。

- [ ] **Step 2: cargo build + 测试**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin/rust && \
cargo build --release --workspace 2>&1 | tail -5
echo '---'
timeout 240 cargo test --release --workspace -- --test-threads=2 2>&1 | grep -E '^test result' | awk '{passed+=$4; failed+=$6} END {print passed " passed, " failed " failed"}'
```

预期：build OK + **420 passed**。

- [ ] **Step 3: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin && \
git add rust/crates/attune-server/src/routes/upload.rs && \
git commit -m "feat(plugin): file_added trigger via PluginRegistry

Replaces hardcoded evidence_chain (deleted Phase D-0). Now:
- vault unlocked + file in project → loop registry.workflows_by_trigger('file_added')
- run_workflow per matched plugin workflow → broadcast workflow_complete (含 plugin_id)
- best-effort: each workflow failure logged, others continue

Empty registry (no plugin installed) → no-op (no spawn cost).

Tests: 420 passed."
```

---

### Task 5: docs sync

**Files:**
- Modify: `docs/superpowers/specs/2026-04-25-industry-attune-design.md`
- Modify: `rust/README.md` + `rust/README.zh.md`

- [ ] **Step 1: spec §9 Sprint 2 行加 Phase A 完成标记**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin && \
grep -n 'Sprint 2' docs/superpowers/specs/2026-04-25-industry-attune-design.md | head
```

找到 §9 Sprint 节奏表 Sprint 2 行，标 "Phase A ✅ 2026-04-25"。

如果 §9 没有 Sprint 2 显式行（可能命名 "S0/S0.5/S1-S7"），在 §16（实施前提）或新加段落中加：

```markdown
### Sprint 2 路径

- **Phase A**（plugin loader + workflow registry）— 2026-04-25 完成
- **Phase B**（attune-pro evidence_chain 接回 + 端到端实测） — TODO
- **Phase C**（Intent Router + skill 路由） — TODO
- **Phase D**（write_annotation 真持久化 / vault DEK） — TODO
```

- [ ] **Step 2: README 加 plugin loader 段（双语）**

`rust/README.md`：

```markdown
### Plugin Loader (Sprint 2 Phase A)

attune-server scans `~/.local/share/attune/plugins/` at startup. Each subdirectory:
- top-level `plugin.yaml` declares the plugin
- `workflows/*.yaml` declares any workflows (loaded via attune-core schema parser)
- `capabilities/<cap_id>/plugin.yaml` declares nested skills

Trigger registry: file_added events match plugin workflows where `trigger.on == 'file_added'`,
each spawn-and-run with workflow_complete pushed via WebSocket.

attune-pro `.attunepkg` bundles unpack into this dir.
```

`rust/README.zh.md` 加同等中文段。

- [ ] **Step 3: Commit**

```bash
cd /data/company/project/attune/.worktrees/sprint-2-plugin && \
git add docs/superpowers/specs/2026-04-25-industry-attune-design.md \
        rust/README.md \
        rust/README.zh.md && \
git commit -m "docs(sprint-2-a): plugin loader + registry section + spec status

Sprint 2 Phase A done. Phase B/C/D pending.
README dual-language updated with plugin scan layout."
```

---

## Self-Review Notes

**Spec coverage:**
- ✅ §3.3 file_added trigger 基于 registry → Task 4
- ✅ §6.5（Cargo workspace + plugin loader）→ Tasks 1-3
- ⏭ §3.1 plugin.yaml chat_trigger（Intent Router 路由 chat） → Phase C
- ⏭ §3.3 attune-pro 仓的 evidence_chain workflow 实际加载测试 → Phase B（须在 attune-pro 仓加 workflow.yaml）
- ⏭ Phase D Task 3 concern（write_annotation stub） → Phase D

**Type consistency:**
- `LoadedPlugin / PluginManifest` 跨 plugin_loader / plugin_registry 一致
- `LoadedWorkflow { plugin_id, workflow }` 包装现有 `workflow::Workflow`
- `Arc<PluginRegistry>` 在 AppState 一致

---

## 完成 Phase A 标志

5 个 Task 全部 checkbox 勾上，且：
- [ ] `cargo test --workspace`: ≥ **420 passed**（414 baseline + 2 manifest + 4 registry = 420）
- [ ] attune-server 启动时扫 plugin 目录（log 看到 "loaded N plugins, M workflows"）
- [ ] file_added 在用户接受 recommender 推荐归类后，触发 registry workflow（**Phase B 前没装任何 plugin → 不触发，正常**）
- [ ] 文档（spec + README 双语）同步

完成后 Phase B 实测：
- 在 attune-pro 仓加一个最简 workflow.yaml（如 `plugins/law-pro/workflows/evidence_chain.yaml`）
- 把 attune-pro 这部分软链或 cp 到 `~/.local/share/attune/plugins/law-pro/`
- 启动 attune-server 看 log 能加载到 + 上传文件 + 归类 → workflow_complete ws 通知
