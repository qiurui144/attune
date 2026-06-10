# Attune P1 Reliability/Security Backlog — 2026-06-10

Worktree branch: `worktree-agent-a09f9246652abe935`
Repo: `/data/company/project/attune` (attune-core + attune-server + tools/)
TMPDIR=/data/tmp-sdlc · CARGO_TARGET_DIR=/data/tmp-sdlc/attune-p1-target · `df -h /data` at start: 215G avail (green).

Scope: 4 P1 items, each with a real test. Did NOT touch cloud/ or attune-pro/. Did NOT push origin/develop.

---

## F-04 — ai_annotator real-LLM hardening (§4.5)

**Problem**: `ai_annotator::generate_annotations` (`attune-core/src/ai_annotator.rs`) was a
mock-only single-shot `llm.chat()` with no schema-guided output / retry / few-shot — the exact
defamation_extractor mock-0.99/real-0.09 trap class.

**Fix**:
- New `pii::llm_chat_redacted_hardened()` (`attune-core/src/pii/mod.rs`) — same F-17 PII
  redact/restore wrapping, but layers §4.5 §A schema-guided JSON (`chat_with_format_json` →
  backend `format=<schema>`) + §B retry-with-validation (validator error fed back, ≤ N attempts)
  + §C few-shot (examples prepended as prior turns). PII redacted across system/user/all
  example pairs with globally-unique placeholders before any token leaves the process.
- `generate_annotations` now routes through it: `findings_schema()` (typed JSON schema),
  `few_shot_examples()` (2 worked examples, both validated to be parseable), `validate_findings_json()`
  (validator; empty findings array is valid), `LLM_MAX_ATTEMPTS=3`. Graceful degradation
  preserved: retries-exhausted falls back to the salvage parser → empty, never Err.

**Tests** (`attune-core --lib ai_annotator`, 24 pass + 1 ignored):
- `findings_schema_is_valid_json_schema_shape`, `few_shot_has_at_least_two_examples_with_parseable_outputs`,
  `validator_accepts_well_formed_and_rejects_garbage`, `retry_recovers_after_first_bad_output`
  (attempt-1 garbage → §B feedback → attempt-2 valid → located finding).
- `real_llm_ai_annotator_produces_located_findings` — **`#[ignore]` nightly real-LLM gate**
  (auto-detects live Ollama; asserts parseable+located findings on a real weak model; the bug
  defamation_extractor hit). Run: `cargo test -p attune-core --release real_llm -- --ignored`.
- Updated 2 existing tests (`generate_tolerates_bad_json_from_llm`, `salvages_partial_findings_from_truncated_json`)
  to feed one mock response per retry attempt (real LLM repeats bad shape each attempt).

## F-15 — MCP shim item_id sanitization (path-traversal / SSRF)

**Problem**: `tools/attune_mcp_shim.py::call_attune_get_item` interpolated `item_id` into
`/api/v1/items/{id}` unsanitized → path traversal (`../../admin`) / SSRF (`http://169.254.169.254/`).

**Fix**: `sanitize_item_id()` — strict allowlist `^[A-Za-z0-9_-]{1,128}$` (UUID/hash-slug shape).
Rejects traversal, embedded slashes, absolute/scheme-relative URLs, query/fragment, whitespace,
non-string. Malicious id short-circuits to an error envelope **before any HTTP call**; valid id is
additionally `urllib.parse.quote(safe="")`-encoded (defence-in-depth). Also fixed a latent missing
`import urllib.parse`.

**Tests** (`python/tests/mcp/test_mcp_shim.py`, 30 pass — see false-green note below):
- `test_sanitize_item_id_accepts_valid_ids` (5 cases), `test_sanitize_item_id_rejects_malicious_ids`
  (15 cases incl. `../../etc/passwd`, `http://169.254.169.254/`, `%2e%2e%2fadmin`, newline injection),
  `test_sanitize_item_id_rejects_non_string`,
  `test_get_item_malicious_id_returns_error_without_http_call` (monkeypatches `_http_call` to assert
  it is NEVER reached), `test_get_item_valid_id_reaches_http_call_with_encoded_path`.

## plugin-sig Official trust path

**Problem**: `OFFICIAL_PUBLIC_KEYS` is an empty `const`, so `Trust::Official` + `verify_strict`
were never exercised (dead branch = no confidence).

**Fix** (`attune-core/src/plugin_sig.rs`): extracted `verify_against_keys(dir, &[&str])` and
`verify_strict_against_keys(dir, &[&str])` internals taking an injectable official-key list;
`verify_loose`/`verify_strict` delegate with the (empty) const. Tests inject a test key to drive
the real Official path. No production behaviour change.

**Tests** (`attune-core --lib plugin_sig`, 20 pass; 6 new):
- `official_key_match_yields_official_trust_and_strict_passes`, `official_trust_matches_second_key_in_rotation_list`
  (key rotation), `tampered_prompt_md_rejected_on_official_path` + `tampered_yaml_rejected_on_official_path`
  (tampered file → drops to Unsigned, strict → Err), `non_official_signer_not_promoted_to_official`,
  `production_official_keys_empty_so_verify_loose_is_unsigned` (guards against committing a real key).

## False-green tests

- **`session_test.rs`**: previously re-implemented chat.rs logic at the `Store` layer
  ("模拟 chat.rs 中…的逻辑") — the real HTTP handlers never asserted, could drift silently.
  Rewritten to drive the REAL routes (`GET/DELETE /api/v1/chat/sessions[/:id]`) over an unlocked
  in-memory vault seeded with conversations: list envelope `{sessions,total}`, pagination
  (limit/offset query), get envelope `{session,messages}`, unknown id → 404, delete → 204 + gone,
  locked-vault → 403. One Store-layer cascade-delete invariant retained (legit store unit). 7 pass.
- **`projects_routes_test.rs`**: previously only the locked-vault rejection path. Added a full CRUD
  round-trip against an UNLOCKED vault through the real router: create → 201 (+ title trim, kind
  default), empty title → 400, list, get → 200, unknown → 404, add-file → 201, add-file-to-missing
  → 404, list-files, timeline → 200. 2 pass.
  (NOTE: the task brief said this file was "empty / 0 #[test]"; it was actually 109 lines with one
  locked-path test — the real gap was missing happy-path/CRUD route coverage, now closed.)
- **Latent silent-skip fixed**: `test_mcp_shim.py` `SHIM_PATH` used `parents[2]` (= `<repo>/python`,
  no `tools/`) so **every test silently `pytest.skip`-ped**. Corrected to `parents[3]` (repo root)
  + module-level `assert SHIM_PATH.exists()` so a moved shim fails loudly. The 7 original protocol
  tests now actually run.

---

## Verification (in worktree)

- `cargo test -p attune-core --lib ai_annotator` → 24 passed, 1 ignored (real-LLM gate).
- `cargo test -p attune-core --lib plugin_sig` → 20 passed.
- `cargo test -p attune-core --lib pii` → 51 passed.
- `cargo test -p attune-server --test projects_routes_test --test session_test` → 2 + 7 passed.
- `python -m pytest tests/mcp/test_mcp_shim.py` → 30 passed (was silently skipping before).
- `cargo clippy -p attune-core --all-targets -- -D warnings` → clean.
- `cargo clippy -p attune-server --all-targets -- -D warnings` → clean (also added the same
  `#[allow(unsafe_code)]` env-isolation annotation to the pre-existing `privacy_endpoints_test.rs`
  helper, which otherwise failed `-D warnings`).

Files touched:
- `rust/crates/attune-core/src/pii/mod.rs` (new hardened helper)
- `rust/crates/attune-core/src/ai_annotator.rs` (wire + tests)
- `rust/crates/attune-core/src/plugin_sig.rs` (injectable official keys + tests)
- `tools/attune_mcp_shim.py` (sanitizer)
- `python/tests/mcp/test_mcp_shim.py` (sanitization tests + SHIM_PATH fix)
- `rust/crates/attune-server/tests/projects_routes_test.rs` (CRUD round-trip)
- `rust/crates/attune-server/tests/session_test.rs` (real-route rewrite)
- `rust/crates/attune-server/tests/privacy_endpoints_test.rs` (unsafe_code allow, clippy-clean)
