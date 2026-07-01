//! SkillRunner — execute a [`Skill`] step chain (spec §3.1).
//!
//! Design (extends `workflow::WorkflowRunner`, spec B.3): fail-soft. Each step writes its typed
//! output to a runtime state map keyed by `output`; later steps reference `${id}` / `${id.field}`.
//! The terminal `export` step renders the Artifact to bytes. The bill aggregates every LLM step,
//! and the run **aborts** if the running bill exceeds [`cost::MAX_TOTAL_TOKENS`] (spec R3).
//!
//! Decoupling: RAG retrieval is behind the [`RagResolver`] trait so the runner is testable
//! without a live encrypted `Store` (the server passes a `Store`-backed resolver; tests pass an
//! in-memory map). The agent step dispatches by capability id: the OSS in-process agents
//! (`compare_to_table` / `research_synthesis` / `reference_generate`) are wired here directly;
//! **any other agent id is a pro plugin agent** and is routed through the optional
//! [`AgentDispatcher`] to that plugin's binary subprocess. With no dispatcher (or a typo'd id the
//! dispatcher rejects) the step fails-soft into a warning, so a misconfigured skill never silently
//! ships an empty deliverable — it ships a degraded one with the failure surfaced.

use crate::document_intelligence::token_bill::TokenBill;
use crate::export::{Artifact, ExportFormat};
use crate::llm::LlmProvider;
use crate::skill_runtime::compare_to_table::{compare_to_table, ParamComparison};
use crate::skill_runtime::cost::{self, MAX_TOTAL_TOKENS};
use crate::skill_runtime::dispatch::{
    agent_doc_to_document, parse_agent_doc, AgentDispatcher, AgentDocOutput,
};
use crate::skill_runtime::doc_render::{sections_to_document, writing_to_document};
use crate::skill_runtime::reference_generate::reference_generate;
use crate::skill_runtime::render::comparison_to_table;
use crate::skill_runtime::research_synthesis::{parse_structure, research_synthesis};
use crate::skill_runtime::schema::{InputType, Skill, SkillStep};
use crate::writing::SourceMaterial;
use serde_json::Value;
use std::collections::BTreeMap;

/// Resolve KB item ids to their decrypted text (the `rag` step's data source).
///
/// Implemented by the server over a `Store` + DEK; in tests, an in-memory map. Returning `Ok`
/// with fewer items than requested is allowed (missing ids are skipped with a warning).
pub trait RagResolver: Send {
    /// Return the concatenated text of the given item ids (in order), skipping ids not found.
    fn resolve_items(&self, item_ids: &[String]) -> Result<String, String>;

    /// Return one [`SourceMaterial`] (item_id + text) per resolved id, skipping ids not found.
    ///
    /// Document-mode skills (synthesis / reference) need **per-source** material so each retrieved
    /// item can be grounded independently (a concatenated blob loses provenance). Default impl
    /// derives a single-source bundle from [`Self::resolve_items`] so existing resolvers stay
    /// valid; resolvers that can preserve ids should override it (the server's does).
    fn resolve_sources(&self, item_ids: &[String]) -> Result<Vec<SourceMaterial>, String> {
        let text = self.resolve_items(item_ids)?;
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![SourceMaterial::new(String::new(), text)])
    }
}

/// An in-memory resolver for tests + offline skills. Preserves item ids for per-source grounding.
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

    fn resolve_sources(&self, item_ids: &[String]) -> Result<Vec<SourceMaterial>, String> {
        let mut out = Vec::new();
        for id in item_ids {
            if let Some(t) = self.0.get(id) {
                out.push(SourceMaterial::new(id.clone(), t.clone()));
            }
        }
        Ok(out)
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
    let obj = inputs
        .as_object()
        .ok_or_else(|| SkillError::InputInvalid("inputs must be a JSON object".to_string()))?;
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
                    InputType::StringList => v
                        .as_array()
                        .is_some_and(|a| a.iter().all(|x| x.is_string())),
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
    run_skill_with_dispatcher(skill, inputs, confirm_cost, resolver, llm, model, None)
}

/// Like [`run_skill`] but with an [`AgentDispatcher`] for **pro plugin agent** steps (law
/// `legal_drafter`, patent `oa_response`, …). OSS agent ids are still handled in-process; any
/// other id is routed to `dispatcher`. `None` ⇒ OSS-only (a plugin agent step then fails-soft to
/// a degraded artifact + warning, never a panic). The server passes a subprocess-backed
/// dispatcher (which enforces the install + entitlement + timeout + LLM-env boundary).
#[allow(clippy::too_many_arguments)]
pub fn run_skill_with_dispatcher(
    skill: &Skill,
    inputs: &Value,
    confirm_cost: bool,
    resolver: &dyn RagResolver,
    llm: &dyn LlmProvider,
    model: &str,
    dispatcher: Option<&dyn AgentDispatcher>,
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
                // Resolve per-source (item_id + text) so document skills keep provenance, and also
                // derive the concatenated text for table/compare skills — one rag step serves both.
                let sources =
                    resolver
                        .resolve_sources(&item_ids)
                        .map_err(|e| SkillError::StepFailed {
                            step_id: s.id.clone(),
                            cause: e,
                        })?;
                let text = sources
                    .iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                state.insert(s.output.clone(), StepValue::Rag { text, sources });
            }
            SkillStep::Agent(s) => {
                // Dispatch by capability id (typo → error, never a silent no-op).
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
                    // research_synthesis (用户例 1/2): multi-domain sources → grounded document.
                    "writing.research_synthesis" => {
                        let sources = lookup_sources(&s.input, "sources", &state);
                        let structure = {
                            let raw = lookup_string(&s.input, "structure", inputs, &state);
                            parse_structure(&raw)
                        };
                        match research_synthesis(&sources, structure, 0, llm) {
                            Ok(wr) => {
                                merge_bill(&mut bill, &wr.token_bill);
                                if !wr.unverified_spans.is_empty() {
                                    warnings.push(format!(
                                        "{} 段综述结论未能回溯到来源，已标记需核实",
                                        wr.unverified_spans.len()
                                    ));
                                    partial = true;
                                }
                                state.insert(s.output.clone(), StepValue::Writing(wr));
                            }
                            Err(e) => {
                                // Degrade: record a warning + an empty writing result so the
                                // terminal export still yields a (degraded) downloadable file.
                                warnings.push(format!("综述生成失败（{}）", e.code()));
                                partial = true;
                                state.insert(s.output.clone(), StepValue::Writing(empty_writing()));
                            }
                        }
                    }
                    // reference_generate (用户例 4): reference doc + source data → new document.
                    "writing.reference_generate" => {
                        let reference = lookup_text(&s.input, "reference", inputs, &state);
                        let sources = lookup_sources(&s.input, "sources", &state);
                        let title = lookup_string(&s.input, "title", inputs, &state);
                        match reference_generate(&reference, &sources, &title, llm) {
                            Ok(doc) => {
                                merge_bill(&mut bill, &doc.token_bill);
                                if !doc.unverified_sections.is_empty() {
                                    warnings.push(format!(
                                        "{} 个章节内容未能回溯到素材，已标记需核实",
                                        doc.unverified_sections.len()
                                    ));
                                    partial = true;
                                }
                                if !doc.warnings.is_empty() {
                                    warnings.extend(doc.warnings.iter().cloned());
                                    partial = true;
                                }
                                state.insert(s.output.clone(), StepValue::Reference(doc));
                            }
                            Err(e) => {
                                warnings.push(format!("参考式生成失败（{}）", e.code()));
                                partial = true;
                                state.insert(
                                    s.output.clone(),
                                    StepValue::Reference(empty_reference()),
                                );
                            }
                        }
                    }
                    // A typo'd OSS capability (an id in the reserved OSS namespaces that didn't
                    // match an in-process arm) is a build error, not a plugin agent — caught hard
                    // so a misspelled built-in is never silently degraded.
                    other if is_reserved_oss_namespace(other) => {
                        return Err(SkillError::StepFailed {
                            step_id: s.id.clone(),
                            cause: format!("unknown OSS agent capability `{other}`"),
                        });
                    }
                    // Any other agent id is a **pro plugin agent** — route to the dispatcher
                    // (subprocess on the server, stub in tests). Fail-soft: a dispatch failure
                    // (no dispatcher / unknown id / not installed / timeout / non-zero exit)
                    // records a warning + an empty doc so the export still yields a (degraded)
                    // file rather than aborting the whole skill.
                    plugin_agent => {
                        let agent_input = build_agent_input(&s.input, inputs, &state);
                        match dispatcher {
                            None => {
                                warnings.push(format!(
                                    "插件 agent `{plugin_agent}` 未接入调度器，已跳过（请安装对应 pro 插件）"
                                ));
                                partial = true;
                                state.insert(
                                    s.output.clone(),
                                    StepValue::AgentDoc(AgentDocOutput::default()),
                                );
                            }
                            Some(d) => match d.dispatch(plugin_agent, &agent_input) {
                                Ok(out) => {
                                    bill.map_llm_tokens.r#in =
                                        bill.map_llm_tokens.r#in.saturating_add(out.llm_tokens);
                                    if bill.map_llm_tokens.model.is_empty() {
                                        bill.map_llm_tokens.model = model.to_string();
                                    }
                                    let doc = parse_agent_doc(&out.envelope);
                                    if !doc.red_lines.is_empty() {
                                        warnings.push(format!(
                                            "插件 agent `{plugin_agent}` 触发 {} 条红线，已在文书顶部标注",
                                            doc.red_lines.len()
                                        ));
                                        partial = true;
                                    }
                                    if !doc.needs_confirm_idx.is_empty() {
                                        warnings.push(format!(
                                            "{} 处需人工确认，已在对应段落标记",
                                            doc.needs_confirm_idx.len()
                                        ));
                                        partial = true;
                                    }
                                    state.insert(s.output.clone(), StepValue::AgentDoc(doc));
                                }
                                Err(e) => {
                                    warnings.push(format!(
                                        "插件 agent `{plugin_agent}` 调度失败（{e}）"
                                    ));
                                    partial = true;
                                    state.insert(
                                        s.output.clone(),
                                        StepValue::AgentDoc(AgentDocOutput::default()),
                                    );
                                }
                            },
                        }
                    }
                }
                // Cost cap check after each LLM step (spec R3).
                let used = bill.actual_billable_tokens();
                if used > MAX_TOTAL_TOKENS {
                    return Err(SkillError::CostCapExceeded {
                        used,
                        cap: cost::MAX_TOTAL_TOKENS,
                    });
                }
            }
            SkillStep::Synthesize(s) => {
                // `synthesize` step = a grounded multi-source synthesis over a `rag` step's sources
                // (same engine as the `writing.research_synthesis` agent; declarable as a step too).
                let sources = lookup_sources(&s.input, "sources", &state);
                let structure =
                    parse_structure(&lookup_string(&s.input, "structure", inputs, &state));
                match research_synthesis(&sources, structure, 0, llm) {
                    Ok(wr) => {
                        merge_bill(&mut bill, &wr.token_bill);
                        if !wr.unverified_spans.is_empty() {
                            warnings.push(format!(
                                "{} 段综述结论未能回溯到来源，已标记需核实",
                                wr.unverified_spans.len()
                            ));
                            partial = true;
                        }
                        state.insert(s.output.clone(), StepValue::Writing(wr));
                    }
                    Err(e) => {
                        warnings.push(format!("综述生成失败（{}）", e.code()));
                        partial = true;
                        state.insert(s.output.clone(), StepValue::Writing(empty_writing()));
                    }
                }
                let used = bill.actual_billable_tokens();
                if used > MAX_TOTAL_TOKENS {
                    return Err(SkillError::CostCapExceeded {
                        used,
                        cap: cost::MAX_TOTAL_TOKENS,
                    });
                }
            }
            SkillStep::Render(s) => {
                let title = lookup_string(&s.input, "title", inputs, &state);
                let title = if title.is_empty() {
                    skill.title.clone()
                } else {
                    title
                };
                let from_key = ref_key(&s.input, "from");
                let from = from_key.and_then(|k| state.get(&k));
                let artifact = match from {
                    // table render ← a parameter comparison.
                    Some(StepValue::Comparison(c)) => comparison_to_table(c, &title),
                    // document render ← a writing result (synthesis).
                    Some(StepValue::Writing(w)) => writing_to_document(w, &title),
                    // document render ← a reference-generated section list.
                    Some(StepValue::Reference(r)) => {
                        sections_to_document(&title, &r.sections, &r.unverified_sections)
                    }
                    // document render ← a pro plugin agent's document-shaped output.
                    Some(StepValue::AgentDoc(d)) => agent_doc_to_document(d, &title),
                    _ => {
                        return Err(SkillError::StepFailed {
                            step_id: s.id.clone(),
                            cause: "render `from` did not resolve to a renderable step output"
                                .to_string(),
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
                let bytes = artifact
                    .render(format)
                    .map_err(|e| SkillError::StepFailed {
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
/// real `Comparison`/`Writing`/`Artifact` without re-parsing JSON.
#[derive(Debug, Clone)]
enum StepValue {
    /// A `rag` step's output: the concatenated text (for table/compare skills) AND the per-source
    /// material with ids preserved (for document/synthesis skills).
    Rag {
        text: String,
        sources: Vec<SourceMaterial>,
    },
    Comparison(ParamComparison),
    /// A writing-engine result (synthesis) destined for a document render.
    Writing(crate::writing::WritingResult),
    /// A reference-generated section list destined for a document render.
    Reference(crate::skill_runtime::reference_generate::ReferenceDoc),
    /// A pro plugin agent's document-shaped output (from the [`AgentDispatcher`]).
    AgentDoc(AgentDocOutput),
    Artifact(Artifact),
}

impl StepValue {
    fn as_text(&self) -> Option<&str> {
        match self {
            StepValue::Rag { text, .. } => Some(text),
            _ => None,
        }
    }
    fn as_sources(&self) -> Option<&[SourceMaterial]> {
        match self {
            StepValue::Rag { sources, .. } => Some(sources),
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

/// An empty-but-valid writing result for the degrade path (so a failed synthesis still exports).
fn empty_writing() -> crate::writing::WritingResult {
    crate::writing::WritingResult {
        schema_version: crate::writing::WRITING_SCHEMA_VERSION,
        mode: crate::writing::WritingMode::Synthesis,
        content: String::new(),
        segments: Vec::new(),
        annotations: Vec::new(),
        unverified_spans: Vec::new(),
        token_bill: TokenBill::default(),
    }
}

/// An empty-but-valid reference doc for the degrade path.
fn empty_reference() -> crate::skill_runtime::reference_generate::ReferenceDoc {
    crate::skill_runtime::reference_generate::ReferenceDoc {
        sections: Vec::new(),
        unverified_sections: Vec::new(),
        warnings: Vec::new(),
        token_bill: TokenBill::default(),
    }
}

fn merge_bill(into: &mut TokenBill, from: &TokenBill) {
    into.map_llm_tokens.r#in = into
        .map_llm_tokens
        .r#in
        .saturating_add(from.map_llm_tokens.r#in);
    into.map_llm_tokens.out = into
        .map_llm_tokens
        .out
        .saturating_add(from.map_llm_tokens.out);
    if into.map_llm_tokens.model.is_empty() {
        into.map_llm_tokens.model = from.map_llm_tokens.model.clone();
    }
    into.reduce_llm_tokens.r#in = into
        .reduce_llm_tokens
        .r#in
        .saturating_add(from.reduce_llm_tokens.r#in);
    into.reduce_llm_tokens.out = into
        .reduce_llm_tokens
        .out
        .saturating_add(from.reduce_llm_tokens.out);
    if into.reduce_llm_tokens.model.is_empty() {
        into.reduce_llm_tokens.model = from.reduce_llm_tokens.model.clone();
    }
}

/// The reserved namespaces of the OSS in-process agents. An agent id in one of these that did
/// not match an in-process arm is a typo (a misspelled built-in), caught as a hard error rather
/// than degraded as if it were a plugin agent. Plugin agent ids (e.g. `legal_drafter`,
/// `oa_response_agent`) are bare names outside these namespaces and route to the dispatcher.
const RESERVED_OSS_NAMESPACES: [&str; 2] = ["document_intelligence.", "writing."];

/// True if `agent` is in a reserved OSS namespace (so an unmatched id is a built-in typo).
fn is_reserved_oss_namespace(agent: &str) -> bool {
    RESERVED_OSS_NAMESPACES
        .iter()
        .any(|ns| agent.starts_with(ns))
}

/// Build the stdin JSON object a **plugin agent** receives, from a skill agent-step's `input`
/// map. Each YAML value is resolved: a `"${ref}"` string is replaced with the resolved value
/// (user input scalar/list, or a prior rag step's text blob); a nested mapping/sequence is
/// recursively resolved; any other literal (string/number/bool) passes through. This is how the
/// declarative skill yaml feeds a typed agent input (e.g. `legal_drafter`'s `{ docType, caseId,
/// facts: { freeText } }`) without the runner knowing each agent's schema.
fn build_agent_input(
    input: &BTreeMap<String, serde_yaml::Value>,
    user_inputs: &Value,
    state: &BTreeMap<String, StepValue>,
) -> Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in input {
        obj.insert(k.clone(), resolve_yaml_value(v, user_inputs, state));
    }
    Value::Object(obj)
}

/// Resolve one yaml value into JSON, expanding `${...}` string refs and recursing into
/// maps/sequences. A `${ref}` that does not resolve becomes JSON null (the agent's schema
/// validation then decides — a required field missing surfaces as the agent's own input error).
fn resolve_yaml_value(
    v: &serde_yaml::Value,
    user_inputs: &Value,
    state: &BTreeMap<String, StepValue>,
) -> Value {
    match v {
        serde_yaml::Value::String(s) => {
            if s.starts_with("${") && s.ends_with('}') {
                // Prefer a list expansion (user string_list) → JSON array; else first scalar; else
                // a rag step's text blob; else null.
                let resolved = resolve_ref_to_strings(s, user_inputs, state);
                match resolved.len() {
                    0 => {
                        // maybe it's a ${rag_output} text ref.
                        let inner = s.trim_start_matches("${").trim_end_matches('}');
                        let head = inner.split('.').next().unwrap_or(inner);
                        if let Some(t) = state.get(head).and_then(|sv| sv.as_text()) {
                            Value::String(t.to_string())
                        } else {
                            Value::Null
                        }
                    }
                    1 => Value::String(resolved.into_iter().next().unwrap()),
                    _ => Value::Array(resolved.into_iter().map(Value::String).collect()),
                }
            } else {
                Value::String(s.clone())
            }
        }
        serde_yaml::Value::Mapping(m) => {
            let mut obj = serde_json::Map::new();
            for (mk, mv) in m {
                if let Some(key) = mk.as_str() {
                    obj.insert(key.to_string(), resolve_yaml_value(mv, user_inputs, state));
                }
            }
            Value::Object(obj)
        }
        serde_yaml::Value::Sequence(seq) => Value::Array(
            seq.iter()
                .map(|e| resolve_yaml_value(e, user_inputs, state))
                .collect(),
        ),
        serde_yaml::Value::Bool(b) => Value::Bool(*b),
        serde_yaml::Value::Number(n) => serde_json::Number::from_f64(n.as_f64().unwrap_or(0.0))
            .map(Value::Number)
            .unwrap_or(Value::Null),
        serde_yaml::Value::Null => Value::Null,
        _ => Value::Null,
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
            (Value::Array(arr), None) => arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect(),
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
    if let Some(t) = state.get(head).and_then(|v| v.as_text()) {
        return vec![t.to_string()];
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

/// Resolve an input field that references a `rag` step's per-source material (`${rag_output}`).
/// Returns the source list with item ids preserved; empty if the ref does not resolve to a rag
/// step output.
fn lookup_sources(
    input: &BTreeMap<String, serde_yaml::Value>,
    field: &str,
    state: &BTreeMap<String, StepValue>,
) -> Vec<SourceMaterial> {
    let Some(raw) = input.get(field).and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    if let Some(inner) = raw.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        let head = inner.split('.').next().unwrap_or(inner);
        if let Some(sources) = state.get(head).and_then(|v| v.as_sources()) {
            return sources.to_vec();
        }
    }
    Vec::new()
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
        let res = run_skill(
            &skill,
            &inputs,
            true,
            &resolver(doc),
            &good_llm(),
            "deepseek",
        )
        .unwrap();
        assert_eq!(res.format, ExportFormat::Xlsx);
        assert!(!res.artifact_bytes.is_empty());
        // xlsx magic: PK zip header.
        assert_eq!(&res.artifact_bytes[..2], b"PK");
        let Artifact::Table(t) = &res.artifact else {
            panic!()
        };
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
        let err = run_skill(
            &skill,
            &inputs,
            true,
            &resolver("x"),
            &good_llm(),
            "deepseek",
        )
        .unwrap_err();
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
        let res = run_skill(
            &skill,
            &inputs,
            true,
            &resolver("设备 A 1080p"),
            &llm,
            "deepseek",
        )
        .unwrap();
        assert!(res.partial, "degraded run flagged partial");
        assert!(!res.warnings.is_empty());
        assert!(
            !res.artifact_bytes.is_empty(),
            "still produces a downloadable file"
        );
    }

    #[test]
    fn entities_list_indexing_resolves() {
        // ${entities.0} / ${entities.1} must pick the right entity labels.
        let skill = parse_skill_yaml(SKILL_YAML).unwrap();
        let inputs = json!({ "doc": "item-1", "entities": ["甲", "乙"] });
        let res = run_skill(
            &skill,
            &inputs,
            true,
            &resolver("甲 5W 乙 12W"),
            &good_llm(),
            "deepseek",
        )
        .unwrap();
        let Artifact::Table(t) = &res.artifact else {
            panic!()
        };
        assert_eq!(t.headers[1], "甲");
        assert_eq!(t.headers[2], "乙");
    }

    // ───────────────── pro plugin agent dispatch (CAP-4b) ─────────────────

    use crate::skill_runtime::dispatch::{AgentDispatcher, DispatchOutput};
    use std::cell::RefCell;

    /// A pro-agent skill: rag → (plugin) agent → document render → export docx.
    const PRO_SKILL_YAML: &str = r#"
id: complaint-draft-pro
type: skill
version: "1.0.0"
title: 起诉状起草
cost_tier: llm_multi_step
trigger: { on: manual, scope: project }
inputs:
  - { name: caseId, type: string, required: true }
  - { name: facts, type: string, required: true }
steps:
  - type: agent
    id: draft
    agent: legal_drafter
    input:
      docType: complaint
      caseId: "${caseId}"
      facts: { freeText: "${facts}", useExtracted: false }
    output: drafted
  - type: render
    id: build
    as_kind: document
    input: { from: "${drafted}", title: "民事起诉状" }
    output: artifact
  - type: export
    id: out
    input: { artifact: "${artifact}", format: docx }
    output: file
"#;

    /// A stub dispatcher recording the (agent_id, input) it was called with and returning a
    /// canned envelope. Lets the runner be exercised without a live plugin subprocess.
    struct StubDispatcher {
        calls: RefCell<Vec<(String, Value)>>,
        result: Result<DispatchOutput, String>,
    }
    impl StubDispatcher {
        fn ok(envelope: Value, tokens: u32) -> Self {
            StubDispatcher {
                calls: RefCell::new(Vec::new()),
                result: Ok(DispatchOutput {
                    envelope,
                    llm_tokens: tokens,
                }),
            }
        }
        fn err(msg: &str) -> Self {
            StubDispatcher {
                calls: RefCell::new(Vec::new()),
                result: Err(msg.to_string()),
            }
        }
    }
    impl AgentDispatcher for StubDispatcher {
        fn dispatch(&self, agent_id: &str, input: &Value) -> Result<DispatchOutput, String> {
            self.calls
                .borrow_mut()
                .push((agent_id.to_string(), input.clone()));
            self.result.clone()
        }
    }

    fn empty_resolver() -> MapResolver {
        MapResolver(BTreeMap::new())
    }

    fn draft_envelope() -> Value {
        json!({
            "computation": {
                "docType": "complaint",
                "draft": "全文……",
                "sections": [
                    { "heading": "诉讼请求", "body": "请求判令被告偿还借款本金 10 万元及利息。" },
                    { "heading": "事实与理由", "body": "原告与被告于 2024 年签订借款合同，被告逾期未还。" }
                ],
                "unresolved": [],
                "disclaimer": "本文书为 AI 辅助初稿，须经执业律师审核后使用。"
            },
            "red_lines_violated": [],
            "missing_evidence": [],
            "followups": []
        })
    }

    #[test]
    fn pro_agent_dispatched_and_rendered_to_docx() {
        let skill = parse_skill_yaml(PRO_SKILL_YAML).unwrap();
        let inputs = json!({ "caseId": "case-1", "facts": "借款 10 万逾期未还" });
        let disp = StubDispatcher::ok(draft_envelope(), 4200);
        let res = run_skill_with_dispatcher(
            &skill,
            &inputs,
            true,
            &empty_resolver(),
            &good_llm(),
            "deepseek-v4-flash",
            Some(&disp),
        )
        .unwrap();
        // dispatcher was called with the right agent id + resolved typed input.
        let calls = disp.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "legal_drafter");
        assert_eq!(calls[0].1["docType"], "complaint");
        assert_eq!(calls[0].1["caseId"], "case-1");
        assert_eq!(calls[0].1["facts"]["freeText"], "借款 10 万逾期未还");
        assert_eq!(calls[0].1["facts"]["useExtracted"], false);
        // a docx file was produced.
        assert_eq!(res.format, ExportFormat::Docx);
        assert!(!res.artifact_bytes.is_empty());
        assert_eq!(&res.artifact_bytes[..2], b"PK", "docx is a zip");
        // the rendered document carries the drafted sections + disclaimer.
        let Artifact::Document(d) = &res.artifact else {
            panic!()
        };
        assert_eq!(d.title.as_deref(), Some("民事起诉状"));
        let s = serde_json::to_string(d).unwrap();
        assert!(s.contains("诉讼请求"));
        assert!(s.contains("执业律师"));
        assert!(!res.partial, "clean draft is not partial");
        assert_eq!(
            res.token_bill.map_llm_tokens.r#in, 4200,
            "agent tokens billed"
        );
    }

    #[test]
    fn pro_agent_without_dispatcher_degrades_not_aborts() {
        // No dispatcher → the plugin agent step fails-soft (warning + empty doc), export still runs.
        let skill = parse_skill_yaml(PRO_SKILL_YAML).unwrap();
        let inputs = json!({ "caseId": "case-1", "facts": "x" });
        let res = run_skill_with_dispatcher(
            &skill,
            &inputs,
            true,
            &empty_resolver(),
            &good_llm(),
            "deepseek",
            None,
        )
        .unwrap();
        assert!(res.partial);
        assert!(res
            .warnings
            .iter()
            .any(|w| w.contains("legal_drafter") && w.contains("未接入调度器")));
        assert!(
            !res.artifact_bytes.is_empty(),
            "degraded run still yields a downloadable file"
        );
    }

    #[test]
    fn pro_agent_dispatch_error_degrades_with_warning() {
        // dispatcher returns Err (unknown id / not installed / timeout) → warning + degraded doc.
        let skill = parse_skill_yaml(PRO_SKILL_YAML).unwrap();
        let inputs = json!({ "caseId": "case-1", "facts": "x" });
        let disp = StubDispatcher::err("agent 'legal_drafter' not found in any loaded plugin");
        let res = run_skill_with_dispatcher(
            &skill,
            &inputs,
            true,
            &empty_resolver(),
            &good_llm(),
            "deepseek",
            Some(&disp),
        )
        .unwrap();
        assert!(res.partial);
        assert!(res
            .warnings
            .iter()
            .any(|w| w.contains("调度失败") && w.contains("not found")));
    }

    #[test]
    fn pro_agent_red_line_surfaced_as_warning_and_marked() {
        let skill = parse_skill_yaml(PRO_SKILL_YAML).unwrap();
        let inputs = json!({ "caseId": "c", "facts": "f" });
        let mut env = draft_envelope();
        env["red_lines_violated"] = json!(["no_hallucinated_citation"]);
        let disp = StubDispatcher::ok(env, 100);
        let res = run_skill_with_dispatcher(
            &skill,
            &inputs,
            true,
            &empty_resolver(),
            &good_llm(),
            "deepseek",
            Some(&disp),
        )
        .unwrap();
        assert!(res.partial);
        assert!(res.warnings.iter().any(|w| w.contains("红线")));
        let s = serde_json::to_string(&res.artifact).unwrap();
        assert!(
            s.contains("no_hallucinated_citation"),
            "red line surfaced in document"
        );
    }

    #[test]
    fn pro_agent_tokens_count_toward_cost_cap() {
        // A dispatched agent reporting > MAX_TOTAL_TOKENS aborts with cost-cap-exceeded.
        let skill = parse_skill_yaml(PRO_SKILL_YAML).unwrap();
        let inputs = json!({ "caseId": "c", "facts": "f" });
        let disp = StubDispatcher::ok(draft_envelope(), MAX_TOTAL_TOKENS + 1);
        let err = run_skill_with_dispatcher(
            &skill,
            &inputs,
            true,
            &empty_resolver(),
            &good_llm(),
            "deepseek",
            Some(&disp),
        )
        .unwrap_err();
        assert_eq!(err.code(), "cost-cap-exceeded");
    }

    #[test]
    fn typod_oss_capability_still_hard_errors_with_dispatcher() {
        // An id in a reserved OSS namespace (document_intelligence.*) that doesn't match a built-in
        // is a typo → hard error EVEN with a dispatcher present (never degraded as a plugin agent).
        let yaml = SKILL_YAML.replace(
            "document_intelligence.compare_to_table",
            "document_intelligence.does_not_exist",
        );
        let skill = parse_skill_yaml(&yaml).unwrap();
        let inputs = json!({ "doc": "item-1", "entities": ["A", "B"] });
        let disp = StubDispatcher::ok(draft_envelope(), 1);
        let err = run_skill_with_dispatcher(
            &skill,
            &inputs,
            true,
            &resolver("x"),
            &good_llm(),
            "deepseek",
            Some(&disp),
        )
        .unwrap_err();
        assert_eq!(err.code(), "partial-failure");
        assert!(
            disp.calls.borrow().is_empty(),
            "typo'd OSS id must NOT hit the dispatcher"
        );
    }

    #[test]
    fn oss_agent_never_reaches_dispatcher() {
        // The compare-to-table OSS agent must run in-process even when a dispatcher is present
        // (the dispatcher must NOT be called for an OSS agent id).
        let skill = parse_skill_yaml(SKILL_YAML).unwrap();
        let inputs = json!({ "doc": "item-1", "entities": ["设备 A", "设备 B"] });
        let disp = StubDispatcher::err("should not be called");
        let res = run_skill_with_dispatcher(
            &skill,
            &inputs,
            true,
            &resolver("设备 A 1080p。设备 B 4K。"),
            &good_llm(),
            "deepseek",
            Some(&disp),
        )
        .unwrap();
        assert!(
            disp.calls.borrow().is_empty(),
            "OSS agent must not hit the dispatcher"
        );
        assert_eq!(res.format, ExportFormat::Xlsx);
    }
}
