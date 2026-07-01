//! Plugin-agent dispatch — the bridge that lets a skill chain a **pro plugin's own agent**
//! (law `legal_drafter`, patent `oa_response`, tech debt, medical de-id, …) as a step, not just
//! the three OSS in-process agents.
//!
//! ## Why a trait
//! `skill_runtime::runner` lives in `attune-core` and must stay testable without a live plugin
//! install / subprocess. So the runner depends on a small [`AgentDispatcher`] capability instead
//! of `agent_runner::run_agent_subprocess` directly: the server wires a subprocess-backed impl
//! (which enforces the install + entitlement + timeout + LLM-env boundary), and tests pass an
//! in-memory stub. An OSS agent-id never reaches the dispatcher — the runner handles those
//! in-process first; the dispatcher is only the route for *plugin* agent-ids.
//!
//! ## What a plugin agent returns
//! Every attune plugin agent emits the `AgentOutput<T>` envelope on stdout
//! (`{ computation, audit_trail, red_lines_violated, missing_evidence, followups, confidence }`),
//! where `computation` is the agent's typed product. A *deliverable* agent's `computation` is a
//! document-shaped object: a `draft` full text + `sections: [{heading, body}]` (+ `disclaimer`,
//! `unresolved`/`citations` provenance). [`AgentDocOutput`] is the **lowest-common-denominator**
//! view of that envelope — it extracts the renderable section list + disclaimer + the unresolved/
//! red-line markers so a `render → export` chain yields a downloadable file regardless of which
//! vertical produced it. A non-document agent (e.g. a pure calculator) simply yields no sections;
//! the skill degrades to a single paragraph carrying the raw computation so the export still runs.

use crate::export::{Artifact, Block, Document};
use serde_json::Value;

/// The marker appended to a section/line a plugin agent flagged as needing human confirmation
/// (`unresolved` of kind needs-lawyer-judgment / missing-citation, or any red line). Mirrors
/// [`crate::skill_runtime::doc_render::UNVERIFIED_MARKER`] so the downloaded artifact surfaces the
/// "do not ship as-is" signal rather than hiding it.
pub const NEEDS_CONFIRM_MARKER: &str = "［待确认］";

/// Dispatch a single **plugin** agent by id, feeding it `input` (JSON object) and returning its
/// raw stdout-parsed JSON (the `AgentOutput<T>` envelope). The implementation owns the security
/// boundary: it MUST verify the agent id belongs to an installed (and, when signing is enforced,
/// trust-allowed) plugin's declared agents, run it with a timeout + resource bound, and forward
/// only the LLM env (never the full parent env). Returns an error string on any failure
/// (unknown agent / not installed / timeout / non-zero exit / I/O); the runner turns that into a
/// fail-soft warning so the skill still produces a (degraded) artifact.
pub trait AgentDispatcher {
    /// Invoke `agent_id` with `input`; on success return the parsed stdout JSON envelope.
    fn dispatch(&self, agent_id: &str, input: &Value) -> Result<DispatchOutput, String>;
}

/// The result of a successful plugin-agent dispatch: the parsed JSON envelope plus the token
/// usage the agent reported (for the skill bill / cost cap).
#[derive(Debug, Clone)]
pub struct DispatchOutput {
    /// The full `AgentOutput<T>` JSON the agent printed on stdout.
    pub envelope: Value,
    /// LLM tokens the agent reported it spent (`cost_used.llm_tokens` if present, else 0). Pro
    /// deliverable agents are 💰 LLM — this feeds the skill token bill + the [`MAX_TOTAL_TOKENS`]
    /// cap so a runaway plugin call still aborts. (Best-effort: a plugin that omits it bills 0,
    /// which is conservative for the *cap* but means the bill under-counts — acceptable for v1.)
    ///
    /// [`MAX_TOTAL_TOKENS`]: crate::skill_runtime::cost::MAX_TOTAL_TOKENS
    pub llm_tokens: u32,
}

/// A renderable view of a plugin agent's document-shaped `computation`. Built from the
/// `AgentOutput` envelope by [`parse_agent_doc`]. Vertical-agnostic: any agent whose computation
/// carries `sections` (or a `draft`/`content`/`text` full body) renders to a [`Document`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentDocOutput {
    /// Section list (heading + body). Empty for a non-document agent.
    pub sections: Vec<(String, String)>,
    /// Indices into `sections` whose body could not be fully grounded / needs human confirm.
    pub needs_confirm_idx: Vec<usize>,
    /// Whole-document fallback body when the agent gave a `draft`/`content`/`text` blob but no
    /// structured sections (so the export still has content).
    pub fallback_body: String,
    /// A mandatory disclaimer (e.g. legal / medical) appended as a trailing block when present.
    pub disclaimer: String,
    /// Red lines the agent flagged (e.g. hallucinated citation). Surfaced as a leading warning
    /// block so a problematic draft is never silently shipped as clean.
    pub red_lines: Vec<String>,
}

/// Extract an [`AgentDocOutput`] from an `AgentOutput<T>` JSON envelope (the dispatcher's return).
///
/// The mapping is deliberately permissive across verticals:
/// - `computation.sections[]` → `(heading, body)` pairs (camelCase `heading`/`body`, the shared
///   law/patent/academic deliverable shape). A `required: false` section with empty body is kept
///   (the export still lists it) — emptiness is the agent's call, not ours.
/// - `computation.unresolved[]` (where + kind) → mark the matching section as needs-confirm; a
///   section whose heading appears in an unresolved `where` gets the marker.
/// - `computation.disclaimer` → trailing disclaimer block.
/// - top-level `red_lines_violated[]` → leading red-line warning block.
/// - if no `sections`, fall back to `computation.draft | content | text` as one body.
pub fn parse_agent_doc(envelope: &Value) -> AgentDocOutput {
    let comp = envelope.get("computation").unwrap_or(&Value::Null);
    let mut out = AgentDocOutput::default();

    // Sections (the document deliverable shape).
    if let Some(arr) = comp.get("sections").and_then(|v| v.as_array()) {
        for s in arr {
            let heading = s
                .get("heading")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let body = s
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.sections.push((heading, body));
        }
    }

    // Unresolved markers → flag the matching section (by heading substring in `where`).
    if let Some(arr) = comp.get("unresolved").and_then(|v| v.as_array()) {
        for u in arr {
            let where_ = u.get("where").and_then(|v| v.as_str()).unwrap_or("");
            if where_.is_empty() {
                continue;
            }
            for (i, (heading, _)) in out.sections.iter().enumerate() {
                if !heading.is_empty()
                    && where_.contains(heading.as_str())
                    && !out.needs_confirm_idx.contains(&i)
                {
                    out.needs_confirm_idx.push(i);
                }
            }
        }
    }

    // Disclaimer (mandatory for legal / medical deliverables).
    out.disclaimer = comp
        .get("disclaimer")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Red lines (top-level envelope field).
    if let Some(arr) = envelope
        .get("red_lines_violated")
        .and_then(|v| v.as_array())
    {
        out.red_lines = arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    // Fallback when there were no structured sections: try a whole-document text field first
    // (covers the deliverable agents whose computation is a single body string under different
    // names — law `draft`, medical `redacted_text`, patent `response_text`, tech `report`/etc),
    // else derive readable sections from the computation's structured fields so a structured
    // deliverable (patent OA grounds / tech postmortem) still renders a meaningful document.
    if out.sections.is_empty() {
        const TEXT_KEYS: [&str; 8] = [
            "draft",
            "content",
            "text",
            "redacted_text",
            "response_text",
            "markdown",
            "report",
            "body",
        ];
        for key in TEXT_KEYS {
            if let Some(s) = comp.get(key).and_then(|v| v.as_str()) {
                if !s.trim().is_empty() {
                    out.fallback_body = s.to_string();
                    break;
                }
            }
        }
        if out.fallback_body.is_empty() {
            out.sections = derive_sections_from_structured(comp);
        }
    }

    out
}

/// Last-resort renderer for a structured computation with no `sections`/text body: turn each
/// top-level field into a `(heading, body)` section. A scalar becomes a one-line body; an array
/// of strings becomes a bullet-ish joined body; an array of objects renders each object's string
/// fields. Internal/noise keys (`schema_version`, `cost_used`, …) are skipped. This keeps a
/// vertical-agnostic deliverable readable (patent OA grounds, tech postmortem chain) without the
/// runner knowing each agent's schema — the agent's *own* deliverable shape is still preferred
/// (sections / text body) when present.
fn derive_sections_from_structured(comp: &Value) -> Vec<(String, String)> {
    const SKIP_KEYS: [&str; 4] = ["schema_version", "cost_used", "doc_type", "docType"];
    let Some(obj) = comp.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, val) in obj {
        if SKIP_KEYS.contains(&key.as_str()) {
            continue;
        }
        let body = match val {
            Value::String(s) if !s.trim().is_empty() => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Array(arr) if !arr.is_empty() => arr
                .iter()
                .map(stringify_item)
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };
        if !body.trim().is_empty() {
            out.push((key.clone(), body));
        }
    }
    out
}

/// Render one array element to a readable line: a string passes through; an object joins its
/// string/scalar fields as `k: v`; anything else is JSON-stringified compactly.
fn stringify_item(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(m) => m
            .iter()
            .filter_map(|(k, val)| match val {
                Value::String(s) if !s.trim().is_empty() => Some(format!("{k}: {s}")),
                Value::Number(n) => Some(format!("{k}: {n}")),
                Value::Bool(b) => Some(format!("{k}: {b}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("；"),
        other => other.to_string(),
    }
}

/// Render an [`AgentDocOutput`] into a [`Document`] [`Artifact`] (zero-cost). Red lines become a
/// leading warning, sections become heading+paragraph blocks (needs-confirm bodies marked), the
/// fallback body is a single paragraph when there were no sections, and the disclaimer trails.
pub fn agent_doc_to_document(doc: &AgentDocOutput, title: &str) -> Artifact {
    let mut blocks = Vec::new();

    if !doc.red_lines.is_empty() {
        blocks.push(Block::Heading {
            level: 2,
            text: format!("{NEEDS_CONFIRM_MARKER} 红线提示"),
        });
        blocks.push(Block::List {
            ordered: false,
            items: doc.red_lines.clone(),
        });
    }

    for (i, (heading, body)) in doc.sections.iter().enumerate() {
        if !heading.trim().is_empty() {
            blocks.push(Block::Heading {
                level: 2,
                text: heading.trim().to_string(),
            });
        }
        let mut body = body.trim().to_string();
        if doc.needs_confirm_idx.contains(&i) && !body.is_empty() {
            body.push_str(NEEDS_CONFIRM_MARKER);
        }
        for para in body.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
            blocks.push(Block::Paragraph {
                text: para.to_string(),
            });
        }
    }

    if doc.sections.is_empty() && !doc.fallback_body.trim().is_empty() {
        for para in doc
            .fallback_body
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            blocks.push(Block::Paragraph {
                text: para.to_string(),
            });
        }
    }

    if !doc.disclaimer.trim().is_empty() {
        blocks.push(Block::Heading {
            level: 2,
            text: "免责声明".to_string(),
        });
        blocks.push(Block::Paragraph {
            text: doc.disclaimer.trim().to_string(),
        });
    }

    Artifact::document(Document {
        title: Some(title.to_string()),
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `DraftResult`-shaped envelope (law `legal_drafter`) → 2 sections + disclaimer.
    #[test]
    fn parse_draft_result_envelope() {
        let env = json!({
            "computation": {
                "docType": "complaint",
                "draft": "全文……",
                "sections": [
                    { "heading": "诉讼请求", "body": "请求判令被告偿还借款本金 10 万元。", "required": true },
                    { "heading": "事实与理由", "body": "原告与被告于 2024 年签订借款合同……", "required": true }
                ],
                "citations": [],
                "unresolved": [],
                "disclaimer": "本文书为 AI 辅助初稿，须经执业律师审核后使用。"
            },
            "red_lines_violated": [],
            "missing_evidence": [],
            "followups": []
        });
        let doc = parse_agent_doc(&env);
        assert_eq!(doc.sections.len(), 2);
        assert_eq!(doc.sections[0].0, "诉讼请求");
        assert!(doc.disclaimer.contains("执业律师"));
        assert!(doc.needs_confirm_idx.is_empty());
        assert!(doc.red_lines.is_empty());
    }

    #[test]
    fn unresolved_where_marks_matching_section() {
        let env = json!({
            "computation": {
                "sections": [
                    { "heading": "诉讼请求", "body": "判令被告支付违约金。" },
                    { "heading": "法律依据", "body": "依据合同法相关规定。" }
                ],
                "unresolved": [
                    { "kind": "needs-lawyer-judgment", "where": "法律依据", "hint": "请律师确认适用法条" }
                ]
            }
        });
        let doc = parse_agent_doc(&env);
        assert_eq!(doc.needs_confirm_idx, vec![1], "法律依据 section flagged");
    }

    #[test]
    fn red_lines_surface_from_envelope() {
        let env = json!({
            "computation": { "sections": [] },
            "red_lines_violated": ["no_hallucinated_citation"]
        });
        let doc = parse_agent_doc(&env);
        assert_eq!(doc.red_lines, vec!["no_hallucinated_citation"]);
    }

    #[test]
    fn fallback_body_when_no_sections() {
        let env = json!({ "computation": { "draft": "整篇没有分段的初稿正文。" } });
        let doc = parse_agent_doc(&env);
        assert!(doc.sections.is_empty());
        assert_eq!(doc.fallback_body, "整篇没有分段的初稿正文。");
    }

    #[test]
    fn render_sections_with_disclaimer_and_redline() {
        let doc = AgentDocOutput {
            sections: vec![
                ("诉讼请求".into(), "请求判令偿还。".into()),
                ("依据".into(), "合同条款。".into()),
            ],
            needs_confirm_idx: vec![1],
            fallback_body: String::new(),
            disclaimer: "须律师审核。".into(),
            red_lines: vec!["no_hallucinated_citation".into()],
        };
        let Artifact::Document(d) = agent_doc_to_document(&doc, "起诉状") else {
            panic!()
        };
        assert_eq!(d.title.as_deref(), Some("起诉状"));
        // leading red-line warning + heading + para ×2 (one marked) + disclaimer heading + para.
        let has_redline = d.blocks.iter().any(|b| matches!(b, Block::List { items, .. } if items.iter().any(|i| i.contains("no_hallucinated_citation"))));
        assert!(has_redline);
        let marked = d.blocks.iter().any(|b| matches!(b, Block::Paragraph { text } if text.contains("合同条款") && text.contains(NEEDS_CONFIRM_MARKER)));
        assert!(marked, "needs-confirm section body must carry the marker");
        let has_disclaimer = d
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Paragraph { text } if text.contains("须律师审核")));
        assert!(has_disclaimer);
        assert!(d.validate().is_ok());
    }

    #[test]
    fn render_fallback_body_yields_paragraph() {
        let doc = AgentDocOutput {
            fallback_body: "无分段正文。".into(),
            ..Default::default()
        };
        let Artifact::Document(d) = agent_doc_to_document(&doc, "文书") else {
            panic!()
        };
        assert!(d
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Paragraph { text } if text == "无分段正文。")));
    }

    #[test]
    fn medical_redacted_text_used_as_body() {
        // medical deidentify computation = { spans, redacted_text } → redacted_text is the body.
        let env =
            json!({ "computation": { "spans": [], "redacted_text": "患者[name]因[date]就诊……" } });
        let doc = parse_agent_doc(&env);
        assert!(doc.sections.is_empty());
        assert_eq!(doc.fallback_body, "患者[name]因[date]就诊……");
    }

    #[test]
    fn structured_computation_derives_sections() {
        // patent OA: computation = { grounds: [{article, ground, response_path}] } (no sections/text)
        // → a section per top-level field, array-of-objects joined readably.
        let env = json!({
            "computation": {
                "schema_version": "1.0",
                "grounds": [
                    { "article": "22.3", "ground": "创造性不足", "response_path": "陈述意见" }
                ]
            }
        });
        let doc = parse_agent_doc(&env);
        assert!(doc.fallback_body.is_empty());
        assert_eq!(doc.sections.len(), 1);
        assert_eq!(doc.sections[0].0, "grounds");
        assert!(doc.sections[0].1.contains("22.3"));
        assert!(doc.sections[0].1.contains("创造性不足"));
        // schema_version is skipped (noise key).
        assert!(!doc.sections.iter().any(|(h, _)| h == "schema_version"));
    }

    #[test]
    fn structured_postmortem_derives_summary_and_chain() {
        let env = json!({
            "computation": {
                "schema_version": "1.0",
                "summary": "服务在高峰期不可用 30 分钟。",
                "root_cause_chain": [
                    { "why": "连接池耗尽", "process_level": false },
                    { "why": "无背压保护", "process_level": true }
                ],
                "action_items": ["加连接池上限告警"]
            }
        });
        let doc = parse_agent_doc(&env);
        let headings: Vec<&str> = doc.sections.iter().map(|(h, _)| h.as_str()).collect();
        assert!(headings.contains(&"summary"));
        assert!(headings.contains(&"root_cause_chain"));
        assert!(headings.contains(&"action_items"));
        let chain = doc
            .sections
            .iter()
            .find(|(h, _)| h == "root_cause_chain")
            .unwrap();
        assert!(chain.1.contains("连接池耗尽"));
    }

    #[test]
    fn missing_computation_is_empty_not_panic() {
        let doc = parse_agent_doc(&json!({}));
        assert!(doc.sections.is_empty());
        assert!(doc.fallback_body.is_empty());
        // an empty doc still renders to a valid (header-only) document.
        let Artifact::Document(d) = agent_doc_to_document(&doc, "空") else {
            panic!()
        };
        assert!(d.validate().is_ok());
    }
}
