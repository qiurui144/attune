# PR Review — S4a oss-patent-migration (G3 §5.2 two-round)

- **Branch**: `feature/oss-patent-migration` (from `develop`, 4 commits)
- **Reviewer role**: SDLC pr-reviewer (G3 Challenger), read-only
- **Spec**: `docs/superpowers/specs/2026-06-01-oss-patent-migration.md` (G1 PASS)
- **Plan**: `docs/superpowers/plans/2026-06-01-oss-patent-migration.md` (G2 PASS, 5 task)
- **Sprint type**: pure deletion (OSS boundary realignment — patent → attune-pro)
- **Verdict**: **PASS** (CHANGES_REQUESTED: none)

---

## Diff stat overview

```
 rust/README.md                                             |   1 -
 rust/README.zh.md                                          |   1 -
 rust/crates/attune-core/src/lib.rs                         |   1 -
 rust/crates/attune-core/src/resource_governor/profiles.rs  |  26 +- (3 +, 23 -)
 rust/crates/attune-core/src/scanner_patent.rs              | 372 -----------------
 rust/crates/attune-core/tests/governor_integration.rs      |   1 -
 rust/crates/attune-server/src/lib.rs                       |   2 -
 rust/crates/attune-server/src/routes/mod.rs                |   1 -
 rust/crates/attune-server/src/routes/patent.rs             | 146 ----------
 9 files changed, 3 insertions(+), 548 deletions(-)
```

**Net LOC: -545** (3 added — all in profiles.rs F1 count-sync test edits; 548 deleted). 4 commits map 1:1 to deletion units.

---

## Round 1 — substance (§5.2 7-item checklist)

**[1] Functional correctness / deletion completeness** — PASS
- patent route: `lib.rs:178-179` (2 `/api/v1/patent/*` registrations) + `routes/mod.rs:28` (`pub mod patent`) + `routes/patent.rs` (whole 146-line file) all removed (dd4effc). Verified.
- scanner_patent: whole 372-line file + `lib.rs:156` module decl removed (57929d1). Verified.
- governor PatentScanner: enum variant + `Display`/`as_str` arm + `budget_for` routing arm `(p, PatentScanner) => p.budget_for(FileScanner)` + inline test `patent_scanner_inherits_file_scanner` + 3 snapshot data rows + `governor_integration.rs:152` array element all removed (9d0a903). Verified.
- `git grep "PatentScanner"` on branch -> **0 residual**. No dangling refs.
- `git grep -i` for exec patent symbols (scanner_patent / PatentScanner / routes::patent / PatentQuery / PatentDatabase / search_patents / ingest_patent_records / PatentRecord) in OSS src/tests -> **0 matches**. Matches orchestrator T5 grep gate.

**[2] Edge cases** — N/A (pure removal, no new logic).

**[3] Error handling / silent failure (AC9)** — PASS. No catch-and-swallow / fallback / unwrap introduced; deletion only. The removed `unwrap_or_else(|e| e.into_inner())` poison-recovery was inside the deleted route and is gone with it. No new error paths.

**[4] Security** — PASS. Removal *reduces* surface (drops a live outbound USPTO reqwest path + auto-ingest DEK handling from OSS). No secrets touched. No new input surface.

**[5] Test coverage** — PASS. F1 self-consistency re-verified independently:
- enum `TaskKind`: **9 variants** (counted).
- `as_str()`: **9 arms** (counted), no `_` wildcard.
- snapshot table: **27 data rows** = 3 profiles x 9 kinds (counted).
- `assert_eq!(cases.len(), 27, ...)` + test name `all_27_combinations_snapshot` + doc comment "全 27 组合 ... 9 task kinds" — all synced and self-consistent.
- `governor_integration.rs` kinds array -> **9 entries**; `snap.len() == kinds.len()` invariant self-consistent (N3).
- Deleted-with-feature: inline `patent_scanner_inherits_file_scanner` correctly removed (asserted a budget that no longer exists). Orchestrator confirms governor 6 tests pass.

**[6] Commit hygiene (AC6)** — PASS. 4 commits, one logical unit each, conventional-commit prefixes, each cites spec section. Bodies accurate vs actual diffs. Chinese per §1.1. `refactor()`/`docs()` prefixes appropriate for pure removal. Task-boundary discipline explicit (dd4effc notes scanner_patent left for T2 to avoid cross-task scope bleed).

**[7] Cross-cutting / scope discipline** — PASS
- **S4b boundary untouched** (required): `git diff --name-only` shows none of `agents.registry.toml` / `agent_flows.toml` / `case_metadata.rs` / defamation tests touched. Only the 9 patent files. Verified.
- **Must-preserve labels retained** (mis-delete = Critical — none occurred):
  - `corpus_domain="patent"` data label retained: `store/mod.rs:1454,1471` insert_item tests, `search.rs:122` patent domain keyword-weight scorer (专利/权利要求/IPC...), `store/items.rs`, `store/dirs.rs`, taxonomy/index/remote doc strings. Verified present on branch.
  - `patent-pro` / `patent_pro` plugin-id retained: `plugin_hub.rs:143`, `plugin_registry.rs` (multiple), `generic_plugins_test.rs:74,193`, `plugin_loader.rs:89`. Verified present.
- Match exhaustiveness after enum-variant removal: no `_` wildcard in `budget_for`/`as_str` masking arms; 9 explicit arms over 9 variants. Compiles (clippy `-D warnings` clean per orchestrator) -> exhaustive confirmed by both compilation and arm count.

**R1 findings: NONE — verified clean (all 7 items affirmatively checked).**

---

## Round 2 — re-check + cross-cutting + doc-sync (AC11)

R1 produced 0 findings, so there is no implementer fix to re-verify; R2 covers cross-cutting + doc-sync independently against the same diff.

- **No new issues**: deletion-only diff re-read; no fix-introduced regressions possible (nothing was fixed).
- **Doc-sync (AC11 / §7.2 Gate1)** — PASS. README EN+ZH USPTO patent-search feature line removed in the SAME sprint (09704f7), preventing doc-drift. Critically, the `Industry plugins (patent / law / ...)` line and pro-vertical framing are correctly RETAINED — that is accurate (patent capability lives in attune-pro/patent-pro). Bilingual parity maintained (§1.1.3). Spec §10 documents that already-ingested patent items stay as generic items (no migration needed).
- **Commit history** — clean; §1.1 language + single-responsibility honored.
- **Cross-cutting** — removing patent does not break other OSS paths: scanner/webdav/browser/governor remaining kinds intact (snapshot table for the 9 survivors unchanged), search domain-routing still lists patent as a weight label.

**R2 findings: NONE.**

---

## Findings by category

- **Critical**: none
- **Important**: none
- **Nit**: none

### Out-of-scope observation (NOT a branch finding — does not block)
The working tree carries uncommitted modifications to `rust/Cargo.lock` (workspace 1.1.0->1.2.0 bump) and `.gitignore` (+`.sdlc/`, +`reports/runs/`), plus untracked `docs/superpowers/{specs,plans,handoffs}/` + `reports/` orchestrator artifacts. These are **NOT part of `develop..feature/oss-patent-migration`** (the committed branch diff is the 9 patent files only) and appear to be pre-existing develop state / orchestrator scaffolding. Flagged for the orchestrator's awareness only — the version-bump should not silently ride into this deletion branch's merge. No action required of the patent diff.

---

## G3 verdict: **PASS**

Per §5.2 two-round protocol, both rounds completed with 0 findings. Deletion is complete and dangling-free, scope is strictly patent-only (S4b boundary + corpus_domain/plugin-id labels correctly preserved), F1 count-sync is self-consistent (9 variants / 27 snapshot rows), match exhaustiveness holds, and README doc-drift is closed in-sprint. Cleared to advance to TEST (G3 co-Challenger).

**One-line summary**: Clean -545 LOC patent removal — fully de-wired (route + scanner + governor variant), F1 30->27 self-consistent, S4b boundary and patent data-labels/plugin-ids correctly preserved, README bilingual doc-sync done; PASS, no findings.

---

```yaml
self_score:
  rubric: pr_reviewer
  criteria_scores:
    r1_checklist_complete: 5      # all 7 §5.2 items checked + logged
    r2_independent_verify: 5      # re-counted enum/arms/rows + greps independently, did not trust impl summary
    doc_sync_checked: 5           # README EN+ZH feature-line drop verified, pro framing retained
    silent_failure_scanned: 5     # AC9 — deletion-only, removed path's poison-recovery gone with it, no new swallow
    commit_hygiene_verified: 5    # 4 commits, 1:1 logical units, spec-cited, bodies match diffs
  overall: 5.0
  weak_points: []
```
