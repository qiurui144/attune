//! Multilingual ingest + search coverage (docs/TESTING.md §2.6, corpus I18N,
//! axis B "language i18n").
//!
//! Dimension owner: this slice re-does the i18n axis B (中英 ✅ was the prior
//! status; JP/KR/繁/RTL/emoji/non-UTF8 were ❌待补). Mindset is §6.1 "证明它会挂":
//! we look for the break cases at the lexical layer and DOCUMENT them honestly
//! rather than asserting every script searches well.
//!
//! Two layers under test:
//!   1. INGEST — `parser::parse_bytes` → `from_utf8_lossy`. Must never panic;
//!      non-UTF8 (GBK / Shift-JIS) degrades gracefully (ASCII markers survive);
//!      emoji round-trips; a non-emoji token in an emoji doc stays present.
//!   2. FTS LEXICAL — `JiebaTokenizer → LowerCaser → Stemmer(English)`. We RECORD
//!      actual per-script behavior. Empirical findings (probed 2026-06-08, locked
//!      below; see reports/2026-06-08_test-expand-i18n.md):
//!        - English: lowercased + stemmed (Running/RUNS → run).
//!        - CJK incl. JP **kanji** + Traditional Chinese: jieba's Han dictionary
//!          segments real words (東京/日本/首都/機械/学習/臺北/股東決議 …) → native
//!          queries hit precisely.
//!        - Korean Hangul / Arabic / Hebrew: jieba has NO model → splits into
//!          per-syllable / per-LETTER tokens (NO word segmentation). Own-word
//!          recall works and multi-syllable cross-word does NOT over-match
//!          (BM25 conjunction filters partial overlaps), BUT a SINGLE-character
//!          query over-matches every doc containing that syllable/letter — the
//!          concrete false-positive face of the gap. Real recall for these
//!          scripts must come from the VECTOR layer (bge-m3, multilingual).
//!          FLAG, not a src change here (see the gap test for the precise pins).
//!
//! Embedding shape: validated with `MockEmbeddingProvider` (deterministic, no
//! network) — real bge-m3 is not run here (no Ollama in CI). The mock proves the
//! provider accepts every script as valid input and returns correct-dim vectors;
//! it is NOT a claim about bge-m3 semantic quality.
//!
//! Run (verbose, prints the lexical support map):
//!   cargo test -p attune-core --test i18n_ingest_search_test -- --nocapture

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use attune_core::crypto::Key32;
use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider};
use attune_core::index::FulltextIndex;
use attune_core::ingest::{ingest_document, IngestOutcome, RawDocument, SourceKind};
use attune_core::parser;
use attune_core::reindex;
use attune_core::store::{DecryptedItem, Store};
use attune_core::vectors::{VectorIndex, VectorMeta};

// ──────────────────────────────────────────────────────────────────────────
// helpers
// ──────────────────────────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/i18n")
}

/// Read a committed fixture, or regenerate the non-UTF8 (gitignored) ones
/// in-process byte-for-byte identical to `generate.sh`.
fn fixture_bytes(name: &str) -> Vec<u8> {
    let p = fixtures_dir().join(name);
    if let Ok(b) = fs::read(&p) {
        return b;
    }
    match name {
        "gbk_simplified.txt" => gen_gbk(),
        "shift_jis_japanese.txt" => gen_shift_jis(),
        _ => panic!("missing fixture {name} at {}", p.display()),
    }
}

/// GBK-encoded Simplified Chinese (legacy Windows-CN). Byte sequence captured
/// from `generate.sh`'s `"...".encode("gbk")`. Starts with an ASCII marker that
/// MUST survive `from_utf8_lossy`.
fn gen_gbk() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"ASCII_GBK_MARKER ");
    v.extend_from_slice(&[
        0xbc, 0xf2, 0xcc, 0xe5, 0xd6, 0xd0, 0xce, 0xc4, // 简体中文
    ]);
    v.extend_from_slice(b" GBK ");
    v.extend_from_slice(&[
        0xb1, 0xe0, 0xc2, 0xeb, 0xb2, 0xe2, 0xca, 0xd4, // 编码测试
    ]);
    v.extend_from_slice(b" ");
    v.extend_from_slice(&[0xb9, 0xc9, 0xb6, 0xab, 0xbe, 0xf6, 0xd2, 0xe9]); // 股东决议
    v.extend_from_slice(b"\n");
    v.extend_from_slice(&[0xba, 0xcb, 0xd0, 0xc4, 0xb4, 0xca]); // 核心词
    v.extend_from_slice(b" ");
    v.extend_from_slice(&[0xbc, 0xec, 0xcb, 0xf7]); // 检索
    v.extend_from_slice(b"\n");
    v
}

/// Shift-JIS-encoded Japanese (legacy Windows-JP). Byte sequence captured from
/// `generate.sh`'s `"...".encode("shift_jis")`.
fn gen_shift_jis() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"ASCII_SJIS_MARKER ");
    v.extend_from_slice(&[0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea]); // 日本語
    v.extend_from_slice(b" Shift_JIS ");
    v.extend_from_slice(&[
        0x83, 0x47, 0x83, 0x93, 0x83, 0x52, 0x81, 0x5b, 0x83, 0x68, // エンコード
    ]);
    v.extend_from_slice(b" ");
    v.extend_from_slice(&[0x83, 0x65, 0x83, 0x58, 0x83, 0x67]); // テスト
    v.extend_from_slice(b"\n");
    v.extend_from_slice(&[0x8b, 0x40, 0x8a, 0x42, 0x8a, 0x77, 0x8f, 0x4b]); // 機械学習
    v.extend_from_slice(b"\n");
    v
}

fn raw_doc(content: Vec<u8>, filename: &str) -> RawDocument {
    RawDocument {
        uri: format!("file:///i18n/{filename}"),
        title: String::new(),
        content,
        mime_hint: None,
        source_kind: SourceKind::LocalFolder,
        source_ref: format!("/i18n/{filename}"),
        modified_marker: None,
        domain: None,
        tags: None,
        corpus_domain: None,
        metadata: HashMap::new(),
    }
}

fn mem_store() -> (Store, Key32) {
    (Store::open_memory().expect("open_memory"), Key32::generate())
}

/// Ingest one fixture and return its stored item (content after parse).
fn ingest_fixture(name: &str) -> (Store, Key32, String, DecryptedItem) {
    let (store, dek) = mem_store();
    let raw = raw_doc(fixture_bytes(name), name);
    let outcome = ingest_document(&store, &dek, &raw)
        .unwrap_or_else(|e| panic!("ingest of {name} must not error: {e}"));
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("{name} should Insert (has searchable content), got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    (store, dek, item_id, item)
}

// ──────────────────────────────────────────────────────────────────────────
// 1. INGEST — no panic, content round-trips, native markers survive
// ──────────────────────────────────────────────────────────────────────────

/// Each UTF-8 script fixture ingests without panic and its ASCII + native
/// markers are preserved in the stored content.
#[test]
fn utf8_scripts_ingest_without_panic_and_preserve_markers() {
    let cases: &[(&str, &[&str])] = &[
        ("japanese.md", &["JPMARKER", "ALPHA_TOKEN_JP", "東京", "機械学習"]),
        ("korean.md", &["KRMARKER", "ALPHA_TOKEN_KR", "서울", "자연어"]),
        ("traditional_chinese.md", &["TWMARKER", "ALPHA_TOKEN_TW", "臺北", "股東決議"]),
        ("arabic_rtl.md", &["ARMARKER", "ALPHA_TOKEN_AR", "القاهرة"]),
        ("hebrew_rtl.txt", &["HEMARKER", "ALPHA_TOKEN_HE", "ירושלים"]),
        ("emoji_heavy.md", &["EMOJIMARKER", "RUSTACEAN_TOKEN", "中文检索", "🦀"]),
    ];
    for (file, markers) in cases {
        let (_s, _d, _id, item) = ingest_fixture(file);
        for m in *markers {
            assert!(
                item.content.contains(m),
                "{file}: marker {m:?} must survive ingest into stored content"
            );
        }
    }
}

/// Non-UTF8 bytes (GBK / Shift-JIS) must NOT panic on the ingest (bytes) path;
/// `from_utf8_lossy` keeps the ASCII marker and replaces invalid bytes with
/// U+FFFD. The non-ASCII legacy-encoded text becomes mojibake (expected — we do
/// no charset sniffing), but the doc is still ingested and the ASCII token is
/// searchable.
#[test]
fn non_utf8_legacy_encodings_ingest_graceful_lossy() {
    for (file, ascii_marker) in [
        ("gbk_simplified.txt", "ASCII_GBK_MARKER"),
        ("shift_jis_japanese.txt", "ASCII_SJIS_MARKER"),
    ] {
        let (_s, _d, _id, item) = ingest_fixture(file);
        assert!(
            item.content.contains(ascii_marker),
            "{file}: ASCII marker {ascii_marker:?} must survive from_utf8_lossy"
        );
        // Invalid GBK/SJIS multibyte sequences are not valid UTF-8 → become the
        // replacement char. We don't require it (some bytes may form accidental
        // valid UTF-8), but the content must be a valid Rust String (no panic
        // reaching here proves it).
        assert!(!item.content.is_empty());
    }
}

/// PANIC GUARD on the *file path* (`parse_file`), which uses
/// `std::fs::read_to_string` and therefore ERRORS (does not lossy-decode) on
/// non-UTF8 input — asymmetric with the bytes path. We pin that asymmetry so a
/// future refactor that routes file ingest through `read_to_string` for
/// arbitrary bytes is caught. (Not a bug: scanner/upload go through the bytes
/// path; documented in the lexical map.)
#[test]
fn parse_file_path_errors_on_non_utf8_bytes_path_is_lossy() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("legacy.txt");
    fs::write(&p, gen_gbk()).unwrap();

    // file path: read_to_string rejects invalid UTF-8 with an Io error (no panic).
    let file_res = parser::parse_file(&p);
    assert!(
        file_res.is_err(),
        "parse_file uses read_to_string → must Err on non-UTF8, not silently mojibake"
    );

    // bytes path: lossy, succeeds, ASCII survives.
    let (_t, content) = parser::parse_bytes(&gen_gbk(), "legacy.txt").expect("bytes path is lossy");
    assert!(content.contains("ASCII_GBK_MARKER"));
}

// ──────────────────────────────────────────────────────────────────────────
// 2. FTS LEXICAL — invariants we ARE confident about
// ──────────────────────────────────────────────────────────────────────────

/// Emoji-heavy doc: ingest + reindex never panic, and a NON-emoji token in the
/// same doc stays searchable via the lexical layer. (Task requirement.)
#[test]
fn emoji_doc_non_emoji_token_still_searchable() {
    let (store, _dek, item_id, item) = ingest_fixture("emoji_heavy.md");

    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    reindex::reindex_item(
        &store, &mut vectors, &fulltext, &item_id, &item.title, &item.content, "file",
    )
    .expect("emoji doc reindex must not panic");

    for token in ["RUSTACEAN_TOKEN", "EMOJIMARKER", "中文检索"] {
        let hits = fulltext.search(token, 10).unwrap();
        assert!(
            hits.iter().any(|(id, _)| id == &item_id),
            "non-emoji token {token:?} in an emoji-heavy doc must stay FTS-searchable"
        );
    }
}

/// CJK incl. Japanese **kanji** + Traditional Chinese: jieba segments real words
/// → native-script queries hit precisely. This is the lexical layer working as
/// designed for Han-script content.
#[test]
fn cjk_and_kanji_native_queries_hit_via_jieba() {
    let idx = FulltextIndex::open_memory().unwrap();
    idx.add_document(
        "jp",
        "日本語テスト",
        "東京は日本の首都です 機械学習 自然言語処理",
        "note",
    )
    .unwrap();
    idx.add_document(
        "tw",
        "繁體中文",
        "臺北 機器學習 股東決議 智慧財產權",
        "note",
    )
    .unwrap();

    // Japanese kanji compounds segment + hit.
    for (q, want) in [("東京", "jp"), ("日本", "jp"), ("首都", "jp"), ("機械学習", "jp")] {
        let hits = idx.search(q, 10).unwrap();
        assert!(
            hits.iter().any(|(id, _)| id == want),
            "JP kanji query {q:?} should hit doc {want}"
        );
    }
    // Traditional-Chinese compounds (incl. legal terms) segment + hit.
    for (q, want) in [("臺北", "tw"), ("機器學習", "tw"), ("股東決議", "tw")] {
        let hits = idx.search(q, 10).unwrap();
        assert!(
            hits.iter().any(|(id, _)| id == want),
            "Traditional-Chinese query {q:?} should hit doc {want}"
        );
    }
}

/// English stemming + case folding through the shared analyzer (regression
/// companion to index.rs's own unit test, exercised on i18n-mixed content).
#[test]
fn english_lowercased_and_stemmed_in_mixed_doc() {
    let idx = FulltextIndex::open_memory().unwrap();
    idx.add_document("mix", "Mixed 混合", "Running 検索 RUSTACEAN searches", "note").unwrap();
    for q in ["running", "run", "rustacean", "search"] {
        let hits = idx.search(q, 10).unwrap();
        assert!(
            hits.iter().any(|(id, _)| id == "mix"),
            "english query {q:?} should hit after LowerCaser+Stemmer"
        );
    }
    // CJK in the same mixed doc still segments.
    assert!(idx.search("検索", 10).unwrap().iter().any(|(id, _)| id == "mix"));
}

// ──────────────────────────────────────────────────────────────────────────
// 3. FTS LEXICAL — RECORDED behavior + documented GAP (Hangul / Arabic / Hebrew)
// ──────────────────────────────────────────────────────────────────────────

/// DOCUMENTED GAP (FLAG, not hidden): for scripts jieba has no model for
/// (Korean Hangul, Arabic, Hebrew), the tokenizer falls back to **per-syllable /
/// per-LETTER** tokens — there is NO word segmentation. Empirically pinned
/// (probed 2026-06-08, exact tokens in reports/2026-06-08_test-expand-i18n.md):
///   - "학생" → ["학","생"]; "학교" → ["학","교"]; "서울" → ["서","울"]
///   - Arabic "القاهرة" → ["ا","ل","ق","ا","ه","ر","ة"] (every letter a token)
///
/// Consequences this test pins (the real, observed behavior — not the naive
/// "any shared char over-matches"):
///   (a) own-word recall works (the word's chars are all present): "서울" hits
///       its doc, "학생" hits its doc.
///   (b) a multi-syllable query that shares only SOME syllables with an unrelated
///       doc does NOT necessarily over-match — BM25 + QueryParser conjunction
///       ranks the doc with all the query's chars far above one with a partial
///       overlap. So "학생" does NOT return the doc that only has "학교".
///   (c) BUT a **single-character** query over-matches BROADLY: "학" returns BOTH
///       kr_a (학교) and kr_b (학생) — meaningless recall. This is the concrete
///       false-positive face of having no segmentation; it would be far worse in
///       a real corpus where common syllables appear everywhere.
/// If a future per-language / ICU tokenizer lands, (c) flips (single char no
/// longer a valid token boundary) → this test fails → forces updating the map.
///
/// Recommendation (FLAG for index.rs follow-up, NOT changed here):
///   - Lexical recall for Hangul/Arabic/Hebrew is unreliable (no segmentation,
///     single-char over-match); rely on the VECTOR layer (bge-m3, multilingual)
///     for these scripts today.
///   - To fix the lexical layer: register an ICU / unicode-word-boundary
///     tokenizer for non-CJK runs (tantivy `icu` feature or a
///     `unicode-segmentation`-based tokenizer), or a per-script analyzer keyed
///     off detected script. That is a `register_tokenizers()` change in
///     src/index.rs:152 + a `TOKENIZER_VERSION` bump (forces index rebuild) —
///     out of scope for this test-only slice.
#[test]
fn hangul_arabic_hebrew_are_character_level_matched_documented_gap() {
    let idx = FulltextIndex::open_memory().unwrap();
    // Two unrelated Korean docs that share the syllable 학 but are different
    // words: 학교(school) in kr_a, 학생(student) in kr_b.
    idx.add_document("kr_a", "t", "서울 학교 도서관", "note").unwrap();
    idx.add_document("kr_b", "t", "부산 학생 공원", "note").unwrap();

    // (a) own-word recall works.
    assert!(
        idx.search("서울", 10).unwrap().iter().any(|(id, _)| id == "kr_a"),
        "Korean own-word 서울 should match its doc kr_a"
    );
    assert!(
        idx.search("학생", 10).unwrap().iter().any(|(id, _)| id == "kr_b"),
        "Korean own-word 학생 should match its doc kr_b"
    );

    // (b) multi-syllable cross-word does NOT over-match: 학생 (only in kr_b) must
    // NOT return kr_a even though kr_a shares the syllable 학 (학교). Pins that
    // the gap is per-CHARACTER, not "any shared syllable poisons recall".
    assert!(
        !idx.search("학생", 10).unwrap().iter().any(|(id, _)| id == "kr_a"),
        "학생 must NOT match kr_a (which only shares 학) — conjunction/BM25 filters it"
    );

    // (c) FALSE-POSITIVE proof: a SINGLE-syllable query 학 over-matches BOTH docs
    // (학교 in kr_a, 학생 in kr_b) — no word boundary, so the syllable alone is a
    // valid query token hitting everything. THIS is the documented lexical gap.
    // If this ever fails, segmentation improved → update §2.6 map + this test.
    let single = idx.search("학", 10).unwrap();
    assert!(
        single.iter().any(|(id, _)| id == "kr_a") && single.iter().any(|(id, _)| id == "kr_b"),
        "DOCUMENTED GAP: single syllable 학 over-matches both unrelated Korean docs \
         (no segmentation). If this fails, the lexical layer gained Korean word \
         boundaries — update the i18n lexical map + remove the gap note."
    );

    // Arabic: same per-LETTER fallback. Single shared letter ة over-matches both.
    let aidx = FulltextIndex::open_memory().unwrap();
    aidx.add_document("ar_a", "t", "القاهرة عاصمة مصر", "note").unwrap();
    aidx.add_document("ar_b", "t", "مدينة كبيرة جميلة", "note").unwrap();
    assert!(
        aidx.search("القاهرة", 10).unwrap().iter().any(|(id, _)| id == "ar_a"),
        "Arabic own-word القاهرة should match its doc ar_a"
    );
    let letter = aidx.search("ة", 10).unwrap();
    assert!(
        letter.iter().any(|(id, _)| id == "ar_a") && letter.iter().any(|(id, _)| id == "ar_b"),
        "DOCUMENTED GAP: single Arabic letter ة over-matches both unrelated docs (no segmentation)"
    );
}

/// Prints the actual lexical support map to stdout (run with --nocapture). This
/// is the human-readable evidence captured in the report; it asserts nothing new
/// but keeps the recorded behavior reproducible alongside the suite.
#[test]
fn print_lexical_multilingual_support_map() {
    let probe = |label: &str, title: &str, content: &str, queries: &[&str]| {
        let idx = FulltextIndex::open_memory().unwrap();
        idx.add_document(label, title, content, "note").unwrap();
        println!("--- {label} ---");
        for q in queries {
            let hit = idx.search(q, 10).unwrap().iter().any(|(id, _)| id == label);
            println!("    {q:<18} hit={hit}");
        }
    };
    println!("\n=== FTS lexical multilingual support map (JiebaTokenizer→LowerCaser→Stemmer) ===");
    probe("en", "t", "Running searches RUSTACEAN", &["running", "run", "rustacean"]);
    probe("zh", "t", "向量检索 股东决议", &["检索", "股东决议"]);
    probe("jp", "t", "東京は日本の首都です 機械学習", &["東京", "機械学習", "首都"]);
    probe("kr", "t", "서울은 대한민국의 수도", &["서울", "수도"]);
    probe("ar", "t", "القاهرة عاصمة مصر", &["القاهرة", "مصر"]);
    probe("he", "t", "ירושלים בירת ישראל", &["ירושלים", "ישראל"]);
    probe("emoji", "t", "🚀 RUSTACEAN 中文检索 😀", &["rustacean", "检索", "🚀"]);
}

// ──────────────────────────────────────────────────────────────────────────
// 4. EMBEDDING — every script is valid embedding input (shape/dim ok)
//    via MockEmbeddingProvider (deterministic, no network; NOT a bge-m3 quality
//    claim — see module doc).
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn every_script_is_valid_embedding_input_correct_dim() {
    const DIM: usize = 1024; // bge-m3 native dim
    let emb = MockEmbeddingProvider::new(DIM);

    let samples: &[(&str, String)] = &[
        ("japanese", "東京は日本の首都です 機械学習".to_string()),
        ("korean", "서울은 대한민국의 수도입니다".to_string()),
        ("traditional", "臺北 機器學習 股東決議".to_string()),
        ("arabic", "القاهرة عاصمة مصر التعلم الآلي".to_string()),
        ("hebrew", "ירושלים בירת ישראל למידת מכונה".to_string()),
        // emoji incl. ZWJ family + skin-tone modifier
        ("emoji", "🚀🔥 RUSTACEAN 👨‍👩‍👧‍👦 👍🏽 中文".to_string()),
        ("empty", String::new()),
    ];

    let texts: Vec<&str> = samples.iter().map(|(_, s)| s.as_str()).collect();
    let (vecs, _usage) = emb.embed(&texts).expect("embed all scripts without error");
    assert_eq!(vecs.len(), samples.len(), "one vector per input");
    for ((label, _), v) in samples.iter().zip(&vecs) {
        assert_eq!(v.len(), DIM, "{label}: embedding must have provider dim {DIM}");
        assert!(
            v.iter().all(|x| x.is_finite()),
            "{label}: embedding must contain no NaN/Inf (cos-distance safety)"
        );
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(norm > 0.0, "{label}: embedding must be non-zero (usearch unit-vector)");
    }
    assert_eq!(emb.dimensions(), DIM);
}

/// End-to-end through the real vector index: a CJK + multi-script doc embeds and
/// indexes into usearch at the provider dim without shape mismatch. Uses the
/// mock so it is deterministic and network-free.
#[test]
fn multiscript_doc_indexes_into_vector_index_via_mock() {
    const DIM: usize = 256;
    let emb = MockEmbeddingProvider::new(DIM);
    let mut vectors = VectorIndex::new(DIM).unwrap();

    let chunks = [
        "東京 機械学習",            // JP kanji
        "股東決議 智慧財產權",       // Traditional Chinese
        "서울 자연어 처리",          // Korean
        "القاهرة التعلم الآلي",     // Arabic
        "🦀 RUSTACEAN 中文检索",     // emoji + ascii + CJK
    ];
    for (i, c) in chunks.iter().enumerate() {
        let (vv, _) = emb.embed(&[c]).unwrap();
        assert_eq!(vv[0].len(), DIM);
        let meta = VectorMeta {
            item_id: format!("item-{i}"),
            chunk_idx: i,
            level: 2,
            section_idx: 0,
        };
        vectors
            .add(&vv[0], meta)
            .unwrap_or_else(|e| panic!("vector add for chunk {i} ({c:?}) failed: {e}"));
    }
    // Query with a CJK chunk → returns nearest (at least itself), no dim panic.
    let (qv, _) = emb.embed(&["東京 機械学習"]).unwrap();
    let res = vectors.search(&qv[0], 3).unwrap();
    assert!(!res.is_empty(), "multiscript vector search must return results");
}
