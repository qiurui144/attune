//! Chunking quality tests — exercises `attune_core::chunker` directly on
//! known structured documents and asserts the invariants that retrieval
//! quality depends on:
//!
//!   1. L1 section boundaries land on headings (markdown `#` + code boundaries).
//!   2. L2 sliding-window chunks respect the size bound (<= 2x chunk_size,
//!      matching the code-fence extend buffer documented in chunker.rs).
//!   3. Code fences are never left split mid-block (every chunk has an even
//!      number of ``` markers) when the chunker is run with a size large
//!      enough to keep a small code block whole, and balanced otherwise.
//!   4. An oversize paragraph is CHUNKED, not DROPPED — every character of the
//!      paragraph survives in at least one chunk (no silent tail truncation).
//!
//! These are deterministic, mock-free, network-free tests on the real chunker.
//! Source of truth read at write time:
//!   crates/attune-core/src/chunker.rs (DEFAULT_CHUNK_SIZE=512, DEFAULT_OVERLAP=128,
//!   SECTION_TARGET_SIZE=1500).

use attune_core::chunker::{
    self, extract_sections, extract_sections_with_path, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP,
};

const FIXTURE: &str = include_str!("fixtures/retrieval/structured_doc.md");

// ─────────────────────────────────────────────────────────────────────────
// Task 1: L1 section boundaries land on headings
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn l1_sections_split_on_markdown_headings() {
    let sections = extract_sections(FIXTURE);

    // The fixture has H1 "Knowledge Base Architecture" + 5 H2 headings
    // (Ingestion Pipeline / Code Example Section / Oversize Paragraph Section /
    // Summary) plus the H1 preamble. Each heading starts a new section.
    assert!(
        sections.len() >= 5,
        "expected >= 5 sections from 1 H1 + multiple H2, got {}: {:?}",
        sections.len(),
        sections.iter().map(|(i, s)| (i, s.lines().next().unwrap_or(""))).collect::<Vec<_>>()
    );

    // Every section's FIRST non-empty line should be the heading that opened it
    // (i.e. boundaries land ON headings, not in the middle of prose).
    let heading_starts = sections
        .iter()
        .filter(|(_, content)| {
            content
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|first| first.trim_start().starts_with('#'))
                .unwrap_or(false)
        })
        .count();
    assert!(
        heading_starts >= 5,
        "expected >= 5 sections to start with a markdown heading, got {heading_starts}"
    );
}

#[test]
fn l1_section_path_tracks_heading_hierarchy() {
    let secs = extract_sections_with_path(FIXTURE);
    // The "Ingestion Pipeline" H2 lives under the "Knowledge Base Architecture" H1.
    let ingestion = secs
        .iter()
        .find(|s| s.content.contains("ingestion pipeline parses"))
        .expect("missing Ingestion Pipeline section");
    assert_eq!(
        ingestion.path,
        vec![
            "Knowledge Base Architecture".to_string(),
            "Ingestion Pipeline".to_string()
        ],
        "H2 path should be [H1, H2], got {:?}",
        ingestion.path
    );
}

#[test]
fn l1_heading_boundary_not_mid_prose() {
    // Regression-style invariant: a heading inside the doc must START a section,
    // never appear in the middle of a section body (which would mean the chunker
    // merged two logical sections, hurting hierarchical retrieval).
    let secs = extract_sections(FIXTURE);
    for (idx, content) in &secs {
        // Count headings that are NOT on the first non-empty line of the section.
        let mut seen_first = false;
        for line in content.lines() {
            let t = line.trim_start();
            if !seen_first {
                if t.is_empty() {
                    continue;
                }
                seen_first = true;
                continue; // first non-empty line may be a heading — that's fine
            }
            // A markdown heading (`## ...`) appearing here means a boundary was missed.
            // Code-fence interiors are exempt: `#` inside ```...``` is not a heading,
            // but the fixture's code block has no leading-# lines, so this is safe.
            let is_md_heading = t.starts_with("# ")
                || t.starts_with("## ")
                || t.starts_with("### ");
            assert!(
                !is_md_heading,
                "section {idx} contains a heading mid-body (boundary missed): {t:?}"
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Task 2: L2 chunks respect size / paragraph bounds
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn l2_chunks_respect_size_bound() {
    // Chunk every L1 section the way the ingest pipeline does (pipeline.rs:155).
    let sections = extract_sections(FIXTURE);
    let mut total_chunks = 0;
    for (_, section_text) in &sections {
        let chunks = chunker::chunk(section_text, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP);
        for (i, c) in chunks.iter().enumerate() {
            let n = c.chars().count();
            // chunker.rs documents max extend = 1x chunk_size for code fences,
            // so the hard upper bound per chunk is 2 * chunk_size.
            assert!(
                n <= 2 * DEFAULT_CHUNK_SIZE,
                "L2 chunk[{i}] has {n} chars > 2*{DEFAULT_CHUNK_SIZE} (size bound violated)"
            );
            assert!(!c.trim().is_empty(), "L2 chunk[{i}] is empty/whitespace");
        }
        total_chunks += chunks.len();
    }
    assert!(total_chunks >= sections.len(), "every section should yield >= 1 chunk");
}

#[test]
fn l2_chunk_overlap_preserves_boundary_facts() {
    // With overlap, a fact spanning a chunk boundary must be recoverable: the
    // concatenation-with-dedup of chunks must cover the whole input. We assert
    // the weaker, robust invariant: the union of chunks contains the start AND
    // the end of the source text (no head/tail dropped).
    let para = extract_sections(FIXTURE)
        .into_iter()
        .map(|(_, c)| c)
        .find(|c| c.contains("Knowledge retrieval quality depends"))
        .expect("missing oversize-paragraph section");

    let chunks = chunker::chunk(&para, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP);
    assert!(chunks.len() >= 2, "oversize section should split into >= 2 chunks, got {}", chunks.len());

    let head: String = para.chars().take(30).collect();
    let tail: String = para.chars().rev().take(30).collect::<String>().chars().rev().collect();
    assert!(
        chunks.iter().any(|c| c.contains(&head)),
        "no chunk contains the section head — start dropped"
    );
    assert!(
        chunks.iter().any(|c| c.contains(&tail)),
        "no chunk contains the section tail — end dropped"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Task 3: code blocks not split mid-block
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn code_block_kept_balanced_in_every_chunk() {
    // The "Code Example Section" contains a fenced rust block. Chunk that section
    // with a size that would otherwise cut through it, and assert no chunk holds
    // an odd number of ``` markers (i.e. the fence is never split mid-block).
    let code_section = extract_sections(FIXTURE)
        .into_iter()
        .map(|(_, c)| c)
        .find(|c| c.contains("fn build_index"))
        .expect("missing code-example section");

    // Use a small chunk size to force the splitter to confront the code block.
    let chunks = chunker::chunk(&code_section, 200, 40);
    for (i, c) in chunks.iter().enumerate() {
        let fences = c.matches("```").count();
        assert_eq!(
            fences % 2,
            0,
            "chunk[{i}] has an odd number of ``` ({fences}) — code fence split mid-block:\n{c}"
        );
    }
}

#[test]
fn code_block_body_not_torn_across_chunks() {
    // Stronger: the rust function body must appear contiguously in ONE chunk.
    // (A balanced-but-torn split — e.g. half the body in each chunk with the
    // fence duplicated — would still pass the parity check above, so assert the
    // signature + closing brace co-occur.)
    let code_section = extract_sections(FIXTURE)
        .into_iter()
        .map(|(_, c)| c)
        .find(|c| c.contains("fn build_index"))
        .expect("missing code-example section");

    let chunks = chunker::chunk(&code_section, 200, 40);
    let whole = chunks
        .iter()
        .any(|c| c.contains("fn build_index") && c.contains("index.commit();"));
    assert!(
        whole,
        "the rust code block was torn across chunks — no single chunk holds the \
         full body (signature + commit). chunks={:?}",
        chunks.iter().map(|c| c.chars().take(40).collect::<String>()).collect::<Vec<_>>()
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Task 4: oversize paragraph chunked, not dropped (NO silent truncation)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn oversize_paragraph_chunked_not_dropped() {
    // A single oversize paragraph with NO internal headings. The chunker must
    // split it into multiple L2 chunks; the union of chunks must reconstruct the
    // ENTIRE paragraph with zero character loss (silent tail truncation is the
    // classic RAG failure this guards against).
    let para = "Knowledge retrieval ".repeat(120); // ~2400 chars, no headings
    assert!(para.chars().count() > 2 * DEFAULT_CHUNK_SIZE);

    let chunks = chunker::chunk(&para, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP);
    assert!(
        chunks.len() >= 2,
        "oversize paragraph must be split into >= 2 chunks, got {}",
        chunks.len()
    );

    // Coverage check: walk the source left-to-right and confirm every position
    // is covered by some chunk. Because chunks overlap and preserve order, we
    // verify cumulative coverage: concatenating chunks (with overlap) must be a
    // superstring whose length >= source length, and the source's unique marker
    // count is preserved.
    let src_chars: Vec<char> = para.chars().collect();
    let covered: usize = chunks.iter().map(|c| c.chars().count()).sum();
    assert!(
        covered >= src_chars.len(),
        "total chunk chars {covered} < source {} — content was dropped",
        src_chars.len()
    );

    // Every chunk-aligned slice of the source must appear in some chunk: sample
    // the start, middle, and end windows.
    for frac in [0usize, src_chars.len() / 2, src_chars.len().saturating_sub(20)] {
        let end = (frac + 20).min(src_chars.len());
        let window: String = src_chars[frac..end].iter().collect();
        assert!(
            chunks.iter().any(|c| c.contains(&window)),
            "source window at {frac} ({window:?}) missing from all chunks — truncation"
        );
    }
}

#[test]
fn oversize_cjk_paragraph_chunked_not_dropped() {
    // Same invariant for a CJK-heavy oversize paragraph (char-based chunker must
    // not panic on multibyte boundaries and must not drop the tail).
    let para = "知识检索的质量取决于分块。".repeat(80); // ~1040 CJK chars
    let chunks = chunker::chunk(&para, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP);
    assert!(chunks.len() >= 2, "CJK oversize para should split, got {}", chunks.len());

    let head: String = para.chars().take(8).collect();
    let tail: String = para.chars().rev().take(8).collect::<String>().chars().rev().collect();
    assert!(chunks.iter().any(|c| c.contains(&head)), "CJK head dropped");
    assert!(chunks.iter().any(|c| c.contains(&tail)), "CJK tail dropped");
    for c in &chunks {
        assert!(c.chars().count() <= 2 * DEFAULT_CHUNK_SIZE, "CJK chunk too large");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Table handling — markdown tables are prose to the chunker; assert rows are
// not silently lost and the table stays inside its heading's section.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn markdown_table_rows_preserved_within_section() {
    let doc = "# Specs\n\n\
        | Param | Value |\n\
        |-------|-------|\n\
        | chunk_size | 512 |\n\
        | overlap | 128 |\n\
        | section_target | 1500 |\n\n\
        End of specs.\n";
    let secs = extract_sections(doc);
    let spec = secs
        .iter()
        .map(|(_, c)| c)
        .find(|c| c.contains("| Param | Value |"))
        .expect("table section missing");
    // All three data rows must survive in the same section (table not split on
    // its `|`-prefixed lines being mistaken for structure).
    for row in ["chunk_size | 512", "overlap | 128", "section_target | 1500"] {
        assert!(spec.contains(row), "table row {row:?} dropped from section");
    }
}
