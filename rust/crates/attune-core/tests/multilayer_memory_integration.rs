//! Multi-layer memory integration test — full L0→L1→L2→L3 lifecycle on a real
//! `Store` (tempfile + MockLlm + MockEmbedding).
//!
//! See `docs/superpowers/plans/2026-05-18-multilayer-memory.md`.
//!
//! Verifies:
//! 1. episodic (L2) memories are embeddable + vector-searchable
//! 2. L2→L3 semantic clustering groups same-topic memories, is idempotent
//! 3. tier-aware assembler routes recall/overview to memory tiers, precise to L0
//! 4. cold demotion excludes old episodic memories covered by an L3 row
//! 5. assembler-off reproduces the L0 path (no regression)

use std::collections::HashMap;

use attune_core::crypto::Key32;
use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider};
use attune_core::memory::{
    apply_semantic_result, assemble_context, classify_query_shape, prepare_semantic_cycle,
    search_memories, MemoryConfig, MemoryVectorIndex, QueryShape,
};
use attune_core::search::SearchResult;
use attune_core::store::Store;

const DIM: usize = 256;
const DAY: i64 = 24 * 3600;

fn temp_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("vault.db")).unwrap();
    (store, dir)
}

/// Seed an episodic memory, return its id.
fn seed_episodic(store: &Store, dek: &Key32, hash: &str, summary: &str, win_start: i64) -> String {
    store
        .insert_memory(
            dek, "episodic", win_start, win_start + DAY, &[hash.into()], summary, "m", win_start,
        )
        .unwrap();
    store
        .list_recent_memories(dek, 1000)
        .unwrap()
        .into_iter()
        .find(|m| m.source_chunk_hashes == vec![hash])
        .unwrap()
        .id
}

/// Build the memory_id → embedding map + populate memory_vectors + index.
fn embed_all(
    store: &Store,
    dek: &Key32,
    emb: &dyn EmbeddingProvider,
    idx: &mut MemoryVectorIndex,
) -> HashMap<String, Vec<f32>> {
    let mut map = HashMap::new();
    for kind in ["episodic", "semantic"] {
        for m in store.list_live_memories(dek, kind, true).unwrap() {
            let v = emb.embed(&[m.summary.as_str()]).unwrap().pop().unwrap();
            store.put_memory_vector(&m.id, &v, "mock", 0).unwrap();
            idx.upsert(&m.id, &v).unwrap();
            map.insert(m.id, v);
        }
    }
    map
}

fn make_l0(n: usize) -> Vec<SearchResult> {
    (0..n)
        .map(|i| SearchResult {
            item_id: format!("doc-{i}"),
            score: 0.85 - (i as f32) * 0.05,
            title: format!("Document {i}"),
            content: "这是一段较长的原始文档 chunk 文本内容 ".repeat(30),
            source_type: "note".into(),
            ..Default::default()
        })
        .collect()
}

#[test]
fn full_lifecycle_l2_to_l3_and_assembler_routing() {
    let (store, _dir) = temp_store();
    let dek = Key32::generate();
    let emb = MockEmbeddingProvider::new(DIM);
    let mut idx = MemoryVectorIndex::new(DIM).unwrap();

    // L2: seed two clearly-separated topics, 6 episodic memories each.
    for i in 0..6 {
        seed_episodic(
            &store, &dek, &format!("rust-{i}"),
            "用户研究了 Rust 所有权 借用 生命周期 并发模型",
            (i as i64) * DAY,
        );
    }
    for i in 0..6 {
        seed_episodic(
            &store, &dek, &format!("cook-{i}"),
            "用户学习了 川菜 烹饪 火候 调味 刀工 技法",
            (10 + i as i64) * DAY,
        );
    }
    assert_eq!(store.memory_count_by_kind("episodic", false).unwrap(), 12);

    // Embed L2 → vector-searchable.
    let embeddings = embed_all(&store, &dek, &emb, &mut idx);
    assert_eq!(idx.len(), 12);

    // A recall query retrieves an episodic memory by relevance.
    let hits = search_memories(&store, &dek, &idx, &emb, "Rust 所有权", "episodic", None, 5).unwrap();
    assert!(!hits.is_empty(), "episodic memory must be vector-retrievable");

    // L2→L3: cluster + summarize. The semantic summary echoes the cluster's own
    // member text so that, under the deterministic token-hash mock embedder, the
    // L3 row stays close to a same-topic query (a real LLM summary would too).
    let clusters = prepare_semantic_cycle(&store, &dek, &embeddings)
        .unwrap()
        .expect("expected topic clusters from two separated topics");
    let summaries: Vec<Option<String>> = clusters
        .iter()
        .map(|c| {
            let topic_terms = c.member_summaries.first().cloned().unwrap_or_default();
            Some(format!("用户对该主题形成了系统认知：{topic_terms}"))
        })
        .collect();
    let (apply_res, _) =
        apply_semantic_result(&store, &dek, &clusters, &summaries, "m", 100 * DAY).unwrap();
    assert!(apply_res.inserted >= 1, "at least one L3 semantic memory");

    // L3 idempotent — rerun yields nothing new.
    let rerun = prepare_semantic_cycle(&store, &dek, &embeddings).unwrap();
    assert!(rerun.is_none(), "L3 build must be idempotent on rerun");

    // Embed the new L3 rows so they are searchable.
    let mut idx2 = MemoryVectorIndex::new(DIM).unwrap();
    embed_all(&store, &dek, &emb, &mut idx2);

    // Tier-aware assembler: overview query → L3 semantic.
    let l0 = make_l0(5);
    let overview = assemble_context(
        &store, &dek, &idx2, &emb,
        "总结 用户研究了 Rust 所有权 借用 生命周期 并发模型",
        &l0, MemoryConfig::default(),
    )
    .unwrap();
    assert_eq!(overview.shape, QueryShape::Overview);
    assert_eq!(overview.tier_used, "L3", "overview query routes to semantic tier");
}

#[test]
fn precise_query_stays_on_l0() {
    let (store, _dir) = temp_store();
    let dek = Key32::generate();
    let emb = MockEmbeddingProvider::new(DIM);
    let mut idx = MemoryVectorIndex::new(DIM).unwrap();
    for i in 0..6 {
        seed_episodic(&store, &dek, &format!("h-{i}"), "用户研究了 Rust 所有权", (i as i64) * DAY);
    }
    embed_all(&store, &dek, &emb, &mut idx);

    let l0 = make_l0(5);
    let out = assemble_context(
        &store, &dek, &idx, &emb,
        "`MemoryVectorIndex::upsert` 的返回类型是什么 / 函数签名",
        &l0, MemoryConfig::default(),
    )
    .unwrap();
    assert_eq!(out.shape, QueryShape::Precise);
    assert_eq!(out.tier_used, "L0", "precise query must never leave L0");
    assert_eq!(out.blocks.len(), 5);
}

#[test]
fn assembler_off_equals_l0_path() {
    let (store, _dir) = temp_store();
    let dek = Key32::generate();
    let emb = MockEmbeddingProvider::new(DIM);
    let idx = MemoryVectorIndex::new(DIM).unwrap();

    let l0 = make_l0(4);
    let off = MemoryConfig { tiered_assembler_enabled: false, memory_confidence: 0.70 };
    let out = assemble_context(&store, &dek, &idx, &emb, "总结一下我学了什么", &l0, off).unwrap();
    // Assembler off → byte-for-byte the L0 results.
    assert_eq!(out.tier_used, "L0");
    assert_eq!(out.blocks.len(), 4);
    for (b, r) in out.blocks.iter().zip(l0.iter()) {
        assert_eq!(b.item_id, r.item_id);
        assert_eq!(b.content, r.content);
        assert_eq!(b.tier, "L0");
    }
}

#[test]
fn cold_demotion_excludes_old_covered_episodic() {
    let (store, _dir) = temp_store();
    let dek = Key32::generate();
    let emb = MockEmbeddingProvider::new(DIM);
    let mut idx = MemoryVectorIndex::new(DIM).unwrap();

    // Old episodic memory (window_end well in the past).
    let old_id = seed_episodic(&store, &dek, "old", "用户研究了 Rust 所有权", DAY);
    // A fresh episodic memory that must stay hot.
    let now = 400 * DAY;
    let fresh_id = seed_episodic(&store, &dek, "fresh", "用户研究了 tokio 调度", now - DAY);
    // A semantic memory covering the old period.
    store
        .insert_semantic_memory(&dek, "topic", &["old".into()], "standing summary", "m", 0, DAY, 10 * DAY)
        .unwrap();
    embed_all(&store, &dek, &emb, &mut idx);

    let demoted = store.demote_cold_memories(now, 180 * DAY).unwrap();
    assert_eq!(demoted, 1, "only the old+covered episodic should be demoted");

    // Hot retrieval excludes the cold memory.
    let hot = store.list_live_memories(&dek, "episodic", false).unwrap();
    let hot_ids: Vec<&str> = hot.iter().map(|m| m.id.as_str()).collect();
    assert!(hot_ids.contains(&fresh_id.as_str()));
    assert!(!hot_ids.contains(&old_id.as_str()), "cold memory excluded from hot set");

    // Vector search over episodic also excludes it.
    let hits = search_memories(&store, &dek, &idx, &emb, "Rust 所有权", "episodic", None, 5).unwrap();
    assert!(hits.iter().all(|h| h.memory.id != old_id), "cold memory not vector-retrieved");
}

#[test]
fn query_shape_classification_table() {
    // Recall — time words.
    assert_eq!(classify_query_shape("昨天的 rust 笔记"), QueryShape::Recall);
    // Overview — broad marker.
    assert_eq!(classify_query_shape("总结我对算法的理解"), QueryShape::Overview);
    // Overview — short topic.
    assert_eq!(classify_query_shape("分布式系统"), QueryShape::Overview);
    // Precise — code identifier.
    assert_eq!(classify_query_shape("`assemble_context` 怎么用"), QueryShape::Precise);
}
