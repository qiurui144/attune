//! Reliability audit R18 — RAG perf P50/P99 + warmup stress
//!
//! 扩大 R17 的 corpus + query 数,统计 e2e latency 分布。
//! 跑法:
//!   cargo test -p attune-core --release --test rag_perf_audit -- --ignored --nocapture

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
fn rag_perf_p50_p99() {
    use std::fs;
    use std::path::Path;

    let emb = match OrtEmbeddingProvider::qwen3_embedding_0_6b() {
        Ok(p) => p,
        Err(e) => { eprintln!("skip: {e}"); return; }
    };
    let rerank = match OrtRerankProvider::bge_reranker_v2_m3() {
        Ok(p) => p,
        Err(e) => { eprintln!("skip: {e}"); return; }
    };

    // 取 30 文件
    let corpus_dir = Path::new("/data/company/project/attune/rust/tests/corpora/rust-book/src");
    let wanted: Vec<String> = fs::read_dir(corpus_dir).unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|n| n.ends_with(".md") && (n.starts_with("ch") || n.starts_with("appendix")))
        .take(30).collect();

    let mut chunks: Vec<(String, String)> = Vec::new();
    for f in &wanted {
        let p = corpus_dir.join(f);
        let txt = fs::read_to_string(&p).unwrap_or_default();
        let chars: Vec<char> = txt.chars().collect();
        let step = 650usize;
        let cs = 800usize;
        let mut i = 0;
        while i < chars.len() {
            let end = (i + cs).min(chars.len());
            let c: String = chars[i..end].iter().collect();
            if c.trim().len() > 100 {
                chunks.push((f.clone(), c));
            }
            if end >= chars.len() { break; }
            i += step;
        }
    }
    eprintln!("📚 corpus: {} files, {} chunks", wanted.len(), chunks.len());

    // embed all
    let t0 = std::time::Instant::now();
    let texts: Vec<&str> = chunks.iter().map(|(_, t)| t.as_str()).collect();
    let chunk_embeds = emb.embed(&texts).expect("embed");
    let chunk_embed_sec = t0.elapsed().as_secs_f64();
    eprintln!("✅ embed {} chunks in {:.1}s ({:.1} chunk/s)", chunk_embeds.len(), chunk_embed_sec, chunks.len() as f64 / chunk_embed_sec);

    // 30 query (warm)
    let queries = [
        "ownership in Rust", "borrowing rules", "lifetimes", "match expression", "Option type",
        "Result error handling", "panic vs Result", "Cargo manifest", "modules visibility", "use statement",
        "structs methods", "trait definition", "generics", "vector", "HashMap",
        "string slice", "if let", "closures", "iterators", "smart pointers",
        "trait object", "tokio async", "channels threads", "mutex shared", "macros",
        "unsafe rust", "tests", "cargo build", "println", "stack heap",
    ];

    // warm-up first 3
    for q in &queries[..3] {
        let q_emb = emb.embed(&[q]).unwrap();
        let mut scored: Vec<(usize, f32)> = chunk_embeds.iter().enumerate()
            .map(|(i, v)| (i, cosine(&q_emb[0], v))).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let docs: Vec<&str> = scored.iter().take(10).map(|(i, _)| chunks[*i].1.as_str()).collect();
        rerank.score(q, &docs).unwrap();
    }
    eprintln!("✅ warm-up 3 query done");

    // measure
    let mut lats = Vec::new();
    let mut embed_lats = Vec::new();
    let mut rerank_lats = Vec::new();
    for q in &queries {
        let t = std::time::Instant::now();

        let t_e = std::time::Instant::now();
        let q_emb = emb.embed(&[q]).unwrap();
        embed_lats.push(t_e.elapsed().as_millis());

        let mut scored: Vec<(usize, f32)> = chunk_embeds.iter().enumerate()
            .map(|(i, v)| (i, cosine(&q_emb[0], v))).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let docs: Vec<&str> = scored.iter().take(10).map(|(i, _)| chunks[*i].1.as_str()).collect();
        let t_r = std::time::Instant::now();
        let _scores = rerank.score(q, &docs).unwrap();
        rerank_lats.push(t_r.elapsed().as_millis());

        lats.push(t.elapsed().as_millis());
    }

    lats.sort();
    embed_lats.sort();
    rerank_lats.sort();
    let p50 = lats[lats.len()*50/100];
    let p90 = lats[lats.len()*90/100];
    let p99 = lats.last().copied().unwrap_or(0);
    let emb_p50 = embed_lats[embed_lats.len()*50/100];
    let rr_p50 = rerank_lats[rerank_lats.len()*50/100];

    eprintln!("\n=== RAG perf (30 query e2e, 30 file corpus) ===");
    eprintln!("queries: {}", queries.len());
    eprintln!("e2e latency:    P50={p50}ms P90={p90}ms P99={p99}ms");
    eprintln!("embed only:     P50={emb_p50}ms");
    eprintln!("rerank only:    P50={rr_p50}ms (top-10 docs)");
    eprintln!("0 failures: {}", lats.len() == queries.len());
}
