//! Perf bench for reindex pipeline — wall-clock duration of `reindex_item`
//! on documents of varying sizes (R1 滚动 review 实测).
//!
//! 跑法：`cargo test -p attune-core --release --test perf_reindex_bench -- --nocapture --ignored`
//! `#[ignore]` 让默认 CI 不跑（耗时不属于 unit test）；手动验收时显式 --ignored.

use attune_core::crypto::Key32;
use attune_core::index::FulltextIndex;
use attune_core::reindex;
use attune_core::store::Store;
use attune_core::vectors::VectorIndex;
use std::time::Instant;
use tempfile::TempDir;

fn setup() -> (TempDir, Store, VectorIndex, FulltextIndex, Key32) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("test.db")).unwrap();
    let vectors = VectorIndex::new(1024).unwrap();
    let fulltext = FulltextIndex::open(&tmp.path().join("ft")).unwrap();
    let dek = Key32::generate();
    (tmp, store, vectors, fulltext, dek)
}

fn gen_doc(size_kb: usize) -> String {
    let para = "This is a sample paragraph with some keywords and content that we will repeat to fill the document up to the target size in kilobytes. \
                它包含中文混合内容以测试 chunker 处理多语言文本的能力。Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n\n";
    let target = size_kb * 1024;
    let mut s = String::with_capacity(target + 128);
    s.push_str("# Document Title\n\n");
    let mut h = 1;
    while s.len() < target {
        s.push_str(&format!("## Section {}\n\n{}\n", h, para));
        h += 1;
    }
    s
}

#[test]
#[ignore]
fn bench_reindex_item_by_size() {
    println!("\n=== reindex_item wall-clock by doc size ===");
    println!("size_kb | insert_ms | reindex_ms | chunks");

    for size_kb in [1, 10, 50, 100, 500] {
        let (_t, store, mut vec, ft, dek) = setup();
        let content = gen_doc(size_kb);

        let t0 = Instant::now();
        let id = store
            .insert_item(&dek, "Doc", &content, None, "note", None, None)
            .unwrap();
        let insert_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = Instant::now();
        let stats = reindex::reindex_item(&store, &mut vec, &ft, &id, "Doc", &content, "note").unwrap();
        let reindex_ms = t1.elapsed().as_secs_f64() * 1000.0;

        println!(
            "{:>7} | {:>9.2} | {:>10.2} | {}",
            size_kb, insert_ms, reindex_ms, stats.chunks_enqueued
        );
    }
}

#[test]
#[ignore]
fn bench_purge_item_indexes() {
    println!("\n=== purge_item_indexes wall-clock by doc size ===");
    println!("size_kb | purge_ms | vectors_deleted | queue_cleared");

    for size_kb in [1, 10, 50, 100] {
        let (_t, store, mut vec, ft, dek) = setup();
        let content = gen_doc(size_kb);
        let id = store
            .insert_item(&dek, "Doc", &content, None, "note", None, None)
            .unwrap();
        // 先 reindex（让 queue 有数据）
        reindex::reindex_item(&store, &mut vec, &ft, &id, "Doc", &content, "note").unwrap();

        let t0 = Instant::now();
        let stats = reindex::purge_item_indexes(&store, &mut vec, &ft, &id).unwrap();
        let purge_ms = t0.elapsed().as_secs_f64() * 1000.0;

        println!(
            "{:>7} | {:>8.2} | {:>15} | {:>13}",
            size_kb, purge_ms, stats.vectors_deleted, stats.queue_cleared
        );
    }
}

#[test]
#[ignore]
fn bench_update_item_short_circuit() {
    println!("\n=== update_item content_hash 短路 wall-clock ===");
    println!("scenario      | duration_ms");

    let (_t, store, _v, _f, dek) = setup();
    let content = gen_doc(50);
    let id = store
        .insert_item(&dek, "Doc", &content, None, "note", None, None)
        .unwrap();

    // 1) 内容真改：必走 BLOB rewrite
    let new_content = format!("{content}\n\nAPPENDED MODIFICATION");
    let t0 = Instant::now();
    let outcome = store.update_item(&dek, &id, None, Some(&new_content)).unwrap();
    let modified_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert!(outcome.content_changed);
    println!("content changed | {:.2}", modified_ms);

    // 2) 内容未变：短路（不刷 BLOB）
    let t1 = Instant::now();
    let outcome2 = store.update_item(&dek, &id, None, Some(&new_content)).unwrap();
    let shortcircuit_ms = t1.elapsed().as_secs_f64() * 1000.0;
    assert!(!outcome2.content_changed, "短路必须不刷 BLOB");
    println!("same content    | {:.2}", shortcircuit_ms);

    // 3) 仅 title：完全跳过 content path
    let t2 = Instant::now();
    let outcome3 = store.update_item(&dek, &id, Some("new title"), None).unwrap();
    let title_ms = t2.elapsed().as_secs_f64() * 1000.0;
    assert!(!outcome3.content_changed);
    println!("title only      | {:.2}", title_ms);

    println!("\n→ 短路节省比例: {:.1}%", (1.0 - shortcircuit_ms / modified_ms) * 100.0);
}
