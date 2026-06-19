//! suggestions 规则引擎测试 — §6.1 六类下限 + 成本契约反偷跑守卫。

use super::*;

// === fixtures ===

fn cluster(n: usize, entities: &[&str]) -> ClusterCandidate {
    ClusterCandidate {
        item_ids: (0..n).map(|i| format!("item-{i}")).collect(),
        top_entities: entities.iter().map(|s| s.to_string()).collect(),
    }
}

fn miss(query: &str, count: u32) -> MissedQuery {
    MissedQuery {
        query: query.to_string(),
        miss_count: count,
    }
}

// === Golden / happy (4 kind 覆盖) ===

#[test]
fn happy_organize_card_from_cluster() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(5, &["合同甲方A", "项目X"])],
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].kind, SuggestionKind::Organize);
    assert_eq!(cards[0].action_kind, ActionKind::OpenOrganizeWizard);
    assert_eq!(cards[0].ref_ids.len(), 5);
    assert!(cards[0].title.contains('5'));
    assert!(cards[0].detail.contains("合同甲方A"));
}

#[test]
fn happy_enrich_card_from_missed_query() {
    let ctx = SignalContext {
        missed_queries: vec![miss("riscv 向量扩展", 4)],
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].kind, SuggestionKind::Enrich);
    assert_eq!(cards[0].action_kind, ActionKind::OpenSearch);
    assert_eq!(cards[0].cost_tier, CostTier::Free);
    assert!(cards[0].title.contains("riscv 向量扩展"));
}

#[test]
fn happy_retrieval_card_from_signal_batch() {
    let ctx = SignalContext {
        unprocessed_search_miss: 7,
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].kind, SuggestionKind::Retrieval);
    assert_eq!(cards[0].action_kind, ActionKind::RunSkillEvolution);
    // 检索优化点击后会调 LLM → 卡明示 Cloud 成本(成本契约 UI 显示)。
    assert_eq!(cards[0].cost_tier, CostTier::Cloud);
}

#[test]
fn happy_profile_card_from_behavior_signals() {
    let ctx = SignalContext {
        browse_signal_count: 8,
        annotation_marker_count: 3,
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].kind, SuggestionKind::Profile);
    assert_eq!(cards[0].action_kind, ActionKind::OpenProfile);
    assert_eq!(cards[0].cost_tier, CostTier::Free);
}

#[test]
fn happy_all_four_kinds_together() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(4, &["e"])],
        missed_queries: vec![miss("q", 5)],
        unprocessed_search_miss: 6,
        browse_signal_count: 12,
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    let kinds: std::collections::HashSet<_> = cards.iter().map(|c| c.kind).collect();
    assert_eq!(kinds.len(), 4, "expected all 4 kinds, got {kinds:?}");
}

#[test]
fn happy_multiple_clusters_multiple_cards() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(3, &["a"]), cluster(6, &["b"])],
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    assert_eq!(cards.len(), 2);
    assert!(cards.iter().all(|c| c.kind == SuggestionKind::Organize));
    // 两张卡 signature 不同(不同 item 集)。
    assert_ne!(cards[0].signature, cards[1].signature);
}

// === Edge (≥5) ===

#[test]
fn edge_empty_signals_no_cards() {
    let ctx = SignalContext::default();
    assert!(evaluate(&ctx).is_empty());
}

#[test]
fn edge_single_below_threshold_no_card() {
    // miss_count=2 < ENRICH_MISS_THRESHOLD(3);cluster size 2 < ORGANIZE_MIN_GROUP(3);
    // search_miss 4 < RETRIEVAL(5);profile 9 < PROFILE(10)。全不出卡。
    let ctx = SignalContext {
        missed_queries: vec![miss("q", 2)],
        cluster_candidates: vec![cluster(2, &[])],
        unprocessed_search_miss: 4,
        browse_signal_count: 9,
        ..Default::default()
    };
    assert!(evaluate(&ctx).is_empty());
}

#[test]
fn edge_exact_thresholds_emit() {
    let ctx = SignalContext {
        missed_queries: vec![miss("q", ENRICH_MISS_THRESHOLD)],
        cluster_candidates: vec![cluster(ORGANIZE_MIN_GROUP, &[])],
        unprocessed_search_miss: RETRIEVAL_SIGNAL_THRESHOLD,
        browse_signal_count: PROFILE_SIGNAL_THRESHOLD,
        ..Default::default()
    };
    assert_eq!(evaluate(&ctx).len(), 4);
}

#[test]
fn edge_all_muted_no_cards() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(5, &[])],
        missed_queries: vec![miss("q", 9)],
        unprocessed_search_miss: 20,
        browse_signal_count: 50,
        muted_kinds: SuggestionKind::ALL.to_vec(),
        ..Default::default()
    };
    assert!(evaluate(&ctx).is_empty());
}

#[test]
fn edge_partial_mute_only_filters_that_kind() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(5, &[])],
        missed_queries: vec![miss("q", 9)],
        muted_kinds: vec![SuggestionKind::Organize],
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].kind, SuggestionKind::Enrich);
}

#[test]
fn edge_ref_ids_capped() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(200, &[])],
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].ref_ids.len(), MAX_REF_IDS);
    // 但标题仍报真实组大小(用户知道实际有多少)。
    assert!(cards[0].title.contains("200"));
}

#[test]
fn edge_empty_query_string_ignored() {
    let ctx = SignalContext {
        missed_queries: vec![miss("   ", 99)],
        ..Default::default()
    };
    assert!(evaluate(&ctx).is_empty());
}

// === Error / 异常 (≥3) ===

#[test]
fn err_dismiss_filters_signature() {
    let ctx0 = SignalContext {
        cluster_candidates: vec![cluster(5, &[])],
        ..Default::default()
    };
    let sig = evaluate(&ctx0)[0].signature.clone();
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(5, &[])],
        dismissed_signatures: vec![sig],
        ..Default::default()
    };
    assert!(evaluate(&ctx).is_empty(), "dismissed card must not reappear");
}

#[test]
fn err_panicking_rule_isolated() {
    // 一条规则 panic 不阻塞其它规则。用一个会 panic 的自定义 rule fn 验证隔离。
    fn boom(_: &SignalContext) -> Vec<SuggestionCard> {
        panic!("rule blew up");
    }
    let ctx = SignalContext {
        missed_queries: vec![miss("q", 9)],
        ..Default::default()
    };
    let mut out = Vec::new();
    run_rule_isolated(&mut out, &ctx, SuggestionKind::Organize, boom);
    // boom 被隔离,out 仍空(没崩)。
    assert!(out.is_empty());
    // 正常规则照常出卡。
    run_rule_isolated(&mut out, &ctx, SuggestionKind::Enrich, rule_enrich);
    assert_eq!(out.len(), 1);
}

#[test]
fn err_dismiss_nonexistent_signature_noop() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(5, &[])],
        dismissed_signatures: vec!["sig-that-does-not-exist".into()],
        ..Default::default()
    };
    // 不存在的 dismiss 无副作用,卡照出。
    assert_eq!(evaluate(&ctx).len(), 1);
}

// === Adversarial ===

#[test]
fn adversarial_malicious_signal_no_dangerous_action() {
    // 恶意/畸形信号(超长 query、注入样字符)不应触发危险动作:action_kind 只能是
    // 4 个枚举之一(全部指向既有 UI handler,无 shell/路径/网络任意执行)。
    let evil = "'; DROP TABLE items;-- <script>".repeat(100);
    let ctx = SignalContext {
        missed_queries: vec![miss(&evil, 5)],
        cluster_candidates: vec![ClusterCandidate {
            item_ids: vec!["../../etc/passwd".into(); 5],
            top_entities: vec!["$(rm -rf /)".into()],
        }],
        ..Default::default()
    };
    let cards = evaluate(&ctx);
    for c in &cards {
        assert!(
            matches!(
                c.action_kind,
                ActionKind::OpenOrganizeWizard
                    | ActionKind::OpenSearch
                    | ActionKind::RunSkillEvolution
                    | ActionKind::OpenProfile
            ),
            "action must be a known safe enum, got {:?}",
            c.action_kind
        );
        // 卡生成永不是 LLM(本卡 tier 不能因恶意输入升级)。
        assert_ne!(
            c.cost_tier,
            CostTier::Cloud,
            "enrich/organize from data must not silently become Cloud-cost"
        );
    }
}

#[test]
fn adversarial_signature_no_collision_across_kinds() {
    // 同语义键在不同 kind 下 signature 必须不同(防跨类去重误删)。
    let s_org = SuggestionCard::make_signature(SuggestionKind::Organize, &["x".into()]);
    let s_enr = SuggestionCard::make_signature(SuggestionKind::Enrich, &["x".into()]);
    assert_ne!(s_org, s_enr);
    assert!(s_org.starts_with("organize-"));
    assert!(s_enr.starts_with("enrich-"));
}

// === serde 契约 ===

#[test]
fn card_serializes_with_snake_case_kind() {
    let ctx = SignalContext {
        cluster_candidates: vec![cluster(5, &[])],
        ..Default::default()
    };
    let card = &evaluate(&ctx)[0];
    let json = serde_json::to_string(card).unwrap();
    assert!(json.contains("\"kind\":\"organize\""));
    assert!(json.contains("\"action_kind\":\"open_organize_wizard\""));
    assert!(json.contains("\"cost_tier\":\"local\""));
}

#[test]
fn kind_parse_roundtrip() {
    for k in SuggestionKind::ALL {
        assert_eq!(SuggestionKind::parse(k.as_str()), Some(k));
    }
    assert_eq!(SuggestionKind::parse("bogus"), None);
}

// === proptest (≥3) — 含成本契约反偷跑核心守卫 ===

mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn arb_ctx() -> impl Strategy<Value = SignalContext> {
        (
            prop::collection::vec(("[a-z ]{0,20}", 0u32..20), 0..5),
            0u32..30,
            prop::collection::vec((1usize..10, 0usize..4), 0..4),
            0u32..40,
            0u32..40,
        )
            .prop_map(|(misses, usm, clusters, browse, anno)| SignalContext {
                missed_queries: misses
                    .into_iter()
                    .map(|(q, c)| MissedQuery {
                        query: q,
                        miss_count: c,
                    })
                    .collect(),
                unprocessed_search_miss: usm,
                cluster_candidates: clusters
                    .into_iter()
                    .map(|(n, e)| ClusterCandidate {
                        item_ids: (0..n).map(|i| format!("it-{i}")).collect(),
                        top_entities: (0..e).map(|i| format!("ent-{i}")).collect(),
                    })
                    .collect(),
                browse_signal_count: browse,
                annotation_marker_count: anno,
                muted_kinds: vec![],
                dismissed_signatures: vec![],
            })
    }

    proptest! {
        /// ⭐ 反偷跑守卫:`evaluate` 对**任意**信号输入,产出的卡 cost_tier 永不让
        /// "卡生成本身"标成有偿。能为 Cloud 的只有 Retrieval(其 action 是点击后才调
        /// LLM 的显式 handler),其它确定性卡必须是 Free/Local。这从输出侧机器证明
        /// "生成建议卡 ≠ 调 LLM"。配合"本模块不 import LlmProvider"的编译期守卫
        /// (见 crate 级测试)构成成本契约双保险。
        #[test]
        fn evaluate_never_marks_data_cards_as_paid(ctx in arb_ctx()) {
            let cards = evaluate(&ctx);
            for c in &cards {
                match c.kind {
                    SuggestionKind::Retrieval => {
                        // 唯一允许 Cloud:点击后才调 LLM,卡上必带成本提示。
                        prop_assert_eq!(c.cost_tier, CostTier::Cloud);
                    }
                    _ => prop_assert_ne!(
                        c.cost_tier, CostTier::Cloud,
                        "non-retrieval card must not be Cloud-cost"
                    ),
                }
            }
        }

        /// 去重幂等:同一信号集多次 evaluate,signature 集合稳定。
        #[test]
        fn signatures_stable_across_runs(ctx in arb_ctx()) {
            let a: Vec<_> = evaluate(&ctx).into_iter().map(|c| c.signature).collect();
            let b: Vec<_> = evaluate(&ctx).into_iter().map(|c| c.signature).collect();
            prop_assert_eq!(a, b);
        }

        /// dismiss 后该 signature 不再出现。
        #[test]
        fn dismiss_removes_signature(ctx in arb_ctx()) {
            let cards = evaluate(&ctx);
            prop_assume!(!cards.is_empty());
            let target = cards[0].signature.clone();
            let mut ctx2 = ctx;
            ctx2.dismissed_signatures = vec![target.clone()];
            let after = evaluate(&ctx2);
            prop_assert!(after.iter().all(|c| c.signature != target));
        }

        /// evaluate 输出无重复 signature(去重不变量)。
        #[test]
        fn no_duplicate_signatures(ctx in arb_ctx()) {
            let cards = evaluate(&ctx);
            let mut sigs: Vec<_> = cards.iter().map(|c| c.signature.clone()).collect();
            let n = sigs.len();
            sigs.sort();
            sigs.dedup();
            prop_assert_eq!(sigs.len(), n);
        }
    }
}
