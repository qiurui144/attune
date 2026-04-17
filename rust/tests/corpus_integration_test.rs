//! Corpus-based integration tests.
//!
//! These tests run against pinned GitHub knowledge repositories to verify
//! real-world behavior that unit tests can't cover: chunking boundaries,
//! multi-language tokenization, cross-document search relevance, classification
//! consistency on large corpora.
//!
//! They are gated behind `#[ignore]` because they:
//!   1. Require `./scripts/download-corpora.sh` to have been run first
//!   2. Take 2-5 minutes each
//!   3. Need a real Ollama running for embeddings
//!
//! Run manually: `cargo test --test corpus_integration_test -- --ignored`
//! Run specific: `cargo test --test corpus_integration_test -- --ignored rust_book_ingestion`

use std::path::{Path, PathBuf};

/// Returns the absolute path to a corpus dir, or None if not downloaded.
fn corpus_path(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?  // rust/ (parent of attune-core/)
        .join("tests/corpora")
        .join(name);
    if p.exists() && p.is_dir() {
        Some(p)
    } else {
        None
    }
}

fn skip_if_no_corpus(name: &str) -> Option<PathBuf> {
    match corpus_path(name) {
        Some(p) => Some(p),
        None => {
            eprintln!(
                "[SKIP] corpus '{name}' not present; run ./scripts/download-corpora.sh {name}"
            );
            None
        }
    }
}

fn count_markdown_files(dir: &Path) -> usize {
    fn walk(p: &Path, count: &mut usize) {
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Skip .git and node_modules
                    if matches!(
                        path.file_name().and_then(|n| n.to_str()),
                        Some(".git") | Some("node_modules")
                    ) {
                        continue;
                    }
                    walk(&path, count);
                } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    *count += 1;
                }
            }
        }
    }
    let mut count = 0;
    walk(dir, &mut count);
    count
}

// ── Corpus A: rust-lang/book ──────────────────────────────────────────────────

#[test]
#[ignore]
fn rust_book_structure() {
    // F-001 smoke: corpus is present and has expected scale
    let corpus = match skip_if_no_corpus("rust-book") {
        Some(p) => p,
        None => return,
    };

    let md_count = count_markdown_files(&corpus);
    assert!(
        md_count >= 100,
        "rust-book should have >= 100 markdown files, got {md_count}"
    );
    eprintln!("[rust-book] {md_count} markdown files");

    // Must contain the known canonical chapter layout
    let toc = corpus.join("src/SUMMARY.md");
    assert!(
        toc.exists(),
        "rust-book/src/SUMMARY.md expected at pinned version"
    );
}

#[test]
#[ignore]
fn rust_book_chunker_preserves_code_blocks() {
    // F-001b: chunker should not split inside a ```rust code fence
    use attune_core::chunker;

    let corpus = match skip_if_no_corpus("rust-book") {
        Some(p) => p,
        None => return,
    };

    // Take chapter 4 ownership, a rich mix of prose + code
    let candidate = corpus.join("src/ch04-01-what-is-ownership.md");
    let src = match std::fs::read_to_string(&candidate) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("[SKIP] expected file missing (corpus pin drifted?): {}",
                candidate.display());
            return;
        }
    };

    let chunks = chunker::chunk(&src, 1000, 100);
    assert!(!chunks.is_empty(), "chunker produced zero chunks");

    // For each chunk, number of ``` fences must be even (balanced).
    // A chunk that splits inside a code fence would have odd count.
    for (i, chunk) in chunks.iter().enumerate() {
        let fence_count = chunk.matches("```").count();
        assert_eq!(
            fence_count % 2,
            0,
            "chunk {i} has unbalanced code fences ({fence_count}):\n{chunk}\n----"
        );
    }
    eprintln!("[rust-book] chunker produced {} balanced chunks for ch04-01",
        chunks.len());
}

// ── Corpus B: CyC2018/CS-Notes (Chinese) ──────────────────────────────────────

#[test]
#[ignore]
fn cs_notes_chinese_content_present() {
    // F-002a: corpus has Chinese markdown files
    let corpus = match skip_if_no_corpus("cs-notes") {
        Some(p) => p,
        None => return,
    };
    let md_count = count_markdown_files(&corpus);
    assert!(md_count >= 100, "cs-notes has {md_count}, expected >= 100");

    // Sample one known file and verify it contains Chinese characters
    let sample = corpus.join("notes/算法 - 算法分析.md");
    if sample.exists() {
        let content = std::fs::read_to_string(&sample).unwrap_or_default();
        let has_chinese = content.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        assert!(has_chinese, "expected Chinese characters in cs-notes sample");
    }
    eprintln!("[cs-notes] {md_count} markdown files");
}

#[test]
#[ignore]
fn cs_notes_indexing_through_fulltext() {
    // F-002b: verify attune_core's FulltextIndex (which uses tantivy-jieba
    // internally) can index real Chinese content from the corpus and recall
    // by Chinese keyword search.
    use attune_core::index::FulltextIndex;

    let corpus = match skip_if_no_corpus("cs-notes") {
        Some(p) => p,
        None => return,
    };

    // Build a temporary index in a scratch dir
    let tmp = tempfile::tempdir().expect("tempdir");
    let ft = FulltextIndex::open(tmp.path()).expect("open index");

    // Find a Chinese sample and index it
    let candidates = [
        corpus.join("notes/算法 - 算法分析.md"),
        corpus.join("notes/Leetcode 题解 - 目录.md"),
    ];
    let mut indexed = 0;
    for (i, path) in candidates.iter().enumerate() {
        if let Ok(content) = std::fs::read_to_string(path) {
            let id = format!("test_{i}");
            let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("sample");
            ft.add_document(&id, title, &content, "file").expect("add_document");
            indexed += 1;
        }
    }
    assert!(indexed > 0, "no sample files found in cs-notes corpus");

    // Search by a Chinese keyword that jieba should tokenize
    let results = ft.search("算法", 10).expect("search");
    assert!(
        !results.is_empty(),
        "Chinese keyword search should return results after indexing Chinese content"
    );
    eprintln!("[cs-notes] indexed {indexed} file(s); search '算法' returned {} hits",
        results.len());
}

// ── Edge cases (Corpus E, synthetic, committed in repo) ───────────────────────

#[test]
fn edge_case_empty_document() {
    // F-006: zero-length content must not crash chunker
    use attune_core::chunker;
    let chunks = chunker::chunk("", 500, 50);
    assert!(chunks.is_empty() || chunks.iter().all(|c| c.is_empty()));
}

#[test]
fn edge_case_very_long_single_line() {
    // Single line far exceeding chunk window must still chunk
    use attune_core::chunker;
    let content = "A".repeat(100_000);
    let chunks = chunker::chunk(&content, 500, 50);
    assert!(!chunks.is_empty());
    // No chunk must exceed chunk_size by much (allow some slack for boundary logic)
    for chunk in &chunks {
        assert!(
            chunk.len() <= 2000,
            "chunk size {} violates bound (size param 500)",
            chunk.len()
        );
    }
}

#[test]
fn edge_case_mixed_unicode() {
    // Multi-script content: chunker must not split mid-codepoint
    use attune_core::chunker;
    let content = "English 中文 日本語 한국어 🚀🌟✨".repeat(200);
    let chunks = chunker::chunk(&content, 500, 50);
    // Each chunk must be valid UTF-8 (it already is, being &str), but we
    // additionally verify no chunk is suspiciously truncated mid-character.
    for chunk in &chunks {
        // Rust &str can't hold invalid UTF-8, so this is really a structural check.
        let _char_count = chunk.chars().count();
    }
    assert!(!chunks.is_empty());
}
