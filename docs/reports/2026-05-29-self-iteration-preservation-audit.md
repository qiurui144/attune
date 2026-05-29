# Self-Iteration Preservation Audit — Agent Learned-State vs Plugin Upgrade

**Date**: 2026-05-29
**Auditor**: read-only audit agent D
**Scope**: how agent self-iteration / learned optimization survives a plugin version upgrade
**Method**: static read of `attune-core` skill_evolution + store + plugin_sync/registry; `attune-pro` plugin.yaml + agent_golden_gate; prior-art spec `2026-05-19-agent-self-learning-design.md`. No build/test run.

---

## TL;DR (4 answers)

1. **Learned-state classes today**: only **two are actually shipped** — (a) `skill_expansions` table (per-query learned expansion terms, SkillClaw-style); (b) `app_settings.search.learned_expansions` blob (legacy topic-keyed). Plus passive signal substrate (`skill_signals`, `browse_signals`) and `memory` consolidation. The richer per-agent `agent_preferences` (followup rank / verbosity / form-defaults / retrieval-weights / prompt-hints) is **spec-only, NOT implemented** (whole-repo grep `agent_preferences` = 0 hits).
2. **Will an upgrade lose it?** **No (by lucky architecture, not by design intent).** All learned state lives in the **vault DB** (`data_dir`), while plugin upgrade only `remove_dir_all` + recopy the **plugin package dir** (`plugins/<id>/`). The two never touch. There is **no explicit migration coupling learned state to plugin version** — preservation is incidental, not guaranteed/tested.
3. **Is there a migration?** **No learned-state migration exists.** DB migrations are generic per-column idempotent `ALTER` guards (`migrate_skill_signals_v07` etc.); none is keyed to a plugin version. **No `PRAGMA user_version`** global schema version. The spec's `schema_version` field for `agent_preferences` is unimplemented.
4. **Biggest risk**: the **plugin-shipped vs user-accumulated boundary is undocumented and unenforced**. Today it accidentally holds (learned state = vault, plugin code = plugins dir). The moment a future plugin ships learned-state defaults, or a plugin's agent-id / signal-kind / query-pattern key format changes across versions, the orphaning is **silent** — keys simply stop matching, the user's accumulated optimization quietly becomes dead rows, and there is no schema-version gate to detect or migrate it.

---

## 1. Learning-state inventory (what / where / who owns)

| Learned thing | Implemented? | Storage | Owner | Evidence |
|---|---|---|---|---|
| **Per-query learned expansions** (heuristic + LLM terms) | ✅ shipped | `skill_expansions` table in **vault DB** (`query_pattern` PK, `expansions` JSON, `generated_by`, `confidence`) | user-accumulated | `store/mod.rs:530` (schema); `store/skill_expansions.rs` (CRUD); `skill_evolution/agent.rs` (47KB) |
| **Legacy topic-keyed expansions** | ✅ shipped | `app_settings.search.learned_expansions` encrypted meta blob | user-accumulated | `skill_evolution/mod.rs` `merge_expansions_into_settings`, `truncate(8)` |
| **Signal substrate** (`search_miss`, `citation_hit`, `click_through`, …) | ✅ shipped | `skill_signals` table (append-only, `kind`+`ref_id`) | user-accumulated | `store/mod.rs:260`; `migrate_skill_signals_v07` `store/mod.rs:870` |
| **Browse / engagement signals** | ✅ shipped | `browse_signals`, `auto_bookmarks` (encrypted url/title) | user-accumulated | `store/mod.rs:443` |
| **Episodic memory consolidation** | ✅ shipped | memory tables (`INSERT OR IGNORE`, idempotent) | user-accumulated | `memory/consolidation_agent.rs` |
| **Per-agent preferences** (followup_rank / verbosity / form_defaults / retrieval_weights / prompt_hints, with `schema_version`) | ❌ **spec only** | designed as `agent_preferences` meta blob | (would be) user-accumulated | spec §4.1; grep `agent_preferences` / `followup_rank` / `start_agent_adapter` = **0 code hits** |
| **Behavior / user profile** | ⚠ partial — `routes/profile.rs` exists but only `HardwareProfile` / OCR `ProfileRegistry` found; **no behavior-preference profile struct/export/import** in core | n/a | `platform/mod.rs:118 HardwareProfile`; `ocr/profile.rs` |
| **Golden-set ratchet threshold** ("quality water level", τ, F1 floor) | ✅ but **dev-time artifact** | `attune-pro/plugins/law-pro/tests/agent_golden_gate.rs` + golden YAML fixtures in `tests/golden/` — **committed to git, shipped inside the plugin pack**, NOT in user vault | **plugin-shipped** (dev/CI) | `attune-pro/.../tests/agent_golden_gate.rs`; `attune-core/tests/golden/skill_evolution/01..10.yaml` |

**Where things physically live (the key fact):**
- Learned state / vault: `attune_core::platform::data_dir()` → `vault.db`, `vectors.encbin`, `tantivy/` (`state.rs:274/301/1942`).
- Plugin packages: `~/.local/share/attune/plugins/<plugin_id>/` (`plugin_registry.rs:20`, `default_plugins_dir` `plugin_registry.rs:343` via `dirs::data_local_dir()`).
- These are **sibling directories under the same data root** but functionally disjoint.

---

## 2. Plugin upgrade flow — does it touch learned state?

Two install paths, both **vault-blind**:

- **Entitlement sync** (`plugin_sync::sync_plugins`): pulls `entitled_plugins`, compares to installed dir list, downloads `.attunepkg`, verifies sig, extracts. `install_one_plugin` ends with: `if dst.exists() { remove_dir_all(&dst) }` then `copy_dir_recursive` (`plugin_sync.rs`, "6. 复制到目标"). → **wipes & recopies the plugin dir only**.
- **Marketplace install** (`install_plugin_package`, called from `routes/marketplace.rs:131`): validates id (path-traversal guard), extracts, loads as `Trusted`, copies into `plugins_dir`. New plugin "经一次 attune-server 重启后由 plugin_registry 装载生效".

**Neither path reads, migrates, backs up, or even references the vault DB.** Conclusion: a v1.0.5 → v1.0.6 law-pro upgrade leaves `skill_expansions` / `skill_signals` / `learned_expansions` **fully intact** — the learned state simply is not in the directory that gets overwritten.

This is **safe today but for the wrong reason**: there is no "preserve user state across upgrade" logic; preservation is a side-effect of where files happen to live. No test asserts it. `plugin_sync` comment even notes "多余的 → 留着 (防误删自装插件)" for the package dir, but says nothing about vault state.

---

## 3. Schema versioning — current state

- **No `PRAGMA user_version`** anywhere in `attune-core`. Global DB version is not tracked.
- Migration strategy = **idempotent additive ALTERs**, each guarded by `pragma_table_info` count check: `migrate_skill_signals_v07`, `migrate_items_content_hash`, `migrate_items_privacy_tier`, `migrate_corpus_domain`, `migrate_memories_multilayer`, `migrate_task_type` (`store/mod.rs:624-654`). New tables use `CREATE TABLE IF NOT EXISTS` so "old vaults auto-migrate on next open".
- This is fine for **column-additive** evolution, but provides **nothing** for *semantic* migration of learned-state **keys** (e.g. if `query_pattern` normalization changes, or an agent-id rename happens across plugin versions — old rows become unreachable, not migrated).
- The only place a learned-state schema version was ever designed is `agent_preferences.schema_version: 1` in the spec (§4.1: "lets a future agent version migrate or discard preferences cleanly") — **unimplemented**.

Per global CLAUDE.md §3.1 spec node 10 (向后兼容 / migration path): the implemented learned-state surface has **no versioned migration path**. This is a real gap for the governance spec to close.

---

## 4. Plugin-shipped vs user-accumulated boundary

| Category | Examples | Lives in | Upgrade behavior |
|---|---|---|---|
| **Plugin-shipped** (overwritten on upgrade) | agent binaries, `plugin.yaml` (`version`, `attune_min_version`, hard_red_lines), prompts, JSON schemas, golden YAML + `agent_golden_gate.rs` ratchet thresholds | `plugins/<id>/` + git/CI | **replaced wholesale** (`remove_dir_all`+recopy) |
| **User-accumulated** (must survive) | `skill_expansions`, `learned_expansions` blob, `skill_signals`/`browse_signals`, memory, (future) `agent_preferences` | vault DB (`data_dir`) | **untouched** (incidentally preserved) |

**Boundary clarity: implicitly clear by directory, but NOT documented or enforced.** Risks:
- The boundary is "wherever the code happened to write" — no invariant, no test, no doc says "learned state must never live in `plugins/`".
- The **golden-set ratchet** ("已达到的质量水位") is **dev-time / plugin-shipped**, so on upgrade the new plugin defines its own threshold; the user's runtime experience never accumulated a "water level" to lose — but equally, a plugin downgrade or a regressed shipped threshold is **not gated against the prior shipped value at the user's machine** (ratchet is enforced in CI only, per spec §2.1 "Golden-gate thresholds … never learnable / a governance act CP-7").

---

## 5. Cross-plugin sharing

- **None today — siblings are isolated.** `skill_expansions` is keyed by `query_pattern` (global, not agent/plugin-scoped); `skill_signals` carry `kind`+`ref_id` but the SkillClaw evolver only consumes `kind='search_miss'` (`get_unprocessed_signals` filter, the R17 P0 fix). The (unimplemented) `agent_preferences` design keys everything by `<agent_id>` → would be per-agent silos, no sharing.
- The "公共行业知识层 submodule" (`legal-prompts-pack`) idea in the three-product matrix is about **shipped** prompt/schema sharing, **not** runtime learned-state sharing. There is no mechanism for one plugin's learned terms to benefit another.
- Note: `skill_expansions` being **global** (not plugin-scoped) is itself a latent issue — terms learned while law-pro is installed persist and apply even after law-pro is uninstalled, with no ownership tag to attribute/clean them.

---

## 6. Six required questions — direct answers

1. **What does self-iteration learn / where?** Shipped: per-query expansion terms (`skill_expansions` table) + legacy topic expansions (`app_settings` blob), driven by `skill_signals`. Memory consolidation is separate. `behavior profile` for *preferences* is essentially absent (only HardwareProfile/OCR profiles exist). `agent_preferences` (the rich per-agent learning) is **spec-only**. Golden-set ratchet is a **CI artifact**, not runtime user state.
2. **Upgrade loss risk (v1.0.5→v1.0.6)?** Learned state is **NOT lost** — it lives in vault DB; upgrade only overwrites `plugins/<id>/`. But preservation is **incidental, untested, undocumented**. No explicit migration.
3. **Schema versioning?** No `PRAGMA user_version`; only idempotent additive ALTERs. Learned-state semantic versioning = **spec only** (`agent_preferences.schema_version`, unimplemented).
4. **Persistence boundary clear?** Clear *by directory layout* (vault vs plugins dir), but **no enforced invariant / doc / test**. Fragile if a future plugin ships learned-state seeds or renames agent/query keys.
5. **Ratchet preserved on upgrade?** The ratchet "water level" is **plugin/CI-shipped**, not user-accumulated, so there is nothing in the user's vault to reset or preserve. Upgrade replaces it with the new plugin's thresholds; ratchet monotonicity is enforced only in CI (spec §2.1), not at the user's installed copy.
6. **Cross-plugin sharing?** No. Silos. `skill_expansions` is global-but-unscoped (latent orphan/leak risk); designed `agent_preferences` is per-agent. Shared "industry knowledge" is shipped-content sharing, not learned-state sharing.

---

## 7. Upgrade-loss risk matrix (for the governance spec)

| Learned state | In vault? | Survives plugin upgrade? | Has schema version? | Has migration? | Residual risk |
|---|---|---|---|---|---|
| `skill_expansions` rows | ✅ | ✅ (incidental) | ❌ | ❌ | key-format / normalization change across versions → silent orphan rows |
| `learned_expansions` blob | ✅ | ✅ (incidental) | ❌ | ❌ | topic-key drift; unbounded-blob growth bounded only by `truncate(8)` |
| `skill_signals` / `browse_signals` | ✅ | ✅ | partial (col ALTER only) | additive only | new `kind` semantics not back-migrated |
| memory consolidation | ✅ | ✅ | partial | additive only | low |
| `agent_preferences` (designed) | (would be) | (would be) | designed v1, **unbuilt** | **none built** | entire rich-learning surface unimplemented; if built without migration, upgrade breakage likely |
| golden-set ratchet τ / F1 floor | ❌ (CI/plugin) | replaced | n/a | n/a | regressed shipped threshold not gated at user machine |

---

## 8. Recommendations for the governance spec

1. **Document & enforce the boundary**: add an invariant + test asserting no learned/user state is ever written under `plugins/`; everything mutable-by-user lives in vault DB.
2. **Introduce learned-state schema versioning**: implement the spec's `schema_version` (and/or a vault-level `PRAGMA user_version` or a meta key) so any future agent-id / query-pattern key change can migrate-or-discard cleanly instead of silently orphaning rows.
3. **Scope learned state to its owner**: tag `skill_expansions` (and future `agent_preferences`) with the plugin/agent id so uninstall/upgrade can GC or migrate that plugin's accumulated terms deliberately.
4. **Add an upgrade-preservation E2E**: real install v1.0.5, accumulate skill_expansions, upgrade to v1.0.6, assert rows intact — turn the incidental guarantee into a tested one.
5. **Decide ratchet ownership**: if a "quality water level" should persist at the user's machine across upgrades, it must move from CI-only into a versioned vault record; otherwise document explicitly that ratchet is a release-gate, not user state.
