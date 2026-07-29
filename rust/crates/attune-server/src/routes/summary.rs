use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;

#[derive(Debug, Deserialize, Default)]
pub struct SummaryWorkflowRequest {
    #[serde(default)]
    pub scenario: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub top_k: Option<usize>,
}

#[derive(Debug, Clone)]
struct SummaryDocument {
    id: String,
    title: String,
    source_type: String,
    content: String,
}

fn normalized_scenario(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "folder" | "collection" | "corpus" => "folder",
        "compare" | "comparison" => "compare",
        "risk" | "risks" | "gap" | "gaps" => "risk",
        _ => "recent",
    }
}

fn scenario_label(scenario: &str) -> &'static str {
    match normalized_scenario(scenario) {
        "folder" => "Folder Summary",
        "compare" => "Cross-document Compare",
        "risk" => "Risk and Gap Review",
        _ => "Recent Document Summary",
    }
}

fn split_terms(text: &str) -> Vec<String> {
    text.split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-' || ch as u32 > 0x7f))
        .map(str::trim)
        .filter(|term| term.chars().count() >= 2)
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

fn doc_score(doc: &SummaryDocument, terms: &[String]) -> usize {
    if terms.is_empty() {
        return 1;
    }
    let haystack = format!("{} {}", doc.title, doc.content).to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.trim().chars().take(max_chars) {
        out.push(ch);
    }
    if text.trim().chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn first_nonempty_lines(text: &str, limit: usize) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(limit)
        .map(|line| truncate_chars(line, 220))
        .collect()
}

fn line_term_score(line: &str, terms: &[String]) -> usize {
    if terms.is_empty() {
        return usize::from(!line.trim().is_empty());
    }
    let lower = line.to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| lower.contains(term.as_str()))
        .count()
}

fn evidence_for_doc(doc: &SummaryDocument, terms: &[String]) -> String {
    let mut best_line = "";
    let mut best_score = 0usize;
    for line in doc
        .content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let score = line_term_score(line, terms);
        if score > best_score {
            best_line = line;
            best_score = score;
        }
    }
    if best_score > 0 {
        return truncate_chars(best_line, 260);
    }
    first_nonempty_lines(&doc.content, 1)
        .into_iter()
        .next()
        .unwrap_or_else(|| truncate_chars(&doc.title, 120))
}

fn load_summary_documents(state: &SharedState, limit: usize) -> AppResult<Vec<SummaryDocument>> {
    let vault = state
        .vault
        .lock()
        .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
    let dek = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let summaries = vault
        .store()
        .list_items(limit.min(50), 0)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut docs = Vec::new();
    for summary in summaries {
        match vault.store().get_item(&dek, &summary.id) {
            Ok(Some(item)) if !item.content.trim().is_empty() => docs.push(SummaryDocument {
                id: item.id,
                title: item.title,
                source_type: item.source_type,
                content: item.content,
            }),
            Ok(_) => {}
            Err(err) => return Err(AppError::Internal(err.to_string())),
        }
    }
    Ok(docs)
}

fn build_summary_workflow_response(
    request: &SummaryWorkflowRequest,
    docs: &[SummaryDocument],
) -> serde_json::Value {
    let scenario = normalized_scenario(&request.scenario);
    let terms = split_terms(&request.detail);
    let top_k = request.top_k.unwrap_or(6).clamp(1, 12);
    let mut ranked = docs.to_vec();
    ranked.sort_by(|a, b| {
        doc_score(b, &terms)
            .cmp(&doc_score(a, &terms))
            .then_with(|| b.id.cmp(&a.id))
    });
    ranked.truncate(top_k);

    let scope = if ranked.is_empty() {
        "No unlocked knowledge-base documents were available for this workflow.".to_string()
    } else {
        format!(
            "{} document(s) selected from the unlocked knowledge base for {}.",
            ranked.len(),
            scenario_label(scenario)
        )
    };
    let focus = if request.detail.trim().is_empty() {
        "Focus was inferred from the most recent indexed documents.".to_string()
    } else {
        format!("User focus: {}", truncate_chars(&request.detail, 180))
    };

    let core_conclusions = if ranked.is_empty() {
        vec![
            "The workflow needs indexed documents before it can produce grounded conclusions."
                .to_string(),
        ]
    } else {
        ranked
            .iter()
            .take(4)
            .map(|doc| {
                let lead = if terms.is_empty() {
                    first_nonempty_lines(&doc.content, 1)
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| doc.title.clone())
                } else {
                    evidence_for_doc(doc, &terms)
                };
                format!("{}: {}", doc.title, lead)
            })
            .collect()
    };
    let key_evidence = ranked
        .iter()
        .take(8)
        .map(|doc| {
            json!({
                "item_id": doc.id,
                "title": doc.title,
                "source_type": doc.source_type,
                "snippet": evidence_for_doc(doc, &terms),
            })
        })
        .collect::<Vec<_>>();
    let document_summaries = ranked
        .iter()
        .take(8)
        .map(|doc| {
            let mut points = first_nonempty_lines(&doc.content, 3);
            if !terms.is_empty() {
                let evidence = evidence_for_doc(doc, &terms);
                if !points.iter().any(|point| point == &evidence) {
                    points.insert(0, evidence);
                }
                points.truncate(3);
            }
            json!({
                "item_id": doc.id,
                "title": doc.title,
                "source_type": doc.source_type,
                "key_points": points,
                "evidence": evidence_for_doc(doc, &terms),
            })
        })
        .collect::<Vec<_>>();
    let risks_or_gaps = match scenario {
        "compare" if ranked.len() < 2 => vec![
            "Comparison workflows need at least two relevant documents; current evidence is insufficient.".to_string(),
        ],
        "risk" => vec![
            "Review unresolved assumptions, missing version context, and source coverage before treating this as final.".to_string(),
            "Use the citations below to inspect original wording for high-impact decisions.".to_string(),
        ],
        _ if ranked.is_empty() => vec![
            "No citations are available, so the result should not be used as a grounded summary.".to_string(),
        ],
        _ => vec![
            "Coverage is limited to the selected local documents and may omit unindexed files.".to_string(),
        ],
    };
    let next_actions = match scenario {
        "folder" => vec![
            "Check whether the folder import included every expected file type.".to_string(),
            "Open cited documents with weak snippets and rescan if content is missing.".to_string(),
        ],
        "compare" => vec![
            "Inspect citations side by side and confirm which source is authoritative.".to_string(),
            "Ask a narrower follow-up when the compared documents use different terminology."
                .to_string(),
        ],
        "risk" => vec![
            "Turn each risk into an explicit verification question against the knowledge base."
                .to_string(),
            "Add missing source documents before relying on the summary for delivery decisions."
                .to_string(),
        ],
        _ => vec![
            "Ask a follow-up question against one cited document when more precision is needed."
                .to_string(),
        ],
    };

    let sections = json!({
        "scope": [scope, focus],
        "core_conclusions": core_conclusions,
        "key_evidence": key_evidence,
        "risks_or_gaps": risks_or_gaps,
        "next_actions": next_actions,
    });
    let mut content = String::new();
    content.push_str(&format!("{}\n\n", scenario_label(scenario)));
    content.push_str("Scope\n");
    for line in sections["scope"].as_array().into_iter().flatten() {
        content.push_str("- ");
        content.push_str(line.as_str().unwrap_or_default());
        content.push('\n');
    }
    content.push_str("\nCore Conclusions\n");
    for line in sections["core_conclusions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        content.push_str("- ");
        content.push_str(line.as_str().unwrap_or_default());
        content.push('\n');
    }
    content.push_str("\nRisks or Gaps\n");
    for line in sections["risks_or_gaps"].as_array().into_iter().flatten() {
        content.push_str("- ");
        content.push_str(line.as_str().unwrap_or_default());
        content.push('\n');
    }
    content.push_str("\nNext Actions\n");
    for line in sections["next_actions"].as_array().into_iter().flatten() {
        content.push_str("- ");
        content.push_str(line.as_str().unwrap_or_default());
        content.push('\n');
    }
    content.push_str("\nWorkflow Stages\n");
    for stage in ["select", "map", "synthesize", "audit"] {
        content.push_str("- ");
        content.push_str(stage);
        content.push('\n');
    }

    let stages = json!([
        {
            "name": "select",
            "status": "completed",
            "input": {
                "scenario": scenario,
                "detail_terms": terms,
                "top_k": top_k,
            },
            "output": {
                "document_count": ranked.len(),
                "selected_item_ids": ranked.iter().map(|doc| doc.id.clone()).collect::<Vec<_>>(),
            }
        },
        {
            "name": "map",
            "status": "completed",
            "input": {
                "document_count": ranked.len(),
            },
            "output": {
                "document_summaries": document_summaries,
            }
        },
        {
            "name": "synthesize",
            "status": "completed",
            "input": {
                "scenario": scenario,
                "document_count": ranked.len(),
            },
            "output": {
                "sections": sections,
            }
        },
        {
            "name": "audit",
            "status": "completed",
            "input": {
                "citation_count": key_evidence.len(),
                "section_count": sections.as_object().map(|value| value.len()).unwrap_or(0),
            },
            "output": {
                "coverage_status": if ranked.is_empty() { "no-evidence" } else { "grounded-local-documents" },
                "missing_sections": [],
            }
        }
    ]);
    let audit = json!({
        "citation_count": key_evidence.len(),
        "document_count": ranked.len(),
        "section_count": sections.as_object().map(|value| value.len()).unwrap_or(0),
        "coverage_status": if ranked.is_empty() { "no-evidence" } else { "grounded-local-documents" },
        "missing_sections": [],
    });

    json!({
        "summary_workflow": {
            "strategy": "multi_stage_extractive",
            "scenario": scenario,
            "document_count": ranked.len(),
            "top_k": top_k,
            "stages": stages,
            "audit": audit,
        },
        "scenario": scenario,
        "scenario_label": scenario_label(scenario),
        "model": request.model.as_deref().unwrap_or("attune-summary-workflow"),
        "content": content.trim(),
        "summary_sections": sections,
        "document_summaries": document_summaries,
        "workflow_stages": stages,
        "audit": audit,
        "citations": key_evidence,
        "knowledge_count": ranked.len(),
    })
}

/// POST /api/v1/summary/workflow — knowledge-base summary workflow for web-demo.
pub async fn workflow(
    State(state): State<SharedState>,
    Json(body): Json<SummaryWorkflowRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if body.detail.chars().count() > 4096 {
        return Err(AppError::PayloadTooLarge("summary detail too long".into()));
    }
    if let Some(model) = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        crate::routes::ai_stack::require_model_capability_ready(&state, model, "summary").await?;
    }
    let docs = load_summary_documents(&state, body.top_k.unwrap_or(12).max(12))?;
    Ok(Json(build_summary_workflow_response(&body, &docs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_builds_structured_sections_without_a_model() {
        let request = SummaryWorkflowRequest {
            scenario: "risk".to_string(),
            detail: "clock configuration".to_string(),
            model: Some("candidate-model".to_string()),
            top_k: Some(2),
        };
        let docs = vec![SummaryDocument {
            id: "doc-1".to_string(),
            title: "Developer Guide".to_string(),
            source_type: "markdown".to_string(),
            content: "Clock configuration requires checking the board-level source.\nSecond line."
                .to_string(),
        }];

        let payload = build_summary_workflow_response(&request, &docs);
        assert_eq!(payload["scenario"], "risk");
        assert_eq!(payload["model"], "candidate-model");
        assert!(payload["summary_sections"]["core_conclusions"].is_array());
        assert_eq!(
            payload["summary_workflow"]["strategy"],
            "multi_stage_extractive"
        );
        assert!(payload["summary_workflow"]["stages"].is_array());
        assert!(payload["document_summaries"].is_array());
        assert_eq!(
            payload["audit"]["coverage_status"],
            "grounded-local-documents"
        );
        assert!(payload["content"]
            .as_str()
            .unwrap()
            .contains("Risk and Gap Review"));
    }

    #[test]
    fn workflow_prefers_query_relevant_evidence_over_marker_lines() {
        let request = SummaryWorkflowRequest {
            scenario: "folder".to_string(),
            detail: "TCP/IP airplane".to_string(),
            model: None,
            top_k: Some(1),
        };
        let docs = vec![SummaryDocument {
            id: "doc-1".to_string(),
            title: "Fixture".to_string(),
            source_type: "markdown".to_string(),
            content: "Marker: TOKEN.\nTCP/IP originated from ARPANET.\nAirplane mechanical design requires fatigue review.".to_string(),
        }];

        let payload = build_summary_workflow_response(&request, &docs);
        let text = payload["content"].as_str().unwrap();
        assert!(text.contains("TCP/IP"));
        assert!(!text.contains("Marker: TOKEN"));
    }

    #[test]
    fn compare_scenario_surfaces_insufficient_evidence() {
        let request = SummaryWorkflowRequest {
            scenario: "compare".to_string(),
            detail: String::new(),
            model: None,
            top_k: Some(4),
        };
        let payload = build_summary_workflow_response(&request, &[]);
        let risks = payload["summary_sections"]["risks_or_gaps"]
            .as_array()
            .unwrap();
        assert!(risks
            .iter()
            .any(|item| item.as_str().unwrap_or_default().contains("at least two")));
    }

    #[test]
    fn workflow_reports_all_multi_stage_steps() {
        let request = SummaryWorkflowRequest {
            scenario: "recent".to_string(),
            detail: "DMA channel".to_string(),
            model: None,
            top_k: Some(1),
        };
        let docs = vec![SummaryDocument {
            id: "doc-1".to_string(),
            title: "RTOS DMAC Guide".to_string(),
            source_type: "pdf".to_string(),
            content: "DMA channel allocation uses the RTOS HAL interface.".to_string(),
        }];
        let payload = build_summary_workflow_response(&request, &docs);
        let stages = payload["summary_workflow"]["stages"].as_array().unwrap();
        let names = stages
            .iter()
            .filter_map(|stage| stage["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["select", "map", "synthesize", "audit"]);
    }
}
