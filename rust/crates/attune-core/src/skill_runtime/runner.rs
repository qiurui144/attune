//! SkillRunner — execute a [`Skill`] step chain (spec §3.1).
//!
//! Design (extends `workflow::WorkflowRunner`, spec B.3): fail-soft. Each step writes its typed
//! output to a runtime state map keyed by `output`; later steps reference `${id}` / `${id.field}`.
//! The terminal `export` step renders the Artifact to bytes. The bill aggregates every LLM step,
//! and the run **aborts** if the running bill exceeds [`cost::MAX_TOTAL_TOKENS`] (spec R3).
//!
//! Decoupling: RAG retrieval is behind the [`RagResolver`] trait so the runner is testable
//! without a live encrypted `Store` (the server passes a `Store`-backed resolver; tests pass an
//! in-memory map). The agent step dispatches by capability id — only the OSS
//! `document_intelligence.compare_to_table` agent is wired here; unknown agents error (so a typo
//! is caught, not silently no-op'd).

use crate::document_intelligence::token_bill::TokenBill;
use crate::export::{Artifact, ExportFormat};
use crate::llm::LlmProvider;
use crate::skill_runtime::compare_to_table::{compare_to_table, ParamComparison};
use crate::skill_runtime::cost::{self, MAX_TOTAL_TOKENS};
use crate::skill_runtime::render::comparison_to_table;
use crate::skill_runtime::schema::{InputType, Skill, SkillStep};
use serde_json::Value;
use std::collections::BTreeMap;

/// Resolve KB item ids to their decrypted text (the `rag` step's data source).
///
/// Implemented by the server over a `Store` + DEK; in tests, an in-memory map. Returning `Ok`
/// with fewer items than requested is allowed (missing ids are skipped with a warning).
pub trait RagResolver {
    /// Return the concatenated text of the given item ids (in order), skipping ids not found.
    fn resolve_items(&self, item_ids: &[String]) -> Result<String, String>;
}

/// An in-memory resolver for tests + offline skills.
pub struct MapResolver(pub BTreeMap<String, String>);

impl RagResolver for MapResolver {
    fn resolve_items(&self, item_ids: &[String]) -> Result<String, String> {
        let mut parts = Vec::new();
        for id in item_ids {
            if let Some(t) = self.0.get(id) {
                parts.push(t.clone());
            }
        }
        Ok(parts.join("\n\n"))
    }
}

/// The result of running a skill (spec §3.1 `SkillRunResult`).
#[derive(Debug, Clone)]
pub struct SkillRunResult {
    pub skill_id: String,
    /// The rendered downloadable artifact bytes + format (terminal export step output).
    pub artifact_bytes: Vec<u8>,
    pub format: ExportFormat,
    /// The Artifact IR (so the caller can also return JSON / re-render another format).
    pub artifact: Artifact,
    /// Aggregated token bill across every LLM step.
    pub token_bill: TokenBill,
    /// Non-fatal warnings (degraded steps, dropped ungrounded values, partial failure).
    pub warnings: Vec<String>,
    /// True if a non-terminal step failed but the skill still produced a (degraded) artifact.
    pub partial: bool,
}

/// Errors that abort a skill run before any artifact is produced (spec §7).
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("input invalid: {0}")]
    InputInvalid(String),
    #[error("cost not confirmed")]
    CostNotConfirmed,
    #[error("step {step_id} failed: {cause}")]
    StepFailed { step_id: String, cause: String },
    #[error("skill exceeded token cap ({used} > {cap})")]
    CostCapExceeded { used: u32, cap: u32 },
    #[error("skill produced no artifact (no export step ran)")]
    NoArtifact,
}

impl SkillError {
    /// Stable kebab error code for the HTTP `{ "code": … }` shape (spec §7).
    pub fn code(&self) -> &'static str {
        match self {
            SkillError::InputInvalid(_) => "input-invalid",
            SkillError::CostNotConfirmed => "cost-not-confirmed",
            SkillError::StepFailed { .. } => "partial-failure",
            SkillError::CostCapExceeded { .. } => "cost-cap-exceeded",
            SkillError::NoArtifact => "partial-failure",
        }
    }
}

/// Validate the run inputs against the skill's `inputs` schema (spec §7 `input-invalid`).
/// Runs **before any step** so a missing required input never reaches an LLM step.
pub fn validate_inputs(skill: &Skill, inputs: &Value) -> Result<(), SkillError> {
    let obj = inputs.as_object().ok_or_else(|| {
        SkillError::InputInvalid("inputs must be a JSON object".to_string())
    })?;
    for spec in &skill.inputs {
        match obj.get(&spec.name) {
            None | Some(Value::Null) => {
                if spec.required {
                    return Err(SkillError::InputInvalid(format!(
                        "missing required input `{}`",
                        spec.name
                    )));
                }
            }
            Some(v) => {
                let ok = match spec.ty {
                    InputType::ItemId | InputType::String => v.is_string(),
                    InputType::StringList => {
                        v.as_array().is_some_and(|a| a.iter().all(|x| x.is_string()))
                    }
                };
                if !ok {
                    return Err(SkillError::InputInvalid(format!(
                        "input `{}` has wrong type (expected {:?})",
                        spec.name, spec.ty
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Run a skill. `inputs` is the validated user payload; `confirm_cost` MUST be true for a paid
/// skill (spec §8 — guards accidental spend). `model` is the logical model name for the bill.
///
/// Fail-soft: an `agent`/`synthesize` step that degrades (e.g. LLM unparseable) records a
/// warning and continues with whatever it produced; the terminal `export` still runs so a
/// (possibly empty) downloadable artifact is always returned (`partial = true`). A hard error
/// (missing input, cost cap, render failure) aborts.
pub fn run_skill(
    skill: &Skill,
    inputs: &Value,
    confirm_cost: bool,
    resolver: &dyn RagResolver,
    llm: &dyn LlmProvider,
    model: &str,
) -> Result<SkillRunResult, SkillError> {
    validate_inputs(skill, inputs)?;

    // Cost-confirmation gate for paid skills (spec §8).
    if skill.cost_tier == crate::skill_runtime::schema::CostTier::LlmMultiStep && !confirm_cost {
        return Err(SkillError::CostNotConfirmed);
    }

    let mut state: BTreeMap<String, StepValue> = BTreeMap::new();
    let mut bill = TokenBill::default();
    let mut warnings = Vec::new();
    let mut partial = false;
    let mut final_artifact: Option<Artifact> = None;
    let mut final_bytes: Option<(Vec<u8>, ExportFormat)> = None;

    for step in &skill.steps {
        match step {
            SkillStep::Rag(s) => {
                let item_ids = resolve_item_ids(&s.input, inputs, &state);
                let text = resolver
                    .resolve_items(&item_ids)
                    .map_err(|e| SkillError::StepFailed { step_id: s.id.clone(), cause: e })?;
                state.insert(s.output.clone(), StepValue::Text(text));
            }
            SkillStep::Agent(s) => {
                // Dispatch by capability id. Only the OSS compare_to_table agent is wired.
                match s.agent.as_str() {
                    "document_intelligence.compare_to_table" => {
                        let text = lookup_text(&s.input, "text", inputs, &state);
                        let entity_a = lookup_string(&s.input, "entity_a", inputs, &state);
                        let entity_b = lookup_string(&s.input, "entity_b", inputs, &state);
                        let cmp = compare_to_table(&text, &entity_a, &entity_b, llm, model);
                        merge_bill(&mut bill, &cmp.token_bill);
                        if !cmp.warnings.is_empty() {
                            warnings.extend(cmp.warnings.iter().cloned());
                            partial = true;
                        }
                        state.insert(s.output.clone(), StepValue::Comparison(cmp));
                    }
                    other => {
                        return Err(SkillError::StepFailed {
                            step_id: s.id.clone(),
                            cause: format!("unknown agent capability `{other}`"),
                        });
                    }
                }
                // Cost cap check after each LLM step (spec R3).
                let used = bill.actual_billable_tokens();
                if used > MAX_TOTAL_TOKENS {
                    return Err(SkillError::CostCapExceeded { used, cap: cost::MAX_TOTAL_TOKENS });
                }
            }
            SkillStep::Synthesize(s) => {
                // Reserved for document skills; not used by compare_to_table. A skill that
                // declares it without wiring is a config error → fail (never silent no-op).
                return Err(SkillError::StepFailed {
                    step_id: s.id.clone(),
                    cause: "synthesize step not supported in this OSS slice".to_string(),
                });
            }
            SkillStep::Render(s) => {
                let title = lookup_string(&s.input, "title", inputs, &state);
                let title = if title.is_empty() { skill.title.clone() } else { title };
                // The render source must be a comparison (compare_to_table skill).
                let from_key = ref_key(&s.input, "from");
                let cmp = from_key
                    .and_then(|k| state.get(&k))
                    .and_then(|v| v.as_comparison());
                let artifact = match cmp {
                    Some(c) => comparison_to_table(c, &title),
                    None => {
                        return Err(SkillError::StepFailed {
                            step_id: s.id.clone(),
                            cause: "render `from` did not resolve to a comparison".to_string(),
                        });
                    }
                };
                state.insert(s.output.clone(), StepValue::Artifact(artifact));
            }
            SkillStep::Export(s) => {
                let from_key = ref_key(&s.input, "artifact");
                let artifact = from_key
                    .and_then(|k| state.get(&k))
                    .and_then(|v| v.as_artifact())
                    .cloned()
                    .ok_or_else(|| SkillError::StepFailed {
                        step_id: s.id.clone(),
                        cause: "export `artifact` did not resolve to an Artifact".to_string(),
                    })?;
                let fmt_str = lookup_string(&s.input, "format", inputs, &state);
                let format = ExportFormat::parse(&fmt_str).unwrap_or(ExportFormat::Xlsx);
                let bytes = artifact.render(format).map_err(|e| SkillError::StepFailed {
                    step_id: s.id.clone(),
                    cause: format!("{} ({})", e, e.code()),
                })?;
                final_artifact = Some(artifact);
                final_bytes = Some((bytes, format));
            }
        }
    }

    let (artifact_bytes, format) = final_bytes.ok_or(SkillError::NoArtifact)?;
    let artifact = final_artifact.ok_or(SkillError::NoArtifact)?;

    Ok(SkillRunResult {
        skill_id: skill.id.clone(),
        artifact_bytes,
        format,
        artifact,
        token_bill: bill,
        warnings,
        partial,
    })
}

/// A value flowing between steps. Typed (not stringly) so the render/export steps can pull a
/// real `Comparison`/`Artifact` without re-parsing JSON.
#[derive(Debug, Clone)]
enum StepValue {
    Text(String),
    Comparison(ParamComparison),
    Artifact(Artifact),
}

impl StepValue {
    fn as_text(&self) -> Option<&str> {
        match self {
            StepValue::Text(t) => Some(t),
            _ => None,
        }
    }
    fn as_comparison(&self) -> Option<&ParamComparison> {
        match self {
            StepValue::Comparison(c) => Some(c),
            _ => None,
        }
    }
    fn as_artifact(&self) -> Option<&Artifact> {
        match self {
            StepValue::Artifact(a) => Some(a),
            _ => None,
        }
    }
}

fn merge_bill(into: &mut TokenBill, from: &TokenBill) {
    into.map_llm_tokens.r#in = into.map_llm_tokens.r#in.saturating_add(from.map_llm_tokens.r#in);
    into.map_llm_tokens.out = into.map_llm_tokens.out.saturating_add(from.map_llm_tokens.out);
    if into.map_llm_tokens.model.is_empty() {
        into.map_llm_tokens.model = from.map_llm_tokens.model.clone();
    }
    into.reduce_llm_tokens.r#in = into.reduce_llm_tokens.r#in.saturating_add(from.reduce_llm_tokens.r#in);
    into.reduce_llm_tokens.out = into.reduce_llm_tokens.out.saturating_add(from.reduce_llm_tokens.out);
    if into.reduce_llm_tokens.model.is_empty() {
        into.reduce_llm_tokens.model = from.reduce_llm_tokens.model.clone();
    }
}

/// Extract the `${...}` reference key inside an input field (e.g. `"${diff}"` → `"diff"`,
/// `"${entities.0}"` → `"entities"` with index split handled by the caller). Returns the
/// state key (the part before the first `.`) for non-input refs.
fn ref_key(input: &BTreeMap<String, serde_yaml::Value>, field: &str) -> Option<String> {
    let raw = input.get(field)?.as_str()?;
    let inner = raw.strip_prefix("${")?.strip_suffix('}')?;
    Some(inner.split('.').next().unwrap_or(inner).to_string())
}

/// Resolve a list of item ids from a `rag` step's `input.item_ids` (each entry may be a
/// `${input_name}` reference into the user inputs, or a literal id).
fn resolve_item_ids(
    input: &BTreeMap<String, serde_yaml::Value>,
    user_inputs: &Value,
    state: &BTreeMap<String, StepValue>,
) -> Vec<String> {
    let Some(serde_yaml::Value::Sequence(seq)) = input.get("item_ids") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for v in seq {
        if let Some(s) = v.as_str() {
            for resolved in resolve_ref_to_strings(s, user_inputs, state) {
                out.push(resolved);
            }
        }
    }
    out
}

/// Resolve a single `${...}` ref or literal into one-or-more strings. Supports:
/// - `${name}` → a user input string OR string_list (expands a list to multiple ids),
/// - `${name.N}` → the Nth element of a user input string_list,
/// - `${step_output}` → a Text step value,
/// - a literal string (no `${}`) → itself.
fn resolve_ref_to_strings(
    raw: &str,
    user_inputs: &Value,
    state: &BTreeMap<String, StepValue>,
) -> Vec<String> {
    let Some(inner) = raw.strip_prefix("${").and_then(|s| s.strip_suffix('}')) else {
        return vec![raw.to_string()];
    };
    let mut parts = inner.splitn(2, '.');
    let head = parts.next().unwrap_or("");
    let tail = parts.next();

    // Prefer user inputs, then step state.
    if let Some(v) = user_inputs.get(head) {
        return match (v, tail) {
            (Value::String(s), None) => vec![s.clone()],
            (Value::Array(arr), None) => {
                arr.iter().filter_map(|x| x.as_str().map(String::from)).collect()
            }
            (Value::Array(arr), Some(idx)) => idx
                .parse::<usize>()
                .ok()
                .and_then(|i| arr.get(i))
                .and_then(|x| x.as_str())
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
            _ => Vec::new(),
        };
    }
    if let Some(StepValue::Text(t)) = state.get(head) {
        return vec![t.clone()];
    }
    Vec::new()
}

/// Resolve an input field to a single string (input ref / step Text value / literal).
fn lookup_string(
    input: &BTreeMap<String, serde_yaml::Value>,
    field: &str,
    user_inputs: &Value,
    state: &BTreeMap<String, StepValue>,
) -> String {
    let Some(raw) = input.get(field).and_then(|v| v.as_str()) else {
        return String::new();
    };
    resolve_ref_to_strings(raw, user_inputs, state)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Resolve an input field to a text blob (a step's Text output, e.g. the RAG bundle).
fn lookup_text(
    input: &BTreeMap<String, serde_yaml::Value>,
    field: &str,
    user_inputs: &Value,
    state: &BTreeMap<String, StepValue>,
) -> String {
    let Some(raw) = input.get(field).and_then(|v| v.as_str()) else {
        return String::new();
    };
    if let Some(inner) = raw.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        let head = inner.split('.').next().unwrap_or(inner);
        if let Some(v) = state.get(head).and_then(|v| v.as_text()) {
            return v.to_string();
        }
    }
    lookup_string(input, field, user_inputs, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::RecordingMockLlm;
    use crate::skill_runtime::schema::parse_skill_yaml;
    use serde_json::json;

    const SKILL_YAML: &str = r#"
id: compare-to-table
type: skill
title: 设备参数比对成表
cost_tier: llm_multi_step
trigger: { on: manual, scope: project }
inputs:
  - { name: doc, type: item_id, required: true }
  - { name: entities, type: string_list, required: true }
steps:
  - type: rag
    id: load
    input: { item_ids: ["${doc}"] }
    output: docs
  - type: agent
    id: extract
    agent: document_intelligence.compare_to_table
    input: { text: "${docs}", entity_a: "${entities.0}", entity_b: "${entities.1}" }
    output: diff
  - type: render
    id: build
    as_kind: table
    input: { from: "${diff}", title: "设备参数比对" }
    output: artifact
  - type: export
    id: out
    input: { artifact: "${artifact}", format: xlsx }
    output: file
"#;

    fn resolver(doc: &str) -> MapResolver {
        let mut m = BTreeMap::new();
        m.insert("item-1".to_string(), doc.to_string());
        MapResolver(m)
    }

    fn good_llm() -> RecordingMockLlm {
        RecordingMockLlm::new("deepseek").with_response(
            r#"{"rows":[
                {"name":"分辨率","value_a":"1080p","value_b":"4K"},
                {"name":"功耗","value_a":"5W","value_b":"12W"}
            ]}"#,
        )
    }

    #[test]
    fn end_to_end_produces_xlsx() {
        let skill = parse_skill_yaml(SKILL_YAML).unwrap();
        let doc = "设备 A 分辨率 1080p 功耗 5W。设备 B 分辨率 4K 功耗 12W。";
        let inputs = json!({ "doc": "item-1", "entities": ["设备 A", "设备 B"] });
        let res = run_skill(&skill, &inputs, true, &resolver(doc), &good_llm(), "deepseek").unwrap();
        assert_eq!(res.format, ExportFormat::Xlsx);
        assert!(!res.artifact_bytes.is_empty());
        // xlsx magic: PK zip header.
        assert_eq!(&res.artifact_bytes[..2], b"PK");
        let Artifact::Table(t) = &res.artifact else { panic!() };
        assert_eq!(t.headers, vec!["参数", "设备 A", "设备 B", "差异"]);
        assert_eq!(t.rows.len(), 2);
        assert!(!res.partial);
        assert!(res.token_bill.actual_billable_tokens() > 0);
    }

    #[test]
    fn missing_required_input_rejected_before_llm() {
        let skill = parse_skill_yaml(SKILL_YAML).unwrap();
        let llm = good_llm();
        let inputs = json!({ "entities": ["A", "B"] }); // no `doc`
        let err = run_skill(&skill, &inputs, true, &resolver("x"), &llm, "deepseek").unwrap_err();
        assert_eq!(err.code(), "input-invalid");
        assert_eq!(llm.call_count(), 0, "no LLM call when input invalid");
    }

    #[test]
    fn paid_skill_requires_cost_confirm() {
        let skill = parse_skill_yaml(SKILL_YAML).unwrap();
        let llm = good_llm();
        let inputs = json!({ "doc": "item-1", "entities": ["A", "B"] });
        let err = run_skill(&skill, &inputs, false, &resolver("x"), &llm, "deepseek").unwrap_err();
        assert_eq!(err.code(), "cost-not-confirmed");
        assert_eq!(llm.call_count(), 0);
    }

    #[test]
    fn unknown_agent_errors_not_silent() {
        let yaml = SKILL_YAML.replace(
            "document_intelligence.compare_to_table",
            "document_intelligence.does_not_exist",
        );
        let skill = parse_skill_yaml(&yaml).unwrap();
        let inputs = json!({ "doc": "item-1", "entities": ["A", "B"] });
        let err = run_skill(&skill, &inputs, true, &resolver("x"), &good_llm(), "deepseek").unwrap_err();
        assert_eq!(err.code(), "partial-failure");
    }

    #[test]
    fn llm_degrade_still_exports_with_partial_flag() {
        // LLM returns garbage → compare_to_table degrades to empty + warning; the skill still
        // exports a (headers-only) xlsx with partial=true (graceful partial failure).
        let skill = parse_skill_yaml(SKILL_YAML).unwrap();
        let mut llm = RecordingMockLlm::new("deepseek");
        for _ in 0..6 {
            llm = llm.with_response("无关散文无 JSON");
        }
        let inputs = json!({ "doc": "item-1", "entities": ["设备 A", "设备 B"] });
        let res = run_skill(&skill, &inputs, true, &resolver("设备 A 1080p"), &llm, "deepseek").unwrap();
        assert!(res.partial, "degraded run flagged partial");
        assert!(!res.warnings.is_empty());
        assert!(!res.artifact_bytes.is_empty(), "still produces a downloadable file");
    }

    #[test]
    fn entities_list_indexing_resolves() {
        // ${entities.0} / ${entities.1} must pick the right entity labels.
        let skill = parse_skill_yaml(SKILL_YAML).unwrap();
        let inputs = json!({ "doc": "item-1", "entities": ["甲", "乙"] });
        let res = run_skill(&skill, &inputs, true, &resolver("甲 5W 乙 12W"), &good_llm(), "deepseek").unwrap();
        let Artifact::Table(t) = &res.artifact else { panic!() };
        assert_eq!(t.headers[1], "甲");
        assert_eq!(t.headers[2], "乙");
    }
}
