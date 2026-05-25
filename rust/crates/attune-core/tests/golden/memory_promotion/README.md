# memory_consolidation_agent golden set

Real-corpus-derived golden cases for the deterministic L2 → L3 promotion agent.

## Corpus provenance

Episodic-memory `summary` strings are derived from `tests/fixtures/parse_corpus/`
(see `manifest.yaml` there) — the same pinned, version-stable corpus the K2
parse golden gate uses. We do not duplicate the source markdown here; what we
need is just the *topical content* an episodic summary would carry, which can
be paraphrased deterministically.

## Schema

```yaml
id: <kebab>                       # case id (matches filename)
description: <one-line>           # human description of the scenario
episodic_memories:                # 1..N L2 rows seeded into a tempfile vault
  - id: <stable-id>               # used as the episodic memory's stable handle in assertions
    chunk_hashes: [...]           # source_chunk_hashes (member IDs)
    summary: <chinese text>       # carried up to L3 verbatim if promoted
    created_at_offset_days: <int> # days BEFORE now_secs (always ≥ 0)
citation_hits:                    # signals seeded into skill_signals(kind='citation_hit')
  - chunk: <chunk_hash>
    count: <int>                  # ≥ 1
config:
  promotion_window_days: <u32>
  min_access_count: <u32>
  min_score: <f64>
  max_promotions_per_run: <usize>
expected:
  promoted_ids: [<episodic-id>]   # ids that MUST get a new semantic_id
  gated_by_access: <usize>        # exact count
  gated_by_score: <usize>         # exact count
  already_promoted: <usize>       # for re-run scenarios; default 0
reviewer:
  approved: true                  # only approved cases enter the gate
```

## Ground-truth invariant (per attune CLAUDE.md "Agent 验证铁律")

Each `expected.promoted_ids` is **independently computed by hand**, not by
calling `agent.run()`. The score formula is fixed:

```
score = 1.0 * access + 2.0 / (1 + days * 0.1) + 0.5 * ln(max(1, chunks))
```

`gated_by_access` = #candidates where access < `min_access_count`.
`gated_by_score`  = #candidates that passed access gate but score < `min_score`.

## CI gate

`tests/memory_consolidation_agent_golden_gate.rs` walks every `*.yaml` file with
`reviewer.approved: true`, seeds the scenario into a fresh `Store::open_memory`,
runs `run_promotion_cycle`, and **panics on any disagreement** with `expected.*`.

Pass rate is **1.00** (deterministic agent — single miss = bug, no statistical
tolerance allowed).

The companion `error/` subfolder uses the same schema with an extra
`expected_no_promotions: true` flag instead of `promoted_ids`, for scenarios
that intentionally produce zero promotions (idempotency / starve / etc.).
