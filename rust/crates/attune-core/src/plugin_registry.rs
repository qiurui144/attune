//! PluginRegistry — attune-core 加载 + 索引所有外部 plugin（attune-pro / 用户 / 社区）。
//!
//! ## 目录约定
//!
//! ```text
//! ~/.local/share/attune/plugins/
//! ├── <vertical-pack>/         # 例：medical-pro / academic-pro / 用户自研
//! │   ├── plugin.yaml          # type: industry / 名称 / 版本
//! │   ├── workflows/
//! │   │   └── <workflow_name>.yaml
//! │   └── capabilities/
//! │       └── <capability_name>/
//! │           ├── plugin.yaml  # type: skill
//! │           └── prompt.md
//! └── user-custom/
//!     └── ...
//! ```
//!
//! 启动时 `PluginRegistry::scan(plugins_root)` 扫所有子目录加载。
//! 商业插件包 (`.attunepkg`) 解压到 `~/.local/share/attune/plugins/<plugin_id>/`。

use crate::error::{Result, VaultError};
use crate::plugin_loader::LoadedPlugin;
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
    /// 每个 plugin dir 必须有 `plugin.yaml`；可选 `workflows/*.yaml` 和 `capabilities/<cap_id>/plugin.yaml`。
    ///
    /// **best-effort 加载** — 单个 plugin 失败不影响其他。返回错误数量供 caller 决定是否告警。
    pub fn scan(plugins_root: &Path) -> Result<(Self, Vec<String>)> {
        let mut reg = Self::new();
        let mut errors: Vec<String> = Vec::new();

        if !plugins_root.exists() {
            return Ok((reg, errors));
        }

        let entries = std::fs::read_dir(plugins_root).map_err(VaultError::Io)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
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
                                                Err(e) => errors.push(format!(
                                                    "{}: workflow yaml parse: {}",
                                                    wfp.display(),
                                                    e
                                                )),
                                            },
                                            Err(e) => errors.push(format!(
                                                "{}: read: {}",
                                                wfp.display(),
                                                e
                                            )),
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
                                            Err(e) => errors.push(format!(
                                                "{}: capability load: {}",
                                                cap_path.display(),
                                                e
                                            )),
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
        fs::write(
            wf_dir.join("good.yaml"),
            r#"
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
"#,
        )
        .expect("write good");
        fs::write(wf_dir.join("broken.yaml"), "this is not yaml: [::").expect("write broken");

        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert_eq!(reg.workflows().len(), 1);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("broken.yaml"));
    }
}
