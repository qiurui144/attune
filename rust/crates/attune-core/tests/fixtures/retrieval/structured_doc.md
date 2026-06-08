# Knowledge Base Architecture

This document describes how attune chunks and indexes structured content.
It is used as a deterministic chunking fixture: the headings, code block, and
oversize paragraph below are load-bearing for `chunking_quality_test.rs`.

## Ingestion Pipeline

The ingestion pipeline parses a file, extracts Level 1 sections on headings,
then splits each section into Level 2 sliding-window chunks for embedding.
Both levels enter the embedding queue.

## Code Example Section

Here is a fenced code block that must never be split across chunk boundaries.

```rust
fn build_index(documents: &[Document]) -> Index {
    let mut index = Index::new();
    for doc in documents {
        let sections = extract_sections(&doc.content);
        for (idx, text) in sections {
            index.add_section(doc.id, idx, &text);
        }
    }
    index.commit();
    index
}
```

The prose after the code block continues the same section.

## Oversize Paragraph Section

Knowledge retrieval quality depends on chunking large paragraphs into bounded units instead of dropping them. The following paragraph is intentionally long and contains no internal headings so that the sliding-window chunker must split it into multiple Level 2 chunks rather than emitting a single oversize chunk or discarding the tail. Retrieval-augmented generation systems fail silently when an oversize paragraph is truncated, because the truncated tail is never embedded and therefore can never be retrieved, which means a user query that matches only the tail returns nothing even though the source document clearly contains the answer. To defend against that failure mode, the chunker uses a sliding window with overlap so that every character of the paragraph appears in at least one chunk, and adjacent chunks share an overlap region so that a fact spanning a chunk boundary is still recoverable from at least one chunk. This paragraph is padded with additional sentences to comfortably exceed the default chunk size of five hundred and twelve characters, guaranteeing that the chunker produces two or more Level 2 chunks for this single paragraph and that the union of those chunks reconstructs the original text without loss. Each additional clause here exists purely to grow the character count past the threshold so the test has a real oversize case to exercise rather than a synthetic edge that never occurs in production corpora.

## Summary

Structured documents are split on headings at Level 1 and into bounded
sliding-window chunks at Level 2, with code fences kept balanced.
