# Quality Gating + Telemetry + Self-Tuning Audit (read-only)

**Date**: 2026-05-29
**Auditor**: read-only audit agent B
**Scope**: 10 golden-gate harnesses + telemetry.rs + skill_eval.rs + skill_evolution + golden sets + CI wiring
**Method**: static read only — no `cargo build/test`, no code change.

---

## 1. The 10 (actually 11) golden-gate harnesses — comparison table

| # | Gate file | Judge / metric | Threshold | Ratchet enforced? | CI-blocking (PR)? | Real-LLM or Mock/Deterministic |
|---|-----------|----------------|-----------|-------------------|-------------------|-------------------------------|
| 1 | `attune-core/.../chat_reliability_golden_gate.rs` | citation/contradiction/hallucination/confidence facet exact-match vs YAML `expected.*` | ENFORCE: **0 violations**; fixture floor ≥10 top + ≥3 error (`chat_reliability_golden_gate.rs:161,165,254`) | No numeric ratchet (binary 0-violation gate; fixture-count floor only) | **Yes** — runs under `cargo test --workspace --release`, no `#[ignore]` (0 ignore) | **Deterministic** — `evaluate_response()` over canned chunks, no LLM (`:186`) |
| 2 | `attune-core/.../document_classifier_agent_golden_gate.rs` | per-field exact/`>=` asserts (confidence range, missing_evidence/followups/entity counts); floor ≥11 cases, ≥10 approved, ≥3 error, sentinel required (`:312-334`) | No numeric ratchet; per-fixture exact + count floor | **Yes** — 0 `#[ignore]` | **Deterministic** — classifier run over fixtures, no LLM call in gate |
| 3 | `attune-core/.../linker_golden_gate.rs` | each fixture emits expected link-kinds; ≥10 fixtures; symmetric/idempotent/out-degree proptests (`:116,174`) | No numeric ratchet | **Yes** — 0 `#[ignore]` | **Deterministic** — `compute_links_for_item()`, no LLM |
| 4 | `attune-core/.../memory_consolidation_agent_golden_gate.rs` | `memory_promotion_golden_gate_pass_rate_must_be_one` — ≥14 cases, pass rate = 1.00; six-class floor (`:258`, ENFORCE via `ATTUNE_ENFORCE_MEMORY_FLOOR`, `:305,376`) | **pass_rate must == 1.00** (hard, deterministic) | **Yes** — 0 `#[ignore]`; floor escalation env-gated | **Deterministic** — promotion logic over fixtures |
| 5 | `attune-core/.../self_evolving_skill_agent_golden_gate.rs` | `skill_evolution_golden_gate_pass_rate_must_be_one` — ≥13 cases, exact record-set match; six-class floor ≥11 main + ≥3 error (`:309,352,356`) | **pass_rate == 1.00** (heuristic path) | **Yes** — 0 `#[ignore]` | **Deterministic (heuristic path)** — GT recomputed independently in harness |
| 6 | `attune-server/.../office_ocr_golden_gate.rs` | per-scene `accuracy() >= min_field_accuracy` + `p50_ms <= max` red lines (document/receipt/table/card 0.92; id/license 0.95) (`:62-67,302-319`) | Numeric red lines, hardcoded in `red_line()` (not a manifest; no ratchet doc) | **Yes** — 0 `#[ignore]` (runs in workspace test) | **Deterministic engine** — real PP-OCR over fixture images, no LLM |
| 7 | `attune-server/.../office_asr_golden_gate.rs` | char-level WER red lines: zh ≤0.15, en ≤0.10, mixed ≤0.18; RTF p50 ≤0.5 (`:198,217,226,234-245`) | Numeric red lines, hardcoded per-lang | **Yes** — 0 `#[ignore]` | **Deterministic engine** — real whisper.cpp over fixture audio, no LLM |
| 8 | `attune-pro/tech-pro/.../code_reviewer_golden_gate.rs` | findings-count/severity asserts; ≥11 cases, ≥10 approved, ≥3 error, sentinel (`:120,206-231`) | No numeric ratchet; per-fixture + count floor | **Yes** — 0 `#[ignore]`; runs in attune-pro `cargo test --workspace` | **Deterministic** — reviewer over fixtures |
| 9 | `attune-pro/law-pro/.../agent_golden_gate.rs` (deterministic lane) | `deterministic_agent_golden_gate` — loads `thresholds.yaml`, per-agent `pass_rate() >= threshold` else `panic!` (`:1493,1525-1534`) | **YES — manifest-driven ratchet** (`thresholds.yaml`, only-up rule documented `:1708`) | **Yes** — attune-pro `ci.yml:56-58` explicit step | **Deterministic** — 12 pure-function agents, all threshold = **1.00** |
| 10 | `attune-pro/law-pro/.../agent_golden_gate.rs` (LLM lane, PR) | `llm_agent_golden_gate` — fact_extractor micro_F1 ≥ 0.85 over holdout via **mock** replay (`mock_llm_response` YAML field) (`:1713-1718`) | **YES — `thresholds.yaml` `llm:` section, 0.85 floor, only-up** | **Yes (mock path)** — runs in workspace test | **Mock** — `HoldoutMockLlm.chat()` returns canned JSON; tests scorer+grounding+threshold, NOT the model |
| 11 | `attune-core/.../oss_agent_real_llm_gate.rs` | memory_consolidation + self_evolving real-LLM: `passed >= 4` of N (`:288,420`) | Fixed accept floor; "DO NOT lower threshold" comment `:291,423` | **NO** — all 3 tests `#[ignore]` (`:226,340`); **not referenced in any attune `.github/workflows/`** | **Real LLM** — `require_llm()` reads `ATTUNE_LLM_PROVIDER`/`ATTUNE_LLM_ENDPOINT`, default Ollama qwen2.5:3b (`:64-96`) |

**`#[ignore]` census**: only `oss_agent_real_llm_gate.rs` (3) and law-pro `agent_golden_gate.rs` (6, the real-LLM lane) carry `#[ignore]`. All 8 deterministic/mock gates have **0** `#[ignore]` → they run on every PR.

---

## 2. Answers to the 6 mandated questions

### Q1 — Gate mechanism: pass criteria / threshold / ratchet / CI hard-gate?

- **Pass criteria vary by gate type**: deterministic gates use **exact-match / 0-violation / pass_rate==1.00** (gates 1-9); engine gates (OCR/ASR) use **numeric red lines** (accuracy ≥0.92-0.95, WER ≤0.10-0.18, p50/RTF latency); the one LLM-scored gate (law-pro fact_extractor) uses **micro_F1 ≥ 0.85**.
- **Ratchet (only-up) is genuinely implemented in exactly ONE place**: law-pro `thresholds.yaml` (`deterministic:` 12 agents @1.00 + `llm:` fact_extractor @0.85), with the only-up rule documented in-code (`agent_golden_gate.rs:1708` "micro_F1 floor (≥ 0.85) per ratchet 规则只升不降") and in the manifest header ("阈值只能升, 不能降; 降阈值 PR 必须由 lawyer-reviewer 显式 +1"). **The other 9 gates hardcode thresholds inline in `.rs` source** — no manifest, no machine-checkable ratchet; "only-up" is enforced only by code-review convention, not by structure.
- **CI hard-gate**: deterministic + mock + engine gates (1-10) run in `cargo test --workspace --release` (attune `ci.yml:127`, attune-pro `ci.yml:49`), so a failure blocks the PR. law-pro additionally has an explicit dedicated step (attune-pro `ci.yml:56-58`). Parse golden gate is a separate explicit blocking step (attune `ci.yml:122`). The **real-LLM gate (11) is NOT CI-blocking on PRs** — it is `#[ignore]` and absent from all attune workflows.

### Q2 — Uniformity gap: are the 10 gates consistent? Is there an orchestration/dashboard layer?

**No unified orchestration layer. The gates are siloed.** Concrete divergences:

- **Threshold storage**: only law-pro externalizes thresholds to a YAML manifest with a ratchet contract. The other 9 hardcode numbers in `.rs` (`0.92` in `office_ocr_golden_gate.rs:62`, `0.15` in `office_asr_golden_gate.rs:235`, `>= 10/11/13/14` fixture floors scattered across files, `passed >= 4` in oss gate).
- **Metric vocabulary is inconsistent**: `exact_match_rate` / `field_exact_rate` / `status_correctness_rate` / `relation_gap_set_match_rate` (law-pro), `min_field_accuracy` (OCR), `WER`/`RTF` (ASR), `micro_F1` (fact_extractor), "0 violations" (chat_reliability), "pass_rate==1.00" (memory/skill). No shared `GateReport` schema across crates.
- **Fixture-floor conventions differ**: ≥10 (chat, linker), ≥11 (doc_classifier, code_reviewer), ≥13 (skill), ≥14 (memory) — no single source of truth for "the 6-class floor".
- **No aggregator / dashboard**: there is no top-level harness that runs all gates and emits a single roll-up. `skill_eval.rs` is a *generic* reusable eval framework (`evaluate_skill()` → `SkillEvalReport`) but it is NOT wired as the common gate substrate — the golden gates each reimplement their own loader+scorer. **This is the largest structural gap.**

### Q3 — Self-monitoring: what does telemetry.rs record? Is the §4.5-F (agent×model failure-rate >30% → UI "switch to higher tier") loop wired or a stub?

**telemetry.rs is a STUB, and it is a *different* telemetry than §4.5-F.**

- It is 157 lines, **default-OFF, opt-in only**, shipped as "Task 5 of v1.0.6 Privacy Logic". `TelemetryEvent` carries only redacted metadata (`vault_lock` | `outbound_call` | `dsar_export` | `settings_changed`) — explicitly "never chat prompts, never response text, never API keys".
- `Telemetry::send()` **has no HTTP backend**: returns `SkippedDisabled` when off, `SkippedNotImplemented` when on. Tests literally assert `never_returns_sent_in_v1_0_6` (`telemetry.rs:147`).
- **There is NO agent×model failure-rate tracking, NO error_kind(parse/grounding/timeout) accounting, NO retry_count, and NO 30%-threshold UI "switch to higher tier" prompt anywhere in the codebase.** A repo-wide grep for `30%`/`0.30`/`failure_rate`/`切高 tier`/"higher tier" surfaced only unrelated source (asr/search/cost/ocr) — no failure-telemetry hit. → **§4.5-F (failure telemetry + UI degradation hint) is entirely unimplemented.** The only model-tier guidance lives as static comments in the real-LLM gate ("label Beta or defer to v1.1"), not as runtime telemetry.

### Q4 — Self-tuning: is there a "failure signal → auto-adjust" feedback loop?

**Partially — one real loop exists (self_evolving_skill), but it is not failure-rate-driven tuning of agents.**

- `skill_evolution/agent.rs` (`self_evolving_skill_agent`, SkillClaw-style) is a genuine learning loop: it consumes `search_miss` / `doc_*` signals via `Store::record_signal_event` / `record_skill_signal`, buckets per-query, and upserts query→expansion rows into the `skill_expansions` table (3-phase prepare→generate→apply). It learns **query-expansion vocabulary** so future searches hit. Two paths: zero-cost `Heuristic` (always on) + optional `Llm` (only when `enable_llm`). Provenance kept per-row (`generated_by`+`confidence`) so the UI can show/delete learned terms.
- **What it does NOT do**: it does not adjust agent thresholds, does not re-tune prompts, and does not react to agent×model failure rates. There is **no closed loop from "agent F1 dropped on model X" → any automatic action.** Threshold ratchet is manual (PR review). So the "monitoring → fine-tune → quality gate" loop is **open**: monitoring of agent reliability is gate-time only (CI), and the only auto-feedback is search-expansion learning, a different concern.

### Q5 — Regression protection: do old gates re-run after a new agent / prompt change? #[ignore] spike detection?

- **Yes for deterministic/mock gates** — every PR runs `cargo test --workspace --release`, so all 8 zero-`#[ignore]` gates + the two law-pro lanes (deterministic + mock-LLM) re-run on any change. Sentinel/regression fixtures are baked in (doc_classifier `11-sentinel-regression.yaml` `:327`; code_reviewer sentinel `:224`; memory/skill require minimum counts that only grow).
- **`#[ignore]` spike detection is NOT automated.** CLAUDE.md §Gate-2 / 7.2 calls for "new `#[ignore]` count must not spike (≤ prev + 2)", but no CI step or script counts `#[ignore]` deltas. Slow tests are moved to nightly (`ci.yml:170` `rust-test-slow-nightly` with `--include-ignored`) — so ignored tests DO run nightly in attune-core, but the *count guard* is convention-only.
- **The real-LLM agent gate (oss) is the regression blind spot**: it is `#[ignore]` and absent from attune workflows → it does not even run nightly in attune-core (unlike law-pro which has a dedicated `nightly-real-llm.yml`). A prompt change that degrades the OSS memory/skill agents on a real model would not be caught by any scheduled job.

### Q6 — Real-LLM vs mock: which agents have real-LLM gate coverage? (per defamation mock 0.99 / real 0.09 history)

| Agent | Deterministic gate | Mock-LLM gate | Real-LLM gate | Real-LLM in CI? |
|-------|-------------------|---------------|---------------|-----------------|
| chat_reliability | ✅ | — | — | n/a (no LLM in agent path) |
| document_classifier | ✅ | — | — | — |
| linker | ✅ | — | — | n/a |
| memory_consolidation | ✅ (1.00) | — | ✅ `oss_agent_real_llm_gate` `passed>=4` | **No** (`#[ignore]`, not in workflows) — manual only |
| self_evolving_skill | ✅ (1.00, heuristic) | — | ✅ `oss_agent_real_llm_gate` `passed>=4` | **No** — manual only |
| office OCR / ASR | ✅ (real engine, not LLM) | — | — | runs in workspace test |
| tech-pro code_reviewer | ✅ | — | — | — |
| law-pro 12 deterministic agents | ✅ (1.00, pure fn) | — | — | yes (PR) |
| law-pro fact_extractor (LLM) | — | ✅ micro_F1≥0.85 (PR) | ✅ real Ollama (`nightly-real-llm.yml`, F1 floor 0.85) | **Yes — scheduled nightly** |

**Coverage verdict**: the defamation lesson (mock 0.99 / real 0.09) is only fully institutionalized in **law-pro fact_extractor**, which has both a CI mock lane AND a scheduled `nightly-real-llm.yml` cron with the same 0.85 floor. The **OSS LLM-touching agents (memory_consolidation, self_evolving_skill) have a real-LLM gate written but NOT scheduled** — it relies on a human running `cargo test -- --ignored`. Every other agent is deterministic (no LLM in its decision path), so mock/real divergence does not apply.

---

## 3. Telemetry reality conclusion

`telemetry.rs` is a **privacy-gated, default-off, send-unimplemented event queue** (vault/outbound/dsar metadata). It is **not** the §4.5-F agent-reliability failure telemetry. The §4.5-F mechanism (per-(agent×model) failure-rate accounting, error_kind classification, retry_count, and the ">30% → UI suggest higher tier" prompt) **does not exist anywhere in the codebase** — confirmed by repo-wide grep returning zero relevant hits. Any governance spec must treat "agent×model failure telemetry + degradation UI" as net-new work, not as wiring up an existing stub.

---

## 4. Biggest gaps (for governance spec)

1. **No unified gate orchestration / threshold manifest** outside law-pro. 9 of 11 gates hardcode thresholds in `.rs` with no machine-checkable ratchet and no roll-up dashboard. Recommend: lift law-pro's `thresholds.yaml` pattern to a workspace-level manifest + a single aggregator harness emitting one `GateReport`.
2. **§4.5-F failure telemetry is fully absent** (not a stub to finish — nothing exists). No runtime agent×model reliability signal, no UI tier-degradation prompt.
3. **OSS real-LLM gate is orphaned** — written (`oss_agent_real_llm_gate.rs`) but not in any attune CI/nightly workflow, unlike law-pro's `nightly-real-llm.yml`. The mock-0.99/real-0.09 class of bug can re-enter undetected for the OSS memory/skill agents.
4. **#[ignore]-spike guard is convention-only** — CLAUDE.md mandates it; no CI step counts it.
5. **Self-tuning loop is narrow** — only search-query expansion (`skill_evolution`) is a real auto-feedback loop; it does not close back onto agent quality/thresholds.

---

## Evidence index (file:line)
- chat_reliability gate: `chat_reliability_golden_gate.rs:161,165,174,186,254`
- document_classifier gate: `document_classifier_agent_golden_gate.rs:169-297,309-334`
- linker gate: `linker_golden_gate.rs:116,124,174`
- memory gate: `memory_consolidation_agent_golden_gate.rs:255-300,304-394`
- self_evolving gate: `self_evolving_skill_agent_golden_gate.rs:144-196,306-357`
- office_ocr gate: `office_ocr_golden_gate.rs:60-69,267-319`
- office_asr gate: `office_asr_golden_gate.rs:1-12,188-245`
- tech-pro gate: `code_reviewer_golden_gate.rs:120-231`
- law-pro gate: `agent_golden_gate.rs:1336-1540 (det), 1700-1850 (llm/mock), 1708 (ratchet)`
- law-pro thresholds: `plugins/law-pro/tests/golden/thresholds.yaml` (12 det @1.00 + llm fact_extractor @0.85)
- oss real-LLM gate: `oss_agent_real_llm_gate.rs:7,34-96,226,288,340,420` (3 #[ignore])
- telemetry stub: `telemetry.rs:1-15 (header), 72 (send), 147 (never-Sent test)` — 157 lines total
- skill_eval framework: `skill_eval.rs:1-60` (generic, NOT the gate substrate)
- skill_evolution loop: `skill_evolution/agent.rs:1-50`; `store/signals.rs`, `store/skill_expansions.rs`
- attune CI: `ci.yml:122 (parse gate), 127 (workspace test), 170-188 (nightly --include-ignored)`
- attune-pro CI: `ci.yml:49 (workspace), 56-58 (law-pro det gate)`; `nightly-real-llm.yml:107-108 (real-LLM cron)`
- attune workflows do NOT reference `oss_agent_real_llm`/the 7 named gates (grep = empty)
