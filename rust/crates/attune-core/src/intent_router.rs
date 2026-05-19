//! IntentRouter — 把用户 chat 消息路由到 plugin 注册的 skill。
//!
//! 设计：纯函数 + 正则 + 关键词；不调 LLM。
//! 调用方传 PluginRegistry + 消息 + 上下文（是否含文件）+ 禁用集，
//! 返回排序好的候选 skill 列表（按 priority 降序）。

use crate::plugin_registry::PluginRegistry;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillMatch {
    pub skill_id: String,
    pub skill_name: String,
    pub priority: i32,
    pub needs_confirm: bool,
    pub description: String,
    /// 命中时机：哪些 keyword / pattern 触发了
    pub matched_via: Vec<String>,
}

pub struct IntentRouter<'a> {
    registry: &'a PluginRegistry,
}

impl<'a> IntentRouter<'a> {
    pub fn new(registry: &'a PluginRegistry) -> Self {
        Self { registry }
    }

    /// 路由 chat 消息到候选 skill。
    ///
    /// - `message`: 用户消息
    /// - `has_pending_document`: chat 上下文是否含文件（影响 requires_document filter）
    /// - `disabled_skills`: 用户禁用的 skill_id 集合
    ///
    /// 返回按 priority 降序排列的候选。
    pub fn route(
        &self,
        message: &str,
        has_pending_document: bool,
        disabled_skills: &HashSet<String>,
    ) -> Vec<SkillMatch> {
        let mut matches = Vec::new();

        for plugin in self.registry.plugins() {
            if plugin.manifest.plugin_type != "skill" {
                continue;
            }
            if disabled_skills.contains(&plugin.manifest.id) {
                continue;
            }
            let Some(trigger) = &plugin.manifest.chat_trigger else {
                continue;
            };
            if !trigger.enabled {
                continue;
            }
            if trigger.requires_document && !has_pending_document {
                continue;
            }
            if Self::matches_any_regex(message, &trigger.exclude_patterns) {
                continue;
            }

            let mut matched_via = Vec::new();
            if let Some(p) = Self::first_matching_pattern(message, &trigger.patterns) {
                matched_via.push(format!("pattern: {}", p));
            }
            let kw_hits = Self::keyword_hits(message, &trigger.keywords);
            if kw_hits.len() >= trigger.min_keyword_match.max(1) {
                for k in kw_hits.iter().take(3) {
                    matched_via.push(format!("keyword: {}", k));
                }
            }
            if matched_via.is_empty() {
                continue;
            }

            matches.push(SkillMatch {
                skill_id: plugin.manifest.id.clone(),
                skill_name: plugin.manifest.name.clone(),
                priority: trigger.priority,
                needs_confirm: trigger.needs_confirm,
                description: if trigger.description.is_empty() {
                    plugin.manifest.description.clone()
                } else {
                    trigger.description.clone()
                },
                matched_via,
            });
        }

        matches.sort_by_key(|b| std::cmp::Reverse(b.priority));
        matches
    }

    fn matches_any_regex(message: &str, patterns: &[String]) -> bool {
        patterns
            .iter()
            .any(|p| Regex::new(p).map(|re| re.is_match(message)).unwrap_or(false))
    }

    fn first_matching_pattern(message: &str, patterns: &[String]) -> Option<String> {
        patterns
            .iter()
            .find(|p| Regex::new(p).map(|re| re.is_match(message)).unwrap_or(false))
            .cloned()
    }

    fn keyword_hits(message: &str, keywords: &[String]) -> Vec<String> {
        keywords.iter().filter(|k| message.contains(k.as_str())).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_skill_yaml(dir: &std::path::Path, id: &str, yaml: &str) {
        let pdir = dir.join(id);
        fs::create_dir_all(&pdir).expect("mkdir");
        fs::write(pdir.join("plugin.yaml"), yaml).expect("write yaml");
    }

    #[test]
    fn route_skill_with_keyword_match() {
        let tmp = TempDir::new().expect("tmp");
        write_skill_yaml(
            tmp.path(),
            "contract-skill",
            r#"
id: contract-skill
name: 合同审查
type: skill
version: "1.0.0"
chat_trigger:
  enabled: true
  priority: 5
  keywords: ['合同', '审查']
  min_keyword_match: 1
  description: AI 合同风险审查
"#,
        );

        let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
        assert!(errs.is_empty(), "scan errors: {errs:?}");
        let router = IntentRouter::new(&reg);

        let disabled: HashSet<String> = HashSet::new();
        let matches = router.route("帮我审查这份合同", false, &disabled);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].skill_id, "contract-skill");
        assert_eq!(matches[0].priority, 5);
        assert!(matches[0].matched_via.iter().any(|m| m.starts_with("keyword:")));
    }

    #[test]
    fn route_filters_disabled() {
        let tmp = TempDir::new().expect("tmp");
        write_skill_yaml(
            tmp.path(),
            "disabled-skill",
            r#"
id: disabled-skill
name: 已禁用
type: skill
version: "1.0.0"
chat_trigger:
  enabled: true
  keywords: ['hello']
"#,
        );

        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        let router = IntentRouter::new(&reg);
        let mut disabled = HashSet::new();
        disabled.insert("disabled-skill".into());

        let matches = router.route("hello", false, &disabled);
        assert!(matches.is_empty());
    }

    #[test]
    fn route_requires_document_filter() {
        let tmp = TempDir::new().expect("tmp");
        write_skill_yaml(
            tmp.path(),
            "doc-skill",
            r#"
id: doc-skill
name: 需要文件
type: skill
version: "1.0.0"
chat_trigger:
  enabled: true
  keywords: ['分析']
  requires_document: true
"#,
        );

        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        let router = IntentRouter::new(&reg);
        let disabled = HashSet::new();

        let m1 = router.route("帮我分析", false, &disabled);
        assert!(m1.is_empty(), "no document → no match");

        let m2 = router.route("帮我分析", true, &disabled);
        assert_eq!(m2.len(), 1);
    }

    #[test]
    fn route_exclude_pattern_vetoes() {
        let tmp = TempDir::new().expect("tmp");
        write_skill_yaml(
            tmp.path(),
            "draft-skill",
            r#"
id: draft-skill
name: 起草
type: skill
version: "1.0.0"
chat_trigger:
  enabled: true
  keywords: ['合同']
  exclude_patterns: ['审查']
"#,
        );

        let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");
        let router = IntentRouter::new(&reg);
        let disabled = HashSet::new();

        let m = router.route("帮我审查合同", false, &disabled);
        assert!(m.is_empty(), "exclude pattern should veto");

        let m2 = router.route("帮我起草合同", false, &disabled);
        assert_eq!(m2.len(), 1, "no exclude trigger → match");
    }
}
