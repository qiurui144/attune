# attune OSS — Full Documentation Audit + Completion + Correction

**Date**: 2026-06-10 · **Branch**: develop @ b84011d · **Scope**: docs-only (no code, no cloud/attune-pro)
**Standard**: CLAUDE.md §3.2 文档体系铁律 (whitelist / DRY / no over-claim / en↔zh parity / 4-section RELEASE)

Files touched: `README.md`, `README.zh.md`, `rust/README.md`, `rust/DEVELOP.md`, `rust/RELEASE.md`,
`docs/INSTALL.md`, `docs/DEPLOY.md`, `CLAUDE.md` — 8 files, **+64 / −227 lines**.

---

## Findings by dimension

### (a) DRIFT — docs vs current code

| # | Location | Drift | Fix |
|---|----------|-------|-----|
| D1 | `rust/DEVELOP.md:15` | `cargo test --workspace # 237+ tests (210 core + 27 server)` — actual is far higher (5 crates, attune-core alone has 2000+ test fns) | Removed fragile number; phrase as full-suite + office-shard note |
| D2 | `rust/README.md:401` | `# 376+ tests` stale | Removed number; added pointer to `docs/TESTING.md` + office-shard note |
| D3 | root `README.md` dev-table | `~734 tests in attune-core` / `1145+` workspace — stale, dev-facing | Section removed entirely (see V1) |
| D4 | `CLAUDE.md:72` | `237+ tests (210+27)` | → "全量套件覆盖 5 crate … 以 rust/RELEASE.md (SSOT) + cargo test 为准" |
| D5 | `CLAUDE.md:73` | `最新里程碑：v0.5.x 改名为 Attune` (now v1.2.0) | → SSOT pointer + current-version note |
| D6 | `CLAUDE.md:186` | `最新 GA 是 v0.6.0`（now v1.2.0） | → "最新版本以 rust/RELEASE.md (SSOT) 为准 (v1.0 GA → v1.1.0 ACP → v1.2.0)" |

Recently-merged features (SPKI cert-pin / W1 anchor allowlist / OutboundGate+L0 / GitConnector / WASM)
were **verified present in code** (`outbound_gate.rs`, `cert_pin.rs`, `plugin_anchor.rs`, `ingest/git.rs`,
`wasm_runtime.rs`) and are accurately described in the README "What's new" + rust/DEVELOP capability matrix
+ rust/RELEASE Unreleased section — no drift there.

### (b) GAPS — undocumented / completeness

- `rust/RELEASE.md` **Unreleased** block was missing `### Breaking` (had Highlights / Migration / Known
  Limitations only). Added explicit `### Breaking`（无对外破坏性变更; only user-visible change = one-time
  FTS index auto-rebuild, already in Migration）→ 4-section completeness per §1.1.4.
- All shipped versions (v1.0.0 / v1.0.5 / v1.0.6 / v1.0.7 / v1.1.0 / v1.2.0) already carry all 4 sections — OK.

### (c) ERRORS — wrong commands / versions / dead links

- **Stale version pins** (would keep re-rotting): `docs/DEPLOY.md` pinned `v0.6.3` in 5 download URLs +
  one `v0.6.3 起` feature note; `docs/INSTALL.md` pinned `desktop-v0.6.0` / `0.6.0` in .deb/.rpm/AppImage/NSIS.
  → Converted to `VERSION=1.2.0` placeholder pattern + package-manager pointer (winget/apt/dnf, v1.0+ recommended).
- **Dead links fixed** (7):
  - `README.md` Documentation → `docs/v07-gap-analysis.md` (deleted) — removed; added live links (INSTALL/TESTING/DEPLOY/DEVELOP/oss-pro-strategy).
  - `README.zh.md` ×2 → `docs/oss-pro-strategy.zh.md` (no such file) → `docs/oss-pro-strategy.md`.
  - `README.md` + `README.zh.md` → `CONTRIBUTING.md` (missing) → point to DEVELOP.md + NOTICE (matches EN's existing wording).
  - `docs/INSTALL.md` → `auto-updater-setup.md` → actual file is `updater.md`.
  - `rust/DEVELOP.md` ×4 + `rust/RELEASE.md` ×1 → deleted one-time sprint specs/plan
    (`2026-04-27-w2-rag-quality-batch1`, `2026-04-27-w3-batch-a`, `2026-04-27-resource-governor`,
    `2026-05-18-multilayer-memory`). Replaced: resource-governor → ADR `0006`; others → RELEASE.md / inline note (§3.2 plan-deleted-after-impl).
- **Version SSOT consistent**: Cargo workspace = `1.2.0`; member crates `1.2.0` (agent-sdk `0.1.0`, intentional
  per §1.1.8 leaf-crate independent versioning); `rust/RELEASE.md` latest = v1.2.0 + Unreleased — aligned. No tauri.conf.json version found in workspace (desktop app is separate workspace; not in scope).

### (d) §3.2 VIOLATIONS — fixed

- root `README.md` `## v0.7 sprint highlights` + `## v0.6.0-rc.5 highlights` = release-notes in README (§1.1.2 violation) → **removed**; condensed into one pointer to `rust/RELEASE.md` (SSOT) under the v1.0 GA blockquote.
- root `README.md` `## 代码模块视角（开发者用）` = ~100-line dev module/test table (dev-facing, drifts from code) → **removed**; replaced with a pointer to `rust/DEVELOP.md → 能力矩阵 × 技术栈选型`. (zh README never had this table — it already pointed to DEVELOP, so this also improves parity.)
- root `README.md` `## v1.0 GA highlights` long release block → condensed to a 1-line pointer.
- Repo-wide: **no stray** `-report/-tasks/-analysis.md` at `docs/` top, **no** `.zh.md` other than the two READMEs, **no** version-named release-notes files. `docs/reports/*` and `docs/superpowers/specs/*` are in sanctioned locations.

### (e) en↔zh PARITY

- Both READMEs now share identical top-level structure: `Download → What's new (v1.0 GA pointer) → product lines → …`. The EN had previously kept v1.0 GA / v0.7 / v0.6 detail blocks the zh had already dropped — now both carry a single matching v1.0-GA pointer blockquote.
- Capability-matrix anchor `#能力矩阵--技术栈选型` resolves correctly in all 5 referencing docs (EN/zh README ×2, rust/README, the new EN dev-section pointer).

### (f) COMPLETENESS — INSTALL / TESTING / DEPLOY

- INSTALL/DEPLOY accurate after the version-pin fix; both now cross-link the package-manager path. TESTING.md (5-layer pyramid + A–K doc-intelligence gate) accurate, no edits needed.

---

## Correctly NOT changed (verified accurate)

- **doc-intelligence** is correctly NOT documented as shipped. `rust/RELEASE.md` Unreleased explicitly states
  the *feature* is deferred to v1.3.0 (independent RC branch) and only its test/security-hardening commits
  merged to develop. No doc claims it as shipped. ✅
- root `RELEASE.md` is correctly scoped to the Python-prototype-line history and points to `rust/RELEASE.md`
  as the version SSOT (lines 4/22/157/195) — not a violation, left intact.
- `CLAUDE.md` `file://…/.claude/CLAUDE.md` refs are intentional project convention (I added none).
- CLAUDE.md AI-instruction intent preserved — only factual drift (test counts / stale version) corrected.

## Residuals (out of scope / not fixed)

- `CONTRIBUTING.md` still does not exist (both READMEs now gracefully point to DEVELOP.md instead of a dead link — creating the file is a content task, not a doc-audit fix).
- Forward-looking `v0.7 候选` roadmap notes in `docs/DEPLOY.md` (macOS .dmg, multi-vault) left as-is — they describe future work, not stale shipped claims.
- develop office-shard CI may be red (known-DEFERRED, irreducibly-heavy office tests) — not a docs concern.

## Verification

- Round-1 + Round-2 self-review (accuracy-vs-code + §3.2 + en/zh parity + dead-link sweep).
- Final dead-link sweep across all 8 touched docs: **0 real dead links** (only intentional `file://` refs remain).
- No `include_str!` embeds these docs → build unaffected.
