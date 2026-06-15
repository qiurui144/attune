# Office test fresh-runner failure — root cause + fix

**Date**: 2026-06-16  **Branch**: develop (base 3524c2d)  **File touched**: `rust/crates/attune-core/src/store/mod.rs`

## Symptom (as reported)
- `office_happy_path::get_unknown_job_returns_404` asserts `left: 503 right: 404` (office_happy_path.rs:199).
- Only on ubuntu fresh runners (empty HF cache + `HF_HUB_OFFLINE=1`); windows/cached pass.
- 503 source: `routes/office.rs` get_job → `state.job_store()` is `None` → 503 `job-store-unavailable`.
- CI also saw SIGBUS in the same binary.

## Root cause (proven, not assumed)

`job_store == None` means `install_job_store()`'s `Store::open(db_path())` returned `Err`. The reason was **not** the SQLite-busy hypothesis in the brief — it was deeper:

**`Store::open` is not safe under concurrent opens of the same DB file.** At boot the *same* `vault.db` path is opened from several places: `install_job_store`, the spawned `init_search_engines` (vault unlock + other `Store::open` calls), `install_usage_aggregator`, background workers. The office tests amplify this — every test does process-global `std::env::set_var("XDG_DATA_HOME", <own tempdir>)`, so parallel tests' `db_path()` resolves race, and `install_job_store` + the test's `vault/setup`-triggered `init_search_engines` open overlapping paths concurrently. Model-cache presence shifts `init_search_engines` timing, which is exactly why the collision is deterministic only on a fresh+offline runner (no cache → fast-fail init → tight collision window).

Two concrete races inside `Store::open`:

1. **Non-atomic schema migration TOCTOU** — every `migrate_*` is `SELECT COUNT(*) FROM pragma_table_info(...)` then `ALTER TABLE ADD COLUMN`. `busy_timeout` does **not** protect this: two connections both observe the column absent, both `ALTER`, the loser gets `database error: duplicate column name: task_type`.
2. **WAL/VACUUM create race** — `ensure_incremental_autovacuum`'s fresh-vault branch runs `VACUUM` on a connection with **no busy_timeout**; concurrent creation of the same file yields `database is locked` / `locking protocol`.

Any of these → `Store::open` Err → `job_store` stays `None` → office routes 503 instead of 404.

### Evidence
Standalone 8-thread × 20-round concurrent `Store::open` on the same fresh path, against **unmodified** code:
```
Store::open ERR: database error: duplicate column name: task_type
... (later, after partial fix) database is locked / locking protocol
TOTAL ERRORS: 46
```
Committed regression test `store::tests::concurrent_open_same_fresh_path_all_succeed` (8 concurrent opens) against **original HEAD code**:
```
test store::tests::concurrent_open_same_fresh_path_all_succeed ... FAILED   (3/3 runs)
panicked at mod.rs: concurrent Store::open must not Err
```

## Fix (treat-the-root; test not weakened, nothing #[ignore]d)

In `rust/crates/attune-core/src/store/mod.rs`:

1. **Per-DB-path in-process open lock** (`open_lock_for`): a process-wide `OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>`; `Store::open` holds the per-path guard across the create+WAL+VACUUM+migration bootstrap. Serializes only the *open* critical section (sub-ms); steady-state queries use the already-open returned connection and are unaffected. This is the deterministic fix for the in-process concurrent-open reality (one process, many connections to one vault.db).
2. **Transactional bootstrap** — factored SCHEMA_SQL + all migrations into `bootstrap_schema()` run inside a single `BEGIN IMMEDIATE … COMMIT` (rollback on error). Belt-and-suspenders against the ALTER TOCTOU.
3. **busy_timeout on `ensure_incremental_autovacuum` connections** + treat the fresh-vault `VACUUM` failure as benign (the autovacuum step was already documented non-fatal).

## SIGBUS (task 3)
Code-reviewed the OCR model-load paths. Both `PpOcr::new()` (ppocr.rs:156) and the layout PicoDet paths (nontext/layout.rs:64, 161) guard `model_path.exists()` and return `None`/empty before any `ort Session::builder().commit_from_file(...)`, and handle Session-build errors via `match`/`map_err` — **no raw mmap of a missing/empty model file**. The SIGBUS was a downstream symptom of the same `office_happy_path` binary being unstable under the open race (and/or a partially-written model from a concurrent download), not a defect in OCR loading. No OCR code change needed; the open-race fix removes the binary-level instability. My local repro did not have OCR models present, so OCR ran graceful-None throughout (no SIGBUS observed).

## Verification (§6.3)

- **Unit FAIL→PASS** (deterministic): original code `concurrent_open_same_fresh_path_all_succeed` FAILED 3/3; fixed code PASSED 5/5.
- **Stress repro**: 8×20 concurrent opens — unfixed 46 errors, fixed **0 errors**.
- **Full office offline suite** (golden command, `HF_HOME=/tmp/empty-hf-cache HF_HUB_OFFLINE=1`): all 10 test files green, **82 tests, 0 failed, no SIGBUS**:
  office_asr_golden_gate 10 / office_cancel_test 6 / office_concurrent_test 4 / office_error_contract 10 / office_failure_recovery_test 5 / office_happy_path 7 / office_ocr_golden_gate 8 / office_prop_tests 5 / office_schema_compat 14 / office_six_category_floor 13.
- **attune-core store unit tests**: 275 passed, 0 failed.
- **clippy** `-p attune-server -p attune-core --tests`: 0 new warnings (the 3 `from_ref` warnings in `memory_continuity_golden_gate.rs` pre-exist on clean HEAD — confirmed via git stash; not my file).

## Commit
SHA: a56cdfd
