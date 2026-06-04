# Audit: attune-core (Rust)

- **Path**: `/data/company/project/attune/rust/crates/attune-core/src`
- **Size**: 63,790 LOC / 189 `.rs` files (largest crate). Of this, ~1,563 `#[test]`/`#[tokio::test]` functions — a large fraction of LOC is in-file `#[cfg(test)]` blocks (e.g. store/mod.rs 2242 LOC but tests start at line 1245; agents/flow/tests.rs 1099 LOC).
- **Method**: Grep/Glob structure survey + targeted reads of hotspots (store, llm, search, skill_evolution, agents/flow, chat_reliability, cloud_client, ingest). Not line-by-line full read (per cost discipline).
- **Date**: 2026-06-03

## Scorecard

| Dimension | Score (1–5) | Note |
|---|---|---|
| code_quality | 4 | Idempotent migrations w/ column guards; AppError/AppResult discipline; few-shot+schema-guided LLM agents. Some justified-but-noisy `let _ =`. |
| complexity | 4 | No runaway functions — longest non-test fn ~152 lines (search_with_context). store/mod.rs is big but cleanly sharded into 30+ submodules. ocr/structured is a clean strategy pattern. |
| simplification_potential | 3 | Moderate: migration-version consolidation, scene_* shared scaffolding, llm.rs triple `chat_with_format_json` retry/validate logic could centralize. Est ~400–700 LOC reducible. |
| doc_accuracy | 3 | CLAUDE.md test-count claim badly stale (claims "210 attune-core", actual ~1563 test fns); cloud_client carries an acknowledged unfixed FIXME bug. |

## Findings by dimension

### Correctness / silent failure

- **MEDIUM** `cloud_client.rs:131-136` — `logout()` is buggy by acknowledged design: `let _ = self.http.post(&url).send().map_err(http_err)?;` — the `let _ =` is misleading because the `?` still **propagates** network errors, so `session_cookie = None` is never reached on network failure. Result: user "logs out" but local session token survives if server is unreachable. Documented as `FIXME(v1.1)` at line 509 with a test that locks the *current* (buggy) behavior. A correct sibling `wipe_session()` (line 153) exists and unconditionally clears — `logout()` should delegate to the same pattern. (severity: privacy/UX, not data-loss)
- **LOW** `scanner.rs:98/102/106` — `let _ = store.upsert_indexed_file(...)` swallows the "mark file as indexed" write. If this errors silently the file is never recorded as indexed → potential repeated re-ingest of the same file each scan. Worth a `log::warn!` at minimum.
- **INFO (justified)** Remaining 108 `let _ =` sites are overwhelmingly legitimate: best-effort cleanup (`std::fs::remove_file` of tmp), process teardown (`child.kill()/wait()`, `h.join()`), idempotent startup housekeeping (`purge_completed_embed_queue`, `wal_checkpoint`), and signal recording that "fails silently by contract." No action needed.
- **INFO** All `unreachable!()` / `panic!()` outside tests reduce to: wasm_runtime init panic (acceptable hard-fail) and `#[cfg(test)]` blocks (scene_*.rs, mcp_client, ollama_setup). No production panic risk found.

### LLM agent robustness (per §4.5 bottom-line rule — positive)

- **POSITIVE** `skill_evolution/agent.rs` is a model citizen of the documented defamation_extractor/self_evolving_skill lessons: schema-guided output (`chat_with_format_json(.., Some(&schema))` line 488), script-aware prompt hint (Simplified vs Traditional, `detect_cjk_script`), and `normalize_to_script` + case-insensitive dedupe (lines 564/634-669) — exactly the Traditional→Simplified normalization fix from the historical incident. No regression here.
- **NOTE** `chat_with_format_json` has a trait-default impl (llm.rs:327) + two overrides (Ollama:784, OpenAi:1314). This is correct trait design, but the retry-validate-with-feedback loop is implemented per-provider; centralizing the loop in the default and having providers expose only the raw call would cut duplication.

### Complexity hotspots

- `store/mod.rs` 2242 LOC — large but the bulk is `#[cfg(test)]` (from ~1245) + `SCHEMA_SQL`. Non-test code is well-factored; 30+ table modules already split out (`store/items.rs`, `store/audit.rs`, etc.). No single oversized function.
- `llm.rs` 1925 LOC — three provider impls (Ollama/OpenAi/Mock) + shared trait. Longest fn ~110 lines (`chat_with_format_json`). Acceptable.
- `search.rs` `search_with_context` 152 lines — the longest non-test function in the crate; candidate for extracting the time-filter and domain-penalty stages into helpers (both already exist as `parse_time_filter_with_now` / `apply_cross_domain_penalty`).
- No deep-nesting or god-function red flags found.

### Dead code

- Genuinely-dead-but-allowed (all explicitly `#[allow(dead_code)]`, low risk): `plugin_sig.rs:120 verify_strict` (commented "预留, PluginHub 上线后激活"), `sync/webdav.rs:160 _hint_pathbuf_is_used` (a no-op hint stub — can be deleted, ~2 LOC), `memory/assembler.rs:386 seed_episodic` (test helper mis-scoped). The `schema`/`jsonrpc` field allows are deserialization-required fields, not dead.
- **SUGGESTION** Delete `sync/webdav.rs _hint_pathbuf_is_used` stub. Re-evaluate `plugin_sig::verify_strict` — if PluginHub strict-verify isn't on any near-term path, it's untested dead weight.

### Doc-drift

- **CLAUDE.md** (project): "237+ tests (210 attune-core + 27 attune-server)" — attune-core alone now has **~1563** test functions. ~7x stale. Should be refreshed or de-quantified.
- **CLAUDE.md**: still references `SkillClaw`-style evolution and module list under "已实现模块" describes the *Python* line; the Rust module inventory (157+ `pub mod`) is not mirrored — acceptable (CLAUDE is not an API doc) but the test number is a hard factual error worth fixing.
- **cloud_client.rs:509** `FIXME(v1.1)` — known logout-on-network-failure bug carried forward; should be in RELEASE.md Known Limitations if not already.

### Security (§1.4)

- **CLEAN** — no real secrets. The two literal hits are test fixtures: `capture/telegram.rs:113 bot_token: "111:secret"` (fake) and `store/mod.rs:2102` a test content string. No hardcoded API keys / tokens / passwords in production paths.
- `OutboundGate::enforce` result intentionally swallowed in `wipe_session` (line 155) — correct (user-initiated wipe must always proceed). Documented + grep-guarded by `scripts/privacy-audit.sh`.

### Dependencies

- Not flagged from src survey (Cargo.toml not in scope of this pass). Heavy native deps (rusqlite-bundled, usearch, tantivy) are core, not redundant.

## Simplification / compression opportunities (est. LOC)

1. **llm.rs retry-validate centralization** — hoist the per-provider retry/validate/feedback loop into the trait default; providers expose only `raw_chat`. Est. **−150 to −250 LOC**, single point to harden.
2. **store migration consolidation** — 8 `migrate_*` fns are each a column-existence check + ALTER. A small `add_column_if_missing(conn, table, col, ddl)` helper + a version-gated runner (currently `SCHEMA_VERSION = 1` is unused for gating) would collapse the boilerplate. Est. **−80 to −120 LOC** + clearer upgrade path.
3. **ocr/structured scene_* scaffolding** — 5 files share `extract(lines) -> StructuredFields` shape with similar line-scan/field-match prologue. A shared `LineScanner` helper for the common prologue could cut **~100–150 LOC** across scene_card/receipt/document/table.
4. **search.rs `search_with_context`** — extract pipeline stages (already-existing helpers) to shrink the 152-line fn; modest LOC win, big readability win.
5. **Delete** `sync/webdav.rs:160` hint stub (~2 LOC) and re-evaluate `plugin_sig::verify_strict` dead path.

Total realistic compression: **~400–700 LOC** without behavior change.

## doc-drift checklist

- [ ] CLAUDE.md attune-core test count "210" → ~1563 (fix or de-quantify)
- [ ] CLAUDE.md "237+ tests" aggregate stale
- [ ] cloud_client logout FIXME(v1.1) bug → ensure listed in RELEASE.md Known Limitations
- [ ] CLAUDE.md "已实现模块" reflects Python line only; Rust inventory not mirrored (low priority)
