//! Reliability audit R17 — 真 RAG flow stress (embedding + reranker + retrieve)
//!
//! 用本地 ORT bge-m3 (Xenova_bge-m3) embedding + reranker 模型,
//! 跑 rust-book chunk + 5 query,验证全链路:
//! 1. chunks → embed via ORT
//! 2. query → embed + cosine top-K
//! 3. top-K → reranker score
//! 4. 输出最终 ranking + latency
//!
//! 跑法:
//!   cargo test -p attune-core --release --test rag_flow_audit -- --ignored --nocapture

use attune_core::embed::EmbeddingProvider;
use attune_core::infer::RerankProvider;
use attune_core::infer::embedding::OrtEmbeddingProvider;
use attune_core::infer::reranker::OrtRerankProvider;

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[test]
#[ignore]
fn rag_flow_e2e_audit() {
    use std::fs;
    use std::path::Path;

    // 1. embedding provider — ORT bge-m3
    let emb = match OrtEmbeddingProvider::qwen3_embedding_0_6b() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skip: ORT embedding init failed: {e}");
            return;
        }
    };
    eprintln!("✅ ORT embedding loaded ({}d)", emb.dimensions());

    // 2. reranker
    let rerank = match OrtRerankProvider::bge_reranker_v2_m3() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skip: reranker init failed: {e}");
            return;
        }
    };
    eprintln!("✅ Reranker loaded");

    // 3. corpus — rust-book first 10 files chunked
    let corpus_dir = Path::new("/data/company/project/attune/rust/tests/corpora/rust-book/src");
    if !corpus_dir.exists() {
        eprintln!("skip: corpus missing");
        return;
    }

    let wanted = [
        "ch04-00-understanding-ownership.md",
        "ch04-01-what-is-ownership.md",
        "ch04-02-references-and-borrowing.md",
        "ch04-03-slices.md",
        "ch06-00-enums.md",
        "ch06-02-match.md",
        "ch09-00-error-handling.md",
        "ch10-02-traits.md",
        "ch17-00-async-await.md",
        "ch08-01-vectors.md",
    ];

    let mut chunks: Vec<(String, String)> = Vec::new();
    for f in wanted {
        let p = corpus_dir.join(f);
        if !p.exists() { continue; }
        let txt = fs::read_to_string(&p).unwrap_or_default();
        // simple sliding window chunking
        let chars: Vec<char> = txt.chars().collect();
        let chunk_size = 800;
        let overlap = 150;
        let step = chunk_size - overlap;
        let mut i = 0;
        while i < chars.len() {
            let end = (i + chunk_size).min(chars.len());
            let c: String = chars[i..end].iter().collect();
            if c.trim().len() > 100 {
                chunks.push((f.to_string(), c));
            }
            if end >= chars.len() { break; }
            i += step;
        }
    }
    eprintln!("📚 chunks: {}", chunks.len());

    // 4. embed all chunks (single-call batch)
    let t0 = std::time::Instant::now();
    let texts: Vec<&str> = chunks.iter().map(|(_, t)| t.as_str()).collect();
    let chunk_embeds = match emb.embed(&texts) {
        Ok(v) => v,
        Err(e) => { eprintln!("embed err: {e}"); return; }
    };
    eprintln!("✅ embed {} chunks in {:.1}s", chunk_embeds.len(), t0.elapsed().as_secs_f64());

    // 5. queries
    let queries = [
        ("ownership", "ch04"),
        ("references and borrowing", "ch04"),
        ("error handling Result", "ch09"),
        ("trait definition", "ch10"),
        ("async await", "ch17"),
    ];

    eprintln!("\n=== Per-query RAG flow ===");
    let mut total_e2e_lat = 0u128;
    let mut hits = 0;
    for (q, gt_prefix) in &queries {
        let t = std::time::Instant::now();

        // 5a. embed query
        let q_emb = emb.embed(&[q]).expect("query embed");
        let q_vec = &q_emb[0];

        // 5b. cosine top-10
        let mut scored: Vec<(usize, f32)> = chunk_embeds.iter().enumerate()
            .map(|(i, v)| (i, cosine(q_vec, v))).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let top10_idx: Vec<usize> = scored.iter().take(10).map(|(i, _)| *i).collect();

        // 5c. reranker on top-10
        let docs: Vec<&str> = top10_idx.iter().map(|&i| chunks[i].1.as_str()).collect();
        let rerank_scores = rerank.score(q, &docs).expect("rerank");
        let mut paired: Vec<(usize, f32)> = top10_idx.iter().copied().zip(rerank_scores).collect();
        paired.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let lat_ms = t.elapsed().as_millis();
        total_e2e_lat += lat_ms;
        let top1 = &chunks[paired[0].0].0;
        let hit = top1.starts_with(gt_prefix);
        if hit { hits += 1; }
        eprintln!(
            "  q='{}' → top1={} (rr score={:.3}) {}ms {}",
            q,
            top1,
            paired[0].1,
            lat_ms,
            if hit { "✅ HIT" } else { "❌ MISS" }
        );
    }

    eprintln!("\n=== SUMMARY ===");
    eprintln!("queries: {}", queries.len());
    eprintln!("hit@1 (after reranker): {}/{} = {:.0}%", hits, queries.len(), hits as f32 / queries.len() as f32 * 100.0);
    eprintln!("avg e2e latency: {}ms (embed + cosine + rerank)", total_e2e_lat / queries.len() as u128);
}
