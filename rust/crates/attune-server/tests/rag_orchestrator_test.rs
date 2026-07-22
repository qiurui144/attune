use attune_server::rag_orchestrator::{
    build_local_scheduler_extractive_answer, build_local_scheduler_extractive_summary,
    local_scheduler_source_lookup_query,
};

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
