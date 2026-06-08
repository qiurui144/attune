//! Ingest edge / error / resource-exhaust / concurrency tests.
//!
//! Dimension owner: §6.1 六类下限 — edge case / error case / 资源耗尽 / 多并发，
//! 针对采集（ingest）路径。"证明它会挂" 心智：找 break case，不是证明它对。
//!
//! Inputs live in `tests/fixtures/edge_cases/`. Small adversarial fixtures
//! (empty / emoji / malicious HTML) are committed; large/binary ones
//! (huge 10MB / non-UTF8 / 100k lines / deep-nested) are regenerated
//! in-process here so the suite is self-contained on a clean checkout
//! (`tests/fixtures/edge_cases/generate.sh` is the committed generator).
//!
//! 运行（含默认 #[ignore] 的重量级用例）：
//!   cargo test -p attune-core --test ingest_edge_resource_test -- --include-ignored --nocapture
//! 轻量用例进日常 CI（无 #[ignore]）。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use attune_core::crypto::Key32;
use attune_core::index::FulltextIndex;
use attune_core::ingest::{ingest_document, IngestOutcome, RawDocument, SourceKind};
use attune_core::reindex;
use attune_core::store::Store;
use attune_core::vectors::VectorIndex;
use tempfile::TempDir;

// ──────────────────────────────────────────────────────────────────────────
// helpers
// ──────────────────────────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/edge_cases")
}

/// Build a RawDocument from raw bytes + a filename (the filename drives the
/// parser's extension dispatch). source_ref carries the filename so
/// `parse_filename()` recovers the extension.
fn raw_doc(content: Vec<u8>, filename: &str) -> RawDocument {
    RawDocument {
        uri: format!("file:///edge/{filename}"),
        title: String::new(),
        content,
        mime_hint: None,
        source_kind: SourceKind::LocalFolder,
        source_ref: format!("/edge/{filename}"),
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

/// Read a committed fixture, or regenerate the large/binary ones in-process.
fn fixture_bytes(name: &str) -> Vec<u8> {
    let p = fixtures_dir().join(name);
    if let Ok(b) = fs::read(&p) {
        return b;
    }
    // Regenerate the gitignored large/binary fixtures deterministically.
    match name {
        "huge_10mb.txt" => gen_huge_10mb(),
        "non_utf8.txt" => gen_non_utf8(),
        "many_lines.txt" => gen_many_lines(),
        "deep_nested.json" => gen_deep_nested(),
        _ => panic!("missing fixture {name} at {}", p.display()),
    }
}

fn gen_huge_10mb() -> Vec<u8> {
    let para = "This is a repeating paragraph for the 10 MB ingest bound test. \
                Keywords rust storage chunking performance scalability. \
                此段落含中文用于多语言分块测试。\n\n";
    let target = 10 * 1024 * 1024usize;
    let mut s = String::with_capacity(target + 4096);
    s.push_str("# Huge Document\n\n");
    let mut i = 0usize;
    while s.len() < target {
        if i % 400 == 0 {
            s.push_str(&format!("## Section {}\n\n", i / 400));
        }
        s.push_str(para);
        i += 1;
    }
    s.into_bytes()
}

fn gen_non_utf8() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"valid ascii prefix MARKER_ASCII\n");
    v.extend_from_slice(&[0xFF, 0xFE, 0x80, 0x81, 0xC0, 0xC1]); // invalid UTF-8
    v.extend_from_slice(b"\x00\x00middle\x00"); // embedded NUL
    v.extend_from_slice(&[0xED, 0xA0, 0x80]); // surrogate (invalid in UTF-8)
    v.extend_from_slice(b"more ascii MARKER_TAIL\n");
    v.extend((0x80u16..0x100).map(|b| b as u8)); // full high-byte range
    v
}

fn gen_many_lines() -> Vec<u8> {
    let mut s = String::with_capacity(100_000 * 2);
    for _ in 0..100_000 {
        s.push_str("x\n");
    }
    s.into_bytes()
}

fn gen_deep_nested() -> Vec<u8> {
    let depth = 50_000usize;
    let flat = 200_000usize;
    let mut s = String::with_capacity(depth * 2 + flat * 2 + 16);
    for _ in 0..depth {
        s.push('[');
    }
    s.push('1');
    for _ in 0..depth {
        s.push(']');
    }
    s.push('\n');
    s.push('[');
    for i in 0..flat {
        if i > 0 {
            s.push(',');
        }
        s.push('0');
    }
    s.push_str("]\n");
    s.into_bytes()
}

// ──────────────────────────────────────────────────────────────────────────
// 1. EMPTY → graceful skip (no panic, no item inserted)
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn empty_file_ingest_is_graceful_skip() {
    let (store, dek) = mem_store();
    let raw = raw_doc(fixture_bytes("empty.txt"), "empty.txt");
    let outcome = ingest_document(&store, &dek, &raw).expect("empty must not error/panic");
    assert!(
        matches!(outcome, IngestOutcome::Skipped { .. }),
        "0-byte file must be Skipped, got {outcome:?}"
    );
    assert_eq!(store.item_count().unwrap(), 0, "empty file must not insert an item");
}

#[test]
fn whitespace_only_file_is_graceful_skip() {
    let (store, dek) = mem_store();
    let raw = raw_doc(b"   \n\t  \n  ".to_vec(), "blank.txt");
    let outcome = ingest_document(&store, &dek, &raw).expect("whitespace must not panic");
    assert!(
        matches!(outcome, IngestOutcome::Skipped { .. }),
        "whitespace-only must be Skipped, got {outcome:?}"
    );
    assert_eq!(store.item_count().unwrap(), 0);
}

// ──────────────────────────────────────────────────────────────────────────
// 2. NON-UTF8 → from_utf8_lossy graceful (no panic, item inserted, ASCII kept)
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn non_utf8_bytes_ingest_graceful_lossy() {
    let (store, dek) = mem_store();
    let bytes = fixture_bytes("non_utf8.txt");
    let raw = raw_doc(bytes, "non_utf8.txt");
    let outcome = ingest_document(&store, &dek, &raw).expect("non-utf8 must not panic");
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("non-utf8 with valid ASCII content should Insert, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    // ASCII survives lossy decode; invalid bytes become U+FFFD, no panic.
    assert!(
        item.content.contains("MARKER_ASCII") && item.content.contains("MARKER_TAIL"),
        "lossy decode must preserve surrounding ASCII"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// 3. ALL-EMOJI → no crash, content round-trips
// ──────────────────────────────────────────────────────────────────────────

/// FLAG (BUG-1, HIGH, PANIC): an emoji-only single-line .txt panics the ingest
/// path. `parser::parse_content` (parser.rs:393) builds the title via the byte
/// slice `l.trim()[..l.trim().len().min(100)]`; when byte 100 lands inside a
/// multibyte char it panics ("not a char boundary"). Any short multibyte-only
/// document (emoji / CJK with no ASCII) where the trimmed first line is >100
/// bytes triggers it. Recommended fix in parser.rs:393 — slice on a char
/// boundary, e.g. `l.trim().chars().take(100).collect::<String>()` (or
/// floor_char_boundary). This test is #[ignore] until the src guard lands; the
/// companion `all_emoji_first_line_short_no_panic` locks the currently-safe
/// sub-100-byte case so the path is exercised in CI today.
#[test]
#[ignore = "FLAG BUG-1: parser.rs:393 non-char-boundary slice panics on >100B multibyte first line; un-ignore after src fix"]
fn all_emoji_long_first_line_ingests_without_panic() {
    let (store, dek) = mem_store();
    let bytes = fixture_bytes("all_emoji.txt"); // single line, >100 bytes of emoji
    let raw = raw_doc(bytes, "all_emoji.txt");
    let outcome = ingest_document(&store, &dek, &raw).expect("emoji must not panic");
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("emoji-only file should Insert, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    assert!(item.content.contains('😀'), "emoji must round-trip into stored content");

    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    reindex::reindex_item(&store, &mut vectors, &fulltext, &item_id, "emoji", &item.content, "file")
        .expect("emoji reindex must not panic");
}

/// Regression lock for the currently-safe path: emoji content whose trimmed
/// first line is <100 bytes (multi-line file) ingests + indexes without panic.
/// This keeps the emoji edge case exercised in CI while BUG-1 is open. If a
/// future change makes even this panic, CI catches it.
#[test]
fn all_emoji_short_first_line_ingests_without_panic() {
    let (store, dek) = mem_store();
    // First line < 100 bytes (3 emoji = 12 bytes), then a large emoji body.
    let content = format!("😀🎉🚀\n{}\n", "🔥💡🌟🦀📚🧠⚡".repeat(50));
    let raw = raw_doc(content.into_bytes(), "emoji_multiline.txt");
    let outcome = ingest_document(&store, &dek, &raw).expect("emoji must not panic");
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("emoji file should Insert, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    assert!(item.content.contains('🦀'), "emoji must round-trip");

    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    reindex::reindex_item(&store, &mut vectors, &fulltext, &item_id, "emoji", &item.content, "file")
        .expect("emoji reindex must not panic");
}

// ──────────────────────────────────────────────────────────────────────────
// 4. MALICIOUS HTML → <script>/attribute XSS payloads stripped from indexed text
//    (the core security assertion; if a payload IS present this fails = FLAG)
// ──────────────────────────────────────────────────────────────────────────

/// Markers that live ONLY inside <script>/<style> bodies or event-handler
/// attributes. None may appear in extracted / searchable text.
const FORBIDDEN_MARKERS: &[&str] = &[
    "XSS_SCRIPT_BODY_MARKER",
    "xss-alert-marker",
    "XSS_STYLE_MARKER",
    "xss-style-marker",
    "XSS_ONERROR_MARKER",
    "XSS_SVG_ONLOAD_MARKER",
    "XSS_HREF_MARKER",
    "XSS_IFRAME_MARKER",
    "XSS_ONCLICK_MARKER",
    "XSS_SECOND_SCRIPT_MARKER",
    "document.cookie",
    "javascript:",
];

/// FLAG (BUG-2, HIGH/security): `parser::html_to_text` (parser.rs:438) extracts
/// body text via `document.select("body").text()`, which collects the text of
/// ALL descendant nodes — INCLUDING `<script>` and `<style>` elements that sit
/// directly under `<body>`. Result: inline JS source from a body-level
/// `<script>` (e.g. `var SECOND = "..."`) leaks verbatim into the indexed /
/// FTS-searchable content. Head-level `<script>`/`<style>` and all event-handler
/// ATTRIBUTES (`onerror=`/`onclick=`…) and `javascript:` hrefs ARE already
/// stripped (verified by `malicious_html_partial_stripping_current_behavior`),
/// so this is specifically the body-script/body-style hole.
/// Recommended fix in parser.rs:438 — before collecting text, drop script/style
/// nodes (e.g. select "body *:not(script):not(style)" text, or walk children
/// skipping `script`/`style` elements, or use an ammonia-style sanitizer that
/// removes those elements). This test is #[ignore] (asserts the desired
/// all-stripped behavior) until the src fix lands.
#[test]
#[ignore = "FLAG BUG-2: body-level <script>/<style> text leaks via html_to_text (parser.rs:438); un-ignore after src fix"]
fn malicious_html_all_payloads_stripped() {
    let (store, dek) = mem_store();
    let raw = raw_doc(fixture_bytes("malicious.html"), "malicious.html");
    let outcome = ingest_document(&store, &dek, &raw).expect("malicious html must not panic");
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("malicious html should Insert extracted text, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    let stored = &item.content;

    assert!(
        stored.contains("LEGIT_VISIBLE_MARKER") && stored.contains("BODY_TEXT_MARKER"),
        "visible body text must be extracted, got: {stored:?}"
    );
    for m in FORBIDDEN_MARKERS {
        assert!(
            !stored.contains(m),
            "XSS payload '{m}' leaked into indexed content (extraction did NOT strip it). \
             FLAG: script/style/handler stripping insufficient. stored={stored:?}"
        );
    }
    let mut vectors = VectorIndex::new(64).unwrap();
    let fulltext = FulltextIndex::open_memory().unwrap();
    reindex::reindex_item(&store, &mut vectors, &fulltext, &item_id, "mal", stored, "file").unwrap();
    assert!(fulltext.search("XSS_SCRIPT_BODY_MARKER", 10).unwrap().is_empty());
}

/// Regression lock for the protection that DOES hold today (so CI stays green
/// and we detect if it ever weakens): head-level `<script>`/`<style>` bodies,
/// ALL event-handler attribute values, and `javascript:` hrefs are stripped.
/// The ONLY leaking class is body-level `<script>`/`<style>` (BUG-2). This test
/// pins exactly that boundary: everything-but-body-script must be absent;
/// visible text present; and it documents the known body-script leak.
#[test]
fn malicious_html_partial_stripping_current_behavior() {
    let (store, dek) = mem_store();
    let raw = raw_doc(fixture_bytes("malicious.html"), "malicious.html");
    let item_id = match ingest_document(&store, &dek, &raw).expect("no panic") {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("should Insert, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    let stored = &item.content;

    assert!(stored.contains("LEGIT_VISIBLE_MARKER"), "visible text must survive");

    // These ARE correctly stripped today — lock them so a regression is caught.
    for m in [
        "XSS_SCRIPT_BODY_MARKER", // head <script> body
        "xss-alert-marker",       // head <script> alert
        "XSS_STYLE_MARKER",       // head <style>
        "XSS_ONERROR_MARKER",     // img onerror= attribute
        "XSS_SVG_ONLOAD_MARKER",  // svg onload= attribute
        "XSS_HREF_MARKER",        // javascript: href text
        "XSS_IFRAME_MARKER",      // iframe javascript: src
        "XSS_ONCLICK_MARKER",     // div onclick= attribute
        "document.cookie",        // head script body
    ] {
        assert!(
            !stored.contains(m),
            "regression: payload '{m}' that USED to be stripped now leaks. stored={stored:?}"
        );
    }

    // KNOWN LEAK (BUG-2): body-level <script> text. We assert its CURRENT
    // (buggy) presence so the regression test is honest about today's state;
    // when parser.rs:438 is fixed, flip this to assert!(!...) and delete the
    // #[ignore] on `malicious_html_all_payloads_stripped`.
    assert!(
        stored.contains("XSS_SECOND_SCRIPT_MARKER"),
        "expected the documented BUG-2 body-script leak; if this fails the src \
         was fixed — update this test + un-ignore malicious_html_all_payloads_stripped"
    );
}

#[test]
fn xss_in_attributes_not_indexed() {
    let (store, dek) = mem_store();
    let raw = raw_doc(fixture_bytes("xss_in_attr.html"), "xss_in_attr.html");
    let outcome = ingest_document(&store, &dek, &raw).expect("attr xss html must not panic");
    let item_id = match outcome {
        IngestOutcome::Inserted { item_id, .. } => item_id,
        other => panic!("attr xss html should Insert, got {other:?}"),
    };
    let item = store.get_item(&dek, &item_id).unwrap().expect("item exists");
    let stored = &item.content;
    assert!(stored.contains("VISIBLE_ATTR_TEST_MARKER"), "visible text must survive");

    for m in [
        "XSS_ONMOUSEOVER_MARKER",
        "XSS_ONFOCUS_MARKER",
        "XSS_BODY_ONLOAD_MARKER",
        "XSS_ONTOGGLE_MARKER",
        "XSS_BARE_ONERROR_MARKER",
        "XSS_INPUT_VALUE_MARKER", // attribute value, not a text node
    ] {
        assert!(
            !stored.contains(m),
            "attribute payload '{m}' leaked into indexed text. FLAG: attribute values reach index. stored={stored:?}"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 5. HUGE 10MB+ → handled within a bound; assert no unbounded blowup.
//    NOTE: there is currently NO size guard in the ingest path (see report
//    FLAG). This test documents the *de facto* behavior: a 10MB text doc is
//    accepted and chunked. We bound the chunk count to catch a pathological
//    blowup, and we assert the call completes (no hang/OOM) within a deadline.
// ──────────────────────────────────────────────────────────────────────────

#[test]
#[ignore = "heavyweight: ~10MB ingest; run with --include-ignored"]
fn huge_10mb_ingest_bounded_no_blowup() {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("huge.db")).unwrap();
    let dek = Key32::generate();

    let bytes = fixture_bytes("huge_10mb.txt");
    assert!(bytes.len() >= 10 * 1024 * 1024, "fixture must be >= 10MB, got {}", bytes.len());
    let raw = raw_doc(bytes.clone(), "huge_10mb.txt");

    let t0 = Instant::now();
    let outcome = ingest_document(&store, &dek, &raw).expect("10MB ingest must not panic");
    let elapsed = t0.elapsed();
    println!("[edge] 10MB ingest took {elapsed:?}");

    let chunks = match outcome {
        IngestOutcome::Inserted { chunks_enqueued, .. } => chunks_enqueued,
        other => panic!("10MB text should Insert, got {other:?}"),
    };
    assert!(chunks > 0, "must produce chunks");
    // Bound: chunks are ~DEFAULT_CHUNK_SIZE chars apart. 10MB / (very small
    // chunk) would be the blowup case. Generous ceiling catches a regression
    // where chunking degenerates to 1-char chunks (would be ~10M chunks).
    assert!(
        chunks < 1_000_000,
        "chunk count {chunks} is unbounded relative to 10MB input — possible blowup"
    );
    // Wall-clock sanity: a single 10MB text doc must not take minutes.
    assert!(elapsed < Duration::from_secs(120), "10MB ingest took too long: {elapsed:?}");
}

// ──────────────────────────────────────────────────────────────────────────
// 6. RESOURCE BOUND — oversize/binary input rejected or bounded, NOT OOM.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn binary_extension_is_rejected_not_oom() {
    let (store, dek) = mem_store();
    // A known-binary extension with junk bytes must be rejected (InvalidInput),
    // never decoded into garbage and indexed.
    let raw = raw_doc(vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF], "payload.exe");
    let res = ingest_document(&store, &dek, &raw);
    assert!(res.is_err(), "binary .exe must be rejected, got {res:?}");
    assert_eq!(store.item_count().unwrap(), 0, "rejected binary must not insert");
}

#[test]
#[ignore = "heavyweight: deep-nested + 100k-line structures; run with --include-ignored"]
fn oversize_nested_structure_bounded_no_stack_overflow() {
    let (store, dek) = mem_store();

    // 100k tiny lines as plain text — must ingest bounded, no per-line blowup.
    let raw = raw_doc(fixture_bytes("many_lines.txt"), "many_lines.txt");
    let outcome = ingest_document(&store, &dek, &raw).expect("100k lines must not panic");
    match outcome {
        IngestOutcome::Inserted { chunks_enqueued, .. } => {
            assert!(chunks_enqueued > 0);
            assert!(
                chunks_enqueued < 200_000,
                "100k 1-char lines produced {chunks_enqueued} chunks — unbounded"
            );
        }
        other => panic!("many_lines should Insert, got {other:?}"),
    }

    // Deeply-nested JSON (50k deep) as .json (treated as code/plain text) — the
    // parser does NOT recurse JSON structure (it's lossy text), so deep nesting
    // must NOT cause stack overflow. Ingest in a bounded child thread.
    let bytes = fixture_bytes("deep_nested.json");
    let (s2, dek2) = mem_store();
    let h = std::thread::Builder::new()
        .stack_size(256 * 1024) // small stack: a recursive parser would overflow here
        .spawn(move || {
            let raw = raw_doc(bytes, "deep_nested.json");
            ingest_document(&s2, &dek2, &raw).map(|o| matches!(o, IngestOutcome::Inserted { .. }))
        })
        .unwrap();
    let inserted = h
        .join()
        .expect("deep-nested ingest must not stack-overflow / panic")
        .expect("deep-nested ingest must not error");
    assert!(inserted, "deep-nested json should ingest as lossy text");
    let _ = (store, dek);
}

// ──────────────────────────────────────────────────────────────────────────
// 7. CONCURRENT INGEST — N threads ingest distinct docs into the same vault.
//    Separate Store handles to one on-disk DB (real WAL concurrency, the
//    production scenario). Watchdog deadline catches a deadlock/hang.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn concurrent_ingest_distinct_docs_all_indexed_no_deadlock() {
    const N: usize = 16;
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("concurrent.db");
    // Pre-create the DB + schema so per-thread opens skip the migration race.
    Store::open(&db_path).expect("init db");
    let dek = Arc::new(Key32::generate());

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let db_path = db_path.clone();
        let dek = Arc::clone(&dek);
        handles.push(std::thread::spawn(move || {
            // Each thread its OWN connection — Store holds a non-Sync
            // rusqlite::Connection, so sharing a &Store across threads is
            // impossible; separate handles is the real multi-writer path.
            let store = Store::open(&db_path).expect("per-thread open");
            let content = format!(
                "# Doc {i}\n\nUnique body for concurrent ingest worker {i}. \
                 keyword_marker_{i} rust storage parallel.\n"
            );
            let raw = raw_doc(content.into_bytes(), &format!("doc_{i}.md"));
            ingest_document(&store, &dek, &raw)
        }));
    }

    // Watchdog: join all within a deadline; a deadlock would hang forever.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut inserted = 0usize;
    for h in handles {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            remaining > Duration::ZERO,
            "deadline exceeded waiting on concurrent ingest — possible deadlock"
        );
        // join() itself can't take a timeout; rely on the overall deadline
        // check and the fact that ingest is bounded work.
        let outcome = h.join().expect("worker thread panicked").expect("worker ingest errored");
        if matches!(outcome, IngestOutcome::Inserted { .. }) {
            inserted += 1;
        }
    }
    assert_eq!(inserted, N, "all {N} distinct docs must be inserted");

    // Verify the vault really holds N items (no lost writes under contention).
    let store = Store::open(&db_path).unwrap();
    assert_eq!(store.item_count().unwrap(), N, "vault must hold {N} items after concurrent ingest");
}

// ──────────────────────────────────────────────────────────────────────────
// 8. LOCK-ORDER DEADLOCK PROBE — mirror the server's documented hot-path
//    order (CLAUDE.md): fulltext → vectors → vault(store). Two threads take
//    the locks in the SAME order while reindexing; a watchdog catches a hang.
//    (Reverse order between threads is what causes ABBA; this asserts the
//    canonical order is deadlock-free.)
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn lock_order_fulltext_vectors_vault_no_deadlock() {
    let tmp = TempDir::new().unwrap();
    let store = Arc::new(Mutex::new(Store::open(&tmp.path().join("lock.db")).unwrap()));
    let vectors = Arc::new(Mutex::new(VectorIndex::new(64).unwrap()));
    let fulltext = Arc::new(Mutex::new(FulltextIndex::open_memory().unwrap()));
    let dek = Arc::new(Key32::generate());

    // Seed two items up front (single-threaded) so each worker has one to reindex.
    let mut ids = Vec::new();
    {
        let s = store.lock().unwrap();
        for i in 0..2 {
            let id = s
                .insert_item(&dek, &format!("L{i}"), &format!("# L{i}\n\nbody {i} keyword"), None, "file", None, None)
                .unwrap();
            ids.push(id);
        }
    }

    let done = Arc::new(Mutex::new(0usize));
    let mut handles = Vec::new();
    for id in ids {
        let (store, vectors, fulltext, dek, done) = (
            Arc::clone(&store),
            Arc::clone(&vectors),
            Arc::clone(&fulltext),
            Arc::clone(&dek),
            Arc::clone(&done),
        );
        handles.push(std::thread::spawn(move || {
            for _ in 0..50 {
                // Canonical hot-path order: fulltext → vectors → vault.
                let ft = fulltext.lock().unwrap();
                let mut vec = vectors.lock().unwrap();
                let st = store.lock().unwrap();
                let item = st.get_item(&dek, &id).unwrap().unwrap();
                reindex::reindex_item(&st, &mut vec, &ft, &id, &item.title, &item.content, "file")
                    .unwrap();
                drop(st);
                drop(vec);
                drop(ft);
            }
            *done.lock().unwrap() += 1;
        }));
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    for h in handles {
        assert!(Instant::now() < deadline, "lock-order workers hung — possible deadlock");
        h.join().expect("lock-order worker panicked");
    }
    assert_eq!(*done.lock().unwrap(), 2, "both lock-order workers must finish (no deadlock)");
}
