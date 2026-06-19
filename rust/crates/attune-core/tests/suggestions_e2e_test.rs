//! 零成本建议引擎 E2E 集成测试(能力 A §9 集成 E2E 下限)。
//!
//! 模拟 route 层路径:注入确定性信号 → 从 Store 聚合成 SignalContext → evaluate
//! 出卡 → dismiss → 复查消失。这是 route::suggestions::list 内部逻辑的端到端验证
//! (不起 HTTP server,直接走 Store + 引擎,与 route 同一组调用)。
//!
//! 另含**成本契约源级守卫**:断言 suggestions 模块源码不 import LlmProvider —— 编译期
//! 它就拿不到 LLM 句柄,机器可执行地证明"生成建议卡 ≠ 调 LLM"。

use attune_core::store::Store;
use attune_core::suggestions::{
    self, ClusterCandidate, SignalContext, ENRICH_MISS_THRESHOLD, SuggestionKind,
};

/// 端到端:注入 search_miss + browse 信号 → 聚合 → 出卡 → dismiss → 消失。
#[test]
fn e2e_signals_to_cards_then_dismiss() {
    let store = Store::open_memory().unwrap();

    // 注入信号(模拟用户行为)。
    for _ in 0..4 {
        store.record_skill_signal("riscv vector intrinsics", 0, false).unwrap();
    }
    for i in 0..12 {
        store
            .record_signal_event("annotation_marker", &format!("anno-{i}"), None)
            .unwrap();
    }

    // === route 同款聚合 ===
    let build_ctx = |s: &Store| SignalContext {
        missed_queries: s
            .aggregate_missed_queries(ENRICH_MISS_THRESHOLD, 20)
            .unwrap(),
        unprocessed_search_miss: s.count_unprocessed_signals().unwrap() as u32,
        cluster_candidates: vec![ClusterCandidate {
            item_ids: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            top_entities: vec!["项目X".into()],
        }],
        browse_signal_count: s.browse_signals_count().unwrap() as u32,
        annotation_marker_count: s
            .count_unprocessed_signals_by_kind("annotation_marker")
            .unwrap() as u32,
        muted_kinds: s.list_muted_suggestion_kinds().unwrap(),
        dismissed_signatures: s.list_dismissed_signatures().unwrap(),
    };

    let cards = suggestions::evaluate(&build_ctx(&store));
    // 至少出 enrich(query×4)+ retrieval(4 unprocessed < 5? — record_skill_signal 4 条
    // 都是 search_miss 未处理 → unprocessed=4 < 5,retrieval 不出)+ organize + profile(12)。
    let kinds: Vec<_> = cards.iter().map(|c| c.kind).collect();
    assert!(kinds.contains(&SuggestionKind::Enrich), "expected enrich, got {kinds:?}");
    assert!(kinds.contains(&SuggestionKind::Organize), "expected organize");
    assert!(kinds.contains(&SuggestionKind::Profile), "expected profile (12 anno ≥ 10)");

    // dismiss enrich 卡 → 持久化 → 复查应消失。
    let enrich = cards.iter().find(|c| c.kind == SuggestionKind::Enrich).unwrap();
    store.dismiss_suggestion(&enrich.signature).unwrap();

    let after = suggestions::evaluate(&build_ctx(&store));
    assert!(
        after.iter().all(|c| c.kind != SuggestionKind::Enrich),
        "dismissed enrich card must not reappear"
    );
}

/// mute 持久化端到端:mute organize → 复查无 organize 卡 → unmute → 恢复。
#[test]
fn e2e_mute_persists_across_evaluate() {
    let store = Store::open_memory().unwrap();
    let cluster = ClusterCandidate {
        item_ids: vec!["x".into(), "y".into(), "z".into()],
        top_entities: vec![],
    };
    let build = |s: &Store| SignalContext {
        cluster_candidates: vec![cluster.clone()],
        muted_kinds: s.list_muted_suggestion_kinds().unwrap(),
        dismissed_signatures: s.list_dismissed_signatures().unwrap(),
        ..Default::default()
    };

    assert_eq!(suggestions::evaluate(&build(&store)).len(), 1);
    store.mute_suggestion_kind("organize").unwrap();
    assert!(suggestions::evaluate(&build(&store)).is_empty(), "muted kind suppressed");
    store.unmute_suggestion_kind("organize").unwrap();
    assert_eq!(suggestions::evaluate(&build(&store)).len(), 1, "unmute restores");
}

/// ⭐ 成本契约源级守卫:suggestions 模块源码不得 import LLM provider。
/// 编译期它就无法持有 LLM 句柄 → "为生成建议卡而调 LLM"不可能发生。
/// 这是成本契约最高风险项(spec §11)的机器可执行守卫,进 CI 硬门。
#[test]
fn cost_contract_no_llm_import_in_suggestions_module() {
    let src = include_str!("../src/suggestions/mod.rs");
    // 注释里可以提及 LlmProvider(解释为什么不用),但不得有真 import / 类型引用。
    for (lineno, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        // 跳过注释行(// 或 //! 或 块注释内的 *)。
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        assert!(
            !trimmed.contains("LlmProvider"),
            "suggestions/mod.rs line {} references LlmProvider in code (cost-contract violation): {}",
            lineno + 1,
            line
        );
        assert!(
            !(trimmed.starts_with("use ") && trimmed.contains("llm")),
            "suggestions/mod.rs line {} imports an llm module (cost-contract violation): {}",
            lineno + 1,
            line
        );
    }
}
