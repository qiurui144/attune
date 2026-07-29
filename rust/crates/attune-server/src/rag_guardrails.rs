use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RagIntent {
    Lookup,
    Diagnostic,
    Summary,
    Comparison,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvidenceCoverage {
    pub intent: RagIntent,
    pub score: f32,
    pub status: &'static str,
    pub found_evidence: bool,
    pub missing: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RagRuntimePolicy {
    pub profile_id: String,
    pub retrieval_strategy: String,
    pub top_k: usize,
    pub recovery_top_k: usize,
    pub min_citations: usize,
    pub refuse_without_evidence: bool,
    pub allow_extractive_repair: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RagRetrievalTrace {
    pub profile_id: String,
    pub strategy: String,
    pub passes: Vec<&'static str>,
    pub queries: Vec<String>,
    pub final_top_k: usize,
    pub vector_results: Option<usize>,
    pub bm25_results: Option<usize>,
    pub citations_required: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RagWorkflowStageTrace {
    pub stage: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RagWorkflowStageTrace {
    pub fn completed(stage: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            status: "completed".to_string(),
            reason: None,
        }
    }

    pub fn blocked(stage: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            status: "blocked".to_string(),
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RagWorkflowTrace {
    pub mode: String,
    pub stages: Vec<RagWorkflowStageTrace>,
    pub clarification_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_layer: Option<String>,
    pub repair_attempted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RagClarification {
    pub reason: &'static str,
    pub scopes: Vec<String>,
    pub message: String,
}

impl EvidenceCoverage {
    pub fn sufficient(&self) -> bool {
        self.status == "sufficient"
    }
}

pub fn classify_rag_intent(query: &str) -> RagIntent {
    let q = query.to_ascii_lowercase();
    if contains_any(
        &q,
        &["总结", "概括", "摘要", "summary", "summarize", "overview"],
    ) {
        return RagIntent::Summary;
    }
    if contains_any(
        &q,
        &["对比", "区别", "差异", "compare", "contrast", "difference"],
    ) {
        return RagIntent::Comparison;
    }
    if contains_any(
        &q,
        &[
            "定位资料",
            "定位手册",
            "定位章节",
            "查找资料",
            "查找手册",
            "引用来源",
            "引用资料",
            "source lookup",
            "manual lookup",
            "locate source",
            "locate manual",
        ],
    ) {
        return RagIntent::Lookup;
    }
    if contains_any(
        &q,
        &[
            "排查",
            "定位",
            "故障",
            "诊断",
            "连接失败",
            "访问不了",
            "不通",
            "配置",
            "部署",
            "修复",
            "解决",
            "处理",
            "操作步骤",
            "配置步骤",
            "实践步骤",
            "troubleshoot",
            "diagnose",
            "debug",
            "failure",
            "root cause",
            "how to fix",
            "how to configure",
            "how do i configure",
            "steps to",
            "procedure",
            "runbook",
        ],
    ) {
        return RagIntent::Diagnostic;
    }
    if contains_any(&q, &["如何", "怎么", "怎样", "how to", "what steps"])
        && contains_any(
            &q,
            &[
                "配置",
                "部署",
                "使用",
                "操作",
                "安装",
                "修复",
                "解决",
                "处理",
                "configure",
                "deploy",
                "install",
                "operate",
                "use",
            ],
        )
    {
        return RagIntent::Diagnostic;
    }
    RagIntent::Lookup
}

pub fn evaluate_evidence_coverage(query: &str, knowledge: &[Value]) -> EvidenceCoverage {
    let intent = classify_rag_intent(query);
    let found_evidence = !knowledge.is_empty();
    if !found_evidence {
        return EvidenceCoverage {
            intent,
            score: 0.0,
            status: "insufficient",
            found_evidence,
            missing: vec!["knowledge_evidence"],
        };
    }

    let evidence = knowledge
        .iter()
        .map(knowledge_text)
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let score = match intent {
        RagIntent::Diagnostic => diagnostic_coverage_score(&evidence),
        RagIntent::Summary => summary_coverage_score(knowledge, &evidence),
        RagIntent::Comparison => comparison_coverage_score(knowledge, &evidence),
        RagIntent::Lookup => lookup_coverage_score(&evidence),
    };
    let sufficient = score >= coverage_threshold(intent);
    let mut missing = Vec::new();
    if !sufficient {
        missing.push(match intent {
            RagIntent::Diagnostic => "diagnostic_evidence",
            RagIntent::Summary => "summary_evidence",
            RagIntent::Comparison => "comparison_evidence",
            RagIntent::Lookup => "direct_evidence",
        });
    }

    EvidenceCoverage {
        intent,
        score,
        status: if sufficient {
            "sufficient"
        } else {
            "insufficient"
        },
        found_evidence,
        missing,
    }
}

pub fn rag_profile_for_intent(intent: RagIntent) -> &'static str {
    match intent {
        RagIntent::Diagnostic => "default_kb_diagnostic",
        RagIntent::Summary => "default_kb_summary",
        RagIntent::Comparison | RagIntent::Lookup => "default_kb_chat",
    }
}

pub fn runtime_policy_for_intent(
    intent: RagIntent,
    registry: &attune_core::plugin_registry::PluginRegistry,
) -> RagRuntimePolicy {
    runtime_policy_for_intent_from_profiles(
        intent,
        registry
            .list_rag_profiles()
            .into_iter()
            .map(|(_, profile)| profile),
    )
}

pub fn runtime_policy_for_intent_from_profiles<'a, I>(
    intent: RagIntent,
    profiles: I,
) -> RagRuntimePolicy
where
    I: IntoIterator<Item = &'a attune_core::plugin_loader::RagProfileSpec>,
{
    let default = default_runtime_policy(intent);
    let expected_id = rag_profile_for_intent(intent);
    if let Some(profile) = profiles.into_iter().find(|profile| {
        profile.id == expected_id
            || profile
                .intents
                .iter()
                .any(|profile_intent| profile_intent == intent_profile_key(intent))
    }) {
        return policy_from_profile(default, profile);
    }
    default
}

#[allow(dead_code)] // Unit-tested metadata helper; integration-test builds compile without cfg(test).
pub fn rag_metadata(
    coverage: &EvidenceCoverage,
    answer_mode: &'static str,
    degraded_reason: Option<&'static str>,
    knowledge_count: usize,
) -> Value {
    rag_metadata_with_retrieval(
        coverage,
        answer_mode,
        degraded_reason,
        knowledge_count,
        &["first_pass"],
        &[],
    )
}

#[allow(dead_code)] // Unit-tested metadata helper; integration-test builds compile without cfg(test).
pub fn rag_metadata_with_retrieval(
    coverage: &EvidenceCoverage,
    answer_mode: &'static str,
    degraded_reason: Option<&'static str>,
    knowledge_count: usize,
    passes: &[&'static str],
    queries: &[String],
) -> Value {
    let trace = RagRetrievalTrace {
        profile_id: rag_profile_for_intent(coverage.intent).to_string(),
        strategy: "hybrid_rrf".to_string(),
        passes: passes.to_vec(),
        queries: queries.to_vec(),
        final_top_k: knowledge_count,
        vector_results: None,
        bm25_results: None,
        citations_required: 1,
    };
    rag_metadata_with_trace(
        coverage,
        answer_mode,
        degraded_reason,
        knowledge_count,
        &trace,
    )
}

pub fn rag_metadata_with_trace(
    coverage: &EvidenceCoverage,
    answer_mode: &'static str,
    degraded_reason: Option<&'static str>,
    knowledge_count: usize,
    trace: &RagRetrievalTrace,
) -> Value {
    serde_json::json!({
        "rag_profile": trace.profile_id,
        "intent": coverage.intent,
        "answer_mode": answer_mode,
        "degraded": degraded_reason.is_some(),
        "degraded_reason": degraded_reason,
        "retrieval": {
            "strategy": trace.strategy,
            "passes": trace.passes,
            "queries": trace.queries,
            "final_top_k": trace.final_top_k,
            "vector_results": trace.vector_results,
            "bm25_results": trace.bm25_results,
            "stage_counts_available": trace.vector_results.is_some() || trace.bm25_results.is_some(),
            "citations_required": trace.citations_required,
            "coverage_score": coverage.score,
            "coverage_status": coverage.status,
            "found_evidence": coverage.found_evidence,
            "missing": coverage.missing,
        },
        "citations_count": knowledge_count,
        "knowledge_count": knowledge_count,
    })
}

#[allow(dead_code)] // Unit-tested workflow metadata helper; route code may attach richer traces directly.
pub fn rag_metadata_with_workflow(
    coverage: &EvidenceCoverage,
    answer_mode: &'static str,
    degraded_reason: Option<&'static str>,
    knowledge_count: usize,
    workflow: &RagWorkflowTrace,
) -> Value {
    let mut meta = rag_metadata(coverage, answer_mode, degraded_reason, knowledge_count);
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("rag_workflow".to_string(), serde_json::json!(workflow));
    }
    meta
}

pub fn workflow_clarification_for_query(
    query: &str,
    knowledge: &[Value],
    profile: &attune_core::plugin_loader::RagProfileSpec,
) -> Option<RagClarification> {
    let clarification = &profile.workflow.clarification;
    if clarification.enabled == Some(false) {
        return None;
    }
    if clarification.require_when_multiple_scopes == Some(false) {
        return None;
    }
    if clarification.scope_terms.is_empty() {
        return None;
    }

    let query_l = query.to_ascii_lowercase();
    let mut scopes = Vec::new();
    for term in &clarification.scope_terms {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let term_l = term.to_ascii_lowercase();
        if query_l.contains(&term_l) {
            return None;
        }
        if knowledge.iter().any(|item| knowledge_matches_scope(item, &term_l)) {
            scopes.push(term_l);
        }
    }
    scopes.dedup();
    if scopes.len() < 2 {
        return None;
    }

    Some(RagClarification {
        reason: "multiple_source_scopes",
        message: format!(
            "当前知识库命中了多个来源范围：{}。请先说明你要问哪个平台或资料范围。",
            scopes.join("、")
        ),
        scopes,
    })
}

pub fn expanded_retrieval_queries(query: &str, intent: RagIntent) -> Vec<String> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    match intent {
        RagIntent::Diagnostic => vec![
            format!("{q} 故障 诊断 排查 连通性 检查 网关 路由 端口"),
            format!(
                "{q} troubleshoot diagnosis connectivity failure verify gateway route dns firewall port"
            ),
        ],
        RagIntent::Summary => vec![format!("{q} 总结 概括 摘要 主要内容 要点 overview summary")],
        RagIntent::Comparison => vec![format!("{q} 对比 区别 差异 compare contrast difference")],
        RagIntent::Lookup => Vec::new(),
    }
}

pub fn build_insufficient_evidence_refusal(
    _query: &str,
    coverage: &EvidenceCoverage,
    knowledge: &[Value],
) -> String {
    let missing = if coverage.missing.contains(&"diagnostic_evidence") {
        "缺少可引用的排障/诊断证据"
    } else if coverage.missing.contains(&"summary_evidence") {
        "缺少足够的总结覆盖证据"
    } else if coverage.missing.contains(&"comparison_evidence") {
        "缺少可对比的多方证据"
    } else {
        "缺少直接支持该问题的知识库证据"
    };
    let found = if knowledge.is_empty() {
        "当前没有检索到相关知识库片段".to_string()
    } else {
        let titles = knowledge
            .iter()
            .filter_map(|k| k.get("title").and_then(|v| v.as_str()))
            .take(3)
            .collect::<Vec<_>>();
        if titles.is_empty() {
            "已检索到片段，但证据类型不足".to_string()
        } else {
            format!("已检索到：{}", titles.join("、"))
        }
    };
    format!(
        "无法基于当前知识库证据回答这个实际操作类问题：{missing}。{found}。请上传包含故障现象、诊断步骤、网络连通性检查或操作流程的相关文档后再提问。"
    )
}

fn default_runtime_policy(intent: RagIntent) -> RagRuntimePolicy {
    match intent {
        RagIntent::Diagnostic => RagRuntimePolicy {
            profile_id: "default_kb_diagnostic".to_string(),
            retrieval_strategy: "expanded_diagnostic_cited_chunks".to_string(),
            top_k: 8,
            recovery_top_k: 8,
            min_citations: 1,
            refuse_without_evidence: true,
            allow_extractive_repair: false,
        },
        RagIntent::Summary => RagRuntimePolicy {
            profile_id: "default_kb_summary".to_string(),
            retrieval_strategy: "recent_or_source_diverse_cited_chunks".to_string(),
            top_k: 12,
            recovery_top_k: 12,
            min_citations: 1,
            refuse_without_evidence: true,
            allow_extractive_repair: false,
        },
        RagIntent::Comparison | RagIntent::Lookup => RagRuntimePolicy {
            profile_id: "default_kb_chat".to_string(),
            retrieval_strategy: "source_diverse_cited_chunks".to_string(),
            top_k: 8,
            recovery_top_k: 8,
            min_citations: 1,
            refuse_without_evidence: true,
            allow_extractive_repair: true,
        },
    }
}

fn policy_from_profile(
    mut policy: RagRuntimePolicy,
    profile: &attune_core::plugin_loader::RagProfileSpec,
) -> RagRuntimePolicy {
    if !profile.id.trim().is_empty() {
        policy.profile_id = profile.id.clone();
    }
    if !profile.retrieval.strategy.trim().is_empty() {
        policy.retrieval_strategy = profile.retrieval.strategy.clone();
    }
    if let Some(attune_core::plugin_loader::RagTopKSpec::Fixed(top_k)) = profile.retrieval.top_k {
        policy.top_k = (top_k as usize).clamp(1, 20);
        policy.recovery_top_k = policy.recovery_top_k.max(policy.top_k);
    }
    if let Some(min_citations) = profile.grounding.min_citations {
        policy.min_citations = min_citations.max(1);
    }
    if let Some(refuse_without_evidence) = profile.grounding.refuse_without_evidence {
        policy.refuse_without_evidence = refuse_without_evidence;
    }
    if let Some(allow_extractive_repair) = profile.grounding.allow_extractive_repair {
        policy.allow_extractive_repair = allow_extractive_repair;
    }
    policy
}

fn intent_profile_key(intent: RagIntent) -> &'static str {
    match intent {
        RagIntent::Lookup => "chat.rag.lookup",
        RagIntent::Diagnostic => "chat.rag.diagnostic",
        RagIntent::Summary => "chat.rag.summary",
        RagIntent::Comparison => "chat.rag.comparison",
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn knowledge_text(k: &Value) -> String {
    [
        k.get("title").and_then(|v| v.as_str()),
        k.get("source_scope").and_then(|v| v.as_str()),
        k.get("inject_content").and_then(|v| v.as_str()),
        k.get("content").and_then(|v| v.as_str()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
}

fn knowledge_matches_scope(k: &Value, scope: &str) -> bool {
    let scope = scope.trim().to_ascii_lowercase();
    if scope.is_empty() {
        return false;
    }
    knowledge_text(k).to_ascii_lowercase().contains(&scope)
}

fn coverage_threshold(intent: RagIntent) -> f32 {
    match intent {
        RagIntent::Diagnostic => 0.60,
        RagIntent::Summary => 0.50,
        RagIntent::Comparison => 0.55,
        RagIntent::Lookup => 0.25,
    }
}

fn lookup_coverage_score(evidence: &str) -> f32 {
    if evidence.trim().is_empty() {
        0.0
    } else {
        0.70
    }
}

fn diagnostic_coverage_score(evidence: &str) -> f32 {
    let signals = [
        "diagnosis",
        "diagnose",
        "troubleshoot",
        "connectivity",
        "verify",
        "check",
        "link status",
        "ip address",
        "subnet",
        "gateway",
        "route",
        "dns",
        "firewall",
        "packet loss",
        "port",
        "reachability",
        "configure",
        "configuration",
        "procedure",
        "steps",
        "runbook",
        "故障",
        "诊断",
        "排查",
        "检查",
        "配置",
        "步骤",
        "流程",
        "操作",
        "验证",
        "网关",
        "路由",
        "连通性",
        "端口",
        "丢包",
        "防火墙",
    ];
    let hits = signals
        .iter()
        .filter(|signal| evidence.contains(**signal))
        .count();
    (hits as f32 / 5.0).min(1.0)
}

fn summary_coverage_score(knowledge: &[Value], evidence: &str) -> f32 {
    let mut score: f32 = if knowledge.len() >= 2 { 0.55 } else { 0.35 };
    if contains_any(evidence, &["overview", "summary", "总结", "概括", "核心"]) {
        score += 0.15;
    }
    score.min(1.0)
}

fn comparison_coverage_score(knowledge: &[Value], evidence: &str) -> f32 {
    let mut source_keys = std::collections::HashSet::new();
    for k in knowledge {
        if let Some(id) = k.get("item_id").and_then(|v| v.as_str()) {
            source_keys.insert(id.to_string());
        } else if let Some(title) = k.get("title").and_then(|v| v.as_str()) {
            source_keys.insert(title.to_string());
        }
    }
    let mut score: f32 = if source_keys.len() >= 2 { 0.60 } else { 0.30 };
    if contains_any(
        evidence,
        &["compare", "difference", "versus", "对比", "区别", "差异"],
    ) {
        score += 0.15;
    }
    score.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_lookup_and_diagnostic_intents() {
        assert_eq!(
            classify_rag_intent("TCP/IP 起源于哪里？"),
            RagIntent::Lookup
        );
        assert_eq!(
            classify_rag_intent("我们应该如何排查 TCP/IP 连接失败？"),
            RagIntent::Diagnostic
        );
    }

    #[test]
    fn classifies_manual_source_location_as_lookup() {
        assert_eq!(
            classify_rag_intent("说明如何定位 A320 QRH 中的异常处置资料，并引用来源。"),
            RagIntent::Lookup
        );
    }

    #[test]
    fn summary_intent_takes_precedence_over_embedded_diagnostic_terms() {
        assert_eq!(
            classify_rag_intent("总结这份文档里的 TCP/IP 排查步骤"),
            RagIntent::Summary
        );
    }

    #[test]
    fn classifies_procedure_and_configuration_questions_as_diagnostic() {
        assert_eq!(
            classify_rag_intent("如何配置 TCP/IP 地址？"),
            RagIntent::Diagnostic
        );
        assert_eq!(
            classify_rag_intent("what steps should I follow to configure TCP/IP?"),
            RagIntent::Diagnostic
        );
    }

    #[test]
    fn origin_evidence_is_insufficient_for_troubleshooting_question() {
        let knowledge = vec![serde_json::json!({
            "title": "TCP/IP 起源",
            "content": "TCP/IP 起源于 ARPA/DARPA 资助的 ARPANET 互联网络研究。"
        })];

        let coverage = evaluate_evidence_coverage("我们应该如何排查 TCP/IP 连接失败？", &knowledge);

        assert_eq!(coverage.intent, RagIntent::Diagnostic);
        assert_eq!(coverage.status, "insufficient");
        assert!(coverage.score < 0.6);
        assert!(coverage.missing.contains(&"diagnostic_evidence"));
    }

    #[test]
    fn diagnostic_evidence_is_sufficient_for_troubleshooting_question() {
        let knowledge = vec![serde_json::json!({
            "title": "Connectivity diagnosis",
            "content": "Network connectivity diagnosis should verify link status, IP address, subnet mask, gateway route, DNS resolution, firewall policy, packet loss, and application port reachability."
        })];

        let coverage = evaluate_evidence_coverage("我们应该如何排查 TCP/IP 连接失败？", &knowledge);

        assert_eq!(coverage.intent, RagIntent::Diagnostic);
        assert_eq!(coverage.status, "sufficient");
        assert!(coverage.score >= 0.6);
        assert!(coverage.missing.is_empty());
    }

    #[test]
    fn procedure_evidence_is_sufficient_for_configuration_question() {
        let knowledge = vec![serde_json::json!({
            "title": "TCP/IP 配置步骤",
            "content": "配置步骤：检查网卡状态，设置 IP 地址、子网掩码和默认网关，保存配置后验证连通性。"
        })];

        let coverage = evaluate_evidence_coverage("如何配置 TCP/IP 地址？", &knowledge);

        assert_eq!(coverage.intent, RagIntent::Diagnostic);
        assert_eq!(coverage.status, "sufficient");
        assert!(coverage.missing.is_empty());
    }

    #[test]
    fn refusal_names_missing_diagnostic_evidence_without_inventing_steps() {
        let knowledge = vec![serde_json::json!({
            "title": "TCP/IP 起源",
            "content": "TCP/IP 起源于 ARPANET 研究。"
        })];
        let coverage = evaluate_evidence_coverage("如何排查 TCP/IP？", &knowledge);
        let refusal =
            build_insufficient_evidence_refusal("如何排查 TCP/IP？", &coverage, &knowledge);

        assert!(refusal.contains("排障") || refusal.contains("诊断"));
        assert!(!refusal.contains("DNS"));
        assert!(!refusal.contains("防火墙"));
        assert!(!refusal.contains("ping"));
    }

    #[test]
    fn maps_intents_to_runtime_profiles() {
        assert_eq!(
            rag_profile_for_intent(RagIntent::Diagnostic),
            "default_kb_diagnostic"
        );
        assert_eq!(
            rag_profile_for_intent(RagIntent::Summary),
            "default_kb_summary"
        );
        assert_eq!(rag_profile_for_intent(RagIntent::Lookup), "default_kb_chat");
    }

    #[test]
    fn default_runtime_policy_matches_builtin_profile_semantics() {
        let lookup = runtime_policy_for_intent_from_profiles(RagIntent::Lookup, []);
        assert_eq!(lookup.profile_id, "default_kb_chat");
        assert_eq!(lookup.retrieval_strategy, "source_diverse_cited_chunks");
        assert_eq!(lookup.top_k, 8);
        assert_eq!(lookup.min_citations, 1);
        assert!(lookup.refuse_without_evidence);
        assert!(lookup.allow_extractive_repair);

        let diagnostic = runtime_policy_for_intent_from_profiles(RagIntent::Diagnostic, []);
        assert_eq!(diagnostic.profile_id, "default_kb_diagnostic");
        assert_eq!(
            diagnostic.retrieval_strategy,
            "expanded_diagnostic_cited_chunks"
        );
        assert_eq!(diagnostic.top_k, 8);
        assert!(!diagnostic.allow_extractive_repair);

        let summary = runtime_policy_for_intent_from_profiles(RagIntent::Summary, []);
        assert_eq!(summary.profile_id, "default_kb_summary");
        assert_eq!(
            summary.retrieval_strategy,
            "recent_or_source_diverse_cited_chunks"
        );
        assert_eq!(summary.top_k, 12);
        assert!(!summary.allow_extractive_repair);
    }

    #[test]
    fn runtime_policy_uses_plugin_rag_profile_when_available() {
        let profile = attune_core::plugin_loader::RagProfileSpec {
            id: "default_kb_summary".to_string(),
            intents: vec!["chat.rag.summary".to_string()],
            workflow: Default::default(),
            retrieval: attune_core::plugin_loader::RagRetrievalSpec {
                strategy: "custom_summary".to_string(),
                fallback_when_empty: Some("refuse".to_string()),
                top_k: Some(attune_core::plugin_loader::RagTopKSpec::Fixed(10)),
            },
            answer: Default::default(),
            grounding: attune_core::plugin_loader::RagGroundingSpec {
                min_citations: Some(2),
                refuse_without_evidence: Some(true),
                allow_extractive_repair: Some(false),
            },
        };

        let policy = runtime_policy_for_intent_from_profiles(RagIntent::Summary, [&profile]);

        assert_eq!(policy.profile_id, "default_kb_summary");
        assert_eq!(policy.retrieval_strategy, "custom_summary");
        assert_eq!(policy.top_k, 10);
        assert_eq!(policy.min_citations, 2);
        assert!(policy.refuse_without_evidence);
        assert!(!policy.allow_extractive_repair);
    }

    #[test]
    fn metadata_reports_complete_retrieval_trace() {
        let coverage = EvidenceCoverage {
            intent: RagIntent::Summary,
            score: 0.75,
            status: "sufficient",
            found_evidence: true,
            missing: vec![],
        };
        let trace = RagRetrievalTrace {
            profile_id: "default_kb_summary".to_string(),
            strategy: "recent_or_source_diverse_cited_chunks".to_string(),
            passes: vec!["first_pass", "expanded_retrieval"],
            queries: vec![
                "总结 TCP/IP".to_string(),
                "总结 TCP/IP overview summary".to_string(),
            ],
            final_top_k: 12,
            vector_results: None,
            bm25_results: None,
            citations_required: 1,
        };

        let meta = rag_metadata_with_trace(&coverage, "llm-summary", None, 3, &trace);

        assert_eq!(meta["rag_profile"], "default_kb_summary");
        assert_eq!(
            meta["retrieval"]["strategy"],
            "recent_or_source_diverse_cited_chunks"
        );
        assert_eq!(meta["retrieval"]["queries"][0], "总结 TCP/IP");
        assert_eq!(meta["retrieval"]["final_top_k"], 12);
        assert_eq!(meta["retrieval"]["vector_results"], serde_json::Value::Null);
        assert_eq!(meta["retrieval"]["bm25_results"], serde_json::Value::Null);
        assert_eq!(meta["retrieval"]["stage_counts_available"], false);
        assert_eq!(meta["retrieval"]["citations_required"], 1);
    }

    #[test]
    fn metadata_reports_refusal_mode_and_coverage() {
        let coverage = EvidenceCoverage {
            intent: RagIntent::Diagnostic,
            score: 0.2,
            status: "insufficient",
            found_evidence: true,
            missing: vec!["diagnostic_evidence"],
        };

        let meta = rag_metadata(
            &coverage,
            "refusal-insufficient-evidence",
            Some("insufficient_evidence"),
            1,
        );

        assert_eq!(meta["rag_profile"], "default_kb_diagnostic");
        assert_eq!(meta["intent"], "diagnostic");
        assert_eq!(meta["answer_mode"], "refusal-insufficient-evidence");
        assert_eq!(meta["degraded"], true);
        assert_eq!(meta["retrieval"]["coverage_status"], "insufficient");
        assert_eq!(meta["citations_count"], 1);
    }

    #[test]
    fn diagnostic_expansion_adds_troubleshooting_terms() {
        let queries = expanded_retrieval_queries("排查 TCP/IP", RagIntent::Diagnostic);

        assert_eq!(queries.len(), 2);
        assert!(queries.iter().any(|q| q.contains("连通性")));
        assert!(queries.iter().any(|q| q.contains("troubleshoot")));
    }

    #[test]
    fn metadata_can_report_recovery_passes_and_queries() {
        let coverage = EvidenceCoverage {
            intent: RagIntent::Diagnostic,
            score: 0.8,
            status: "sufficient",
            found_evidence: true,
            missing: vec![],
        };
        let queries = vec!["排查 TCP/IP troubleshoot".to_string()];
        let meta = rag_metadata_with_retrieval(
            &coverage,
            "llm-chat",
            None,
            2,
            &["first_pass", "expanded_retrieval"],
            &queries,
        );

        assert_eq!(meta["retrieval"]["passes"][1], "expanded_retrieval");
        assert_eq!(meta["retrieval"]["queries"][0], "排查 TCP/IP troubleshoot");
    }

    #[test]
    fn workflow_clarification_detects_multiple_configured_source_scopes() {
        let profile = attune_core::plugin_loader::RagProfileSpec {
            id: "default_kb_chat".to_string(),
            intents: vec!["chat.rag.question".to_string()],
            workflow: attune_core::plugin_loader::RagWorkflowSpec {
                clarification: attune_core::plugin_loader::RagWorkflowClarificationSpec {
                    enabled: Some(true),
                    scope_terms: vec!["rtos".to_string(), "linux".to_string()],
                    require_when_multiple_scopes: Some(true),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let knowledge = vec![
            serde_json::json!({
                "title": "RTOS DMAC guide",
                "source_scope": "rtos",
                "content": "hal_dma_chan_request requests an RTOS DMA channel."
            }),
            serde_json::json!({
                "title": "Linux DMAC guide",
                "source_scope": "linux",
                "content": "dma_request_chan requests a Linux DMA channel."
            }),
        ];

        let clarification =
            workflow_clarification_for_query("dmac 申请 dma 通道的函数接口是什么", &knowledge, &profile)
                .expect("multiple configured scopes should require clarification");

        assert_eq!(clarification.reason, "multiple_source_scopes");
        assert_eq!(clarification.scopes, vec!["rtos", "linux"]);
        assert!(clarification.message.contains("rtos"));
        assert!(clarification.message.contains("linux"));
    }

    #[test]
    fn workflow_clarification_respects_user_named_scope() {
        let profile = attune_core::plugin_loader::RagProfileSpec {
            id: "default_kb_chat".to_string(),
            workflow: attune_core::plugin_loader::RagWorkflowSpec {
                clarification: attune_core::plugin_loader::RagWorkflowClarificationSpec {
                    enabled: Some(true),
                    scope_terms: vec!["rtos".to_string(), "linux".to_string()],
                    require_when_multiple_scopes: Some(true),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let knowledge = vec![
            serde_json::json!({"source_scope": "rtos", "content": "RTOS DMA interface"}),
            serde_json::json!({"source_scope": "linux", "content": "Linux DMA interface"}),
        ];

        assert!(workflow_clarification_for_query(
            "rtos 中 dmac 申请 dma 通道的函数接口是什么",
            &knowledge,
            &profile,
        )
        .is_none());
    }

    #[test]
    fn metadata_can_report_rag_workflow_trace() {
        let coverage = EvidenceCoverage {
            intent: RagIntent::Lookup,
            score: 0.8,
            status: "sufficient",
            found_evidence: true,
            missing: vec![],
        };
        let workflow = RagWorkflowTrace {
            mode: "reliable".to_string(),
            stages: vec![
                RagWorkflowStageTrace::completed("intent_analyze"),
                RagWorkflowStageTrace::blocked("clarification", "multiple_source_scopes"),
            ],
            clarification_required: true,
            failure_layer: Some("prompt_profile".to_string()),
            repair_attempted: false,
        };

        let meta = rag_metadata_with_workflow(&coverage, "clarification", Some("ambiguous_source_scope"), 2, &workflow);

        assert_eq!(meta["rag_workflow"]["mode"], "reliable");
        assert_eq!(meta["rag_workflow"]["clarification_required"], true);
        assert_eq!(meta["rag_workflow"]["stages"][1]["status"], "blocked");
        assert_eq!(
            meta["rag_workflow"]["stages"][1]["reason"],
            "multiple_source_scopes"
        );
        assert_eq!(meta["rag_workflow"]["failure_layer"], "prompt_profile");
    }
}
