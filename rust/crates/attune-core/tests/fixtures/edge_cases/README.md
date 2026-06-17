# edge_cases fixtures

Adversarial / boundary / resource-exhaustion inputs for the ingest path
(`attune_core::ingest::ingest_document` + `parser::parse_bytes`).

Consumed by `tests/ingest_edge_resource_test.rs` (dimension: edge / error /
resource-exhaust / concurrency for ingestion, per docs/TESTING.md §E).

## Committed-as-is (small, deterministic)

| file | what it is | break case it probes |
|------|------------|----------------------|
| `empty.txt` | 0 bytes | graceful skip, no panic, no empty item |
| `all_emoji.txt` | emoji-only UTF-8 | tokenizer / grapheme handling, no crash |
| `malicious.html` | `<script>` + `onerror=` + `<img>`/`<svg>` XSS payloads | extraction MUST strip script content from indexed text |
| `xss_in_attr.html` | XSS purely in event-handler attributes | attribute payloads MUST NOT reach indexed text |

## Generated (large / binary — produced by `generate.sh`, NOT committed)

Run `bash generate.sh` from this directory to (re)produce:

| file | size | what it is | break case it probes |
|------|------|------------|----------------------|
| `huge_10mb.txt` | ~10 MB | repeated UTF-8 text | bounded memory, no unbounded blowup |
| `non_utf8.bin` (`.txt`) | small | invalid UTF-8 byte sequences | `from_utf8_lossy` graceful, no panic |
| `many_lines.txt` | 100k tiny lines | 1-char lines | line-handling does not blow up |
| `deep_nested.json` | deeply nested | 50k-deep `[[[…]]]` + huge flat array | oversize/nested structure bounded, no stack overflow / OOM |

`generate.sh` is deterministic (no randomness) so regenerated fixtures are
byte-stable. The test creates large fixtures in-process when the generated
files are absent, so the suite is self-contained on a clean checkout.
