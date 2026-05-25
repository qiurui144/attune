# Chat-reliability golden corpus

10 hand-derived (chat response, retrieved chunks, query) triples with
ground-truth `expected_*` blocks. Each fixture targets one specific signal
dimension so the gate measures the agent on the precise behavior being tested.

Per `attune-pro/docs/agent-skill-training-methodology.md` §2 "cardinal rule:
independent derivation" — ground truth here is hand-derived by reading the
response text, counting cite markers manually, and listing dates / numbers /
names that do not appear in the chunk text. **The agent's own output is never
used as ground truth.**

Each fixture YAML carries:

- `id` — fixture identifier (filename prefix)
- `description` — one-line scenario summary
- `query` — the user's chat question (currently unused by the agent but kept
  for forward compatibility and documentation)
- `response` — the LLM's answer text (inline `[item:<id>]` markers preserved)
- `chunks` — list of `{ item_id, chunk_text }` RAG hits the answer was
  supposedly grounded in
- `expected.citation_grounded` — list of `{ item_id, status }` the agent must
  emit (one per inline marker, deduped)
- `expected.contradictions_count` — number of date / money contradictions
  the agent must find
- `expected.hallucination_kinds` — multiset of hallucination kinds (sorted)
  the agent must flag (e.g. `["date", "number"]`)
- `expected.confidence_range` — `[lo, hi]` inclusive bounds the
  `overall_confidence` must fall into. Bounds are computed by hand from the
  formula in `chat_reliability::agent::confidence_from_signals` and the
  expected signal counts.
- `reviewer.approved` — only fixtures with `true` participate in the gate
- `# DERIVATION:` comment block — shows the hand calculation that produced
  the expected values (required by methodology §3 ENGINEERING_FIXTURE
  sentinel rules — every numeric expectation must carry its arithmetic)

## Source corpora

Ground truth is derived from realistic content paraphrased from:

- **Rust corpus** — paraphrased from rust-lang/book (MIT/Apache-2.0)
  chapters on ownership / borrow / lifetimes / async. Documents are
  engineered to share specific entities / cite specific item ids.
- **Operating systems corpus** — paraphrased from CyC2018/CS-Notes (CC
  BY-SA 4.0): page cache / readahead / IO scheduler write-ups.
- **OpenAI cookbook excerpts** — paraphrased; technical numbers (token
  counts / context windows) are engineered to make hallucination /
  contradiction signals testable.

These are **engineered fixtures** — small documents that target a single
hypothesis. The golden gate `chat_reliability_golden_gate.rs` loads every
`*.yaml` whose `reviewer.approved == true`, runs `evaluate_response`, and
asserts against the file's `expected.*` block.

## Why deterministic agents pass at 1.00 here

Per methodology §5: *deterministic agent → required pass rate **1.00***.
The agent is a pure function (no LLM call, no RNG, no clock); every
fixture's `expected.*` is hand-derived from the same formula the agent
implements. Any mismatch is a real algorithmic regression and must block
the merge — not a fuzzy LLM-judgement target.

## Subdirectory: `error/`

Three error fixtures live in `error/` and exercise the agent's edge handling:

- `error-empty-response.yaml`        — response is `""`, agent must not panic
  and must return neutral confidence 0.5 with empty signal vectors
- `error-no-chunks.yaml`             — response has entities but chunks is
  `[]`, every claim becomes a hallucination flag
- `error-malformed-cite-syntax.yaml` — response contains broken / partial
  cite markers (`[item:` without closing `]`, etc.), agent must skip them
  without panic and report zero citations
