use attune_server::rag_orchestrator::{
    assemble_evidence_pack, assemble_evidence_pack_for_query, build_evidence_pack_prompt,
    build_evidence_pack_prompt_for_model, build_local_scheduler_extractive_answer,
    build_local_scheduler_extractive_summary, local_scheduler_source_lookup_query,
};

fn search_result(
    item_id: &str,
    title: &str,
    node_kind: &str,
    text: &str,
) -> attune_core::search::SearchResult {
    attune_core::search::SearchResult {
        item_id: item_id.to_string(),
        chunk_idx: None,
        score: 0.9,
        title: title.to_string(),
        content: format!("[kind: {node_kind}]\n{text}"),
        source_type: "local".to_string(),
        source_path: None,
        inject_content: None,
        corpus_domain: "general".to_string(),
        breadcrumb: Vec::new(),
        chunk_offset_start: None,
        chunk_offset_end: None,
    }
}

#[test]
fn rag_orchestrator_detects_source_lookup_and_builds_grounded_answer() {
    std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", "1");
    let knowledge = vec![serde_json::json!({
        "item_id": "tcp-origin",
        "title": "TCP Origin",
        "inject_content": "TCP/IP 起源于美国 DARPA 资助的 ARPANET 研究。"
    })];

    assert!(local_scheduler_source_lookup_query("tcp/ip起源于哪里？"));
    let answer = build_local_scheduler_extractive_answer("tcp/ip起源于哪里？", &knowledge)
        .expect("source lookup should produce grounded answer");

    assert!(answer.contains("TCP Origin"));
    assert!(answer.contains("ARPANET"));
    assert!(answer.contains("DARPA"));
}

#[test]
fn rag_orchestrator_builds_grounded_summary() {
    std::env::set_var("ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER", "1");
    let knowledge = vec![serde_json::json!({
        "item_id": "tcp-origin",
        "title": "TCP Origin",
        "inject_content": "TCP/IP 起源于美国 DARPA 资助的 ARPANET 研究。"
    })];

    let summary = build_local_scheduler_extractive_summary("总结这份知识库文档", &knowledge)
        .expect("summary should use cited knowledge");

    assert!(summary.contains("核心结论"));
    assert!(summary.contains("关键证据"));
    assert!(summary.contains("ARPANET"));
    assert!(summary.contains("DARPA"));
}

#[test]
fn evidence_pack_combines_api_procedure_and_troubleshooting_from_same_source() {
    let results = vec![
        search_result(
            "doc-a",
            "Manual A",
            "ApiReference",
            "Prototype: int open_device(void)",
        ),
        search_result(
            "doc-a",
            "Manual A",
            "ProcedureStep",
            "Step 1 Call open_device().",
        ),
        search_result(
            "doc-a",
            "Manual A",
            "Troubleshooting",
            "If output is zero, verify the buffer.",
        ),
        search_result("doc-b", "Manual B", "Paragraph", "Overview only."),
    ];
    let plan = attune_core::retrieval_plan::plan_query(
        "How do I open the device and troubleshoot zero output?",
    );
    let pack = assemble_evidence_pack(&plan, &results);

    assert_eq!(pack.primary_source_id, "doc-a");
    assert_eq!(pack.source_title, "Manual A");
    assert!(pack
        .nodes
        .iter()
        .any(|node| node.node_kind == "ApiReference"));
    assert!(pack
        .nodes
        .iter()
        .any(|node| node.node_kind == "ProcedureStep"));
    assert!(pack
        .nodes
        .iter()
        .any(|node| node.node_kind == "Troubleshooting"));
    assert!(pack.diagnostics.missing_needs.is_empty());
}

#[test]
fn prompt_includes_evidence_pack_and_diagnostics_without_domain_template() {
    let results = vec![
        search_result(
            "doc-a",
            "Manual A",
            "ApiReference",
            "Prototype: int open_device(void)",
        ),
        search_result(
            "doc-a",
            "Manual A",
            "ProcedureStep",
            "Step 1 Call open_device().",
        ),
    ];
    let plan = attune_core::retrieval_plan::plan_query("How do I start a transfer?");
    let pack = assemble_evidence_pack(&plan, &results);

    let prompt = build_evidence_pack_prompt("How do I start a transfer?", &pack);

    assert!(prompt.contains("Evidence Pack"));
    assert!(prompt.contains("Evidence Diagnostics"));
    assert!(prompt.contains("ApiReference"));
    assert!(prompt.contains("ProcedureStep"));
    assert!(prompt.contains("If evidence is incomplete, say what is missing"));
    assert!(!prompt.contains("V821"));
    assert!(!prompt.contains("Rockchip"));
}

#[test]
fn evidence_pack_for_query_filters_cross_source_noise_before_small_model_prompt() {
    let results = vec![
        search_result(
            "manual-noise",
            "Unrelated Camera Guide",
            "Troubleshooting",
            "If camera streaming fails, check ISP graph, sensor power, and video buffer queues.",
        ),
        search_result(
            "manual-target",
            "Industrial Network Manual",
            "ProcedureStep",
            "Step 1 Check the physical link indicator. Step 2 Verify route, DNS, port, firewall, logs, and packet loss.",
        ),
        search_result(
            "manual-noise",
            "Unrelated Camera Guide",
            "ProcedureStep",
            "Step 1 Configure image sensor MIPI lanes and ISP tuning files.",
        ),
    ];
    let plan = attune_core::retrieval_plan::plan_query(
        "网络连接失败时应该如何排查物理链路、路由、DNS、端口和日志？",
    );

    let pack = assemble_evidence_pack_for_query(
        "网络连接失败时应该如何排查物理链路、路由、DNS、端口和日志？",
        &plan,
        &results,
    );

    assert_eq!(pack.primary_source_id, "manual-target");
    assert!(pack
        .nodes
        .iter()
        .all(|node| node.source_id == "manual-target"));
    assert_eq!(pack.diagnostics.sources_considered, 2);
    assert!(pack
        .diagnostics
        .satisfied_needs
        .contains(&"Procedure".to_string()));
    assert!(
        pack.diagnostics
            .satisfied_needs
            .contains(&"Troubleshooting".to_string()),
        "{:?}",
        pack.diagnostics
    );
}

#[test]
fn evidence_pack_prompt_declares_adaptive_small_model_contract() {
    let results = vec![search_result(
        "manual-target",
        "Industrial Network Manual",
        "ProcedureStep",
        "Step 1 Check physical link. Step 2 Verify route, DNS, port, firewall, logs, and packet loss.",
    )];
    let plan =
        attune_core::retrieval_plan::plan_query("How should I troubleshoot network failure?");
    let pack = assemble_evidence_pack_for_query(
        "How should I troubleshoot network failure?",
        &plan,
        &results,
    );

    let prompt = build_evidence_pack_prompt("How should I troubleshoot network failure?", &pack);

    assert!(prompt.contains("Adaptive model discipline"));
    assert!(prompt.contains("Small/weak models"));
    assert!(prompt.contains("copy short evidence-backed facts"));
    assert!(prompt.contains("Do not synthesize across unrelated sources"));
}

#[test]
fn evidence_pack_marks_weak_quality_when_requested_needs_are_missing() {
    let results = vec![search_result(
        "manual-target",
        "Industrial Operations Manual",
        "Paragraph",
        "This section defines the monitoring dashboard and lists its page title.",
    )];
    let plan = attune_core::retrieval_plan::plan_query("设备启动失败时如何排查电源、日志和配置？");

    let pack = assemble_evidence_pack_for_query(
        "设备启动失败时如何排查电源、日志和配置？",
        &plan,
        &results,
    );

    assert_eq!(pack.diagnostics.quality, "weak");
    assert!(pack
        .diagnostics
        .quality_reasons
        .iter()
        .any(|reason| { reason.contains("missing_evidence_needs") }));
}

#[test]
fn weak_model_prompt_uses_conservative_quality_discipline() {
    let results = vec![search_result(
        "manual-target",
        "Industrial Operations Manual",
        "ProcedureStep",
        "Step 1 Check the power input. Step 2 Read the service log.",
    )];
    let plan =
        attune_core::retrieval_plan::plan_query("How should I troubleshoot startup failure?");
    let pack = assemble_evidence_pack_for_query(
        "How should I troubleshoot startup failure?",
        &plan,
        &results,
    );

    let prompt = build_evidence_pack_prompt_for_model(
        "How should I troubleshoot startup failure?",
        &pack,
        &attune_server::rag_orchestrator::RagModelDiscipline::Small,
    );

    assert!(prompt.contains("Evidence quality:"));
    assert!(prompt.contains("Model discipline: small"));
    assert!(prompt.contains("Use short bullet points"));
    assert!(prompt.contains("Do not infer missing steps"));
}
