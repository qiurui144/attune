//! ingest_document 四态行为 + domain/tags 透传 + corpus_domain 前缀集成测试。

use std::collections::HashMap;

use attune_core::crypto::Key32;
use attune_core::ingest::{ingest_document, ingest_document_replacing, IngestOutcome, RawDocument, SourceKind};
use attune_core::store::Store;

fn md_doc(source_ref: &str, body: &str) -> RawDocument {
    RawDocument {
        uri: format!("file://{source_ref}"),
        title: String::new(),
        content: body.as_bytes().to_vec(),
        mime_hint: Some("text/markdown".into()),
        source_kind: SourceKind::LocalFolder,
        source_ref: source_ref.into(),
        modified_marker: None,
        domain: None,
        tags: None,
        corpus_domain: None,
        metadata: HashMap::new(),
    }
}

#[test]
fn first_ingest_returns_inserted_and_enqueues_two_levels() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let doc = md_doc("/tmp/a.md", "# Title\n\nSome body paragraph here.\n\n# Two\n\nMore body.");

    let outcome = ingest_document(&store, &dek, &doc).unwrap();
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, chunks_enqueued } => {
            assert!(chunks_enqueued >= 2, "L1 章节 + L2 段落块都应入队");
            item_id
        }
        other => panic!("expected Inserted, got {other:?}"),
    };
    assert_eq!(store.item_count().unwrap(), 1);

    // L1 (level=1) 与 L2 (level=2) 都必须有任务入队。
    let l1 = store.count_embed_queue_by_level(1).unwrap();
    let l2 = store.count_embed_queue_by_level(2).unwrap();
    assert!(l1 >= 1, "Level-1 章节 embedding 必须入队");
    assert!(l2 >= 1, "Level-2 段落块 embedding 必须入队");

    // classify 任务必须入队。
    assert_eq!(store.pending_count_by_type("classify").unwrap(), 1);

    // breadcrumbs sidecar 必须写入。
    assert!(store.chunk_breadcrumb_count(&item_id).unwrap() >= 1);
}

#[test]
fn duplicate_content_returns_duplicate_and_skips_pipeline() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let doc = md_doc("/tmp/a.md", "# Same\n\nidentical body.");
    let first = ingest_document(&store, &dek, &doc).unwrap();
    let first_id = match first {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };

    // 同内容、不同 source_ref 再入一次 → content_hash 命中 → Duplicate。
    let doc2 = md_doc("/tmp/copy-of-a.md", "# Same\n\nidentical body.");
    let second = ingest_document(&store, &dek, &doc2).unwrap();
    match second {
        IngestOutcome::Duplicate { item_id } => assert_eq!(item_id, first_id),
        other => panic!("expected Duplicate, got {other:?}"),
    }
    assert_eq!(store.item_count().unwrap(), 1, "重复内容不得新增 item");
}

#[test]
fn replacing_old_item_returns_updated() {
    // caller（scanner / scanner_webdav 等）在调用前自行完成增量检测：
    // 查 indexed_files / ETag / Message-ID 等各源标识，确认内容已变后
    // 软删旧 item + enqueue purge，再调 ingest_document_replacing。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let dir_id = store.bind_directory("/tmp", true, &["md"]).unwrap();

    let doc_v1 = md_doc("/tmp/a.md", "# V1\n\noriginal body.");
    let first_id = match ingest_document(&store, &dek, &doc_v1).unwrap() {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    store.upsert_indexed_file(&dir_id, &doc_v1.source_ref, "hash-v1", &first_id).unwrap();

    // caller 检测到 hash 变化后：软删旧 item + enqueue purge。
    store.delete_item(&first_id).unwrap();
    store.enqueue_reindex(&first_id, "purge").unwrap();
    let _ = store.record_signal_event("doc_update", &first_id, None);

    // 再调 ingest_document_replacing 入新内容，期望返回 Updated。
    let doc_v2 = md_doc("/tmp/a.md", "# V2\n\ncompletely new body.");
    let second = ingest_document_replacing(&store, &dek, &doc_v2, &first_id).unwrap();
    match second {
        IngestOutcome::Updated { item_id, old_item_id } => {
            assert_ne!(item_id, old_item_id);
            assert_eq!(old_item_id, first_id);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
}

#[test]
fn empty_content_returns_skipped() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let doc = md_doc("/tmp/blank.md", "   \n  \n");
    let outcome = ingest_document(&store, &dek, &doc).unwrap();
    assert!(matches!(outcome, IngestOutcome::Skipped { .. }));
    assert_eq!(store.item_count().unwrap(), 0);
}

#[test]
fn ingest_passes_through_domain_and_tags() {
    // 决策 1：RawDocument 的 domain / tags 必须透传给 insert_item，
    // 让入库 item 行带上来源域与用户标签（/api/v1/ingest 对外行为不变）。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut doc = md_doc("/tmp/tagged.md", "# Tagged\n\nbody with domain and tags.");
    doc.domain = Some("blog.example.com".into());
    doc.tags = Some(vec!["rust".into(), "ingest".into()]);

    let item_id = match ingest_document(&store, &dek, &doc).unwrap() {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    assert_eq!(item.domain.as_deref(), Some("blog.example.com"), "domain 必须透传");
    let tags = store.get_tags_json(&dek, &item_id).unwrap().expect("tags stored");
    assert!(tags.contains("rust") && tags.contains("ingest"), "tags 必须透传");
}

#[test]
fn ingest_injects_corpus_domain_prefix_into_chunks() {
    // 决策 2：corpus_domain != "general" 时，L1/L2 每个 chunk_text 必须被注入
    // `[领域: X] ` 前缀（F-Pro 跨域防污染），且 item 行 corpus_domain 被设置。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut doc = md_doc("/tmp/legal.md", "# Case\n\nlegal body paragraph here.");
    doc.corpus_domain = Some("legal".into());

    let item_id = match ingest_document(&store, &dek, &doc).unwrap() {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    // item 级 corpus_domain 标签必须落库（get_item_corpus_domain 独立查询）。
    let cd = store.get_item_corpus_domain(&item_id).unwrap();
    assert_eq!(cd, "legal", "item corpus_domain 必须设置");
    // 入队的每个 chunk_text 都应带 `[领域: legal] ` 前缀。
    let chunks = store.peek_embed_queue_chunk_texts(&item_id).unwrap();
    assert!(!chunks.is_empty(), "应有 chunk 入队");
    for c in &chunks {
        assert!(c.starts_with("[领域: legal] "), "chunk 必须带领域前缀: {c}");
    }
}

#[test]
fn ingest_general_corpus_domain_skips_prefix() {
    // corpus_domain == "general"（或 None）时不注入前缀 —— 通用文档零开销。
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let mut doc = md_doc("/tmp/general.md", "# Note\n\nplain general body.");
    doc.corpus_domain = Some("general".into());
    let item_id = match ingest_document(&store, &dek, &doc).unwrap() {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    let chunks = store.peek_embed_queue_chunk_texts(&item_id).unwrap();
    for c in &chunks {
        assert!(!c.starts_with("[领域:"), "general 不应注入前缀: {c}");
    }
}

#[test]
fn ingest_with_profile_threads_ocr_profile() {
    use attune_core::ingest::ingest_document_with_profile;
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    // 文本文档不触发 OCR；此测试只验证带 profile 入口编译且行为与无 profile 一致。
    let doc = md_doc("/tmp/p.md", "# Profile\n\nbody text.");
    let outcome = ingest_document_with_profile(&store, &dek, &doc, None).unwrap();
    assert!(matches!(outcome, IngestOutcome::Inserted { .. }));
}
