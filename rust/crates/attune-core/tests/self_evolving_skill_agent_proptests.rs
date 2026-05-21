//! self_evolving_skill_agent — property tests (≥3, per "Agent 验证铁律").
//!
//! Invariants verified across 100s of randomized inputs:
//!
//! 1. **Idempotency under re-run** — running the cycle twice on the same data
//!    yields zero new rows on the second cycle (mark_signals_processed +
//!    upsert semantics guarantee this regardless of generation order).
//! 2. **Bounded output** — for any qualifying bucket, the persisted row's
//!    `expansions.len() ≤ MAX_EXPANSIONS_PER_PATTERN`. No matter how many
//!    co-occurring buckets exist, the cap holds.
//! 3. **Monotone in occurrence weight** — adding more occurrences to one
//!    bucket never decreases that bucket's contribution-rank in another
//!    target's heuristic output. Specifically: if bucket B was contributing
//!    token T to target's expansion at weight W, doubling B's occurrence
//!    count produces a weight 2W ≥ W → T is still ranked at least as high.
//!
//! These hit *invariants* not specific data points — they catch classes of
//! bugs that golden tests cannot enumerate.

use attune_core::skill_evolution::agent::{
    heuristic_expansion, run_cycle, QueryBucket, SkillAgentConfig,
};
use attune_core::store::{Store, MAX_EXPANSIONS_PER_PATTERN};
use proptest::prelude::*;

const NOW_SECS: i64 = 1_779_624_000;

// ── strategy helpers ─────────────────────────────────────────────────────────

prop_compose! {
    fn arb_query(idx: u32)(
        // 3-5 ASCII tokens, picked from a finite "shared vocabulary" so we
        // intentionally produce co-occurring buckets.
        prefix_idx in 0u32..6,
        suffix in "[a-z]{3,8}",
    ) -> String {
        let prefix = ["rust", "ssh", "tcp", "git", "elastic", "kubernetes"][prefix_idx as usize];
        format!("{prefix} {suffix} {idx}")
    }
}

prop_compose! {
    fn arb_bundle()(n in 3usize..15)(
        queries in proptest::collection::vec(arb_query(0), n..=n),
        counts in proptest::collection::vec(3u32..10, n..=n),
    ) -> Vec<(String, u32)> {
        queries.into_iter().zip(counts).collect()
    }
}

// ── invariant 1: idempotent re-run ───────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn idempotent_under_rerun(bundle in arb_bundle()) {
        let store = Store::open_memory().unwrap();
        // Seed signals.
        for (q, c) in &bundle {
            for _ in 0..*c {
                store.record_skill_signal(q, 0, false).unwrap();
            }
        }
        let cfg = SkillAgentConfig {
            window_days: 0,
            min_signal_count: 3,
            max_signals_per_cycle: 1000,
            enable_llm: false,
        };
        let stats1 = run_cycle(&store, None, &cfg, NOW_SECS).unwrap();
        // After cycle 1, all signals are processed → next cycle pulls 0.
        let stats2 = run_cycle(&store, None, &cfg, NOW_SECS).unwrap();
        prop_assert_eq!(stats2.signals_considered, 0,
            "second cycle must find no unprocessed signals; \
             first wrote {} rows", stats1.rows_written);
        prop_assert_eq!(stats2.rows_written, 0);
    }
}

// ── invariant 2: bounded output ──────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn expansions_never_exceed_cap(bundle in arb_bundle()) {
        let buckets: Vec<QueryBucket> = bundle.iter().enumerate().map(|(i, (q, c))| QueryBucket {
            query_pattern: q.to_lowercase(),
            occurrences: *c,
            signal_ids: vec![i as i64],
        }).collect();
        for bucket in &buckets {
            let exp = heuristic_expansion(&bucket.query_pattern, &buckets);
            prop_assert!(exp.len() <= MAX_EXPANSIONS_PER_PATTERN,
                "expansion cap breached: pattern={} got {} > {}",
                bucket.query_pattern, exp.len(), MAX_EXPANSIONS_PER_PATTERN);
            // Dedup invariant — no token appears twice.
            let mut sorted = exp.clone();
            sorted.sort();
            let original_len = sorted.len();
            sorted.dedup();
            prop_assert_eq!(sorted.len(), original_len, "duplicate expansion token");
        }
    }
}

// ── invariant 3: monotone in occurrence weight ───────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn monotone_in_occurrence_weight(
        // Two buckets sharing token "rust" so target's expansion considers them.
        suffix1 in "[a-z]{3,8}",
        suffix2 in "[a-z]{3,8}",
        base_weight in 3u32..20,
        boost in 1u32..50,
    ) {
        prop_assume!(suffix1 != suffix2);
        let target = format!("rust {suffix1}");
        let other = format!("rust {suffix2}");
        // Baseline.
        let base = vec![
            QueryBucket { query_pattern: target.clone(), occurrences: 3, signal_ids: vec![1] },
            QueryBucket { query_pattern: other.clone(),  occurrences: base_weight, signal_ids: vec![2] },
        ];
        let exp_base = heuristic_expansion(&target, &base);

        // Boost other bucket's weight.
        let boosted = vec![
            QueryBucket { query_pattern: target.clone(), occurrences: 3, signal_ids: vec![1] },
            QueryBucket { query_pattern: other.clone(),  occurrences: base_weight + boost, signal_ids: vec![2] },
        ];
        let exp_boost = heuristic_expansion(&target, &boosted);

        // If any token from other appeared in base, it must still appear (and be no later
        // than its previous position) in boost. Both produce the same set of candidate
        // tokens (suffix2 only) — they should be byte-for-byte equal.
        prop_assert_eq!(exp_base, exp_boost,
            "boosting other-bucket's occurrence count must not remove its contribution");
    }
}
