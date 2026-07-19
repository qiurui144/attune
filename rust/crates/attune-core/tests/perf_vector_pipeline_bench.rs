//! Attune-owned vector pipeline perf probes.
//!
//! Run with:
//! `cargo test -p attune-core --release --test perf_vector_pipeline_bench -- --ignored --nocapture`
//!
//! These benches intentionally use deterministic vectors / MockEmbeddingProvider
//! so scheduler and model inference latency stay out of Attune core measurements.

use attune_core::crypto::Key32;
use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider};
use attune_core::index::FulltextIndex;
use attune_core::queue::{index_embedding_results, QueueWorker};
use attune_core::store::Store;
use attune_core::vectors::{VectorIndex, VectorMeta};
use std::time::Instant;
use tempfile::TempDir;

const DIMS: usize = 256;
const CHUNKS: usize = 2048;

fn setup() -> (TempDir, Store, FulltextIndex, VectorIndex, Key32) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("attune.db")).unwrap();
    let fulltext = FulltextIndex::open(&tmp.path().join("fulltext")).unwrap();
    let vectors = VectorIndex::new(DIMS).unwrap();
    let dek = Key32::generate();
    (tmp, store, fulltext, vectors, dek)
}

fn bench_text(i: usize) -> String {
    format!(
        "vector pipeline bench chunk {i} topic-{} bucket-{} \
         retrieval embedding queue writeback attune core deterministic payload",
        i % 37,
        i % 11
    )
}

fn enqueue_chunks(store: &Store, dek: &Key32, chunks: usize) -> String {
    let item_id = store
        .insert_item(
            dek,
            "vector pipeline bench corpus",
            "synthetic corpus for queue and vector index performance",
            None,
            "perf",
            None,
            None,
        )
        .unwrap();
    for i in 0..chunks {
        let level = if i % 8 == 0 { 1 } else { 2 };
        store
            .enqueue_embedding(&item_id, i, &bench_text(i), 1, level, i / 8)
            .unwrap();
    }
    item_id
}

fn unit_vector(seed: u64, dims: usize) -> Vec<f32> {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut out = vec![0.0f32; dims];
    for value in &mut out {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let raw = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        *value = ((raw % 2001) as f32 - 1000.0) / 1000.0;
    }
    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut out {
            *value /= norm;
        }
    } else {
        out[0] = 1.0;
    }
    out
}

fn percentile(mut samples: Vec<f64>, q: f64) -> f64 {
    assert!(!samples.is_empty());
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = (((samples.len() - 1) as f64) * q).ceil() as usize;
    samples[idx]
}

#[test]
#[ignore]
fn bench_precomputed_embedding_writeback() {
    let (_tmp, store, fulltext, mut vectors, dek) = setup();
    enqueue_chunks(&store, &dek, CHUNKS);
    assert_eq!(store.pending_embedding_count().unwrap(), CHUNKS);

    let tasks = store.dequeue_embeddings(CHUNKS).unwrap();
    assert_eq!(tasks.len(), CHUNKS);
    let embeddings: Vec<Vec<f32>> = (0..tasks.len())
        .map(|i| unit_vector(i as u64, DIMS))
        .collect();

    let started = Instant::now();
    let done_ids =
        index_embedding_results(&store, &mut vectors, &fulltext, &tasks, &embeddings).unwrap();
    for id in done_ids {
        store.mark_embedding_done(id).unwrap();
    }
    let elapsed = started.elapsed();

    assert_eq!(vectors.len(), CHUNKS);
    assert_eq!(store.pending_embedding_count().unwrap(), 0);
    println!("\n=== precomputed embedding writeback ===");
    println!("chunks              | {CHUNKS}");
    println!(
        "elapsed_ms          | {:.2}",
        elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "chunks_per_second   | {:.2}",
        CHUNKS as f64 / elapsed.as_secs_f64()
    );
}

#[test]
#[ignore]
fn bench_mock_embedding_queue_process_all() {
    let (_tmp, store, fulltext, mut vectors, dek) = setup();
    enqueue_chunks(&store, &dek, CHUNKS);

    let provider = MockEmbeddingProvider::new(DIMS);
    let texts: Vec<String> = (0..CHUNKS).map(bench_text).collect();
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();

    let embed_started = Instant::now();
    let (mock_vectors, _usage) = provider.embed(&text_refs).unwrap();
    let embed_elapsed = embed_started.elapsed();
    assert_eq!(mock_vectors.len(), CHUNKS);

    let process_started = Instant::now();
    let processed = QueueWorker::process_all(&store, &provider, &mut vectors, &fulltext).unwrap();
    let process_elapsed = process_started.elapsed();

    assert_eq!(processed, CHUNKS);
    assert_eq!(vectors.len(), CHUNKS);
    assert_eq!(store.pending_embedding_count().unwrap(), 0);
    let (query_vecs, _usage) = provider
        .embed(&["vector pipeline topic-7 writeback"])
        .unwrap();
    assert!(!vectors.search(&query_vecs[0], 10).unwrap().is_empty());

    println!("\n=== mock embedding + queue process_all ===");
    println!("chunks                    | {CHUNKS}");
    println!(
        "mock_embed_elapsed_ms     | {:.2}",
        embed_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "mock_embed_chunks_per_sec | {:.2}",
        CHUNKS as f64 / embed_elapsed.as_secs_f64()
    );
    println!(
        "queue_elapsed_ms          | {:.2}",
        process_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "queue_chunks_per_sec      | {:.2}",
        CHUNKS as f64 / process_elapsed.as_secs_f64()
    );
}

#[test]
#[ignore]
fn bench_vector_index_add_search() {
    println!("\n=== VectorIndex add/search ===");
    println!("vectors | add_ms | add_per_sec | search_p50_ms | search_p95_ms");

    for count in [1_000usize, 10_000, 20_000] {
        let mut vectors = VectorIndex::new(DIMS).unwrap();

        let add_started = Instant::now();
        for i in 0..count {
            vectors
                .add(
                    &unit_vector(i as u64, DIMS),
                    VectorMeta {
                        item_id: format!("doc-{i}"),
                        chunk_idx: 0,
                        level: 2,
                        section_idx: 0,
                    },
                )
                .unwrap();
        }
        let add_elapsed = add_started.elapsed();
        assert_eq!(vectors.len(), count);

        let mut search_ms = Vec::new();
        for i in 0..200 {
            let query = unit_vector((i * 97 % count) as u64, DIMS);
            let started = Instant::now();
            let hits = vectors.search(&query, 10).unwrap();
            search_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            assert!(!hits.is_empty());
        }

        let p50 = percentile(search_ms.clone(), 0.50);
        let p95 = percentile(search_ms, 0.95);
        println!(
            "{count:>7} | {:>6.2} | {:>11.2} | {:>13.3} | {:>13.3}",
            add_elapsed.as_secs_f64() * 1000.0,
            count as f64 / add_elapsed.as_secs_f64(),
            p50,
            p95
        );
    }
}
