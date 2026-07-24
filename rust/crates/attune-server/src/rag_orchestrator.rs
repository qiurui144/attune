use serde_json::Value;

fn local_scheduler_extractive_answer_enabled() -> bool {
    crate::local_scheduler::env_bool_any(
        &[
            "ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER",
            "ATTUNE_LOCAL_EXTRACTIVE_ANSWER",
        ],
        true,
    )
}

fn compact_ascii_lower(text: &str) -> String {
    text.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

pub fn contains_any_ascii(text: &str, needles: &[&str]) -> bool {
    let haystack = text.to_ascii_lowercase();
    needles.iter().any(|needle| haystack.contains(needle))
}

pub fn local_scheduler_operational_safety_query(query: &str) -> bool {
    let q = query.to_ascii_lowercase();
    let operational = contains_any_ascii(
        &q,
        &[
            "real flight",
            "emergency steps",
            "engine fire",
            "flight emergency",
            "operational",
            "maintenance signoff",
            "维修步骤",
            "真实飞行",
            "应急步骤",
        ],
    );
    let urgent = contains_any_ascii(
        &q,
        &["now", "immediately", "exact steps", "step by step", "马上"],
    );
    operational || (urgent && contains_any_ascii(&q, &["qrh", "emergency", "fire", "飞行", "应急"]))
}

pub fn local_scheduler_source_lookup_query(query: &str) -> bool {
    contains_any_ascii(
        query,
        &[
            "source",
            "manual",
            "reference",
            "lookup",
            "description",
            "origin",
            "originated",
            "system",
            "systems",
            "fcom",
            "qrh",
            "fctm",
            "amm",
            "ata",
            "sop",
            "standard operating",
            "mel",
            "abbreviation",
            "abbreviations",
            "hydraulic",
            "electrical",
            "fuel",
            "navigation",
            "powerplant",
            "landing gear",
            "flight controls",
            "air conditioning",
            "minimum equipment",
            "起源",
            "来自哪里",
        ],
    )
}

fn source_title_from_knowledge(k: &Value) -> String {
    k.get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| k.get("item_id").and_then(|v| v.as_str()).map(str::trim))
        .unwrap_or("local KB source")
        .to_string()
}

fn snippet_from_knowledge(k: &Value, max_chars: usize) -> String {
    let text = k
        .get("inject_content")
        .and_then(|v| v.as_str())
        .or_else(|| k.get("content").and_then(|v| v.as_str()))
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut snippet: String = text.chars().take(max_chars).collect();
    snippet.push_str("...");
    snippet
}

fn local_scheduler_source_lines(knowledge: &[Value], limit: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for k in knowledge {
        let title = source_title_from_knowledge(k);
        let key = compact_ascii_lower(&title);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        let snippet = snippet_from_knowledge(k, 180);
        if snippet.is_empty() {
            lines.push(format!("- {title}"));
        } else {
            lines.push(format!("- {title}: {snippet}"));
        }
        if lines.len() >= limit {
            break;
        }
    }
    lines
}

pub fn build_local_scheduler_safety_refusal(query: &str, knowledge: &[Value]) -> Option<String> {
    if knowledge.is_empty() || !local_scheduler_operational_safety_query(query) {
        return None;
    }

    let source_lines = local_scheduler_source_lines(knowledge, 5);
    if source_lines.is_empty() {
        return None;
    }

    Some(format!(
        "I cannot provide exact real-flight or maintenance emergency procedure steps. Do not use this response for operational flight decisions; consult the official QRH/manual and qualified crew or maintenance personnel.\n\nRelevant local KB sources for citation only:\n{}",
        source_lines.join("\n")
    ))
}

pub fn build_local_scheduler_extractive_answer(query: &str, knowledge: &[Value]) -> Option<String> {
    if knowledge.is_empty() || !local_scheduler_extractive_answer_enabled() {
        return None;
    }

    let source_lines = local_scheduler_source_lines(knowledge, 5);
    if source_lines.is_empty() {
        return None;
    }

    if !local_scheduler_source_lookup_query(query) {
        return None;
    }

    Some(format!(
        "根据本地知识库检索，优先使用以下已引用来源回答该问题。若需要复杂推理或跨文档综合，应切换到 scheduler answer worker 或云端高质量模式。\n\n{}",
        source_lines.join("\n")
    ))
}

pub fn local_scheduler_out_of_manual_boundary_query(query: &str) -> bool {
    let q = query.to_ascii_lowercase();
    let explicit_boundary = contains_any_ascii(
        &q,
        &[
            "未直接覆盖",
            "没有直接覆盖",
            "手册没有",
            "资料没有",
            "out-of-manual",
            "not directly covered",
            "not covered",
            "industry-general",
            "行业通用",
        ],
    );
    let kb_missing_practice = contains_any_ascii(&q, &["知识库没有", "knowledge base does not"])
        && contains_any_ascii(
            &q,
            &[
                "直接覆盖",
                "做法",
                "实践",
                "practice",
                "method",
                "整改",
                "建议",
                "segmentation",
                "zero trust",
                "零信任",
            ],
        );
    let missing_evidence_question = contains_any_ascii(
        &q,
        &[
            "审计日志",
            "audit log",
            "audit evidence",
            "日志",
            "记录",
            "证据",
        ],
    ) && contains_any_ascii(&q, &["能否直接", "直接判定", "合规", "determine", "compliant"]);

    (explicit_boundary || kb_missing_practice) && !missing_evidence_question
}

pub fn build_local_scheduler_out_of_manual_boundary(
    query: &str,
    knowledge: &[Value],
) -> Option<String> {
    if knowledge.is_empty()
        || !local_scheduler_extractive_answer_enabled()
        || !local_scheduler_out_of_manual_boundary_query(query)
    {
        return None;
    }

    let source_lines = local_scheduler_source_lines(knowledge, 5);
    if source_lines.is_empty() {
        return None;
    }

    Some(format!(
        "知识库未直接覆盖该做法，因此不能当作手册结论或来源结论。可以作为行业通用建议讨论，但必须明确证据不足，并继续索取或收集缺失的日志、记录、审批、配置、测量或其他材料。\n\n已检索到的相关知识库来源：\n{}",
        source_lines.join("\n")
    ))
}

pub fn local_scheduler_summary_query(query: &str) -> bool {
    contains_any_ascii(
        query,
        &[
            "summary",
            "summarize",
            "overview",
            "recap",
            "总结",
            "概括",
            "摘要",
            "核心结论",
            "关键证据",
        ],
    )
}

pub fn build_local_scheduler_extractive_summary(
    query: &str,
    knowledge: &[Value],
) -> Option<String> {
    if knowledge.is_empty()
        || !local_scheduler_extractive_answer_enabled()
        || !local_scheduler_summary_query(query)
    {
        return None;
    }

    let source_lines = local_scheduler_source_lines(knowledge, 5);
    if source_lines.is_empty() {
        return None;
    }

    Some(format!(
        "根据本地知识库检索，摘要如下：\n\n1. 核心结论：{}\n2. 关键证据：\n{}\n3. 风险或待办：当前摘要只基于以上已引用知识库片段；如需更完整综合，请继续上传相关文档。",
        source_lines.first().map(|line| line.trim_start_matches("- ")).unwrap_or("知识库片段提供了相关事实。"),
        source_lines.join("\n")
    ))
}
