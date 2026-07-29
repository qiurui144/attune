# OSS attune reliability backfill — real-LLM gates + run-log evidence (2026-06-19)

> Branch: `feat/oss-reliability-backfill` (worktree, not pushed).
> Goal: complete OSS attune's "可靠" 北极星 — close `docs/agent-capabilities.md` §5.2 gaps
> #1 (ai_annotation weakest gate), #2 (doc-intel run-log not landed), #6 (vision F1 eprintln-only).
> Honesty (§6.3): every number below is from a real provider run; raw logs in `reports/runs/<ts>/`
> (gitignored scratch per repo convention — this committed `.md` is the SSOT summary). API keys
> verified absent from every run.log (§1.4).

## 1. ai_annotation 4-angle real-LLM gate (gap #1 — the biggest短板)

**Was**: one `#[ignore]` test, Highlights-only, **silent-pass on missing Ollama** (returned Ok →
green test that never hit an LLM), no F1 / no floor / no N=3 / no ratchet.

**Now** (`crates/attune-core/tests/ai_annotation_real_llm_gate.rs` + `tests/golden/ai_annotation/`):
- 4 per-angle real-LLM gates (risk / outdated / highlights / questions), each N≥3 seeds.
- Human-authored holdout corpus, ≥10 cases/angle (risk 13, outdated 13, highlights 12, questions
  13), GT = verbatim target spans + precision-control empty notes. GT NEVER agent-generated.
- Real **micro-F1** (TP=located finding overlaps a GT target span; FP=finding matching no target
  / any finding on a precision-control note; FN=unmatched target), floor **0.50**, ratchet-only-up
  in `agent_quality_manifest.yaml`.
- **Silent-pass FIXED**: `require_llm()` panics when no model reachable (fail-to-run, never silent
  green); run-vs-SKIP is decided at the CI **job** level (skip-not-pass).
- Deterministic guards (corpus shape + scoring-overlap correctness) run on every PR (no LLM).

**Real DeepSeek (deepseek-chat) N=3 — all 4 angles ≥ floor 0.50** (4 passed; 262.58s):
`reports/runs/20260619-201722_ai-annotation-deepseek/`

| Angle | micro-F1 mean±std | located | verdict |
|-------|------------------:|--------:|---------|
| highlights | 0.735 ± 0.000 | 66 | PASS |
| outdated   | 0.826 ± 0.018 | 44 | PASS |
| questions  | 0.544 ± 0.102 | 41 | PASS (seed-2 dipped to 0.40 — fuzzy snippet matching) |
| risk       | 0.553 ± 0.038 | 41 | PASS |

questions/risk sit just above the floor — honestly recorded, not floor-relaxed. Snippet-overlap
scoring is intrinsically fuzzier than the doc-intel 4-class verdict; many "FP" findings are valid
annotations on spans the human GT did not enumerate.

## 2. doc-intel real-LLM run-log (gap #2 — claim was unsubstantiated)

The gate (`doc_intel_real_llm_gate.rs`) was already in the secret-gated CI lane (#93) + nightly,
but no committed run-log proved a real DeepSeek execution behind RELEASE v1.3.0 §9.2's
"deepseek 实测三 tier 全过 floor". Landed:
`reports/runs/20260619-202901_doc-intel-deepseek/` (3 passed; 225.59s)

| Agent | metric | mean±std | floor | verdict |
|-------|--------|---------:|------:|---------|
| compare verdict | macro-F1 (30-case) | 1.000 ± 0.000 | 0.80 | PASS (0 parse failures) |
| deep_summary | keypoint-recall | 0.944 ± 0.039 | 0.80 | PASS |
| chapters ask | grounded-rate | 1.000 ± 0.000 | 0.80 | PASS |

compare macro-F1=1.000 with zero parse failures confirms §4.5.A schema-guided + retry holds on
the real provider.

## 3. vision N=3 real-VLM run-log (gap #6 — "qwen3-vl-plus F1 1.0" was eprintln-only)

Ran the `vision-eval` feature's real-VLM `#[ignore]` lane against DashScope qwen3-vl-plus:
`reports/runs/20260619-204227_vision-qwen3vl-dashscope/` (3 passed; 36.99s)

| Test | metric | value | note |
|------|--------|------:|------|
| grounding_n3_on_known_text | token-F1 / grounding-prec | **1.000±0.000 / 1.000** | floors 0.5 / 0.99 — PASS |
| failover_dead_primary_to_live_qwen | failover | failed_over=true, winning=qwen3-vl-plus | real backend failover |
| eval_smoke_n3 (blank figure) | value-F1 | 0.000 | wiring smoke (blank image), grounding 1.000 |

The "qwen3-vl-plus F1 1.0" catalog line is now substantiated by a persisted run-log (not eprintln).

## 4. CI wiring

- `ci.yml` `real-llm-secret-gated`: added `ai_annotation_real_llm_gate` to the DeepSeek/DashScope
  N=3 loop + a new **vision real-VLM step** (qwen3-vl-plus, `vision-eval` feature) gated on
  `DASHSCOPE_API_KEY` (skip-not-block when the secret is unset).
- `nightly-real-llm.yml`: added `ai_annotation_real_llm_gate` to the Ollama loop.
- `agent_quality_manifest.yaml`: registered the `ai_annotation` gate (tier llm, micro_f1 floor
  0.50, ignored, secret-gated) → NEW-AGENT guard now claims `tests/golden/ai_annotation/`; bumped
  attune-core `#[ignore]` baseline 46→51 (+4 env-gated real-LLM legs, NOT failure-skips).

## 5. Verification

- `cargo test -p attune-core --test ai_annotation_real_llm_gate` → 2 deterministic pass, 4 ignored.
- `cargo test -p attune-core --test agent_gate_orchestrator` → 12 pass (ignore-spike within budget;
  micro_f1 in metric whitelist; manifest oss-core-only).
- `bash scripts/test-floor-check.sh` → hard-fails=0 (ai_annotation claimed; 1 pre-existing
  linker-error-case WARN unrelated).
- `cargo clippy -p attune-core --tests` and `--features vision-eval --lib` → clean (`-D warnings`).
- Real provider runs above (DeepSeek + DashScope), N=3, keys never echoed/committed.

## 6. Backlog (recorded, NOT implemented this task — per scope)

- ai_annotation Tier-1 weak-local (qwen2.5:3b) floor not yet run separately (DeepSeek=Tier-2 passed).
- `vlm_extract` not exposed through the doc-intel HTTP route (gap #3, confirmed a real gap).
- `table_structure` real SLANet inference not wired (gap #4).
- layout detection accuracy not validated against a labeled set (no mAP, gap #5).

## Commits (worktree branch `feat/oss-reliability-backfill`, not pushed)

- `763b5ac` test(oss): ai_annotation 4-angle real-LLM gate + human-GT holdout corpus
- `035df06` ci(oss): wire ai_annotation real-LLM gate + vision real-VLM lane into CI
- (this report + catalog收口 committed in a follow-up doc commit)
