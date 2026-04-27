//! Workflow YAML schema 类型映射 + 解析器（spec §3.3）

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
    /// 'file_added' / 'manual'
    pub on: String,
    /// 'project' / 'global'
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
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeterministicStep {
    pub id: String,
    pub operation: String,
    #[serde(default)]
    pub input: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub output: Option<String>,
}

/// 解析 workflow yaml 字符串。
pub fn parse_workflow_yaml(yaml: &str) -> Result<Workflow, String> {
    serde_yaml::from_str(yaml).map_err(|e| format!("parse workflow yaml: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CROSS_ENTITY_WORKFLOW_YAML: &str = r#"
id: examples/cross_entity_inference
type: workflow
trigger:
  on: file_added
  scope: project
steps:
  - id: extract_entities
    type: skill
    skill: examples/entity_extraction
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
    fn parse_cross_entity_workflow() {
        let wf = parse_workflow_yaml(CROSS_ENTITY_WORKFLOW_YAML).expect("parse");
        assert_eq!(wf.id, "examples/cross_entity_inference");
        assert_eq!(wf.kind, "workflow");
        assert_eq!(wf.trigger.on, "file_added");
        assert_eq!(wf.trigger.scope, "project");
        assert_eq!(wf.steps.len(), 3);

        match &wf.steps[0] {
            WorkflowStep::Skill(s) => {
                assert_eq!(s.id, "extract_entities");
                assert_eq!(s.skill, "examples/entity_extraction");
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
        assert!(parse_workflow_yaml(bad).is_err());
    }
}
