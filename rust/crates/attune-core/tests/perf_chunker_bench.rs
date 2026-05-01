//! Performance baseline for chunker hot path.
//!
//! Layer: 跨层质量门 — Performance (per docs/TESTING.md §2.2 P-005 "Tantivy 索引
//! 写入吞吐 > 500 chunks/s" — chunker 是 ingest pipeline 的前置, throughput
//! 直接决定全 corpus 索引时间).
//!
//! Strategy: 用真实 rust-book corpus (112 markdown files) 跑 chunk + extract_sections,
//! 输出 docs/s + chunks/s + sections/s. std::time::Instant + simple wall clock —
//! 不引入 criterion dev-dep (binary size + compile time tradeoff).
//!
//! Run:
//!   cargo test -p attune-core --test perf_chunker_bench --release -- --ignored --nocapture
//!
//! 默认 #[ignore] — perf benchmarks 不在每次 PR run 阻塞 CI, 由 release-prep
//! workflow 或人工触发. 输出可粘贴到 docs/benchmarks/ 累积历史。

use std::path::PathBuf;
use std::time::Instant;

const CORPUS_REL: &str = "../../tests/corpora/rust-book/src";

fn corpus_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(manifest).join(CORPUS_REL)
}

fn load_corpus_markdown() -> Vec<(String, String)> {
    let dir = corpus_dir();
    if !dir.is_dir() {
        eprintln!("[skip] corpus dir not found: {}", dir.display());
        eprintln!("       run: bash scripts/download-corpora.sh");
        return Vec::new();
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read corpus dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let content = std::fs::read_to_string(&path).expect("read file");
        files.push((name, content));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[test]
#[ignore]
fn perf_chunk_sliding_window_baseline() {
    let corpus = load_corpus_markdown();
    if corpus.is_empty() {
        eprintln!("corpus empty; skipping perf test");
        return;
    }

    let total_bytes: usize = corpus.iter().map(|(_, c)| c.len()).sum();
    let total_docs = corpus.len();
    println!(
        "\n=== chunker::chunk (sliding window, 512 chunk / 128 overlap) ===",
    );
    println!("corpus: {} markdown files, {:.2} MB total",
        total_docs,
        total_bytes as f64 / 1024.0 / 1024.0,
    );

    let start = Instant::now();
    let mut total_chunks = 0;
    for (_, content) in &corpus {
        let chunks = attune_core::chunker::chunk(content, 512, 128);
        total_chunks += chunks.len();
    }
    let elapsed = start.elapsed();

    let docs_per_sec = total_docs as f64 / elapsed.as_secs_f64();
    let chunks_per_sec = total_chunks as f64 / elapsed.as_secs_f64();
    let mb_per_sec = (total_bytes as f64 / 1024.0 / 1024.0) / elapsed.as_secs_f64();

    println!("elapsed: {:.3}s", elapsed.as_secs_f64());
    println!("docs/s: {:.1}", docs_per_sec);
    println!("chunks/s: {:.0}", chunks_per_sec);
    println!("MB/s: {:.2}", mb_per_sec);
    println!("avg chunks per doc: {:.1}", total_chunks as f64 / total_docs as f64);

    // 阈值断言：chunker 是纯 CPU 算法，应该 >100 docs/s on dev hardware.
    // 实际数字由 docs/benchmarks/ 累积；本断言只防护重大退化。
    assert!(
        docs_per_sec > 50.0,
        "chunker throughput dropped below 50 docs/s ({:.1}) — possible regression",
        docs_per_sec
    );
}

#[test]
#[ignore]
fn perf_extract_sections_baseline() {
    let corpus = load_corpus_markdown();
    if corpus.is_empty() {
        return;
    }

    let total_docs = corpus.len();
    println!("\n=== chunker::extract_sections (semantic section split) ===");

    let start = Instant::now();
    let mut total_sections = 0;
    for (_, content) in &corpus {
        let sections = attune_core::chunker::extract_sections(content);
        total_sections += sections.len();
    }
    let elapsed = start.elapsed();

    let docs_per_sec = total_docs as f64 / elapsed.as_secs_f64();
    let sections_per_sec = total_sections as f64 / elapsed.as_secs_f64();

    println!("elapsed: {:.3}s", elapsed.as_secs_f64());
    println!("docs/s: {:.1}", docs_per_sec);
    println!("sections/s: {:.0}", sections_per_sec);
    println!("avg sections per doc: {:.1}", total_sections as f64 / total_docs as f64);

    assert!(
        docs_per_sec > 50.0,
        "extract_sections throughput dropped below 50 docs/s ({:.1})",
        docs_per_sec
    );
}

#[test]
#[ignore]
fn perf_extract_sections_with_path_baseline() {
    let corpus = load_corpus_markdown();
    if corpus.is_empty() {
        return;
    }

    let total_docs = corpus.len();
    println!("\n=== chunker::extract_sections_with_path (J1 path-prefix) ===");

    let start = Instant::now();
    let mut total_sections = 0;
    for (_, content) in &corpus {
        let sections = attune_core::chunker::extract_sections_with_path(content);
        total_sections += sections.len();
    }
    let elapsed = start.elapsed();

    let docs_per_sec = total_docs as f64 / elapsed.as_secs_f64();
    println!("elapsed: {:.3}s", elapsed.as_secs_f64());
    println!("docs/s: {:.1}", docs_per_sec);
    println!("sections (with path): {}", total_sections);
}
