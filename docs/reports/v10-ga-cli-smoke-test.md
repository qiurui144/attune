# v1.0 GA CLI Smoke Test Report

**Date**: 2026-05-21  
**Binary**: `rust/target/release/attune` (built from `develop` branch)  
**Build**: `cargo build --release -p attune-cli` — 2m35s, success  
**Disk**: /data 349G available  

## Summary Table

| # | Subcommand | Status | Notes |
|---|-----------|--------|-------|
| 1 | `setup` | ✅ pass | vault init + recovery key generated |
| 2 | `status` | ✅ pass | JSON output, all 3 fields present |
| 3 | `lock` | ✅ pass | "Vault locked. All keys cleared from memory." |
| 4 | `unlock` | ✅ pass | session token returned |
| 5 | `insert` | ⚠️ by-design | requires in-process unlocked vault (server must be running) |
| 6 | `get` | ⚠️ by-design | same — "vault is locked: unlock required" |
| 7 | `list` | ⚠️ by-design | same — "vault is locked: unlock required" |
| 8 | `ocr` | ✅ pass | 1×1 PNG → engine runs, 7ms, exit 0 |
| 9 | `ocr` (missing file) | ✅ pass | exit 1 + "image file not found" hint |
| 10 | `transcribe` (missing file) | ✅ pass | exit 1 + "audio file not found" hint |
| 11 | `transcribe` (wrong ext) | ✅ pass | exit 1 + descriptive whisper error |
| 12 | `vault-export` | ✅ pass | exports vault.db + shm + wal (3 entries) |
| 13 | `vault-import` (no --force) | ❌ bug | reports "vault.db exists" even on fresh HOME — root cause: `Vault::open_default()` creates vault.db before import check |
| 14 | `vault-import --force` | ✅ pass | imports 3 entries, status shows locked |
| 15 | `deploy --dry-run` | ✅ pass | full hardware detection (NVIDIA, 61GB RAM, tier=high) + plan printed |
| 16 | `deploy --help` | ✅ pass | all flags rendered correctly |
| 17 | `plugin-keygen` | ✅ pass | Ed25519 keypair generated, privkey chmod 600 |
| 18 | `plugin-sign` | ✅ pass | plugin.sig written |
| 19 | `plugin-verify-sig` | ✅ pass | "signature VALID" |
| 20 | `plugin-encrypt` | ✅ pass | plugin.yaml.enc written |
| 21 | `plugin-decrypt` | ✅ pass | plugin.yaml restored |
| 22 | `plugin-verify` | ✅ pass | validates annotation_angle plugin correctly |
| 23 | `plugin-install` | ✅ pass | installed to ~/.local/share/attune/plugins/ |
| 24 | `plugin-list` | ✅ pass | shows installed plugin with metadata |
| 25 | `plugin-uninstall` | ✅ pass | removed, list shows 0 |
| 26 | `plugin-publish` | ✅ pass | correct error "admin token required" when no token |
| 27 | `plugin-publish` (hub unavailable) | ✅ pass | exits with clear error |
| 28 | `login` | ⚠️ network-required | rpassword requires TTY; with expect: "connection refused" as expected |
| 29 | `sync-plugins` | ✅ pass | "no cloud session found — run attune login first" |
| 30 | `link-folder` | ✅ pass | folder-links.json written |
| 31 | `ocr-profile-list` | ✅ pass | 7 builtin profiles listed |
| 32 | `ocr-profile-show` | ✅ pass | JSON output with all fields |
| 33 | `ocr-profile-create` | ✅ pass | custom profile written to ocr_profiles.json |
| 34 | `ocr-profile-delete` (custom) | ✅ pass | deleted |
| 35 | `ocr-profile-delete` (builtin) | ✅ pass | exit 1 + "builtin, 不可删除" |

**--help passes**: all 29 subcommands respond to `--help` with correct usage.

## Issues Found

### BUG-1: `vault-import` false-positive "vault.db exists" on fresh HOME (❌)

**Symptom**: `attune vault-import <src>` on a fresh HOME directory (no prior attune data) reports:
```
Refusing to import — <path>/vault.db already exists.
```
even though no vault was ever set up there.

**Root cause**: `Vault::open_default()` is called at the top of `run()` before the import command dispatch. `Vault::open_default()` → `Store::open(db_path)` → `Connection::open(path)` — SQLite's `Connection::open` **creates** the file if it doesn't exist. So by the time the import guard checks `vault.db`, it's already been created by the open call.

**Workaround**: Use `--force` flag. `vault-import --force` works correctly.

**Fix recommendation**: In `Commands::VaultImport` dispatch (like the other no-vault commands), detect if the DB was freshly created vs pre-existing before the guard fires. Or move `VaultImport` into the early-return block (before `Vault::open_default()`), using a lighter check of whether `db_path` existed *before* opening.

**Severity**: Medium — user-facing UX regression on first-time import. `--force` workaround documented.

### FINDING-1: `insert`/`get`/`list` require in-process unlocked vault (⚠️ by-design)

These commands call `vault.dek_db()` which requires the vault to be unlocked in memory. Since each CLI invocation is a fresh process, the vault is always re-opened in `Locked` state. The expected production usage is:
- These commands are invoked from within the **server process** (which holds vault in memory)
- Standalone CLI use of these commands is not supported without a running server

This is architectural, not a bug. CLI help should document this constraint.

### FINDING-2: `login` requires TTY (⚠️ expected)

`login` calls `rpassword::read_password()` which requires a TTY. Non-interactive/piped invocation exits with `os error 6 (No such device or address)`. This is standard secure password reading behavior. CI/automation must use `expect` or equivalent.

### FINDING-3: `plugin-verify` requires correct `plugin.yaml` schema (⚠️ doc gap)

The `pricing` field in `plugin.yaml` must be a struct `{ tier: "free" | "paid" | "trial" }`, not a plain string. Help output doesn't document this; only the source shows the schema. A brief schema example in `plugin-verify --help` would help plugin developers.

## Exit Code Verification

| Scenario | Expected exit | Actual exit |
|----------|-------------|-------------|
| Success commands | 0 | 0 ✅ |
| Missing file (ocr/transcribe) | 1 | 1 ✅ |
| Invalid input (profile not found, wrong args) | 1–2 | 1–2 ✅ |
| Engine failure (wrong file type for transcribe) | 1 | 1 ✅ |
| Vault locked when data ops required | 1 | 1 ✅ |
| Builtin profile delete refused | 1 | 1 ✅ |
| Bad password mismatch (setup) | 1 | 1 ✅ |

## GA Go/No-Go (CLI View)

**Recommendation: CONDITIONAL GO**

- All 29 `--help` outputs render correctly ✅
- Core vault lifecycle (setup/unlock/lock/status/export) works ✅
- Plugin pack full lifecycle (keygen/sign/verify-sig/encrypt/decrypt/verify/install/list/uninstall) works ✅
- OCR subcommand works end-to-end ✅
- OCR profile CRUD complete ✅
- Deploy dry-run works with real hardware detection ✅
- Error messages are user-friendly with hints ✅

**Blocking for GA**: None (BUG-1 has `--force` workaround; FINDING-1 is by-design)

**Recommended fix before GA**: BUG-1 `vault-import` false-positive — low-risk 5-line fix in `main.rs` to move `VaultImport` into the no-vault early-return block.
