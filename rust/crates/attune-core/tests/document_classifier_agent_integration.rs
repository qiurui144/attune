//! document_classifier_agent — integration / E2E test (≥1 required).
//!
//! 6-class coverage: this file covers the "Integration E2E ≥ 1" floor.
//!
//! Calls the agent via the `Agent` trait surface (the same path a
//! capability_dispatch subprocess or future plugin runtime would take).

use attune_core::agents::Agent;
use attune_core::agents::document_classifier::DocumentClassifierAgent;

#[test]
fn integration_agent_trait_run_end_to_end() {
    let agent = DocumentClassifierAgent;

    assert_eq!(agent.id(), "document_classifier_agent");
    assert!(!agent.description().is_empty());
    assert!(agent.case_kinds().is_empty(), "should be generic (no case_kind binding)");

    // Realistic 3-doc mixed pool: borrowing-doc + bank-statement + chat.
    let inputs: Vec<(String, String)> = vec![
        ("借条.pdf".to_string(),
         "借条\n出借人: 张三\n借款人: 李四\n本金: 500000 元\n月利率 1%".to_string()),
        ("流水.pdf".to_string(),
         "交易日期 2023-01-15 对方户名 李四 交易金额 +500000 余额 1000000 汇入".to_string()),
        ("聊天.txt".to_string(),
         "[微信] 张三: 已转 50 万 [图片]\n李四: 收到".to_string()),
    ];

    let out = agent
        .run(inputs.clone())
        .expect("agent.run should succeed on valid input");

    // contract: 3 in → 3 out
    assert_eq!(out.computation.classified.len(), 3);

    // contract: every input file appears in output
    let out_files: Vec<&str> = out
        .computation
        .classified
        .iter()
        .map(|c| c.file.as_str())
        .collect();
    for (f, _) in &inputs {
        assert!(out_files.contains(&f.as_str()), "file {} missing in output", f);
    }

    // contract: kind_summary partition
    let total: usize = out.computation.kind_summary.values().sum();
    assert_eq!(total, 3);

    // contract: audit_trail non-empty and references each file
    assert!(out.audit_trail.contains("借条.pdf"));
    assert!(out.audit_trail.contains("流水.pdf"));
    assert!(out.audit_trail.contains("聊天.txt"));

    // contract: red_lines empty (document classifier has no hard red lines)
    assert!(out.red_lines_violated.is_empty());

    // contract: overall confidence ∈ [0, 1]
    assert!(out.confidence >= 0.0 && out.confidence <= 1.0);
}

#[test]
fn integration_agent_trait_handles_empty_input() {
    let agent = DocumentClassifierAgent;
    let out = agent.run(vec![]).expect("empty input must not error");
    assert!(out.computation.classified.is_empty());
    assert_eq!(out.confidence, 0.0);
}
