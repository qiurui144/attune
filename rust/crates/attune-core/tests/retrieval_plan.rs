use attune_core::retrieval_plan::{
    plan_query, plan_retrieval, score_sras_candidate, EvidenceNeed, IndexPartitionField,
    RetrievalChannel, RetrievalLatencyClass, RetrievalPlanRequest, RetrievalTarget,
    SrasCandidateSignal, SrasSelector,
};
use attune_core::store::audit::PrivacyTier;

#[test]
fn local_scheduler_interactive_plan_caps_rerank_and_evidence_budget() {
    let plan = plan_retrieval(
        RetrievalPlanRequest::local_scheduler_interactive("总结这份反洗钱材料的关键处罚依据")
            .with_privacy_tier(PrivacyTier::L0)
            .with_vault("vault-a")
            .with_corpus_domain("legal"),
    );

    assert_eq!(plan.target, RetrievalTarget::LocalScheduler);
    assert_eq!(plan.latency_class, RetrievalLatencyClass::Interactive);
    assert_eq!(plan.final_top_k, 6);
    assert_eq!(plan.rerank_candidate_cap, 20);
    assert_eq!(plan.evidence_token_budget, 2048);
    assert!(
        plan.partitions.local_only,
        "L0 and Local scheduler must stay local"
    );
    assert!(plan
        .channels
        .iter()
        .any(|c| c.channel == RetrievalChannel::Summary));

    let required: Vec<_> = plan
        .partitions
        .filters
        .iter()
        .filter(|f| f.required)
        .map(|f| (f.field, f.value.as_str()))
        .collect();
    assert!(required.contains(&(IndexPartitionField::Vault, "vault-a")));
    assert!(required.contains(&(IndexPartitionField::PrivacyTier, "L0")));
    assert!(required.contains(&(IndexPartitionField::CorpusDomain, "legal")));
    assert!(required.contains(&(IndexPartitionField::EmbeddingModel, "bge-m3")));

    let search = plan.to_search_params();
    assert_eq!(search.top_k, 6);
    assert_eq!(search.intermediate_k, 20);
    assert_eq!(search.min_score, Some(0.65));
    assert_eq!(search.domain_hint.as_deref(), Some("legal"));
}

#[test]
fn exact_identifier_query_rewards_metadata_and_bm25() {
    let plan = plan_retrieval(RetrievalPlanRequest::local_scheduler_interactive(
        "E0502 borrow checker error 如何修复",
    ));

    assert!(plan.query_features.exactish);
    let metadata = plan
        .channels
        .iter()
        .find(|c| c.channel == RetrievalChannel::Metadata)
        .expect("exact query should enable metadata channel");
    assert!(metadata.required);

    let bm25 = plan
        .channels
        .iter()
        .find(|c| c.channel == RetrievalChannel::Bm25)
        .unwrap();
    let vector = plan
        .channels
        .iter()
        .find(|c| c.channel == RetrievalChannel::Vector)
        .unwrap();
    assert!(
        bm25.weight > vector.weight,
        "exact identifiers should bias lexical retrieval before dense semantic recall"
    );
}

#[test]
fn sras_selector_prefers_grounded_exact_same_domain_candidate() {
    let selector =
        SrasSelector::new(attune_core::retrieval_plan::SrasWeights::local_scheduler_interactive());
    let candidates = vec!["semantic", "exact", "blocked"];

    let ranked = selector.rank(
        &candidates,
        |id| match *id {
            "exact" => SrasCandidateSignal {
                base_score: 0.2,
                exact_match: true,
                entity_match: true,
                same_domain: true,
                same_language: true,
                privacy_allowed: true,
                has_citation_span: true,
                vector_score: Some(0.72),
                bm25_rank: Some(0),
                recency_days: Some(3),
                chunk_level: Some(2),
            },
            "semantic" => SrasCandidateSignal {
                base_score: 0.4,
                exact_match: false,
                entity_match: false,
                same_domain: false,
                same_language: true,
                privacy_allowed: true,
                has_citation_span: false,
                vector_score: Some(0.96),
                bm25_rank: None,
                recency_days: Some(2),
                chunk_level: Some(2),
            },
            _ => SrasCandidateSignal {
                base_score: 10.0,
                privacy_allowed: false,
                ..SrasCandidateSignal::default()
            },
        },
        3,
    );

    assert_eq!(*ranked[0].candidate, "exact");
    assert_eq!(*ranked[2].candidate, "blocked");
    assert!(ranked[2].score < 0.0);
}

#[test]
fn local_scheduler_background_broad_query_can_expand_but_stays_bounded() {
    let plan = plan_retrieval(
        RetrievalPlanRequest::local_scheduler_interactive(
            "对比这些会议纪要和产品文档，综合整理风险点",
        )
        .background(),
    );

    assert_eq!(plan.latency_class, RetrievalLatencyClass::Background);
    assert_eq!(plan.rerank_candidate_cap, 40);
    assert_eq!(plan.evidence_token_budget, 4096);
    assert!(plan.final_top_k <= 16);
    assert!(plan
        .channels
        .iter()
        .any(|c| c.channel == RetrievalChannel::Summary));
}

#[test]
fn cloud_plan_is_not_forced_local_unless_privacy_requires_it() {
    let mut req = RetrievalPlanRequest::local_scheduler_interactive("quarterly roadmap summary");
    req.target = RetrievalTarget::Cloud;
    req.privacy_tier = PrivacyTier::L1;

    let plan = plan_retrieval(req);
    assert!(!plan.partitions.local_only);
    assert!(plan.evidence_token_budget >= 3072);
}

#[test]
fn privacy_disallowed_candidate_is_hard_demoted() {
    let score = score_sras_candidate(
        &SrasCandidateSignal {
            base_score: 100.0,
            exact_match: true,
            privacy_allowed: false,
            ..SrasCandidateSignal::default()
        },
        &attune_core::retrieval_plan::SrasWeights::local_scheduler_interactive(),
    );
    assert!(score < -999_999.0);
}

#[test]
fn planner_maps_howto_api_question_to_api_and_procedure_evidence() {
    let plan = plan_query("How do I initialize and start a transfer?");

    assert!(plan.evidence_needs.contains(&EvidenceNeed::ApiReference));
    assert!(plan.evidence_needs.contains(&EvidenceNeed::Procedure));
}

#[test]
fn planner_maps_debug_question_to_troubleshooting_and_command_evidence() {
    let plan = plan_query("How do I verify the module and troubleshoot zero output?");

    assert!(plan.evidence_needs.contains(&EvidenceNeed::Troubleshooting));
    assert!(plan.evidence_needs.contains(&EvidenceNeed::Command));
}

#[test]
fn planner_extracts_explicit_source_constraints_without_known_corpus_mapping() {
    let plan = plan_query("For ABC123 RTOS in /docs/sdk, how do I configure the driver?");

    assert!(plan
        .source_constraints
        .required_terms
        .iter()
        .any(|term| term == "ABC123"));
    assert!(plan
        .source_constraints
        .required_terms
        .iter()
        .any(|term| term == "RTOS"));
    assert!(plan
        .source_constraints
        .required_terms
        .iter()
        .any(|term| term == "/docs/sdk"));
}
