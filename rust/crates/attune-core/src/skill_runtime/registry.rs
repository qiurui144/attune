//! Skill registry — built-in OSS skills + a `register_plugin_skills` extension point for pro
//! verticals (spec §6.4). Built-in skill YAML is embedded via `include_str!` so it ships in the
//! binary (no on-disk asset lookup, works the same in the desktop bundle and the K3 image).

use crate::skill_runtime::schema::{parse_skill_yaml, validate_skill, Skill};
use std::collections::BTreeMap;

/// The embedded compare-to-table skill (用户例 3).
const COMPARE_TO_TABLE_YAML: &str = include_str!("../../assets/skills/compare-to-table.yaml");

/// A loaded skill plus its provenance (`oss` or `pro:<vertical>`), for the `/skills` listing.
#[derive(Debug, Clone)]
pub struct RegisteredSkill {
    pub skill: Skill,
    /// `"oss"` for built-ins, `"pro:<vertical>"` for plugin-registered skills.
    pub source: String,
}

/// The in-process skill registry. Built once at startup; pro plugin skills are injected via
/// [`SkillRegistry::register_plugin_skill`] by the plugin loader.
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    by_id: BTreeMap<String, RegisteredSkill>,
}

impl SkillRegistry {
    /// Build the registry with all embedded OSS skills loaded. Panics only if a *built-in* skill
    /// fails to parse — that is a build-time error, caught by the unit test below.
    pub fn with_builtins() -> Self {
        let mut reg = SkillRegistry::default();
        reg.load_builtin(COMPARE_TO_TABLE_YAML);
        reg
    }

    fn load_builtin(&mut self, yaml: &str) {
        let skill = parse_skill_yaml(yaml)
            .unwrap_or_else(|e| panic!("built-in skill yaml failed to parse: {e}"));
        self.by_id.insert(
            skill.id.clone(),
            RegisteredSkill { skill, source: "oss".to_string() },
        );
    }

    /// Register a pro plugin skill (spec §6.4 extension point). Validates the skill before
    /// inserting; returns an error (never panics) so a bad plugin manifest doesn't crash boot.
    pub fn register_plugin_skill(
        &mut self,
        vertical: &str,
        yaml: &str,
    ) -> Result<(), String> {
        let skill = parse_skill_yaml(yaml)?;
        validate_skill(&skill)?;
        self.by_id.insert(
            skill.id.clone(),
            RegisteredSkill { skill, source: format!("pro:{vertical}") },
        );
        Ok(())
    }

    /// Look up a skill by id.
    pub fn get(&self, id: &str) -> Option<&RegisteredSkill> {
        self.by_id.get(id)
    }

    /// List all registered skills (stable id order).
    pub fn list(&self) -> Vec<&RegisteredSkill> {
        self.by_id.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_runtime::schema::CostTier;

    #[test]
    fn builtins_load_and_compare_to_table_present() {
        let reg = SkillRegistry::with_builtins();
        let s = reg.get("compare-to-table").expect("compare-to-table registered");
        assert_eq!(s.source, "oss");
        assert_eq!(s.skill.cost_tier, CostTier::LlmMultiStep);
        assert_eq!(s.skill.trigger.on, "manual", "paid skill must be manual");
    }

    #[test]
    fn pro_skill_registration_tags_source() {
        let mut reg = SkillRegistry::with_builtins();
        let yaml = r#"
id: bid-doc-pro
type: skill
cost_tier: llm_multi_step
trigger: { on: manual, scope: project }
steps:
  - type: agent
    id: cmp
    agent: document_intelligence.compare_to_table
    input: {}
    output: diff
  - type: render
    id: build
    as_kind: table
    input: {}
    output: artifact
  - type: export
    id: out
    input: {}
    output: file
"#;
        reg.register_plugin_skill("presales", yaml).unwrap();
        assert_eq!(reg.get("bid-doc-pro").unwrap().source, "pro:presales");
        assert!(reg.list().len() >= 2);
    }

    #[test]
    fn bad_pro_skill_rejected_not_panicked() {
        let mut reg = SkillRegistry::with_builtins();
        // A paid skill with a background trigger must be rejected (cost guard).
        let bad = r#"
id: sneaky
type: skill
cost_tier: llm_multi_step
trigger: { on: file_added, scope: project }
steps:
  - type: export
    id: out
    input: {}
    output: file
"#;
        let err = reg.register_plugin_skill("evil", bad).unwrap_err();
        assert!(err.contains("must be trigger.on=manual"));
        assert!(reg.get("sneaky").is_none(), "rejected skill not registered");
    }
}
