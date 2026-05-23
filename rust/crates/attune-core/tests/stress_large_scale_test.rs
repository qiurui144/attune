//! 大规模 stress test — 验证大量数据下的延迟与召回率。
//!
//! 跑法（slow lane，需 --ignored + --release 建议）：
//!   cargo test -p attune-core --release --test stress_large_scale_test -- --nocapture --ignored
//!
//! 所有测试默认 #[ignore]，不进日常 CI。
//!
//! Mock 策略：
//! - embedding 用 64-dim seeded deterministic random vector（不调 Ollama）
//! - VectorIndex 用内存模式（不写磁盘）

use attune_core::crypto::Key32;
use attune_core::index::FulltextIndex;
use attune_core::reindex;
use attune_core::store::Store;
use attune_core::vectors::{VectorIndex, VectorMeta};
use std::time::Instant;
use tempfile::TempDir;

// ──────────────────────────────────────────────────────────────────────────────
// helpers
// ──────────────────────────────────────────────────────────────────────────────

/// 确定性 64-dim 向量（seed = item 编号），满足 usearch cosine 输入。
fn mock_vector_64(seed: u64) -> Vec<f32> {
    // LCG: x_{n+1} = (a * x_n + c) % m
    let mut x = seed.wrapping_add(1);
    let mut v = Vec::with_capacity(64);
    for _ in 0..64 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        // map to [-1, 1]
        let f = ((x >> 32) as f32) / (u32::MAX as f32) * 2.0 - 1.0;
        v.push(f);
    }
    // l2-normalize for cosine
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn make_doc_content(idx: usize) -> String {
    format!(
        "# Document {idx}\n\n\
         This document is about topic {idx}. \
         It contains keywords such as rust, performance, indexing, search, and memory. \
         Section content for comprehensive full-text retrieval testing at scale.\n\n\
         ## Details\n\nAdditional paragraph with index {idx} for embedding coverage.\n"
    )
}

/// 向 VectorIndex 批量写入 n 个确定性向量（dims=64）
fn bulk_insert_vectors(vectors: &mut VectorIndex, n: usize) {
    for i in 0..n {
        let v = mock_vector_64(i as u64);
        let meta = VectorMeta {
            item_id: format!("item-{i}"),
            chunk_idx: 0,
            level: 2,
            section_idx: 0,
        };
        vectors.add(&v, meta).unwrap();
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 1: insert 10000 items + full reindex → FTS search latency P99 < 500ms
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn stress_10k_items_fts_search_latency_p99_under_500ms() {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("v.db")).unwrap();
    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open(&tmp.path().join("ft")).unwrap();
    let dek = Key32::generate();

    const N: usize = 10_000;
    println!("\n[stress] inserting {N} items...");
    let t_insert = Instant::now();

    for i in 0..N {
        let content = make_doc_content(i);
        let id = store
            .insert_item(&dek, &format!("Doc {i}"), &content, None, "note", None, None)
            .unwrap();
        // reindex for FTS (skip embedding write — queue only)
        reindex::reindex_item(&store, &mut vectors, &fulltext, &id, &format!("Doc {i}"), &content, "note")
            .unwrap();
    }
    let insert_ms = t_insert.elapsed().as_millis();
    println!("[stress] insert+reindex {N} items: {insert_ms}ms");

    let count = store.item_count().unwrap();
    assert_eq!(count, N, "item count 必须 = {N}，实际 = {count}");

    // FTS search latency benchmark — 100 queries，取 P99
    let queries = [
        "rust performance indexing",
        "search memory retrieval",
        "document content keywords",
        "topic section paragraph",
        "embedding coverage testing",
    ];
    let mut latencies_ms: Vec<u64> = Vec::with_capacity(100);
    for _ in 0..20 {
        for q in &queries {
            let t0 = Instant::now();
            let results = fulltext.search(q, 10).unwrap();
            let elapsed = t0.elapsed().as_millis() as u64;
            latencies_ms.push(elapsed);
            let _ = results; // consume
        }
    }
    latencies_ms.sort_unstable();
    let p50 = latencies_ms[latencies_ms.len() / 2];
    let p99 = latencies_ms[(latencies_ms.len() * 99) / 100];
    println!("[stress] FTS search latency — P50={p50}ms P99={p99}ms (100 queries)");

    assert!(
        p99 < 500,
        "FTS P99 latency must be < 500ms, actual P99={p99}ms"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 2: vector index 10万+ vectors → 召回率不退化（top-k 命中已知向量）
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn stress_100k_vectors_recall_does_not_degrade() {
    const N: usize = 100_000;
    println!("\n[stress] building vector index with {N} vectors (64-dim)...");

    let mut vectors = VectorIndex::new(64).unwrap();
    let t_build = Instant::now();
    bulk_insert_vectors(&mut vectors, N);
    let build_ms = t_build.elapsed().as_millis();
    println!("[stress] index build: {build_ms}ms, size={}", vectors.len());
    assert_eq!(vectors.len(), N);

    // 精确召回率测试：查询 20 个已知向量，top-1 应命中自身（cosine distance ≈ 0）
    let mut hits = 0usize;
    let probe_indices: Vec<usize> = (0..20).map(|i| i * (N / 20)).collect();
    for &idx in &probe_indices {
        let query = mock_vector_64(idx as u64);
        let results = vectors.search(&query, 5).unwrap();
        // top result 应该是 item-{idx}（相同向量）
        if let Some((meta, score)) = results.first() {
            if meta.item_id == format!("item-{idx}") && *score > 0.99 {
                hits += 1;
            }
        }
    }
    let recall = hits as f64 / probe_indices.len() as f64;
    println!("[stress] recall@1 over {} probes: {:.1}%", probe_indices.len(), recall * 100.0);
    assert!(
        recall >= 0.8,
        "10万向量下 recall@1 必须 >= 80%，实际 = {:.1}%",
        recall * 100.0
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 3: tantivy fulltext 10万+ docs → query latency P95 < 200ms
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn stress_100k_docs_fulltext_query_latency_p95_under_200ms() {
    const N: usize = 100_000;
    println!("\n[stress] indexing {N} docs into fulltext...");

    let fulltext = FulltextIndex::open_memory().unwrap();
    let t_build = Instant::now();
    for i in 0..N {
        fulltext
            .add_document(
                &format!("item-{i}"),
                &format!("Document {i}"),
                &make_doc_content(i),
                "note",
            )
            .unwrap();
    }
    let build_ms = t_build.elapsed().as_millis();
    println!("[stress] fulltext build {N} docs: {build_ms}ms");

    let doc_count = fulltext.doc_count().unwrap();
    assert!(doc_count >= N / 2, "doc count 应接近 {N}，实际 = {doc_count}");

    // 50 queries，取 P95
    let queries = [
        "rust performance",
        "indexing search",
        "memory retrieval",
        "document content",
        "section paragraph",
    ];
    let mut latencies_ms: Vec<u64> = Vec::with_capacity(50);
    for _ in 0..10 {
        for q in &queries {
            let t0 = Instant::now();
            let results = fulltext.search(q, 20).unwrap();
            let elapsed = t0.elapsed().as_millis() as u64;
            latencies_ms.push(elapsed);
            let _ = results;
        }
    }
    latencies_ms.sort_unstable();
    let p50 = latencies_ms[latencies_ms.len() / 2];
    let p95 = latencies_ms[(latencies_ms.len() * 95) / 100];
    println!("[stress] FTS latency (100k docs) — P50={p50}ms P95={p95}ms");

    assert!(
        p95 < 200,
        "100k docs FTS P95 latency must be < 200ms, actual={p95}ms"
    );
}
