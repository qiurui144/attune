use attune_core::retrieval_plan::{EvidenceNeed, RetrievalDiversity, RetrievalPlan};
use attune_core::search::SearchResult;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceNode {
    pub source_id: String,
    pub source_title: String,
    pub node_kind: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceDiagnostics {
    pub requested_needs: Vec<String>,
    pub satisfied_needs: Vec<String>,
    pub missing_needs: Vec<String>,
    pub sources_considered: usize,
    pub quality: String,
    pub quality_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidencePack {
    pub primary_source_id: String,
    pub source_title: String,
    pub nodes: Vec<EvidenceNode>,
    pub diagnostics: EvidenceDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RagModelDiscipline {
    Small,
    Balanced,
    Strong,
}

impl RagModelDiscipline {
    pub fn as_str(self) -> &'static str {
        match self {
            RagModelDiscipline::Small => "small",
            RagModelDiscipline::Balanced => "balanced",
            RagModelDiscipline::Strong => "strong",
        }
    }
}

pub fn assemble_evidence_pack(plan: &RetrievalPlan, results: &[SearchResult]) -> EvidencePack {
    assemble_evidence_pack_for_query("", plan, results)
}

pub fn assemble_evidence_pack_for_query(
    question: &str,
    plan: &RetrievalPlan,
    results: &[SearchResult],
) -> EvidencePack {
    let sources_considered = results
        .iter()
        .map(|result| result.item_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let query_terms = evidence_query_terms(question);
    let primary_source_id = select_primary_evidence_source(plan, results, &query_terms);
    let primary_source_id = primary_source_id.unwrap_or_default();
    let source_title = results
        .iter()
        .find(|result| result.item_id == primary_source_id)
        .map(|result| result.title.clone())
        .unwrap_or_default();

    let source_diverse = matches!(plan.diversity, RetrievalDiversity::PreferDiverseSources);
    let mut nodes = Vec::new();
    for result in results {
        if !source_diverse && result.item_id != primary_source_id {
            continue;
        }
        let text = result
            .inject_content
            .as_deref()
            .unwrap_or(&result.content)
            .to_string();
        let node_kind = evidence_node_kind(&text);
        if matches!(node_kind.as_str(), "Toc" | "HeaderFooter") {
            continue;
        }
        if !source_diverse
            && !query_terms.is_empty()
            && result.item_id != primary_source_id
            && evidence_query_overlap_score(&text, &query_terms) == 0
        {
            continue;
        }
        nodes.push(EvidenceNode {
            source_id: result.item_id.clone(),
            source_title: result.title.clone(),
            node_kind,
            text,
        });
    }

    let diagnostics = evidence_diagnostics(plan, &nodes, sources_considered);
    EvidencePack {
        primary_source_id,
        source_title,
        nodes,
        diagnostics,
    }
}

pub fn build_evidence_pack_prompt(question: &str, pack: &EvidencePack) -> String {
    build_evidence_pack_prompt_for_model(question, pack, &RagModelDiscipline::Small)
}

pub fn build_evidence_pack_prompt_for_model(
    question: &str,
    pack: &EvidencePack,
    discipline: &RagModelDiscipline,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("Question\n");
    prompt.push_str(question.trim());
    prompt.push_str("\n\nEvidence Pack\n");
    prompt.push_str(&format!(
        "Primary source: {} ({})\n",
        pack.source_title, pack.primary_source_id
    ));
    for (idx, node) in pack.nodes.iter().enumerate() {
        prompt.push_str(&format!(
            "[{}] source={} title={} kind={}\n{}\n",
            idx + 1,
            node.source_id,
            node.source_title,
            node.node_kind,
            node.text.trim()
        ));
    }
    prompt.push_str("\nEvidence Diagnostics\n");
    prompt.push_str(&format!(
        "- requested_needs: {}\n",
        join_or_none(&pack.diagnostics.requested_needs)
    ));
    prompt.push_str(&format!(
        "- satisfied_needs: {}\n",
        join_or_none(&pack.diagnostics.satisfied_needs)
    ));
    prompt.push_str(&format!(
        "- missing_needs: {}\n",
        join_or_none(&pack.diagnostics.missing_needs)
    ));
    prompt.push_str(&format!("- quality: {}\n", pack.diagnostics.quality));
    prompt.push_str(&format!(
        "- quality_reasons: {}\n",
        join_or_none(&pack.diagnostics.quality_reasons)
    ));
    prompt.push_str(&format!(
        "- sources_considered: {}\n",
        pack.diagnostics.sources_considered
    ));
    prompt.push_str("\nResponse Contract\n");
    prompt.push_str(&format!(
        "Evidence quality: {}. Model discipline: {}.\n",
        pack.diagnostics.quality,
        discipline.as_str()
    ));
    prompt.push_str(
        "Answer only from the Evidence Pack.\n\
         If the pack contains ProcedureStep nodes, present an ordered procedure.\n\
         If the pack contains ApiReference nodes, include exact API names, parameters, and return semantics shown in the evidence.\n\
         If the pack contains CommandBlock or ConfigBlock nodes, include the exact command/config lines shown in evidence.\n\
         If evidence is incomplete, say what is missing instead of guessing.\n\
         Do not use outside platform knowledge.\n\
         Do not synthesize across unrelated sources; preserve the primary source unless the evidence pack explicitly asks for comparison.\n\
         Adaptive model discipline: strong models may synthesize only after citing supporting nodes; Small/weak models should copy short evidence-backed facts, steps, symbols, paths, and values before adding minimal connective wording.\n",
    );
    match discipline {
        RagModelDiscipline::Small => prompt.push_str(
            "Small-model response rules: Use short bullet points. Copy exact evidence facts before explanation. Do not infer missing steps, parameters, causes, or platform details.\n",
        ),
        RagModelDiscipline::Balanced => prompt.push_str(
            "Balanced-model response rules: Briefly synthesize across cited nodes from the primary source, and call out any missing evidence before recommendations.\n",
        ),
        RagModelDiscipline::Strong => prompt.push_str(
            "Strong-model response rules: You may consolidate related cited facts, but every conclusion must remain traceable to listed evidence nodes.\n",
        ),
    }
    if pack.diagnostics.quality != "strong" {
        prompt.push_str(
            "Evidence-quality rule: State the evidence gap before giving a limited answer; do not upgrade partial or weak evidence into a confident procedure.\n",
        );
    }
    prompt
}

pub fn rag_model_discipline_from_runtime_profile(
    profile: Option<&attune_core::edge_cloud::ModelRuntimeProfile>,
) -> RagModelDiscipline {
    let Some(profile) = profile else {
        return RagModelDiscipline::Small;
    };
    if let Some(explicit) = discipline_from_quality_profile(&profile.quality_profile) {
        return explicit;
    }
    let context_cap = profile
        .async_context_cap()
        .max(profile.sync_context_cap())
        .max(profile.tested_async_input_tokens)
        .max(profile.tested_sync_input_tokens);
    let output_cap = profile
        .async_output_cap()
        .max(profile.sync_output_cap())
        .max(profile.recommended_output_tokens);
    if context_cap >= 8192 && output_cap >= 768 {
        RagModelDiscipline::Strong
    } else if context_cap >= 4096 && output_cap >= 256 {
        RagModelDiscipline::Balanced
    } else {
        RagModelDiscipline::Small
    }
}

fn discipline_from_quality_profile(value: &Value) -> Option<RagModelDiscipline> {
    match value {
        Value::String(s) => discipline_from_str(s),
        Value::Object(map) => map
            .get("model_discipline")
            .or_else(|| map.get("discipline"))
            .or_else(|| map.get("tier"))
            .or_else(|| map.get("reasoning"))
            .and_then(Value::as_str)
            .and_then(discipline_from_str),
        _ => None,
    }
}

fn discipline_from_str(value: &str) -> Option<RagModelDiscipline> {
    match value.trim().to_ascii_lowercase().as_str() {
        "small" | "weak" | "extractive" | "conservative" => Some(RagModelDiscipline::Small),
        "balanced" | "medium" | "standard" => Some(RagModelDiscipline::Balanced),
        "strong" | "large" | "reasoning" | "synthesis" => Some(RagModelDiscipline::Strong),
        _ => None,
    }
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

fn select_primary_evidence_source(
    plan: &RetrievalPlan,
    results: &[SearchResult],
    query_terms: &[String],
) -> Option<String> {
    let mut scores: HashMap<&str, f32> = HashMap::new();
    let has_query_overlap_candidate = !query_terms.is_empty()
        && results.iter().any(|result| {
            let text = result.inject_content.as_deref().unwrap_or(&result.content);
            evidence_query_overlap_score(text, query_terms) > 0
        });
    for (order, result) in results.iter().enumerate() {
        let text = result.inject_content.as_deref().unwrap_or(&result.content);
        let node_kind = evidence_node_kind(text);
        let overlap_score = evidence_query_overlap_score(text, query_terms);
        let mut score = result.score.max(0.0) + 1.0 / ((order + 1) as f32);
        for need in &plan.evidence_needs {
            if evidence_need_satisfied(*need, &node_kind, text) {
                score += 2.0;
            }
        }
        score += overlap_score as f32 * 0.35;
        if has_query_overlap_candidate && overlap_score == 0 {
            score *= 0.25;
        }
        *scores.entry(result.item_id.as_str()).or_insert(0.0) += score;
    }
    scores
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(source_id, _)| source_id.to_string())
}

fn evidence_diagnostics(
    plan: &RetrievalPlan,
    nodes: &[EvidenceNode],
    sources_considered: usize,
) -> EvidenceDiagnostics {
    let requested_needs = plan
        .evidence_needs
        .iter()
        .map(|need| evidence_need_label(*need).to_string())
        .collect::<Vec<_>>();
    let mut satisfied_needs = Vec::new();
    let mut missing_needs = Vec::new();
    for need in &plan.evidence_needs {
        let label = evidence_need_label(*need);
        if nodes
            .iter()
            .any(|node| evidence_need_satisfied(*need, &node.node_kind, &node.text))
        {
            satisfied_needs.push(label.to_string());
        } else {
            missing_needs.push(label.to_string());
        }
    }
    let (quality, quality_reasons) = evidence_quality(
        requested_needs.len(),
        satisfied_needs.len(),
        missing_needs.len(),
        nodes,
        sources_considered,
    );
    EvidenceDiagnostics {
        requested_needs,
        satisfied_needs,
        missing_needs,
        sources_considered,
        quality,
        quality_reasons,
    }
}

fn evidence_quality(
    requested_count: usize,
    satisfied_count: usize,
    missing_count: usize,
    nodes: &[EvidenceNode],
    sources_considered: usize,
) -> (String, Vec<String>) {
    let mut reasons = Vec::new();
    if nodes.is_empty() {
        return (
            "weak".to_string(),
            vec!["no_usable_evidence_nodes".to_string()],
        );
    }
    if missing_count == 0 && requested_count > 0 {
        reasons.push("all_requested_evidence_needs_satisfied".to_string());
    }
    if missing_count > 0 {
        reasons.push(format!("missing_evidence_needs={missing_count}"));
    }
    if sources_considered > 1 {
        let primary = nodes
            .first()
            .map(|node| node.source_id.as_str())
            .unwrap_or("");
        let primary_nodes = nodes
            .iter()
            .filter(|node| node.source_id.as_str() == primary)
            .count();
        if primary_nodes == nodes.len() {
            reasons.push("single_source_after_noise_filter".to_string());
        } else {
            reasons.push("multi_source_evidence".to_string());
        }
    }
    if nodes.len() >= 2 {
        reasons.push("multiple_evidence_nodes".to_string());
    }
    let quality = if requested_count > 0 && missing_count == 0 && nodes.len() >= 1 {
        "strong"
    } else if satisfied_count > 0 || (requested_count == 0 && !nodes.is_empty()) {
        "partial"
    } else {
        "weak"
    };
    if reasons.is_empty() {
        reasons.push(format!("usable_evidence_nodes={}", nodes.len()));
    }
    (quality.to_string(), reasons)
}

fn evidence_need_label(need: EvidenceNeed) -> &'static str {
    match need {
        EvidenceNeed::Definition => "Definition",
        EvidenceNeed::ApiReference => "ApiReference",
        EvidenceNeed::Procedure => "Procedure",
        EvidenceNeed::Command => "Command",
        EvidenceNeed::Config => "Config",
        EvidenceNeed::Troubleshooting => "Troubleshooting",
        EvidenceNeed::Comparison => "Comparison",
        EvidenceNeed::Summary => "Summary",
    }
}

fn evidence_need_satisfied_by_kind(need: EvidenceNeed, node_kind: &str) -> bool {
    match need {
        EvidenceNeed::Definition => matches!(node_kind, "Definition" | "Paragraph" | "Section"),
        EvidenceNeed::ApiReference => node_kind == "ApiReference",
        EvidenceNeed::Procedure => matches!(node_kind, "Procedure" | "ProcedureStep"),
        EvidenceNeed::Command => node_kind == "CommandBlock",
        EvidenceNeed::Config => node_kind == "ConfigBlock",
        EvidenceNeed::Troubleshooting => node_kind == "Troubleshooting",
        EvidenceNeed::Comparison => false,
        EvidenceNeed::Summary => matches!(node_kind, "Section" | "Paragraph"),
    }
}

fn evidence_need_satisfied(need: EvidenceNeed, node_kind: &str, text: &str) -> bool {
    if evidence_need_satisfied_by_kind(need, node_kind) {
        return true;
    }
    let text_l = text.to_ascii_lowercase();
    match need {
        EvidenceNeed::Troubleshooting => {
            matches!(
                node_kind,
                "Procedure" | "ProcedureStep" | "Paragraph" | "Section"
            ) && (contains_any_ascii(
                &text_l,
                &[
                    "check",
                    "verify",
                    "diagnose",
                    "troubleshoot",
                    "failure",
                    "failed",
                    "error",
                    "route",
                    "dns",
                    "port",
                    "log",
                    "packet loss",
                ],
            ) || [
                "检查", "验证", "诊断", "排查", "失败", "错误", "路由", "端口", "日志", "丢包",
            ]
            .iter()
            .any(|term| text.contains(term)))
        }
        EvidenceNeed::Procedure => {
            matches!(node_kind, "Paragraph" | "Section")
                && (contains_any_ascii(&text_l, &["step", "procedure"])
                    || ["步骤", "流程", "操作"]
                        .iter()
                        .any(|term| text.contains(term)))
        }
        EvidenceNeed::ApiReference => {
            matches!(node_kind, "Paragraph" | "Section")
                && (contains_any_ascii(&text_l, &["prototype", "signature", "parameter", "return"])
                    || ["原型", "接口", "参数", "返回"]
                        .iter()
                        .any(|term| text.contains(term)))
        }
        EvidenceNeed::Command => {
            contains_any_ascii(&text_l, &[" command", " run ", " shell", "$ "])
                || ["命令", "执行"].iter().any(|term| text.contains(term))
        }
        EvidenceNeed::Config => {
            contains_any_ascii(&text_l, &["config", "setting", ".conf", ".json", ".yaml"])
                || ["配置", "设置"].iter().any(|term| text.contains(term))
        }
        EvidenceNeed::Definition | EvidenceNeed::Comparison | EvidenceNeed::Summary => false,
    }
}

fn evidence_query_terms(question: &str) -> Vec<String> {
    let mut terms = HashSet::new();
    let lower = question.to_ascii_lowercase();
    let mut ascii = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/' | '.') {
            ascii.push(ch);
        } else if !ascii.is_empty() {
            if ascii.chars().count() >= 2 && !generic_query_stopword(&ascii) {
                terms.insert(ascii.clone());
            }
            ascii.clear();
        }
    }
    if ascii.chars().count() >= 2 && !generic_query_stopword(&ascii) {
        terms.insert(ascii);
    }

    let mut cjk_run = String::new();
    for ch in question.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            cjk_run.push(ch);
        } else if !cjk_run.is_empty() {
            push_cjk_terms(&cjk_run, &mut terms);
            cjk_run.clear();
        }
    }
    if !cjk_run.is_empty() {
        push_cjk_terms(&cjk_run, &mut terms);
    }

    let mut out = terms
        .into_iter()
        .filter(|term| term.chars().count() >= 2)
        .collect::<Vec<_>>();
    out.sort_by_key(|term| std::cmp::Reverse(term.chars().count()));
    out.truncate(64);
    out
}

fn push_cjk_terms(run: &str, terms: &mut HashSet<String>) {
    let chars = run.chars().collect::<Vec<_>>();
    if chars.len() < 2 {
        return;
    }
    if !generic_query_stopword(run) && chars.len() <= 8 {
        terms.insert(run.to_string());
    }
    for window in 2..=4 {
        if chars.len() < window {
            continue;
        }
        for slice in chars.windows(window) {
            let term = slice.iter().collect::<String>();
            if !generic_query_stopword(&term) {
                terms.insert(term);
            }
        }
    }
}

fn generic_query_stopword(term: &str) -> bool {
    matches!(
        term,
        "how"
            | "what"
            | "why"
            | "the"
            | "and"
            | "or"
            | "with"
            | "when"
            | "where"
            | "should"
            | "please"
            | "如何"
            | "怎么"
            | "怎样"
            | "应该"
            | "哪些"
            | "什么"
            | "时候"
            | "一个"
            | "这个"
            | "那个"
            | "继续"
            | "基于"
    )
}

fn evidence_query_overlap_score(text: &str, query_terms: &[String]) -> usize {
    if query_terms.is_empty() || text.trim().is_empty() {
        return 0;
    }
    let text_l = text.to_ascii_lowercase();
    let mut score = 0usize;
    for term in query_terms {
        let hit = if term.is_ascii() {
            text_l.contains(&term.to_ascii_lowercase())
        } else {
            text.contains(term)
        };
        if !hit {
            continue;
        }
        score += if term.chars().count() >= 4 { 2 } else { 1 };
    }
    score
}

fn evidence_node_kind(text: &str) -> String {
    let Some(first_line) = text.lines().next().map(str::trim) else {
        return "Paragraph".to_string();
    };
    let Some(rest) = first_line.strip_prefix("[kind:") else {
        return "Paragraph".to_string();
    };
    rest.trim()
        .trim_end_matches(']')
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
}

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

fn compact_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

fn evidence_text_from_knowledge(k: &Value) -> &str {
    k.get("inject_content")
        .and_then(|v| v.as_str())
        .or_else(|| k.get("content").and_then(|v| v.as_str()))
        .unwrap_or("")
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

fn local_scheduler_api_signature_query(query: &str) -> bool {
    contains_any_ascii(
        query,
        &[
            "api",
            "interface",
            "prototype",
            "signature",
            "接口",
            "函数",
            "原型",
            "函数接口",
            "调用接口",
        ],
    )
}

fn local_scheduler_focused_excerpt_query(query: &str) -> bool {
    let query_l = query.to_ascii_lowercase();
    let interface_query = contains_any_ascii(
        query,
        &[
            "what",
            "api",
            "interface",
            "接口",
            "函数",
            "调用接口",
            "函数接口",
        ],
    );
    let local_hal_howto_query = contains_any_ascii(&query_l, &["rtos", "hal"])
        && contains_any_ascii(
            query,
            &[
                "怎么",
                "如何",
                "哪些",
                "哪个",
                "配置",
                "申请",
                "启动",
                "设置",
                "初始化",
                "读写",
                "收发",
                "查询",
                "查找",
                "type",
                "id",
            ],
        );
    interface_query || local_hal_howto_query
}

fn focused_query_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let query_l = query.to_ascii_lowercase();
    let mut current = String::new();
    for ch in query.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/' | '.' | '+' | '#') {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            if current.len() >= 2 {
                terms.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if current.len() >= 2 {
        terms.push(current);
    }

    for term in [
        "交叉编译",
        "工具链",
        "编译",
        "镜像",
        "配置",
        "申请",
        "启动",
        "传输",
        "通道",
        "接口",
        "函数",
        "流程",
        "时间",
        "闹钟",
        "输出",
        "生成",
        "进入",
        "缓存",
        "内存",
        "高电平",
        "输出",
        "读写",
        "收发",
    ] {
        if query.contains(term) {
            terms.push(term.to_string());
        }
    }

    if contains_any_ascii(
        &query_l,
        &["sdk", "build", "compile", "package", "toolchain", "image"],
    ) || query.contains("编译")
        || query.contains("打包")
        || query.contains("环境")
        || query.contains("工具链")
        || query.contains("镜像")
    {
        for term in [
            "build.sh",
            "envsetup",
            "lunch",
            "pack",
            "toolchain",
            "output/image",
            ".img",
            "make",
            "apt-get",
        ] {
            terms.push(term.to_string());
        }
    }
    if query.contains("时间") || query.contains("闹钟") || contains_any_ascii(&query_l, &["rtc"])
    {
        for term in ["rtc_time", "tm_", "alarm", "hal_rtc"] {
            terms.push(term.to_string());
        }
    }
    if query.contains("时钟") || contains_any_ascii(&query_l, &["clock", "clk", "ccu"]) {
        for term in ["clk_type", "clk_id", "hal_clock_get", "ccmu"] {
            terms.push(term.to_string());
        }
    }

    terms.sort();
    terms.dedup();
    terms
}

fn focused_excerpt_score(window: &str, terms: &[String]) -> usize {
    let compact = compact_whitespace(window).to_ascii_lowercase();
    let mut score = 0usize;
    for term in terms {
        if compact.contains(term) {
            score += if term
                .chars()
                .any(|c| matches!(c, '_' | '-' | '/' | '.' | '+'))
            {
                4
            } else if term.len() >= 6 {
                3
            } else {
                2
            };
        }
    }
    score += focused_excerpt_technical_signal_count(window) * 8;
    score = score.saturating_sub(focused_excerpt_front_matter_penalty(window));
    score
}

fn focused_excerpt(text: &str, terms: &[String], max_chars: usize) -> Option<String> {
    let compact = compact_whitespace(text);
    if compact.is_empty() {
        return None;
    }
    if compact.chars().count() <= max_chars {
        return Some(compact);
    }

    let haystack = compact.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    for term in terms {
        let mut search_from = 0usize;
        while let Some(rel) = haystack.get(search_from..).and_then(|s| s.find(term)) {
            let pos = search_from + rel;
            let desired_start = pos.saturating_sub(max_chars / 3);
            let start = floor_char_boundary(&compact, desired_start);
            let end = ceil_char_boundary(&compact, (start + max_chars).min(compact.len()));
            if let Some(window) = compact.get(start..end) {
                let score = focused_excerpt_score(window, terms);
                if best.map(|(_, s)| score > s).unwrap_or(true) {
                    best = Some((start, score));
                }
            }
            search_from = pos.saturating_add(term.len()).min(haystack.len());
            if search_from >= haystack.len() {
                break;
            }
        }
    }

    let (start, score) = best?;
    let end = ceil_char_boundary(&compact, (start + max_chars).min(compact.len()));
    let excerpt = compact.get(start..end)?;
    if score == 0 || focused_excerpt_technical_signal_count(excerpt) == 0 {
        return None;
    }
    Some({
        let s = excerpt;
        let mut out = s.to_string();
        if end < compact.len() {
            out.push_str("...");
        }
        out
    })
}

fn build_local_scheduler_focused_excerpt_answer(
    query: &str,
    knowledge: &[Value],
) -> Option<String> {
    if !local_scheduler_focused_excerpt_query(query) {
        return None;
    }
    let terms = focused_query_terms(query);
    if terms.is_empty() {
        return None;
    }

    let mut excerpts = Vec::new();
    for k in knowledge.iter().take(3) {
        let title = source_title_from_knowledge(k);
        let text = evidence_text_from_knowledge(k);
        let Some(excerpt) = focused_excerpt(text, &terms, 1400) else {
            continue;
        };
        excerpts.push((title, excerpt));
    }
    if excerpts.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    lines.push("根据引用文档，可先按以下证据操作或核对：".to_string());
    for (title, excerpt) in excerpts {
        lines.push(format!("- 来源：《{title}》"));
        lines.push(format!("  - 相关摘录：{excerpt}"));
    }
    lines.push("建议再到上述来源的对应章节核对完整原文。".to_string());
    let answer = lines.join("\n");
    if focused_excerpt_strong_signal_count(&answer) < 2 {
        return None;
    }
    Some(answer)
}

fn focused_excerpt_strong_signal_count(text: &str) -> usize {
    focused_excerpt_technical_signal_count(text)
}

fn focused_excerpt_technical_signal_count(text: &str) -> usize {
    text.split(|c: char| c.is_whitespace() || matches!(c, '，' | '。' | '；' | '：' | ',' | ';'))
        .filter(|token| {
            let token =
                token.trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | '(' | ')' | '[' | ']'));
            token.contains('_')
                || token.starts_with("./")
                || token.contains("/")
                || token.ends_with(".sh")
                || token.ends_with(".img")
                || token.ends_with(".mk")
                || token.ends_with(".c")
                || token.ends_with(".h")
                || token.starts_with("CONFIG_")
                || token.starts_with("RK_")
                || token.starts_with("hal_")
        })
        .take(8)
        .count()
}

fn focused_excerpt_front_matter_penalty(text: &str) -> usize {
    [
        "版本历史",
        "修订记录",
        "目录",
        "免责声明",
        "版权",
        "文档密级",
    ]
    .iter()
    .map(|marker| text.matches(marker).count().min(4) * 6)
    .sum()
}

#[derive(Debug, Clone)]
struct ApiSignatureEvidence {
    title: String,
    signature: String,
    purpose: Option<String>,
    params: Option<String>,
    returns: Option<String>,
    score: usize,
    order: usize,
}

fn take_until_any<'a>(text: &'a str, markers: &[&str]) -> &'a str {
    let mut end = text.len();
    for marker in markers {
        if let Some(pos) = text.find(marker) {
            end = end.min(pos);
        }
    }
    &text[..end]
}

fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn trim_signature_prefix(text: &str) -> &str {
    text.trim_start_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ':' | '：' | '-' | '‐' | '‑' | '–' | '—' | '•' | '*' | '·'
            )
    })
}

fn valid_c_like_signature(signature: &str) -> bool {
    let signature = signature.trim();
    if signature.chars().count() < 8 || signature.chars().count() > 260 {
        return false;
    }
    let Some(open) = signature.find('(') else {
        return false;
    };
    let Some(close) = signature.rfind(')') else {
        return false;
    };
    if close <= open {
        return false;
    }
    let before_open = signature[..open].trim();
    let Some(name) = before_open
        .split_whitespace()
        .last()
        .map(|s| s.trim_matches('*'))
    else {
        return false;
    };
    if name.is_empty()
        || matches!(
            name,
            "if" | "for" | "while" | "switch" | "return" | "sizeof" | "define"
        )
    {
        return false;
    }
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.chars().any(|c| c.is_ascii_alphabetic())
}

fn compact_signature(signature: &str) -> Option<String> {
    let mut signature = compact_whitespace(signature);
    signature = signature
        .trim()
        .trim_end_matches(';')
        .trim_end_matches(',')
        .trim()
        .to_string();
    if valid_c_like_signature(&signature) {
        Some(signature)
    } else {
        None
    }
}

fn field_after_marker(section: &str, markers: &[&str], end_markers: &[&str]) -> Option<String> {
    for marker in markers {
        if let Some(pos) = section.find(marker) {
            let start = pos + marker.len();
            let rest = trim_signature_prefix(&section[start..]);
            let value = compact_whitespace(take_until_any(rest, end_markers));
            let value = value.trim();
            if value.chars().count() >= 2 {
                return Some(value.chars().take(220).collect());
            }
        }
    }
    None
}

fn signature_from_prototype_section(section: &str) -> Option<String> {
    let markers = [
        "原 型",
        "原型",
        "prototype",
        "Prototype",
        "signature",
        "Signature",
    ];
    for marker in markers {
        if let Some(pos) = section.find(marker) {
            let rest = trim_signature_prefix(&section[pos + marker.len()..]);
            let raw = take_until_any(
                rest,
                &[
                    " • 作用",
                    " 作用：",
                    " 作用:",
                    " • 参数",
                    " 参数：",
                    " 参数:",
                    " • 返回",
                    " 返回：",
                    " 返回:",
                    " 版权所有",
                ],
            );
            if let Some(signature) = compact_signature(raw) {
                return Some(signature);
            }
        }
    }
    None
}

fn query_relevance_score(query: &str, section: &str, signature: &str) -> usize {
    let query_l = query.to_ascii_lowercase();
    let section_l = section.to_ascii_lowercase();
    let signature_l = signature.to_ascii_lowercase();
    let mut score = 1usize;
    for token in query
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .map(str::trim)
        .filter(|token| token.len() >= 3)
    {
        let token_l = token.to_ascii_lowercase();
        if signature_l.contains(&token_l) {
            score += 4;
        } else if section_l.contains(&token_l) || query_l.contains(&token_l) {
            score += 1;
        }
    }
    for term in [
        "申请",
        "释放",
        "配置",
        "初始化",
        "启动",
        "停止",
        "查询",
        "获取",
        "注册",
        "通道",
        "回调",
        "状态",
        "内存",
        "描述符",
        "传输",
    ] {
        if query.contains(term) && section.contains(term) {
            score += 2;
        }
    }
    score
}

fn query_focused_api_action_count(query: &str) -> usize {
    [
        "申请",
        "释放",
        "配置",
        "初始化",
        "启动",
        "停止",
        "查询",
        "获取",
        "注册",
        "分配",
        "打开",
        "关闭",
        "request",
        "free",
        "config",
        "init",
        "start",
        "stop",
        "query",
        "get",
        "alloc",
    ]
    .iter()
    .filter(|term| query.contains(**term))
    .count()
}

fn extract_api_signature_evidence(
    query: &str,
    title: &str,
    evidence: &str,
    start_order: usize,
) -> Vec<ApiSignatureEvidence> {
    let text = compact_whitespace(evidence);
    if text.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for marker in [
        "原 型",
        "原型",
        "prototype",
        "Prototype",
        "signature",
        "Signature",
    ] {
        let mut search_from = 0usize;
        while let Some(rel) = text[search_from..].find(marker) {
            let marker_pos = search_from + rel;
            let section_start = floor_char_boundary(&text, marker_pos);
            let section_end = ceil_char_boundary(&text, marker_pos + 900);
            let section = &text[section_start..section_end];
            if let Some(signature) = signature_from_prototype_section(section) {
                let purpose = field_after_marker(
                    section,
                    &["作用：", "作用:", "Description:", "description:"],
                    &[
                        " • 参数",
                        " 参数：",
                        " 参数:",
                        " • 返回",
                        " 返回：",
                        " 返回:",
                        " 版权所有",
                    ],
                );
                let params = field_after_marker(
                    section,
                    &["参数：", "参数:", "Parameters:", "parameters:"],
                    &[" • 返回", " 返回：", " 返回:", " 版权所有"],
                );
                let returns = field_after_marker(
                    section,
                    &["返回：", "返回:", "Returns:", "returns:"],
                    &[" 版权所有", " 文档密级", " 3.", " 4."],
                );
                out.push(ApiSignatureEvidence {
                    title: title.to_string(),
                    score: query_relevance_score(query, section, &signature),
                    order: start_order + out.len(),
                    signature,
                    purpose,
                    params,
                    returns,
                });
            }
            search_from = marker_pos + marker.len();
        }
    }

    let mut seen = std::collections::HashSet::new();
    out.retain(|item| seen.insert(compact_ascii_lower(&item.signature)));
    out
}

fn build_local_scheduler_api_signature_answer(query: &str, knowledge: &[Value]) -> Option<String> {
    if !local_scheduler_api_signature_query(query) {
        return None;
    }

    let mut evidence = Vec::new();
    for (idx, k) in knowledge.iter().take(4).enumerate() {
        let title = source_title_from_knowledge(k);
        let text = evidence_text_from_knowledge(k);
        evidence.extend(extract_api_signature_evidence(
            query,
            &title,
            text,
            idx * 100,
        ));
    }
    if evidence.is_empty() {
        return None;
    }

    evidence.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.order.cmp(&b.order)));
    let action_count = query_focused_api_action_count(query);
    if action_count == 1 {
        evidence.truncate(1);
    } else if action_count > 1 {
        let best_score = evidence.first().map(|item| item.score).unwrap_or(0);
        evidence.retain(|item| item.score == best_score);
    }
    evidence.truncate(2);

    let mut lines = Vec::new();
    lines.push("根据引用文档，相关函数接口如下：".to_string());
    for item in &evidence {
        lines.push(format!("- 来源：《{}》", item.title));
        lines.push(format!("  - 原型：`{}`", item.signature));
        if let Some(purpose) = &item.purpose {
            lines.push(format!("  - 作用：{purpose}"));
        }
        if let Some(params) = &item.params {
            lines.push(format!("  - 参数：{params}"));
        }
        if let Some(returns) = &item.returns {
            lines.push(format!("  - 返回：{returns}"));
        }
    }
    lines.push("原文位置：可在上述引用来源的接口说明章节核对完整原文。".to_string());
    Some(lines.join("\n"))
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

    if let Some(answer) = build_local_scheduler_api_signature_answer(query, knowledge) {
        return Some(answer);
    }
    if let Some(answer) = build_local_scheduler_focused_excerpt_answer(query, knowledge) {
        return Some(answer);
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
    ) && contains_any_ascii(
        &q,
        &["能否直接", "直接判定", "合规", "determine", "compliant"],
    );

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extractive_answer_returns_api_signature_from_rtos_style_docs() {
        let evidence = "3.1 hal_dma_chan_status_t hal_dma_chan_request\n\
            • 原 型：hal_dma_chan_status_t\n\
            hal_dma_chan_request(struct\n\
            sunxi_dma_chan\n\
            **dma_chan)\n\
            • 作用：申请 DMA 通道\n\
            • 参数：\n\
            − dma_chan: 存放 DMA 通道的指针变量\n\
            • 返回：\n\
            − HAL_DMA_CHAN_STATUS_BUSY: 申请失败\n\
            − HAL_DMA_CHAN_STATUS_FREE: 申请成功\n\
            3.10 hal_dma_status_t hal_dma_stop\n\
            • 原型：hal_dma_status_t hal_dma_stop(struct sunxi_dma_chan *chan)\n\
            • 作用：停止 DMA 传输\n\
            • 参数：chan:DMA 通道结构体指针变量";
        let answer = build_local_scheduler_extractive_answer(
            "rtos dmac 申请 dma 通道 函数接口",
            &[json!({
                "title": "RTOS_DMAC_开发指南 - RTOS DMAC",
                "content": evidence,
            })],
        )
        .expect("API signature query should be answered from prototype evidence");

        assert!(
            answer.contains(
                "`hal_dma_chan_status_t hal_dma_chan_request(struct sunxi_dma_chan **dma_chan)`"
            ),
            "{answer}"
        );
        assert!(answer.contains("作用：申请 DMA 通道"), "{answer}");
        assert!(answer.contains("HAL_DMA_CHAN_STATUS_FREE"), "{answer}");
        assert!(!answer.contains("hal_dma_stop"), "{answer}");
        assert!(answer.contains("接口说明章节"), "{answer}");
    }

    #[test]
    fn extractive_answer_handles_multibyte_window_boundaries() {
        let evidence = format!(
            "{}• 原 型：hal_dma_chan_status_t hal_dma_chan_request(struct sunxi_dma_chan **dma_chan) \
             • 作用：申请 DMA 通道 • 参数：dma_chan: 存放 DMA 通道的指针变量 • 返回：HAL_DMA_CHAN_STATUS_FREE: 申请成功",
            "初始版本 ".repeat(900)
        );
        let answer = build_local_scheduler_extractive_answer(
            "rtos dmac 申请 dma 通道 函数接口",
            &[json!({
                "title": "RTOS_DMAC_开发指南 - RTOS DMAC",
                "content": evidence,
            })],
        )
        .expect("multibyte text windowing must not panic");

        assert!(
            answer.contains("hal_dma_chan_request(struct sunxi_dma_chan **dma_chan)"),
            "{answer}"
        );
    }

    #[test]
    fn focused_excerpt_prefers_action_interfaces_over_front_matter() {
        let evidence = format!(
            "RTOS GPIO 开发指南\n版本历史 初始版本\n目录 1 前言 2 接口说明 3 功能开发\n{}\n\
             4.1 功能概述 GPIO 驱动提供引脚配置、读取状态、设置高电平和低电平等功能。\n\
             4.2.2 配置 GPIO 为输出模式，并设置输出电平\n\
             步骤 1 调用 hal_gpio_pinmux_set_function 设置 GPIO_MUXSEL_OUT。\n\
             步骤 2 调用 hal_gpio_set_direction 设置 GPIO_DIRECTION_OUTPUT。\n\
             步骤 3 调用 hal_gpio_set_pull 设置内部上下拉。\n\
             步骤 4 调用 hal_gpio_set_driving_level 设置驱动能力。\n\
             步骤 5 调用 hal_gpio_set_data 设置 GPIO_DATA_HIGH。",
            "目录 GPIO 高电平 输出 ".repeat(50)
        );
        let answer = build_local_scheduler_extractive_answer(
            "V821 RTOS GPIO 配成输出高电平，应该调用哪些接口？",
            &[json!({
                "title": "RTOS_GPIO_开发指南 - RTOS GPIO",
                "content": evidence,
            })],
        )
        .expect("technical action evidence should be extracted");

        assert!(answer.contains("hal_gpio_pinmux_set_function"), "{answer}");
        assert!(answer.contains("hal_gpio_set_direction"), "{answer}");
        assert!(answer.contains("hal_gpio_set_data"), "{answer}");
    }

    #[test]
    fn extractive_answer_keeps_open_ended_non_lookup_queries_out() {
        assert!(build_local_scheduler_extractive_answer(
            "这个模块有什么设计问题？",
            &[json!({
                "title": "RTOS_DMAC_开发指南",
                "content": "• 原型：hal_dma_status_t hal_dma_init(void) • 作用：初始化 DMA 控制器驱动",
            })],
        )
        .is_none());
    }
}
