//! OOM / 资源限制行为测试 — 验证边界情况下的 graceful degradation。
//!
//! 跑法：
//!   cargo test -p attune-core --test oom_behavior_test -- --nocapture --ignored
//!
//! 注：这些测试不真正触发系统 OOM，而是通过超大输入 / 逻辑满载来验证
//! 代码路径在极端情况下不 panic、不 corrupt、能正确降级或报告错误。

use attune_core::crypto::Key32;
use attune_core::index::FulltextIndex;
use attune_core::reindex;
use attune_core::store::Store;
use attune_core::vectors::{VectorIndex, VectorMeta};

// ──────────────────────────────────────────────────────────────────────────────
// Test 1: 超大文件（100 MB+）ingest → 不 panic，graceful chunk
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn large_file_ingest_no_panic_graceful_chunk() {
    // 构造 ~100 MB 的文本（避免真正占 100MB 内存，用 100K chunk × 1KB para）
    const TARGET_KB: usize = 100 * 1024; // 100 MB
    let para = "This is a repeating paragraph for large file ingest test. \
                Keywords: rust storage chunking performance scalability. \
                此段落包含中文内容用于测试分块器处理多语言大文件的能力。\n\n";
    let para_len = para.len();
    let repeats = TARGET_KB * 1024 / para_len + 1;

    let mut content = String::with_capacity(TARGET_KB * 1024 + 64);
    content.push_str("# Large Document\n\n");
    for i in 0..repeats {
        if i % 500 == 0 {
            content.push_str(&format!("## Section {}\n\n", i / 500));
        }
        content.push_str(para);
    }
    let actual_kb = content.len() / 1024;
    println!("[oom] large file: {actual_kb} KB");
    assert!(actual_kb >= 50 * 1024, "测试内容必须 >= 50 MB，实际 = {actual_kb} KB");

    let store = Store::open_memory().unwrap();
    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    let dek = Key32::generate();

    // insert_item 不应 panic
    let id = store
        .insert_item(&dek, "Large File", &content, None, "note", None, None)
        .expect("insert_item on 100MB doc must not panic/error");

    // reindex_item 不应 panic，必须产生 chunk
    let stats = reindex::reindex_item(&store, &mut vectors, &fulltext, &id, "Large File", &content, "note")
        .expect("reindex_item on 100MB doc must not panic");

    assert!(
        stats.chunks_enqueued > 0,
        "大文件必须产生至少 1 个 chunk，实际 = {}",
        stats.chunks_enqueued
    );
    println!(
        "[oom] large_file_ingest: OK — {actual_kb}KB → {} chunks enqueued",
        stats.chunks_enqueued
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 2: vector index 满载（大量向量）→ 仍能搜索，不 corrupt
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn vector_index_heavy_load_search_stable() {
    // VectorIndex::new 内部 reserve(10000)。写满 9_999 个（留 1 槽），验证
    // 接近满载时搜索不 corrupt。超出 10000 需要额外 reserve（目前无公开 API）。
    const N: usize = 9_999;
    const DIMS: usize = 64;

    let mut vectors = VectorIndex::new(DIMS).unwrap();

    // 批量写入，不应 panic 或 error
    for i in 0..N {
        let seed = i as u64;
        let mut v: Vec<f32> = (0..DIMS)
            .map(|d| ((seed.wrapping_mul(31).wrapping_add(d as u64)) as f32).sin())
            .collect();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        v.iter_mut().for_each(|x| *x /= norm);

        vectors
            .add(
                &v,
                VectorMeta {
                    item_id: format!("item-{i}"),
                    chunk_idx: 0,
                    level: 2,
                    section_idx: 0,
                },
            )
            .unwrap_or_else(|e| panic!("add vector {i} failed: {e}"));
    }

    assert_eq!(vectors.len(), N);

    // 搜索不应 panic，结果数 <= top_k
    let query: Vec<f32> = {
        let mut v: Vec<f32> = (0..DIMS).map(|d| (d as f32).sin()).collect();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        v.iter_mut().for_each(|x| *x /= norm);
        v
    };
    let results = vectors.search(&query, 10).expect("search on heavy load must not fail");
    assert!(results.len() <= 10, "搜索结果不得超过 top_k=10");
    assert!(!results.is_empty(), "50k 向量下搜索必须有结果");

    println!("[oom] vector_index_heavy_load: OK — {N} vectors, search returned {} results", results.len());
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 3: LLM context window 超限模拟 → Store 层正常，chunker 截断不 panic
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn oversized_content_chunker_truncates_no_panic() {
    // 构造超出 LLM context window 的内容（通常 4K~32K tokens ≈ ~16KB~128KB 字符）
    // 测试 chunker 在极端长文本下不 panic，产生合理数量的 chunk
    const TARGET_KB: usize = 200; // 200 KB — 远超典型 4K token window
    let chunk_para = "Chunker boundary test paragraph for oversized context window simulation. \
                      This text repeats to test that chunker handles very long documents gracefully. ";
    let mut content = String::with_capacity(TARGET_KB * 1024);
    content.push_str("# Oversized Document\n\n");
    let mut section = 0usize;
    while content.len() < TARGET_KB * 1024 {
        if content.len() % (10 * 1024) < chunk_para.len() {
            section += 1;
            content.push_str(&format!("\n## Section {section}\n\n"));
        }
        content.push_str(chunk_para);
    }
    let actual_kb = content.len() / 1024;
    println!("[oom] oversized content: {actual_kb} KB");

    let store = Store::open_memory().unwrap();
    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    let dek = Key32::generate();

    let id = store
        .insert_item(&dek, "Oversized", &content, None, "note", None, None)
        .expect("insert oversized content must not fail");

    let stats = reindex::reindex_item(&store, &mut vectors, &fulltext, &id, "Oversized", &content, "note")
        .expect("reindex oversized content must not panic");

    // 应该产生很多 chunk（内容大）
    assert!(
        stats.chunks_enqueued >= 10,
        "200KB 文档至少应产生 10 个 chunk，实际 = {}",
        stats.chunks_enqueued
    );

    // embed queue 中应有这些 chunk
    let pending = store.pending_embedding_count().unwrap();
    assert_eq!(pending, stats.chunks_enqueued, "pending 数必须等于入队数");

    println!("[oom] oversized_content: OK — {actual_kb}KB → {} chunks, no panic", stats.chunks_enqueued);
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 4: 空内容 / 极短内容 edge case → 不 panic，不写入 garbage
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn edge_case_empty_and_tiny_content_no_panic() {
    let store = Store::open_memory().unwrap();
    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    let dek = Key32::generate();

    let cases: &[(&str, &str)] = &[
        ("Empty", ""),
        ("Whitespace only", "   \n\t  \n  "),
        ("Single char", "x"),
        ("Single word", "hello"),
        ("Unicode only", "你好世界"),
        ("Newlines only", "\n\n\n\n"),
    ];

    for (title, content) in cases {
        let result = store.insert_item(&dek, title, content, None, "note", None, None);
        // insert may succeed or return error — what matters is no panic
        match result {
            Ok(id) => {
                // reindex 也不应 panic
                let _ = reindex::reindex_item(&store, &mut vectors, &fulltext, &id, title, content, "note");
            }
            Err(e) => {
                println!("[oom] edge case '{title}' insert returned error (acceptable): {e}");
            }
        }
    }

    println!("[oom] edge_case_empty_tiny: OK — no panic on any edge case");
}
