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

/// chat 消息匹配到的 plugin trigger 结果
#[derive(Debug, Clone)]
pub struct ChatTriggerMatch {
    /// plugin id (e.g. "law-pro")
    pub plugin_id: String,
    /// 多 plugin 同时命中时优先级 (高优先)
    pub priority: i32,
    /// 短描述 (UI 提示用户用)
    pub description: String,
    /// 是否需用户确认才执行 (默认 true)
    pub needs_confirm: bool,
    /// 关键词命中数
    pub keyword_hits: usize,
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
    /// ```text
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
    /// ```text
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

    /// 列出所有 plugin 的全部 skills (附带 plugin_id)
    pub fn list_skills(&self) -> Vec<(&str, &crate::plugin_loader::SkillSpec)> {
        let mut out = Vec::new();
        for (pid, p) in &self.plugins {
            for s in &p.manifest.skills {
                out.push((pid.as_str(), s));
            }
        }
        out
    }

    /// 列出所有 plugin 的全部 agents (附带 plugin_id)
    pub fn list_agents(&self) -> Vec<(&str, &crate::plugin_loader::AgentSpec)> {
        let mut out = Vec::new();
        for (pid, p) in &self.plugins {
            for a in &p.manifest.agents {
                out.push((pid.as_str(), a));
            }
        }
        out
    }

    /// 列出所有 plugin 的全部 MCP servers (附带 plugin_id)
    pub fn list_mcp_servers(&self) -> Vec<(&str, &crate::plugin_loader::McpServerSpec)> {
        let mut out = Vec::new();
        for (pid, p) in &self.plugins {
            for m in &p.manifest.mcp_servers {
                out.push((pid.as_str(), m));
            }
        }
        out
    }

    /// 按 case_kind 过滤 agents (调用方按业务场景选 kind, 拿到该 kind 下的 agents)
    pub fn agents_by_case_kind(&self, kind: &str) -> Vec<(&str, &crate::plugin_loader::AgentSpec)> {
        self.list_agents()
            .into_iter()
            .filter(|(_, a)| a.case_kinds.iter().any(|k| k == kind))
            .collect()
    }

    /// 聚合所有 plugin 注册的 case kinds → UI"案件类型选择"下拉数据源.
    /// OSS 裸装无 plugin → 空 Vec.
    pub fn all_registered_case_kinds(&self) -> Vec<&crate::plugin_loader::CaseKindRegistration> {
        let mut out = Vec::new();
        for p in self.plugins.values() {
            for k in &p.manifest.registers_case_kinds {
                out.push(k);
            }
        }
        out
    }

    /// 匹配用户 chat 消息到 plugin trigger.
    ///
    /// 实现 chat 消息 → capability 路由的 OSS 侧入口. attune-pro 装载 capability 后,
    /// chat.rs 调此 API 决定是否提示用户触发 capability (而非走纯 RAG path).
    ///
    /// 匹配规则:
    /// - 任一 pattern (regex) 命中 → match
    /// - keywords 命中数 >= min_keyword_match → match
    /// - 任一 exclude_pattern 命中 → 否决
    /// - 多 plugin 同时命中按 priority desc 取最高
    ///
    /// OSS attune 裸装无 plugin → 永远返 None.
    pub fn match_chat_trigger(&self, user_msg: &str) -> Option<ChatTriggerMatch> {
        use regex::Regex;
        let mut best: Option<ChatTriggerMatch> = None;
        for (plugin_id, p) in &self.plugins {
            let Some(ct) = p.manifest.chat_trigger.as_ref() else { continue };
            if !ct.enabled {
                continue;
            }

            // 否决检查
            let excluded = ct.exclude_patterns.iter().any(|pat| {
                Regex::new(pat).map(|r| r.is_match(user_msg)).unwrap_or(false)
            });
            if excluded {
                continue;
            }

            // pattern 命中
            let pattern_hit = ct.patterns.iter().any(|pat| {
                Regex::new(pat).map(|r| r.is_match(user_msg)).unwrap_or(false)
            });

            // keywords 命中数
            let kw_hits = ct.keywords.iter().filter(|kw| user_msg.contains(kw.as_str())).count();
            let kw_match = kw_hits >= ct.min_keyword_match.max(1);

            if pattern_hit || kw_match {
                let m = ChatTriggerMatch {
                    plugin_id: plugin_id.clone(),
                    priority: ct.priority,
                    description: ct.description.clone(),
                    needs_confirm: ct.needs_confirm,
                    keyword_hits: kw_hits,
                };
                if best.as_ref().map(|b| m.priority > b.priority).unwrap_or(true) {
                    best = Some(m);
                }
            }
        }
        best
    }

    /// 扫描 plugins_root, 自动解密 paid plugin (如提供 key) — 后续扩展用.
    ///
    /// 调用方典型: 在 attune-server 启动时, 从用户 license 拿 decrypt_key 透传.
    pub fn scan_with_key(plugins_root: &Path, decrypt_key: Option<&[u8]>) -> Result<(Self, Vec<String>)> {
        Self::scan_impl(plugins_root, decrypt_key)
    }

    /// 扫描 plugins_root 下每个一级子目录作为一个 plugin。
    /// 每个 plugin dir 必须有 `plugin.yaml`；可选 `workflows/*.yaml` 和 `capabilities/<cap_id>/plugin.yaml`。
    ///
    /// **best-effort 加载** — 单个 plugin 失败不影响其他。返回错误数量供 caller 决定是否告警。
    pub fn scan(plugins_root: &Path) -> Result<(Self, Vec<String>)> {
        Self::scan_impl(plugins_root, None)
    }

    fn scan_impl(plugins_root: &Path, decrypt_key: Option<&[u8]>) -> Result<(Self, Vec<String>)> {
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
            let plugin_yaml_enc = path.join("plugin.yaml.enc");
            if plugin_yaml.exists() || plugin_yaml_enc.exists() {
                // 装到 plugins/ 目录的 plugin 视为用户已通过 attune-cli plugin-install 装载
                // (CLI 已校验签名 + 解密). server 装载时给 Some("Trusted"), 不再二次拒绝.
                // 真实加密 paid plugin 在 scan 中**当前不解密** — 调用方按需扩展.
                match LoadedPlugin::from_dir_with_key(&path, decrypt_key, Some("Trusted")) {
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

    /// match_chat_trigger 路由 API
    #[test]
    fn match_chat_trigger_oss_default_returns_none() {
        // OSS 裸装无 plugin → 永远 None
        let reg = PluginRegistry::new();
        assert!(reg.match_chat_trigger("梁素燕vs任其坤本息计算").is_none());
    }

    #[test]
    fn match_chat_trigger_keyword_hits() {
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
  priority: 10
  description: "律师本息合规计算"
  keywords:
    - 本金
    - 利息
    - 应付
  min_keyword_match: 1
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

        // 命中 1 个关键词 → match (min_keyword_match=1)
        let m = reg.match_chat_trigger("我想问问任其坤应付多少利息").expect("match");
        assert_eq!(m.plugin_id, "law-pro");
        assert_eq!(m.priority, 10);
        assert!(m.keyword_hits >= 2); // "应付" + "利息"
        assert_eq!(m.description, "律师本息合规计算");

        // 不含关键词 → None
        assert!(reg.match_chat_trigger("今天天气怎么样").is_none());
    }

    #[test]
    fn match_chat_trigger_priority_picks_highest() {
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "low-pro",
            r#"
id: low-pro
name: low
type: industry
version: "1.0.0"
chat_trigger:
  enabled: true
  priority: 1
  keywords: ["案件"]
  min_keyword_match: 1
"#,
        );
        write_plugin_dir(
            tmp.path(),
            "high-pro",
            r#"
id: high-pro
name: high
type: industry
version: "1.0.0"
chat_trigger:
  enabled: true
  priority: 100
  keywords: ["案件"]
  min_keyword_match: 1
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        let m = reg.match_chat_trigger("帮我看下这个案件").expect("match");
        assert_eq!(m.plugin_id, "high-pro");
        assert_eq!(m.priority, 100);
    }

    #[test]
    fn match_chat_trigger_disabled_plugin_skipped() {
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
  enabled: false
  priority: 10
  keywords: ["本息"]
  min_keyword_match: 1
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        // enabled=false → 不参与匹配
        assert!(reg.match_chat_trigger("本息计算").is_none());
    }

    /// list_skills / list_agents / list_mcp_servers / case_kind 查询
    #[test]
    fn list_skills_aggregates_across_plugins() {
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "law-pro",
            r#"
id: law-pro
name: 律师插件
type: industry
version: "1.0.0"
skills:
  - id: extract_loan_terms
    description: "借条 OCR → 本金/利率"
    runtime: rust_binary
    binary: bin/skill_extract_loan_terms
  - id: parse_case_no
    description: "案号结构化"
    runtime: rust_binary
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
skills:
  - id: extract_patent_claims
    runtime: rust_binary
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        let skills = reg.list_skills();
        assert_eq!(skills.len(), 3);
        let ids: Vec<&str> = skills.iter().map(|(_, s)| s.id.as_str()).collect();
        assert!(ids.contains(&"extract_loan_terms"));
        assert!(ids.contains(&"parse_case_no"));
        assert!(ids.contains(&"extract_patent_claims"));
    }

    #[test]
    fn list_agents_and_filter_by_case_kind() {
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "law-pro",
            r#"
id: law-pro
name: 律师
type: industry
version: "1.0.0"
agents:
  - id: civil_loan_agent
    case_kinds: [civil-loan]
    runtime: rust_binary
  - id: marriage_property_agent
    case_kinds: [civil-marriage]
    runtime: rust_binary
  - id: criminal_defense_agent
    case_kinds: [criminal-defense]
    runtime: rust_binary
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert_eq!(reg.list_agents().len(), 3);

        let civil = reg.agents_by_case_kind("civil-loan");
        assert_eq!(civil.len(), 1);
        assert_eq!(civil[0].1.id, "civil_loan_agent");

        let nonexistent = reg.agents_by_case_kind("admin-litigation");
        assert!(nonexistent.is_empty());
    }

    #[test]
    fn list_mcp_servers_aggregates() {
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "law-pro",
            r#"
id: law-pro
name: 律师
type: industry
version: "1.0.0"
mcp_servers:
  - id: lpr_history
    transport: stdio
    command: ["bin/mcp_lpr_history"]
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        let mcps = reg.list_mcp_servers();
        assert_eq!(mcps.len(), 1);
        assert_eq!(mcps[0].1.id, "lpr_history");
        assert_eq!(mcps[0].1.transport, "stdio");
        assert_eq!(mcps[0].1.lifecycle, "eager");  // 默认值
        assert_eq!(mcps[0].1.heartbeat_interval_seconds, 30);
    }

    #[test]
    fn all_registered_case_kinds_aggregates() {
        let tmp = TempDir::new().expect("tmp");
        write_plugin_dir(
            tmp.path(),
            "law-pro",
            r#"
id: law-pro
name: 律师
type: industry
version: "1.0.0"
registers_case_kinds:
  - kind: civil-loan
    label: 民事-借贷纠纷
    default_agent: civil_loan_agent
  - kind: civil-marriage
    label: 婚姻-财产分割
    default_agent: marriage_property_agent
"#,
        );
        write_plugin_dir(
            tmp.path(),
            "patent-pro",
            r#"
id: patent-pro
name: 专利
type: industry
version: "1.0.0"
registers_case_kinds:
  - kind: patent-infringement
    label: 知产-专利侵权
    default_agent: patent_infringement_agent
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        let kinds = reg.all_registered_case_kinds();
        assert_eq!(kinds.len(), 3);
        let labels: Vec<&str> = kinds.iter().map(|k| k.label.as_str()).collect();
        assert!(labels.contains(&"民事-借贷纠纷"));
        assert!(labels.contains(&"婚姻-财产分割"));
        assert!(labels.contains(&"知产-专利侵权"));
    }

    #[test]
    fn oss_default_no_plugins_returns_empty_lists() {
        let reg = PluginRegistry::new();
        assert!(reg.list_skills().is_empty());
        assert!(reg.list_agents().is_empty());
        assert!(reg.list_mcp_servers().is_empty());
        assert!(reg.all_registered_case_kinds().is_empty());
    }

    #[test]
    fn match_chat_trigger_exclude_pattern_vetos() {
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
  priority: 10
  keywords: ["利息"]
  min_keyword_match: 1
  exclude_patterns:
    - "利息税"
"#,
        );
        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

        // 一般 query 命中
        assert!(reg.match_chat_trigger("利息怎么算").is_some());
        // 含 exclude pattern → 否决
        assert!(reg.match_chat_trigger("利息税应该咨询税务师").is_none());
    }
}
