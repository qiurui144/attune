//! memory_consolidation_agent — property tests (≥3, per "Agent 验证铁律").
//!
//! Invariants verified across 100s of randomized inputs:
//!
//! 1. **Idempotency under re-run** — running the cycle twice on the same data
//!    yields zero new promotions on the second cycle (already_promoted == 1st-run insert).
//! 2. **Cap monotonicity** — `len(promoted with semantic_id) ≤ min(cap, MAX_PROMOTIONS_HARD_CAP)`
//!    for any candidate set.
//! 3. **Monotone score in access_count** — for fixed (chunk_count, recency_days),
//!    the score is strictly increasing in access_count.
//! 4. **Score function is finite** for any non-NaN inputs (recency_days >= 0).
//!
//! These are *invariants of the algorithm*, not just facts about specific
//! seedings. Property tests catch classes of bugs golden tests can't enumerate.

use attune_core::crypto::Key32;
use attune_core::memory::consolidation_agent::{
    compute_score, rank_candidates, run_promotion_cycle, Candidate, PromotionConfig,
    MAX_PROMOTIONS_HARD_CAP,
};
use attune_core::store::Store;
use proptest::prelude::*;

const NOW_SECS: i64 = 1_764_547_200;

// ── Strategy helpers ─────────────────────────────────────────────────────────

prop_compose! {
    fn arb_candidate(idx: u32)(
        chunk_count in 1usize..20,
        access_count in 0u32..50,
        recency_days in 0u32..120,
    ) -> Candidate {
        let chunks: Vec<String> = (0..chunk_count)
            .map(|i| format!("c-{idx:04}-{i:04}"))
            .collect();
        let created_at = NOW_SECS - (recency_days as i64) * 86400;
        Candidate {
            episodic_id: format!("ep-{idx:08}"),
            source_chunk_hashes: chunks,
            created_at,
            summary: format!("synthetic memory {idx}"),
            window_start: created_at,
            window_end: created_at + 86400,
            access_count,
        }
    }
}

fn arb_candidate_set(max_n: usize) -> impl Strategy<Value = Vec<Candidate>> {
    proptest::collection::vec(arb_candidate(0), 1..=max_n).prop_map(|cands| {
        cands
            .into_iter()
            .enumerate()
            .map(|(i, mut c)| {
                c.episodic_id = format!("ep-{i:08}");
                c.source_chunk_hashes = c
                    .source_chunk_hashes
                    .iter()
                    .map(|h| format!("{h}-{i}"))
                    .collect();
                c
            })
            .collect()
    })
}

prop_compose! {
    fn arb_config()(
        min_access in 0u32..10,
        min_score in (-2.0f64)..10.0f64,
        cap in 1usize..(MAX_PROMOTIONS_HARD_CAP * 2),
        window in 0u32..365,
    ) -> PromotionConfig {
        PromotionConfig {
            promotion_window_days: window,
            min_access_count: min_access,
            min_score,
            max_promotions_per_run: cap,
        }
    }
}

// ── Invariants ───────────────────────────────────────────────────────────────

proptest! {
    /// Invariant 1: cap is honored — pure `rank_candidates`.
    #[test]
    fn prop_rank_respects_cap(
        cands in arb_candidate_set(20),
        cfg in arb_config(),
    ) {
        let cap = cfg.max_promotions_per_run.min(MAX_PROMOTIONS_HARD_CAP);
        let (ranked, _, _) = rank_candidates(cands, &cfg, NOW_SECS);
        prop_assert!(ranked.len() <= cap, "ranked={} cap={}", ranked.len(), cap);
    }

    /// Invariant 2: score strictly monotone in access_count.
    #[test]
    fn prop_score_monotone_in_access(
        chunk_count in 1usize..50,
        recency_days in 0u32..200,
        a in 0u32..1_000_000,
    ) {
        let days = recency_days as f64;
        let s_a   = compute_score(a,   chunk_count, days);
        let s_a1  = compute_score(a+1, chunk_count, days);
        prop_assert!(s_a1 > s_a, "score({})={} not > score({})={}", a+1, s_a1, a, s_a);
    }

    /// Invariant 3: score function is always finite for non-negative recency_days,
    /// regardless of input magnitudes.
    #[test]
    fn prop_score_finite(
        access in 0u32..1_000_000,
        chunk_count in 0usize..10_000,
        recency_days in (0.0f64)..3650.0f64,
    ) {
        let s = compute_score(access, chunk_count, recency_days);
        prop_assert!(s.is_finite(), "non-finite score for ({access},{chunk_count},{recency_days})");
    }

    /// Invariant 4: rerun idempotency on real Store — running run_promotion_cycle
    /// twice on identical seeded data yields 0 new promotions on the second cycle.
    #[test]
    fn prop_full_cycle_idempotent(
        seeds in proptest::collection::vec(
            (1usize..6, 3u32..15),  // (chunk_count, access_count) per episodic
            1..6
        ),
    ) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // Seed N episodics, each with `access_count` citation_hits.
        for (i, (cc, ac)) in seeds.iter().enumerate() {
            let hashes: Vec<String> = (0..*cc).map(|j| format!("h-{i}-{j}")).collect();
            let created_at = NOW_SECS - 86400; // 1 day ago
            store.insert_memory(
                &dek, "episodic", created_at, created_at+86400,
                &hashes, "summary", "m", created_at
            ).unwrap();
            // Distribute citations across the first chunk
            for _ in 0..*ac {
                store.record_signal_event("citation_hit", &hashes[0], None).unwrap();
            }
        }
        let cfg = PromotionConfig {
            promotion_window_days: 7,
            min_access_count: 3,
            min_score: 4.0,
            max_promotions_per_run: 100,
        };
        let r1 = run_promotion_cycle(&store, &dek, &cfg, NOW_SECS, "v1").unwrap();
        let new1 = r1.promoted.iter().filter(|p| p.semantic_id.is_some()).count();
        let r2 = run_promotion_cycle(&store, &dek, &cfg, NOW_SECS, "v1").unwrap();
        let new2 = r2.promoted.iter().filter(|p| p.semantic_id.is_some()).count();
        prop_assert_eq!(new2, 0, "second cycle promoted {} new (expected 0); first cycle promoted {}", new2, new1);
        prop_assert!(r2.already_promoted >= new1,
                     "already_promoted on rerun ({}) should be >= first-cycle inserts ({})",
                     r2.already_promoted, new1);
    }
}
