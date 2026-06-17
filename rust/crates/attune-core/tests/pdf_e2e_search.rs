//! E2E: PDF → real parse → real encrypted ingest → real FTS + real vector
//! search, by a Chinese query AND an English query.
//!
//! Exercises the production retrieval path end-to-end:
//!   1. parser::parse_file on the committed mixed-zhen.pdf (real pdf_extract)
//!   2. ingest_document into a real encrypted Store (Argon2id/AES vault DEK)
//!   3. populate a real FulltextIndex (multilingual jieba+LowerCaser+Stemmer)
//!      and a real VectorIndex (bge-m3 1024-d via live Ollama)
//!   4. search_with_context (real RRF fusion) with a CN query AND an EN query
//!
//! The vector leg uses a REAL bge-m3 embedding from a live Ollama at
//! ATTUNE_OLLAMA_URL (default http://localhost:11434). If Ollama / bge-m3 is
//! unavailable the test runs FTS-only and SAYS SO (it still proves the
//! multilingual FTS leg end-to-end; it does not silently claim real-vector).
//!
//! #[ignore] by default (needs a live embedding backend); run with:
//!   cargo test -p attune-core --test pdf_e2e_search -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use attune_core::crypto::Key32;
use attune_core::embed::{EmbeddingProvider, OllamaProvider};
use attune_core::index::FulltextIndex;
use attune_core::ingest::{ingest_document, IngestOutcome, RawDocument, SourceKind};
use attune_core::search::{search_with_context, SearchContext, SearchParams};
use attune_core::store::Store;
use attune_core::vectors::{VectorIndex, VectorMeta};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pdf")
        .join(name)
}

#[test]
#[ignore = "needs live Ollama bge-m3 (real vector leg); run with --ignored"]
fn pdf_ingest_then_search_cn_and_en_real_vector() {
    let ollama_url =
        std::env::var("ATTUNE_OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".into());

    // ── 1. Real PDF parse ──────────────────────────────────────────────────
    let pdf = fixture("mixed-zhen.pdf");
    let raw_bytes = std::fs::read(&pdf).expect("read mixed-zhen.pdf fixture");
    let (title, content) =
        attune_core::parser::parse_file(&pdf).expect("parse mixed-zhen.pdf via real pdf_extract");
    eprintln!("[1] parsed mixed-zhen.pdf: title={title:?} content={content:?}");
    assert!(!content.trim().is_empty(), "parsed PDF content must be non-empty");

    // ── 2. Real encrypted ingest ───────────────────────────────────────────
    let store = Store::open_memory().expect("open encrypted store");
    let dek = Key32::generate();
    let raw = RawDocument {
        uri: "file:///mixed-zhen.pdf".into(),
        title: String::new(), // let ingest fall back to the parser title
        content: raw_bytes,
        mime_hint: Some("application/pdf".into()),
        source_kind: SourceKind::LocalFolder,
        source_ref: "/mixed-zhen.pdf".into(),
        modified_marker: None,
        domain: None,
        tags: None,
        corpus_domain: None,
        metadata: std::collections::HashMap::new(),
    };
    let item_id = match ingest_document(&store, &dek, &raw).expect("ingest mixed-zhen.pdf") {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("expected Inserted, got {other:?}"),
    };
    eprintln!("[2] ingested item_id={item_id}");

    // Read the stored (decrypted) item — the SSOT the indexes derive from.
    let item = store
        .get_item(&dek, &item_id)
        .expect("get_item")
        .expect("item exists after ingest");

    // ── 3a. Real FTS index (multilingual tokenizer) ────────────────────────
    let fulltext = FulltextIndex::open_memory().expect("open fulltext");
    fulltext
        .add_document(&item.id, &item.title, &item.content, &item.source_type)
        .expect("fts add_document");

    // ── 3b. Real vector index via live bge-m3 (or FTS-only fallback) ────────
    let emb = OllamaProvider::new(&ollama_url, "bge-m3", 1024);
    let real_vector = emb.is_available();
    let mut vectors = VectorIndex::new(1024).expect("vector index");
    let embedding_arc: Option<Arc<dyn EmbeddingProvider>> = if real_vector {
        eprintln!("[3] embedding backend: REAL Ollama bge-m3 @ {ollama_url}");
        let (vecs, _usage) = emb
            .embed(&[item.content.as_str()])
            .expect("bge-m3 embed of document content");
        assert_eq!(vecs[0].len(), 1024, "bge-m3 must return 1024-d vectors");
        vectors
            .add(
                &vecs[0],
                VectorMeta {
                    item_id: item.id.clone(),
                    chunk_idx: 0,
                    level: 1,
                    section_idx: 0,
                },
            )
            .expect("vector add");
        Some(Arc::new(OllamaProvider::new(&ollama_url, "bge-m3", 1024)))
    } else {
        eprintln!(
            "[3] embedding backend: NONE (Ollama bge-m3 unavailable @ {ollama_url}) \
             — running FTS-ONLY; vector leg is MOCKED-OUT (empty), NOT real-vector"
        );
        None
    };

    // ── 4. Real fused search by a CN query AND an EN query ──────────────────
    let ctx = SearchContext {
        fulltext: Some(&fulltext),
        vectors: if real_vector { Some(&vectors) } else { None },
        embedding: embedding_arc,
        reranker: None,
        store: &store,
        dek: &dek,
    };
    let params = SearchParams::with_defaults(10);

    let cn_query = "向量检索";
    let en_query = "search";
    let cn_hits = search_with_context(&ctx, cn_query, &params).expect("cn search");
    let en_hits = search_with_context(&ctx, en_query, &params).expect("en search");

    eprintln!(
        "[4] CN query {cn_query:?} → {} hits; EN query {en_query:?} → {} hits",
        cn_hits.len(),
        en_hits.len()
    );

    assert!(
        cn_hits.iter().any(|r| r.item_id == item_id),
        "Chinese query {cn_query:?} must return the ingested PDF (hits={:?})",
        cn_hits.iter().map(|r| &r.item_id).collect::<Vec<_>>()
    );
    assert!(
        en_hits.iter().any(|r| r.item_id == item_id),
        "English query {en_query:?} must return the ingested PDF (hits={:?})",
        en_hits.iter().map(|r| &r.item_id).collect::<Vec<_>>()
    );

    // Case-insensitivity proof through the full path: uppercase EN query still hits.
    let en_upper = search_with_context(&ctx, "SEARCH", &params).expect("en upper search");
    assert!(
        en_upper.iter().any(|r| r.item_id == item_id),
        "uppercase 'SEARCH' must hit (LowerCaser); hits={:?}",
        en_upper.iter().map(|r| &r.item_id).collect::<Vec<_>>()
    );

    eprintln!(
        "[OK] E2E passed (vector leg: {})",
        if real_vector { "REAL bge-m3" } else { "FTS-ONLY (no real vector)" }
    );
}
