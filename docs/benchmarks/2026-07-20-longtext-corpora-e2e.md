# Standard GitHub Long-Text Corpora E2E

Date: 2026-07-20

Standard corpora:

- Airplane manuals: <https://github.com/shiroinekotfs/airplane-manual-collection.git>
- Mechanical Design handbook: <https://github.com/GEQfa/handbook-of-mechanical-design.git>

The mechanical-design source is pinned at
`86832fd643cb1f9cfa1188d242d34b62dd52e41f`. Its five PDF volumes are Git LFS
objects, about 303 MB total, and are intentionally kept outside this repository.

## Purpose

The long-text E2E gate now has two parallel GitHub corpora:

- `airplane`: English, many manuals, many vendors, manual-type and source-drift
  disambiguation, aviation safety refusal.
- `mechanical_design`: Chinese scanned/PDF handbook, Git LFS materialization,
  OCR-heavy ingest, table/formula-dense pages, exact volume lookup, cross-volume
  mechanical-design questions, and multi-turn source continuity.

This split avoids treating airplane results as a proxy for Chinese OCR and
engineering handbook behavior.

## Generated Artifacts

- `scripts/build-mechanical-design-longtext-dataset.py`
  builds `tests/e2e/mechanical_design_longtext_cases.json` and optional golden
  queries without downloading PDFs.
- `tests/e2e/mechanical_design_longtext_e2e.py`
  materializes the Git LFS PDFs, checks that real PDF objects were fetched, binds
  the corpus through Attune, waits for embeddings, then runs search/chat/multiturn.
- `tests/e2e/longtext_corpora_e2e.py`
  runs the standard corpora list, default `airplane,mechanical_design`.
- `scripts/eval-longtext-corpora-suite.py`
  repeats search/chat/multiturn after ingest and writes aggregated stability
  data.

## Commands

Dry-run the standard corpus plan:

```bash
ATTUNE_LONGTEXT_DRY_RUN=1 \
ATTUNE_LONGTEXT_CORPORA=airplane,mechanical_design \
ATTUNE_LONGTEXT_UI=0 \
python3 tests/e2e/longtext_corpora_e2e.py
```

Run the full API gate on a host where Attune can bind the corpus paths:

```bash
ATTUNE_E2E_LONGTEXT=1 \
ATTUNE_LONGTEXT_CORPORA=airplane,mechanical_design \
ATTUNE_LONGTEXT_PROFILE=edge_scheduler_comprehensive \
ATTUNE_E2E_LOCAL_SCHEDULER=http://127.0.0.1:8090 \
ATTUNE_LONGTEXT_UI=0 \
bash tests/e2e/run_all.sh
```

After ingest has completed, collect repeated chat and multi-turn stability data:

```bash
ATTUNE_LONGTEXT_REPEAT_CHAT=3 \
ATTUNE_LONGTEXT_CORPORA=airplane,mechanical_design \
python3 scripts/eval-longtext-corpora-suite.py \
  --base-url http://127.0.0.1:18905 \
  --profile edge_scheduler_comprehensive \
  --out /tmp/attune-longtext-corpora-suite-summary.json \
  --fail-on-targets
```

## Mechanical-Design Query Coverage

The checked-in manifest covers:

- Easy: inventory and exact volume lookup.
- Medium: components, gear/belt/chain transmission, hydraulic/pneumatic, control
  and reliability topics.
- Hard: cross-volume shaft/bearing/transmission; hydraulic plus control split;
  table/formula/OCR adequacy probe.
- Multi-turn: first grounded gear-transmission answer, source-continuity follow-up,
  then a harder retrieval-path follow-up that must not drift to airplane sources.

Initial targets are lower than airplane because this corpus adds Chinese OCR and
table/formula density:

- vector Hit@5 >= 0.80, Recall@10 >= 0.75, MRR@10 >= 0.60;
- answer citation hit >= 0.85, answer term accuracy >= 0.75;
- edge scheduler p95 answer latency <= 15000 ms;
- repeat-suite terminal error rate <= 0.02.

## Result Contract

Hardware runs must archive:

- `attune-airplane-longtext-<profile>-search/chat/multiturn.json`
- `attune-mechanical-design-longtext-<profile>-search/chat/multiturn.json`
- `attune-*-chat-repeat-01..N.json`
- `attune-*-multiturn-repeat-01..N.json`
- `attune-longtext-corpora-<profile>-suite-summary.json`

Report the aggregate by corpus: query/turn count, terminal error rate, p50/p95
latency median, max p95 latency, citation hit rate, answer accuracy, and any
degraded OCR/document counts observed during bind.
