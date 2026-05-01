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
use crate::plugin_loader::{LoadedPlugin, PiiPatternSpec};
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

    /// v0.6 新增：聚合所有 plugin 的 PII 正则（按 name 去重；同名仅保留第一个）。
    ///
    /// 调用方典型用法：
    /// ```ignore
    /// let mut redactor = attune_core::pii::Redactor::new();
    /// for spec in registry.all_pii_patterns() {
    ///     redactor.add_dict_entry_from_regex(&spec.name, &spec.regex)?;
    /// }
    /// ```
    /// OSS 裸装 → plugins 空 → 返空 Vec → Redactor 仅有内置 12 类正则。
    pub fn all_pii_patterns(&self) -> Vec<&PiiPatternSpec> {
        use std::collections::HashSet;
        let mut seen: HashSet<&str> = HashSet::new();
        let mut out: Vec<&PiiPatternSpec> = Vec::new();
        for p in self.plugins.values() {
            for spec in &p.manifest.pii_patterns {
                if seen.insert(spec.name.as_str()) {
                    out.push(spec);
                }
            }
        }
        out
    }

    /// v0.6 新增：聚合所有 plugin 的 chat_trigger.project_keywords（去重后返回）
    ///
    /// project_recommender::recommend_for_chat 调用方典型用法：
    /// ```ignore
    /// let kws: Vec<&str> = state.plugin_registry.all_chat_trigger_project_keywords()
    ///     .into_iter()
    ///     .collect();
    /// recommend_for_chat(&user_msg, &kws);
    /// ```
    /// OSS 裸装 → plugins 空 → 返空 Vec → recommend_for_chat 永不触发。
    pub fn all_chat_trigger_project_keywords(&self) -> Vec<&str> {
        use std::collections::HashSet;
        let mut seen: HashSet<&str> = HashSet::new();
        let mut out: Vec<&str> = Vec::new();
        for p in self.plugins.values() {
            if let Some(ct) = p.manifest.chat_trigger.as_ref() {
                for kw in &ct.project_keywords {
                    let s = kw.as_str();
                    if seen.insert(s) {
                        out.push(s);
                    }
                }
            }
        }
        out
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
    fn pii_patterns_aggregated_across_plugins_and_deduped_by_name() {
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "law-pro",
            r#"
id: law-pro
name: 律师插件
type: industry
version: "1.0.0"
pii_patterns:
  - name: case_no
    regex: "\\(\\d{4}\\)[\\u4e00-\\u9fa5]+\\d+号"
  - name: court_seal
    regex: "[\\u4e00-\\u9fa5]+人民法院"
"#,
        );
        write_plugin_dir(
            tmp.path(),
            "medical-pro",
            r#"
id: medical-pro
name: 医生插件
type: industry
version: "1.0.0"
pii_patterns:
  - name: medical_record_no
    regex: "MR\\d{8}"
  - name: case_no
    regex: "DUPLICATE_should_be_skipped"
"#,
        );

        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert!(errs.is_empty());
        assert_eq!(reg.plugins().count(), 2);

        let patterns = reg.all_pii_patterns();
        let names: std::collections::HashSet<&str> =
            patterns.iter().map(|p| p.name.as_str()).collect();
        // case_no 去重保留第一次出现的；court_seal + medical_record_no + case_no = 3 个
        assert_eq!(names.len(), 3);
        assert!(names.contains("case_no"));
        assert!(names.contains("court_seal"));
        assert!(names.contains("medical_record_no"));
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

    // ── R11 v0.6.4: chat_trigger.project_keywords 聚合 — chat 路由入口 ──────

    #[test]
    fn all_chat_trigger_project_keywords_empty_oss_default() {
        // OSS 裸装无 plugin → 关键词列表为空 → recommend_for_chat 永不触发.
        // 这是 oss-pro-strategy v2 §4.3 边界规则的代码层验证.
        let reg = PluginRegistry::new();
        let kws = reg.all_chat_trigger_project_keywords();
        assert!(kws.is_empty(), "OSS-only registry must have no keywords, got: {:?}", kws);
    }

    #[test]
    fn all_chat_trigger_project_keywords_aggregated_from_plugins() {
        // attune-pro plugin 装上后, keywords 从 plugin.yaml chat_trigger 段聚合.
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "law-pro",
            r#"
id: law-pro
name: 律师插件
type: industry
version: "1.0.0"
chat_trigger:
  enabled: true
  needs_confirm: true
  priority: 5
  project_keywords:
    - 案件
    - 诉讼
    - 合同
"#,
        );
        write_plugin_dir(
            tmp.path(),
            "patent-pro",
            r#"
id: patent-pro
name: 专利插件
type: industry
version: "1.0.0"
chat_trigger:
  enabled: true
  needs_confirm: true
  priority: 5
  project_keywords:
    - 专利
    - 申请
    - 案件
"#,
        );
        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert!(errs.is_empty(), "scan should not fail: {:?}", errs);

        let kws = reg.all_chat_trigger_project_keywords();
        // dedupe: "案件" 在两个 plugin 中, 只应出现一次
        let unique: std::collections::HashSet<&str> = kws.iter().copied().collect();
        assert_eq!(unique.len(), 5, "5 unique keywords (诉讼/合同/案件/专利/申请), got: {:?}", kws);
        assert!(kws.contains(&"案件"));
        assert!(kws.contains(&"诉讼"));
        assert!(kws.contains(&"专利"));
        assert!(kws.contains(&"申请"));
        // dedupe 验证: 总长度 == unique 大小
        assert_eq!(kws.len(), unique.len(), "no duplicates allowed");
    }

    #[test]
    fn get_plugin_returns_none_for_unknown_id() {
        let reg = PluginRegistry::new();
        assert!(reg.get_plugin("nonexistent").is_none());
    }

    #[test]
    fn workflows_by_trigger_returns_empty_for_unknown_event() {
        let reg = PluginRegistry::new();
        let wfs = reg.workflows_by_trigger("nonexistent_event");
        assert!(wfs.is_empty());
    }
}
