//! 并发 stress test — 验证多线程读写无死锁、无 race condition、exactly-once 处理。
//!
//! 跑法：
//!   cargo test -p attune-core --test concurrent_stress_test -- --nocapture --ignored
//!
//! 注：Store 内部使用 RefCell（非 Sync），必须包在 Arc<Mutex<Store>> 中跨线程传递。
//! FulltextIndex 内部已有 Mutex<IndexWriter>，可以 Arc<FulltextIndex> 直接共享。
//! VectorIndex 无内部锁，需要 Arc<Mutex<VectorIndex>>。

use attune_core::crypto::Key32;
use attune_core::index::FulltextIndex;
use attune_core::reindex;
use attune_core::store::Store;
use attune_core::vectors::{VectorIndex, VectorMeta};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ──────────────────────────────────────────────────────────────────────────────
// helpers
// ──────────────────────────────────────────────────────────────────────────────

fn deterministic_vec(seed: usize, dims: usize) -> Vec<f32> {
    let mut v: Vec<f32> = (0..dims)
        .map(|i| {
            let x = (seed * 31 + i * 17) as f32;
            (x.sin() + x.cos()) / 2.0
        })
        .collect();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 1: 10 thread 同时 read/write same vault → 无死锁，最终状态一致
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn concurrent_read_write_no_deadlock() {
    const WRITERS: usize = 5;
    const READERS: usize = 5;
    const ITEMS_PER_WRITER: usize = 20;

    // Store 用 Arc<Mutex<Store>> 包装才能跨线程
    let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
    let dek = Arc::new(Key32::generate());
    let barrier = Arc::new(Barrier::new(WRITERS + READERS));

    // Writer handles: JoinHandle<Vec<String>>
    let mut writer_handles = Vec::new();
    for w in 0..WRITERS {
        let store = Arc::clone(&store);
        let dek = Arc::clone(&dek);
        let barrier = Arc::clone(&barrier);
        writer_handles.push(thread::spawn(move || -> Vec<String> {
            barrier.wait();
            let mut ids = Vec::new();
            for i in 0..ITEMS_PER_WRITER {
                let content = format!(
                    "# Writer {w} Item {i}\n\nContent from writer {w}, item {i}. \
                     Keywords: concurrent write test durability rust.\n"
                );
                let id = store
                    .lock()
                    .unwrap()
                    .insert_item(&dek, &format!("W{w}-{i}"), &content, None, "note", None, None)
                    .unwrap();
                ids.push(id);
            }
            ids
        }));
    }

    // Reader handles: JoinHandle<Vec<usize>> — 分开放
    let mut reader_handles = Vec::new();
    for _ in 0..READERS {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        reader_handles.push(thread::spawn(move || -> Vec<usize> {
            barrier.wait();
            let mut counts = Vec::new();
            for _ in 0..30 {
                counts.push(store.lock().unwrap().item_count().unwrap());
                thread::sleep(Duration::from_millis(2));
            }
            counts
        }));
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    for h in writer_handles {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(remaining > Duration::ZERO, "writer 超时（死锁？）");
        let _ = h.join();
    }
    for h in reader_handles {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(remaining > Duration::ZERO, "reader 超时（死锁？）");
        let _ = h.join();
    }

    let final_count = store.lock().unwrap().item_count().unwrap();
    let expected = WRITERS * ITEMS_PER_WRITER;
    assert_eq!(
        final_count, expected,
        "最终 item count 必须 = {expected}，实际 = {final_count}"
    );
    println!("[concurrent] read_write: OK — {final_count} items written by {WRITERS} writers");
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 2: 5 thread embed + 5 thread FTS search 同时 → 无 race condition
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn concurrent_embed_and_search_no_race() {
    const EMBED_THREADS: usize = 5;
    const SEARCH_THREADS: usize = 5;
    const VECS_PER_THREAD: usize = 200;
    const DIMS: usize = 64;

    let vectors = Arc::new(Mutex::new(VectorIndex::new(DIMS).unwrap()));
    // FulltextIndex 内部有 Mutex<IndexWriter>，可 Arc 直接共享（实现了 Sync via Mutex）
    let fulltext = Arc::new(FulltextIndex::open_memory().unwrap());

    // Pre-populate fulltext so searches don't always return empty
    for i in 0..50 {
        fulltext
            .add_document(
                &format!("pre-{i}"),
                &format!("Pre-doc {i}"),
                &format!("pre-seeded content about topic {i} rust search concurrent"),
                "note",
            )
            .unwrap();
    }

    let barrier = Arc::new(Barrier::new(EMBED_THREADS + SEARCH_THREADS));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();

    // Embed threads — 并发写向量
    for t in 0..EMBED_THREADS {
        let vectors = Arc::clone(&vectors);
        let errors = Arc::clone(&errors);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for i in 0..VECS_PER_THREAD {
                let seed = t * VECS_PER_THREAD + i;
                let v = deterministic_vec(seed, DIMS);
                let meta = VectorMeta {
                    item_id: format!("item-{seed}"),
                    chunk_idx: 0,
                    level: 2,
                    section_idx: 0,
                };
                if let Err(e) = vectors.lock().unwrap().add(&v, meta) {
                    errors
                        .lock()
                        .unwrap()
                        .push(format!("embed error t={t} i={i}: {e}"));
                }
            }
        }));
    }

    // Search threads — 并发搜索
    for t in 0..SEARCH_THREADS {
        let fulltext = Arc::clone(&fulltext);
        let errors = Arc::clone(&errors);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for i in 0..50 {
                let q = if i % 2 == 0 {
                    "rust search concurrent"
                } else {
                    "pre-seeded content topic"
                };
                if let Err(e) = fulltext.search(q, 5) {
                    errors
                        .lock()
                        .unwrap()
                        .push(format!("search error t={t} i={i}: {e}"));
                }
                thread::sleep(Duration::from_millis(1));
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    let errs = errors.lock().unwrap();
    assert!(errs.is_empty(), "并发错误: {:?}", &*errs);

    let total_vecs = vectors.lock().unwrap().len();
    let expected_vecs = EMBED_THREADS * VECS_PER_THREAD;
    assert_eq!(
        total_vecs, expected_vecs,
        "向量数必须 = {expected_vecs}，实际 = {total_vecs}"
    );
    println!("[concurrent] embed+search: OK — {total_vecs} vectors written, 0 errors");
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 3: embedding queue 多 consumer → exactly-once 处理（每个 task 仅被处理一次）
// ──────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore]
fn embed_queue_multi_consumer_exactly_once() {
    const CONSUMERS: usize = 4;
    const TARGET_TASKS: usize = 40; // 目标积累 40 个 pending embed 任务

    // 主线程 setup: store + reindex 积累 queue（不跨线程操作 store setup 部分）
    let store_setup = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    let mut total_enqueued = 0usize;

    for i in 0..20 {
        let content = format!(
            "# Item {i}\n\n{}\n\n## Section A\n\n{}\n\n## Section B\n\n{}\n",
            "Content paragraph one. ".repeat(5),
            "Section alpha content for embedding. ".repeat(4),
            "Section beta content for retrieval. ".repeat(4),
        );
        let id = store_setup
            .insert_item(&dek, &format!("Item {i}"), &content, None, "note", None, None)
            .unwrap();
        let stats = reindex::reindex_item(
            &store_setup,
            &mut vectors,
            &fulltext,
            &id,
            &format!("Item {i}"),
            &content,
            "note",
        )
        .unwrap();
        total_enqueued += stats.chunks_enqueued;
        if total_enqueued >= TARGET_TASKS {
            break;
        }
    }
    assert!(total_enqueued >= 1, "至少入队 1 个任务");

    // 把 store 放进 Arc<Mutex> 才能跨线程
    let store = Arc::new(Mutex::new(store_setup));
    let processed_ids = Arc::new(Mutex::new(Vec::<i64>::new()));
    let barrier = Arc::new(Barrier::new(CONSUMERS));
    let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();

    for _ in 0..CONSUMERS {
        let store = Arc::clone(&store);
        let processed_ids = Arc::clone(&processed_ids);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            loop {
                let tasks = store.lock().unwrap().dequeue_embeddings(5).unwrap();
                if tasks.is_empty() {
                    break;
                }
                for task in &tasks {
                    processed_ids.lock().unwrap().push(task.id);
                    store.lock().unwrap().mark_embedding_done(task.id).unwrap();
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("consumer thread panicked");
    }

    // 验证 exactly-once: processed_ids 中不存在重复
    let ids = processed_ids.lock().unwrap();
    let unique_count = {
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.len()
    };
    assert_eq!(
        ids.len(),
        unique_count,
        "存在重复处理的 task id — exactly-once 违反 (total={}, unique={})",
        ids.len(),
        unique_count
    );

    println!(
        "[concurrent] exactly-once: OK — {} tasks processed, {} unique (no duplicates)",
        ids.len(),
        unique_count
    );

    // 剩余 pending 必须为 0
    let remaining = store.lock().unwrap().pending_embedding_count().unwrap();
    assert_eq!(remaining, 0, "所有任务必须处理完，remaining={remaining}");
}
