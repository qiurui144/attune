//! Reliability audit R20 — 边界 case + 异常注入
//!
//! 测每个 model provider 在异常输入下的鲁棒性:
//! - empty string
//! - 超长 input
//! - non-utf8 / special chars
//! - concurrent calls (并发)
//! - 重复调用 (idempotency)

use attune_core::embed::EmbeddingProvider;
use attune_core::infer::RerankProvider;
use attune_core::infer::embedding::OrtEmbeddingProvider;
use attune_core::infer::reranker::OrtRerankProvider;

#[test]
#[ignore]
fn embedding_boundary_audit() {
    let emb = match OrtEmbeddingProvider::qwen3_embedding_0_6b() {
        Ok(p) => p,
        Err(e) => { eprintln!("skip: {e}"); return; }
    };

    eprintln!("\n=== Embedding boundary audit ===");

    // 1. empty string — per audit R20 fix: 应返 zero vector (不再 ERROR)
    let r = emb.embed(&[""]);
    match &r {
        Ok((v, _usage)) => {
            eprintln!("✅ empty string → {} dims, val[0]={:.4}", v[0].len(), v[0][0]);
            // regression: empty 应该是 zero vector
            assert_eq!(v.len(), 1, "empty input should still produce 1 vector");
            assert_eq!(v[0].len(), emb.dimensions(), "empty input vector must have correct dims");
            assert!(v[0].iter().all(|&x| x == 0.0), "empty input must produce all-zero vector");
        }
        Err(e) => panic!("❌ empty string regression: should be Ok(zero vec) after fix, got: {e}"),
    }

    // 2. single char
    let r = emb.embed(&["a"]);
    eprintln!("{} single char 'a': {:?}", if r.is_ok() { "✅" } else { "❌" }, r.as_ref().map(|(v, _)| v[0].len()).unwrap_or(0));

    // 3. very long input (~10000 chars,远超 tokenizer max)
    let long_text = "Rust is a systems programming language. ".repeat(250);
    let r = emb.embed(&[long_text.as_str()]);
    eprintln!("{} long {} chars: {:?}", if r.is_ok() { "✅" } else { "❌" }, long_text.len(), r.as_ref().map(|(v, _)| v[0].len()).unwrap_or(0));

    // 4. unicode 中文 + 英文 + emoji
    let mixed = "Rust 是一门系统编程语言 🦀 with great memory safety";
    let r = emb.embed(&[mixed]);
    eprintln!("{} unicode mix '{}'...: {:?}", if r.is_ok() { "✅" } else { "❌" }, &mixed[..20], r.as_ref().map(|(v, _)| v[0].len()).unwrap_or(0));

    // 5. 同一文本重复 5 次 → 5 个向量应完全相同(deterministic)
    let same_text = "Rust ownership and borrowing";
    let mut vecs = Vec::new();
    for _ in 0..5 {
        let (v, _usage) = emb.embed(&[same_text]).unwrap();
        vecs.push(v[0].clone());
    }
    let same_all = vecs.iter().all(|v| v.iter().zip(&vecs[0]).all(|(a, b)| (a - b).abs() < 1e-5));
    eprintln!("{} determinism (5 repeats same): {}", if same_all { "✅" } else { "❌" }, if same_all { "identical" } else { "DRIFT" });

    // 6. 大 batch (100 short)
    let batch: Vec<String> = (0..100).map(|i| format!("item {i}")).collect();
    let batch_refs: Vec<&str> = batch.iter().map(|s| s.as_str()).collect();
    let t = std::time::Instant::now();
    let r = emb.embed(&batch_refs);
    let lat = t.elapsed().as_secs_f64();
    eprintln!("{} batch 100: {:?} in {:.1}s", if r.is_ok() { "✅" } else { "❌" }, r.as_ref().map(|(v, _)| v.len()).unwrap_or(0), lat);
}

#[test]
#[ignore]
fn reranker_boundary_audit() {
    let rerank = match OrtRerankProvider::bge_reranker_v2_m3() {
        Ok(p) => p,
        Err(e) => { eprintln!("skip: {e}"); return; }
    };

    eprintln!("\n=== Reranker boundary audit ===");

    // 1. empty query
    let r = rerank.score("", &["some doc"]);
    eprintln!("{} empty query: {:?}", if r.is_ok() { "✅" } else { "❌" }, r);

    // 2. empty doc
    let r = rerank.score("query", &[""]);
    eprintln!("{} empty doc: {:?}", if r.is_ok() { "✅" } else { "❌" }, r);

    // 3. empty docs list
    let docs: [&str; 0] = [];
    let r = rerank.score("query", &docs);
    eprintln!("{} empty docs list: {:?}", if r.is_ok() { "✅" } else { "❌" }, r);

    // 4. unicode query + doc
    let r = rerank.score("Rust 所有权", &["Rust ownership is unique"]);
    eprintln!("{} unicode q+doc: {:?}", if r.is_ok() { "✅" } else { "❌" }, r);

    // 5. 100 docs at once (batching stress)
    let docs100: Vec<String> = (0..100).map(|i| format!("doc {} about rust programming", i)).collect();
    let refs: Vec<&str> = docs100.iter().map(|s| s.as_str()).collect();
    let t = std::time::Instant::now();
    let r = rerank.score("Rust programming language", &refs);
    let lat = t.elapsed().as_secs_f64();
    eprintln!("{} 100 doc batch in {:.1}s: scores count = {:?}", if r.is_ok() { "✅" } else { "❌" }, lat, r.as_ref().map(|s| s.len()));
}
