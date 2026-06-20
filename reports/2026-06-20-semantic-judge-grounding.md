# Report: W5 Synthesis Semantic-Judge Grounding — close the below-floor item

**Date**: 2026-06-20 · **Branch**: `worktree-agent-a6eedc7b34f34da37` · **Base**: `1d80f17`
**Task**: #105 — A2 综述语义-judge grounding (spec→impl, 闭 below-floor)
**Verdict**: ✅ **达 floor**. grounding 0.826 → **0.951**, fact-consistency **stays 1.000**.

## Spec

`docs/superpowers/specs/2026-06-20-semantic-judge-grounding.md` (11 节齐, commit `6805ce1`).

## Root cause (confirmed by prior agent + this slice)

The deterministic token-overlap grounding validator structurally false-negatives **abstractive**
(paraphrase) synthesis sentences: a sentence that faithfully restates a source point shares too few
literal tokens to clear the absolute/proportional threshold, so it lands `unverified` even though it
IS sourced. deepseek-v4-flash (0.826) vs deepseek-v4-pro (0.816) probe parity proves it is the
validator's recall ceiling, NOT model capability (§4.5H). Correct fix = LLM-judge grounding step.

## Implementation (commit `83b2c4e`)

- `writing/grounding.rs` — `ground_segments_with_judge()`: after the unchanged deterministic
  `ground_segments`, an opt-in judge is asked, per still-unverified factual sentence, whether any
  candidate source span supports it. `JudgeConfig` (enabled / max_judge_calls / min_evidence_chars /
  span_window_chars), `JudgeVerdict {supported, span_id, evidence_quote}`, `candidate_spans()`,
  `relink_verify()`, `JudgeGroundOutcome`. Judge rides the shared §4.5 hardened stack (schema-guided
  JSON + ≤3 retry-validate + few-shot + PII redact).
- **Anti-mis-credit mechanism (the no-fabrication firewall)** — `relink_verify()`: a verdict is
  credited ONLY if `supported && span_id ∈ candidates && normalize(evidence_quote) ⊂ normalize(source)
  && len(quote) ≥ min_evidence_chars`. A fabricated sentence has no real quote in any source, so even
  a **lying judge** (`supported:true` + bogus quote) is rejected. The judge can ONLY flip
  unverified→verified, never the reverse.
- `writing/mod.rs` — additive `GroundingRef.judge_elevated` (serde default, no schema bump).
- `writing/synthesis.rs` — GROUND step uses the judge on the reasoning leg; `SynthesisRequest.judge_grounding`
  (default true); judge tokens billed.
- `document_intelligence/token_bill.rs` — additive `judge_llm_tokens` leg counted in billable/usd.
- `routes/writing.rs` + `writing_real_llm_gate.rs` — judge ON in production + verbatim eval path.

## Before → After (real LLM, deepseek-v4-flash, N=3)

| Metric | Before (token-overlap only) | After (+ judge fallback) | Floor |
|---|---|---|---|
| synthesis grounding-precision | 0.826 ± 0.038 ⚠ | **0.951 ± 0.044** ✅ | 0.90 |
| synthesis fact-consistency | 1.000 ± 0.000 | **1.000 ± 0.000** ✅ | 0.85 |

Per-seed (after): 0.893 / 1.000 / 0.962; fact-consistency 1.000 every seed.
Evidence (raw log, this slice):
`reports/runs/2026-06-20_semantic-judge-grounding/synthesis_judge_on_flash_n3.log`
— RESULT line 45, per-seed lines 19/31/43, `test result: ok` line 48 (246s wall).
"Before" 0.826 is the prior-agent figure recorded in `docs/wiki/writing-engine.md` (pre-judge).

## Adversarial result (no fabrication credited)

- Unit: `judge_cannot_credit_fabricated_sentence_with_bogus_quote` — a lying judge returns
  `supported:true` with a quote NOT in the source; GT computed independently
  (`!normalize(src).contains(normalize(bogus_quote))`); the re-link guard rejects → sentence stays
  unverified, `judge_credited == 0`. PASS.
- Integration: `synthesis_judge_cannot_credit_fabricated_section` — same at the synthesize() level.
- Proptest `prop_lying_judge_with_disjoint_quote_never_credits` — for arbitrary disjoint-vocab
  quotes, `judge_credited == 0` and no `judge_elevated` ref ever forms.
- The real N=3 run independently confirms it: fact-consistency held at **1.000** all 3 seeds (the
  judge lifted grounding without crediting a single fabricated fact).

## Tests (6-category, §6.1)

12 judge unit/edge/error tests + 3 proptests in `grounding.rs` (happy / fabricated-bogus-quote /
span_id-not-in-candidates / quote-too-short / not-supported / judge-None / disabled / unavailable /
zero-budget / invalid-JSON-no-panic / never-unverifies-deterministic) + 2 synthesis E2E in
`synthesis.rs`. **All 115 writing lib tests pass; token_bill 6/6; corpora-parse guard 1/1; clippy
`-D warnings` clean on attune-core (+ all-targets) and attune-server.** No existing
grounding/synthesis/writing test regressed (the pure `ground_segments` path is byte-for-byte
unchanged; existing synthesis mock tests pinned `judge_grounding:false` to stay deterministic).

## Floors / ratchet

real-LLM gate floor constants UNCHANGED (`GROUNDING_PRECISION_FLOOR=0.90` kept as the hard gate).
No floor lowered. nightly synthesis data updated (no new gate added).

## Docs updated

- `rust/RELEASE.md` — Quality + Known Limitations: W5 synthesis judge-grounding 0.951±0.044 /
  fact 1.000, judge cost contract.
- `docs/wiki/writing-engine.md` — replaced the 0.826 ⚠ Beta record with the 0.951 ✅ closed record +
  judge mechanism; added W5 row + grounding-redline note.
- `writing_real_llm_gate.rs` docstring — measured before/after + judge mechanism.

## No-regression statement

Pure `ground_segments` + `GroundingConfig` thresholds untouched; draft/rewrite paths untouched;
`WritingResult`/`SourceMaterial`/`GroundingRef` evolved additively (serde default, schema_version=1
unchanged). Judge unavailable/disabled ⇒ exact legacy deterministic behavior.
