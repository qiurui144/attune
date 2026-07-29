use attune_core::llm::noop_llm;
use attune_core::skill_runtime::{
    parse_skill_yaml, run_skill_with_dispatcher, AgentDispatcher, DispatchOutput, MapResolver,
    MAX_TOTAL_TOKENS,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SERIAL_PLUGIN_SKILL: &str = r#"
id: serial-plugin-skill
type: skill
version: "1.0.0"
title: Serial Plugin Skill
cost_tier: llm_multi_step
trigger: { on: manual, scope: project }
inputs:
  - { name: facts, type: string, required: true }
steps:
  - type: agent
    id: classify
    agent: legal_classifier
    input: { facts: "${facts}" }
    output: classified
  - type: agent
    id: draft
    agent: legal_drafter
    input: { facts: "${facts}" }
    output: drafted
  - type: render
    id: render
    as_kind: document
    input: { from: "${drafted}", title: "Draft" }
    output: artifact
  - type: export
    id: export
    input: { artifact: "${artifact}", format: docx }
    output: file
"#;

struct SerialDispatcher {
    calls: Arc<Mutex<Vec<String>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    first_tokens: u32,
    second_tokens: u32,
}

impl SerialDispatcher {
    fn new(first_tokens: u32, second_tokens: u32) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
            first_tokens,
            second_tokens,
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }
}

impl AgentDispatcher for SerialDispatcher {
    fn dispatch(&self, agent_id: &str, _input: &Value) -> Result<DispatchOutput, String> {
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(10));
        self.calls.lock().unwrap().push(agent_id.to_string());
        self.active.fetch_sub(1, Ordering::SeqCst);

        let llm_tokens = match agent_id {
            "legal_classifier" => self.first_tokens,
            "legal_drafter" => self.second_tokens,
            other => return Err(format!("unexpected agent {other}")),
        };
        Ok(DispatchOutput {
            envelope: envelope_for(agent_id),
            llm_tokens,
        })
    }
}

fn envelope_for(agent_id: &str) -> Value {
    let (heading, body) = match agent_id {
        "legal_classifier" => ("Classification", "case_type: contract"),
        "legal_drafter" => ("Draft", "The defendant should repay the principal."),
        _ => ("Unknown", ""),
    };
    json!({
        "computation": {
            "sections": [{ "heading": heading, "body": body }],
            "disclaimer": "AI-assisted draft; human review required."
        },
        "red_lines_violated": [],
        "missing_evidence": [],
        "followups": []
    })
}

#[test]
fn skill_plugin_agent_steps_are_strictly_serial() {
    let skill = parse_skill_yaml(SERIAL_PLUGIN_SKILL).unwrap();
    let inputs = json!({ "facts": "loan contract overdue" });
    let dispatcher = SerialDispatcher::new(100, 200);
    let llm = noop_llm();

    let result = run_skill_with_dispatcher(
        &skill,
        &inputs,
        true,
        &MapResolver(BTreeMap::new()),
        llm.as_ref(),
        "deepseek-v4-flash",
        Some(&dispatcher),
    )
    .unwrap();

    assert_eq!(
        dispatcher.calls(),
        vec!["legal_classifier".to_string(), "legal_drafter".to_string()],
        "SkillRunner must preserve declared step order"
    );
    assert_eq!(
        dispatcher.max_active(),
        1,
        "SkillRunner must not overlap serial plugin-agent steps"
    );
    assert_eq!(result.token_bill.map_llm_tokens.r#in, 300);
    assert_eq!(result.format, attune_core::export::ExportFormat::Docx);
    assert!(!result.artifact_bytes.is_empty());
}

#[test]
fn cost_cap_stops_downstream_steps_immediately() {
    let skill = parse_skill_yaml(SERIAL_PLUGIN_SKILL).unwrap();
    let inputs = json!({ "facts": "loan contract overdue" });
    let dispatcher = SerialDispatcher::new(MAX_TOTAL_TOKENS + 1, 200);
    let llm = noop_llm();

    let err = run_skill_with_dispatcher(
        &skill,
        &inputs,
        true,
        &MapResolver(BTreeMap::new()),
        llm.as_ref(),
        "deepseek-v4-flash",
        Some(&dispatcher),
    )
    .unwrap_err();

    assert_eq!(err.code(), "cost-cap-exceeded");
    assert_eq!(
        dispatcher.calls(),
        vec!["legal_classifier".to_string()],
        "downstream paid steps must not run after the cost cap is exceeded"
    );
    assert_eq!(dispatcher.max_active(), 1);
}
