# Attune v1.0.1 — Bug Fix + Hardening Release (TBD, ~2026-05-27–31)

> Patch release following v1.0.0 GA. No breaking changes — all v1.0.0 users are encouraged to upgrade.
> Server / CLI builds. See [desktop-v1.0.1.md](desktop-v1.0.1.md) for desktop installer notes.

---

## Bug Fixes

- **CLI vault-import false positive** — `attune-cli vault-import` incorrectly rejected a valid archive with "vault.db already exists" even on a clean target directory. Fixed path existence check. (#61, commit `637dee4`)
- **OCR scene_id_card gender label** — `gender` field extracted the adjacent `民族` (ethnicity) label string instead of the value (`男`/`女`). Fixed field-order parsing in `scene_id_card` extractor. (#62, commit `4879384`)
- **OCR scene_receipt amount_total** — amounts with thousands separators (e.g. `1,234.56`) were not matched by the regex; fixed to cover both plain and comma-separated formats. (#62, commit `4879384`)
- **parse_llm_terms drift** — `attune-core` and `attune-server` had diverged copies of `parse_llm_terms`; the local copy in server was 3 commits behind. Unified: now re-exported as `pub` from core, server import removed. (#77, commit `7dc5ce8`)
- **LLM upstream error pass-through** — 429 (rate limit), 503 (overload), and generic 4xx errors from upstream LLM providers were swallowed and returned as opaque 500s. Now mapped to proper status codes with `code` field (`rate-limited`, `upstream-unavailable`, `upstream-client-error`) so clients can surface actionable messages. (commit `37e0d85`)
- **Docker builder glibc mismatch** — CI builder image `rust:1.88-slim-bookworm` produced binaries linked against glibc 2.36 (Debian 12), causing crashes on Ubuntu 22.04 targets. Switched builder + runtime base to `ubuntu:24.04` (glibc 2.39). (commit `bcf960c`)
- **GA ceremony scripts** — `ga-ceremony.sh` failed on repos with untracked files; `version-audit.sh` excluded rc tags from pair-check. Both fixed. (commit `9b0a132`)

---

## New

- **Tauri auto-updater** — In-app upgrade support for the desktop build. Includes GH Actions workflow, update server scripts, and `tauri-updater-deploy.md` operator guide. Desktop builds will prompt when a new release is available. (commit `ed151e1`, `d74b0ee`)
- **`attune-cli` + `attune-server` OCI images** — Published to `ghcr.io/qiurui144/attune-cli` and `ghcr.io/qiurui144/attune-server`. Suitable for NAS / server self-hosting without a full desktop install.
- **`attune-desktop-installers` OCI image** — All five platform installer artifacts (Win MSI + NSIS, Linux deb + AppImage, aarch64 deb) bundled as a single OCI package for air-gap enterprise distribution. (commit `4cc678b`)
- **WinGet / APT / RPM repo CI workflows** — Package manager submission workflows wired up; WinGet manifest auto-PR on each `vX.Y.Z` tag. (commit `ed151e1`)
- **Crash recovery + concurrent + OOM + large-scale stress tests** — 14 new stress tests added to `attune-core` test suite covering vault crash recovery, concurrent reader/writer contention, OOM-boundary behavior, and 10k-item ingestion. These run in a dedicated `nightly-slow` CI lane. (#123, commit `c9874bb`)
- **Supply chain hardening** — `cargo audit` integrated into CI; `deny.toml` policy file; `SECURITY.md` published with vulnerability disclosure process and supported versions matrix. (commit `f366492`)

---

## Improvements

- **Rust builder bump** — CI Rust toolchain upgraded `1.88 → 1.91` for improved codegen and new lint coverage. (commit `ce994c4`)
- **Clippy clean** — All `clippy -D warnings` cleared across 32 files (~40 individual lints). Build is now warning-free on stable. (commit `3f0bd3f`)
- **LLM call robustness** — `chat_with_retry` wrapper adds `format=json` enforcement, response schema validation, and few-shot re-prompt on parse failure. Reduces `defamation_extractor` null-response rate by ~60% on qwen2.5:3b. (attune-pro cross-repo benefit)
- **`attune-bench` linked** — `docs/benchmarks/` now links to the independent `attune-bench` repo for algorithm quantitative baselines; avoids embedding large binary fixtures in this repo. (commit `3de5978`)

---

## Security

- v1.0 crypto + security audit report published at `docs/v1.0-crypto-security-audit.md`. Covers 7 dimensions: AES-GCM nonce (per-op OsRng), Argon2id parameters (64 MB / 3 iter / 4 thread), HMAC-SHA256 session tokens, vault sealed-state enforcement, TLS certificate handling, dependency audit, and fuzzing surface.
- `cargo audit` now runs in CI on every push to `develop` and `main`. Current status: **0 critical RUSTSEC advisories**.

---

## Known Issues (targeting v1.0.2+)

- `defamation_extractor` F1 = 0.72 on the internal eval set (threshold: 0.75 for default-on). Will remain opt-in until cloud LLM verify pipeline lifts score to ≥ 0.85. (attune-pro)
- Plugin `visibility` + `org_id` schema migration for multi-tenant PluginHub deferred to v1.0.2.
- macOS build not yet available; targeting v1.1.

---

## Test Coverage

- attune-core: 210 tests (all passing, includes 14 new stress tests)
- attune-server: 27 tests (all passing)
- Total: **237 tests, 0 failed, 3 ignored**

---

## Downloads

*(links populated when tag is pushed)*

| Platform | Artifact |
|----------|----------|
| Windows x86_64 | `attune-desktop-v1.0.1-x86_64-windows.msi` / `.exe` (NSIS) |
| Linux x86_64 | `attune-desktop-v1.0.1-x86_64-linux.deb` / `.AppImage` |
| Linux aarch64 | `attune-desktop-v1.0.1-aarch64-linux.deb` |
| Server (all Linux) | `attune-server-v1.0.1-x86_64-linux.tar.gz` |
| CLI (all Linux) | `attune-cli-v1.0.1-x86_64-linux.tar.gz` |
| OCI | `ghcr.io/qiurui144/attune-server:1.0.1` |

---

## Acknowledgments

Thanks to everyone who filed issues and tested rc builds.
Built with [Rust](https://www.rust-lang.org/) · [Tauri](https://tauri.app/) · [tantivy](https://github.com/quickwit-oss/tantivy) · [usearch](https://github.com/unum-cloud/usearch)
