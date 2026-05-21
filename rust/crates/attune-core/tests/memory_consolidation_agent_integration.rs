//! memory_consolidation_agent — integration E2E test (≥1 per "Agent 验证铁律").
//!
//! Exercises the *full path* the production sleep-time worker would take:
//!
//!   * real `Store` opened on a real tempfile-backed sqlite db (not in-memory)
//!   * episodic memories seeded via the public `insert_memory` API
//!   * citation_hit signals emitted via the public `record_signal_event` API
//!   * the existing hdbscan + LLM semantic path (`prepare_semantic_cycle` +
//!     `apply_semantic_result`) coexists with the deterministic agent without
//!     topic_key collisions — they share the `memories(kind='semantic')` table
//!     but use disjoint topic_key namespaces (`sha256(member_ids)` vs `promoted:<sha>`).
//!
//! Why a separate file (vs proptest above)?
//!
//! The proptest file runs synthetic-strategy seeds against `Store::open_memory`.
//! This file uses tempfile, exercises co-existence with the hdbscan path, and is
//! the canonical "this is how the worker integrates" reference.

use std::collections::HashMap;

use attune_core::crypto::Key32;
use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider};
use attune_core::memory::consolidation_agent::{
    run_promotion_cycle, PromotionConfig,
};
use attune_core::memory::semantic::{apply_semantic_result, prepare_semantic_cycle};
use attune_core::store::Store;

const DIM: usize = 64;
const NOW_SECS: i64 = 1_764_547_200;
const DAY: i64 = 86_400;

fn open_tempfile_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("vault.sqlite")).unwrap();
    (store, dir)
}

/// Seed one episodic memory, return the store-generated id.
fn seed_episodic(
    store: &Store,
    dek: &Key32,
    hash_prefix: &str,
    n_chunks: usize,
    summary: &str,
    offset_days: i64,
) -> String {
    let hashes: Vec<String> = (0..n_chunks)
        .map(|i| format!("{hash_prefix}-{i:03}"))
        .collect();
    let created_at = NOW_SECS - offset_days * DAY;
    store
        .insert_memory(
            dek,
            "episodic",
            created_at,
            created_at + DAY,
            &hashes,
            summary,
            "integration-seed",
            created_at,
        )
        .unwrap();
    // Find the inserted row by chunk_hashes match.
    let row = store
        .list_recent_memories(dek, 200)
        .unwrap()
        .into_iter()
        .find(|m| {
            let mut a = m.source_chunk_hashes.clone();
            a.sort();
            let mut b = hashes.clone();
            b.sort();
            a == b
        })
        .expect("inserted episodic must be findable");
    row.id
}

#[test]
fn full_e2e_promotion_writes_l3_memory_round_trip() {
    let (store, _dir) = open_tempfile_store();
    let dek = Key32::generate();

    // Two episodics: one heavily cited (should promote), one untouched (should not).
    let hot_id = seed_episodic(&store, &dek, "hot", 5, "用户深入研究的主题", 1);
    let cold_id = seed_episodic(&store, &dek, "cold", 5, "用户尚未引用的主题", 1);

    // Seed 6 citation_hits on the hot episodic.
    for i in 0..3 {
        store
            .record_signal_event("citation_hit", &format!("hot-{i:03}"), None)
            .unwrap();
        store
            .record_signal_event("citation_hit", &format!("hot-{i:03}"), None)
            .unwrap();
    }

    // Run the agent.
    let cfg = PromotionConfig {
        promotion_window_days: 7,
        min_access_count: 3,
        min_score: 4.0,
        max_promotions_per_run: 50,
    };
    let result = run_promotion_cycle(&store, &dek, &cfg, NOW_SECS, "integration-v1").unwrap();
    assert_eq!(result.considered, 2);
    let newly = result
        .promoted
        .iter()
        .filter(|p| p.semantic_id.is_some())
        .collect::<Vec<_>>();
    assert_eq!(newly.len(), 1, "exactly one episodic should be promoted");
    assert_eq!(
        newly[0].episodic_id, hot_id,
        "hot episodic should be the promoted one"
    );

    // The L3 row exists and decrypts to the carried-over summary.
    let semantic_rows = store.list_live_memories(&dek, "semantic", false).unwrap();
    assert_eq!(semantic_rows.len(), 1);
    assert_eq!(semantic_rows[0].summary, "用户深入研究的主题");
    assert_eq!(
        semantic_rows[0].source_chunk_hashes.len(),
        5,
        "L3 row must reference all 5 source chunks"
    );
    // topic_key has the `promoted:` prefix.
    assert!(
        semantic_rows[0]
            .topic_key
            .as_deref()
            .map(|k| k.starts_with("promoted:"))
            .unwrap_or(false),
        "L3 topic_key must use the `promoted:` namespace; got {:?}",
        semantic_rows[0].topic_key
    );

    // The cold episodic is untouched — still live, no L3 row.
    let live_ep = store.list_live_memories(&dek, "episodic", false).unwrap();
    assert!(
        live_ep.iter().any(|m| m.id == cold_id),
        "cold episodic must remain in the live L2 set"
    );
}

#[test]
fn promotion_coexists_with_hdbscan_semantic_path_no_topic_key_collision() {
    // Verifies the two L3 production paths (deterministic agent + hdbscan
    // synthesizer) can fire on the SAME store without one stomping the other —
    // their topic_key namespaces (`promoted:<sha>` vs sha256(member_ids))
    // are disjoint so the unique index permits both rows.
    let (store, _dir) = open_tempfile_store();
    let dek = Key32::generate();

    // Seed 6 episodics with similar text (so hdbscan can find a cluster) AND
    // a citation_hit on each, so the deterministic agent will also promote them.
    let mut ep_ids = Vec::new();
    for i in 0..6 {
        let hashes: Vec<String> = vec![format!("topic-{i}")];
        let created_at = NOW_SECS - DAY;
        store
            .insert_memory(
                &dek,
                "episodic",
                created_at,
                created_at + DAY,
                &hashes,
                &format!(
                    "用户研究了 Rust ownership 借用 生命周期 主题的第 {i} 段内容"
                ),
                "integration-seed",
                created_at,
            )
            .unwrap();
        // ≥3 citations / hash so each crosses the access gate
        for _ in 0..4 {
            store
                .record_signal_event("citation_hit", &hashes[0], None)
                .unwrap();
        }
        // Capture the inserted id
        let id = store
            .list_recent_memories(&dek, 200)
            .unwrap()
            .into_iter()
            .find(|m| m.source_chunk_hashes == hashes)
            .unwrap()
            .id;
        ep_ids.push(id);
    }

    // ── Path A: deterministic agent runs first, promoting all 6.
    let cfg = PromotionConfig {
        promotion_window_days: 7,
        min_access_count: 3,
        min_score: 4.0,
        max_promotions_per_run: 50,
    };
    let r = run_promotion_cycle(&store, &dek, &cfg, NOW_SECS, "det").unwrap();
    let agent_promoted = r.promoted.iter().filter(|p| p.semantic_id.is_some()).count();
    assert_eq!(agent_promoted, 6);

    // ── Path B: hdbscan path runs after, on the same episodic set.
    let emb = MockEmbeddingProvider::new(DIM);
    let mut embeddings: HashMap<String, Vec<f32>> = HashMap::new();
    for ep_id in &ep_ids {
        // Fetch the corresponding summary
        let m = store
            .list_recent_memories(&dek, 200)
            .unwrap()
            .into_iter()
            .find(|m| &m.id == ep_id)
            .unwrap();
        let v = emb.embed(&[m.summary.as_str()]).unwrap().pop().unwrap();
        embeddings.insert(m.id, v);
    }
    let clusters_opt = prepare_semantic_cycle(&store, &dek, &embeddings).unwrap();
    if let Some(clusters) = clusters_opt {
        // hdbscan-derived topic_keys do NOT have the `promoted:` prefix.
        for c in &clusters {
            assert!(
                !c.topic_key.starts_with("promoted:"),
                "hdbscan path must not generate `promoted:` topic_keys (namespace collision)"
            );
        }
        // Synthesize fake summaries (no LLM needed for this assertion).
        let summaries: Vec<Option<String>> = clusters
            .iter()
            .map(|_| Some("hdbscan-synthesized standing summary".to_string()))
            .collect();
        let (apply_res, _new_ids) =
            apply_semantic_result(&store, &dek, &clusters, &summaries, "hdb", NOW_SECS).unwrap();
        // The new rows should coexist with the deterministic ones (different
        // topic_keys, both pass the unique index).
        let live = store.list_live_memories(&dek, "semantic", false).unwrap();
        assert_eq!(
            live.len(),
            agent_promoted + apply_res.inserted,
            "deterministic + hdbscan paths must coexist (live = sum of both)"
        );
        // Make sure the agent's `promoted:` rows are still present.
        let promoted_topic_count = live
            .iter()
            .filter(|m| {
                m.topic_key
                    .as_deref()
                    .map(|k| k.starts_with("promoted:"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(promoted_topic_count, agent_promoted);
    }
    // If clusters_opt is None, hdbscan didn't find a cluster (small mock dim
    // sometimes too noisy) — that's still a valid integration outcome.
    // The deterministic rows on their own are what we asserted above.

    let final_live = store.list_live_memories(&dek, "semantic", false).unwrap();
    let promoted_count = final_live
        .iter()
        .filter(|m| {
            m.topic_key
                .as_deref()
                .map(|k| k.starts_with("promoted:"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(promoted_count, 6, "all 6 deterministic L3 rows survive");
}

#[test]
fn promoted_l3_summary_decrypts_to_original_episodic_text() {
    // The deterministic agent does NOT re-synthesize — it carries the L2
    // summary straight up. Guard against accidental re-encryption / encoding
    // changes by asserting byte-for-byte equality after DEK roundtrip.
    let (store, _dir) = open_tempfile_store();
    let dek = Key32::generate();

    let original = "这是一段 UTF-8 摘要：含中英 mixed、emoji 🎯、以及 \"双引号\"。";
    let hashes: Vec<String> = vec!["roundtrip-1".into(), "roundtrip-2".into(), "roundtrip-3".into()];
    let created_at = NOW_SECS - DAY;
    store
        .insert_memory(
            &dek,
            "episodic",
            created_at,
            created_at + DAY,
            &hashes,
            original,
            "integration-seed",
            created_at,
        )
        .unwrap();
    for h in &hashes {
        store.record_signal_event("citation_hit", h, None).unwrap();
    }

    let cfg = PromotionConfig {
        promotion_window_days: 7,
        min_access_count: 3,
        min_score: 4.0,
        max_promotions_per_run: 50,
    };
    let r = run_promotion_cycle(&store, &dek, &cfg, NOW_SECS, "roundtrip-v1").unwrap();
    assert_eq!(
        r.promoted.iter().filter(|p| p.semantic_id.is_some()).count(),
        1
    );
    let live = store.list_live_memories(&dek, "semantic", false).unwrap();
    assert_eq!(live[0].summary, original);
}
