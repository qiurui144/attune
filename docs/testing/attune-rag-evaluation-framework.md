# Attune RAG Evaluation Framework

Date: 2026-07-23

This guide explains how to run the manifest-driven RAG evaluation framework.
The framework is defined by:

- Spec: `docs/specs/2026-07-23-attune-rag-evaluation-framework.md`
- Asset design: `docs/specs/2026-07-23-attune-rag-evaluation-assets.md`
- Plan: `docs/specs/2026-07-23-attune-rag-evaluation-framework-plan.md`
- Schemas: `tests/eval/schemas/`
- Asset registry: `tests/eval/assets/public_knowledge_assets.json`
- Corpora: `tests/eval/corpora/`
- Scenarios: `tests/eval/scenarios/`
- Suites: `tests/eval/suites/`
- Runners: `scripts/eval/`

## Local PR Smoke

Validate manifests and generate a dry-run report:

```bash
PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/validate-manifests.py --root . --suite pr_rag_smoke

PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/run-suite.py \
  --root . \
  --suite pr_rag_smoke \
  --base-url http://127.0.0.1:18905 \
  --out /tmp/attune-pr-rag-smoke.json \
  --dry-run
```

Or run through the test pyramid:

```bash
bash scripts/test-pyramid.sh --with-eval-smoke
```

This path does not require K3 hardware or a live Attune server.

The PR smoke also validates the public asset registry contract. That registry
is the source of truth for selected public repositories, fixed commits or
snapshots, redistribution posture, expected sources, required answer terms,
accuracy thresholds, and latency thresholds.

## Live API Smoke

When an Attune API server is running:

```bash
PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/run-suite.py \
  --root . \
  --suite pr_rag_smoke \
  --base-url http://127.0.0.1:18905 \
  --out /tmp/attune-pr-rag-smoke-live.json
```

Live mode currently supports generated Markdown corpora. It uploads fixture
documents, waits for `pending_embeddings=0`, runs search, runs chat, polls async
scheduler jobs returned by `/api/v1/chat`, and emits a report.

## K3 Release Smoke

Use the K3 release suite once a `.deb` has been installed and the server is
configured to use the local scheduler:

```bash
PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/validate-manifests.py --root . --suite k3_rag_release_smoke

PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/run-suite.py \
  --root . \
  --suite k3_rag_release_smoke \
  --base-url http://192.168.100.233:18900 \
  --out reports/release/k3-rag-release-smoke.json \
  --dry-run
```

The same suite can be attached to the standard K3 `.deb` release script:

```bash
ATTUNE_K3_EVAL_SUITE=k3_rag_release_smoke \
ATTUNE_K3_EVAL_OUT=reports/release/k3-rag-release-smoke.json \
ATTUNE_K3_HOST=192.168.100.233 \
ATTUNE_K3_BASE_URL=http://192.168.100.233:18900 \
ATTUNE_K3_SCHEDULER_URL=http://192.168.100.233:8090 \
bash scripts/release/test-k3-nas-web-demo.sh \
  --deb dist/release/riscv64-server-deb/attune-server_<version>_riscv64.deb
```

When `ATTUNE_K3_EVAL_SUITE` is unset, the release script behavior is unchanged.
When it is set, the eval report JSON is archived at `ATTUNE_K3_EVAL_OUT` or at
`reports/release/k3-rag-eval-<suite>-<timestamp>.json`, and any suite failure
blocks the release script.

The current generic runner validates generated fixtures directly. Existing K3
release gates for long-text airplane/mechanical corpora still run through
`scripts/release/test-k3-nas-web-demo.sh` and `tests/e2e/longtext_corpora_e2e.py`
until those legacy corpora are fully migrated to the generic live runner.

## Scale Suites

Validate the thousand-document suite contract:

```bash
PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/validate-manifests.py --root . --suite k3_rag_scale_thousand

PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/run-suite.py \
  --root . \
  --suite k3_rag_scale_thousand \
  --base-url http://192.168.100.233:18900 \
  --out /tmp/attune-k3-scale-thousand.json \
  --dry-run
```

Generate deterministic scale corpus documents:

```bash
PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/generate-scale-corpus.py \
  --documents 1000 \
  --domains security \
  --out /tmp/attune-scale-corpus
```

Scale suites are single-industry gates. `k3_rag_scale_thousand` and
`k3_rag_scale_ten_thousand` currently validate the `security` industry corpus;
mixed-enterprise corpora are reserved for source-drift/routing专项 and do not
count as scale acceptance.

The ten-thousand-document contract is:

```bash
PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/validate-manifests.py --root . --suite k3_rag_scale_ten_thousand
```

## Web-Demo Frontend Gate

`kb-web-demo` is the standard frontend simulation surface for release E2E. It
must render upload progress, vector chunks, Chat RAG, Summary RAG, citations,
and elapsed-time fields. Contract smoke:

```bash
bash tests/scripts/eval_web_demo_frontend_contract_test.sh
```

Live browser gate:

```bash
python3 tests/e2e/playwright/kb_web_demo_eval_frontend_e2e.py \
  --base-url http://192.168.100.233:8890 \
  --api-url http://192.168.100.233:8889 \
  --out reports/release/kb-web-demo-frontend.json
```

## Report Diff

Compare a candidate report against a baseline:

```bash
PYTHONDONTWRITEBYTECODE=1 \
python3 scripts/eval/report-diff.py \
  --baseline reports/eval/baseline.json \
  --candidate reports/eval/candidate.json \
  --out reports/eval/report-diff.json \
  --fail-on-regression
```

The diff checks pass/fail state, terminal error rate, Hit@5, citation hit rate,
answer accuracy, chat p95 latency, and failure-layer counts.

Live suite reports also enforce suite thresholds directly. Violations are
recorded as `failure_layer=threshold` and set `summary.pass=false`; release
scripts that enable `ATTUNE_K3_EVAL_SUITE` therefore block on threshold
regressions without requiring a separate diff step.

## Failure Attribution

Every live failure must classify one primary `failure_layer`:

- `data_source`
- `parser_ocr`
- `indexing`
- `retrieval`
- `prompt_profile`
- `scheduler_contract`
- `scheduler_runtime`
- `model_output`
- `api_surface`
- `ui_proxy`
- `packaging_config`

Current generic live runner emits:

- `indexing`: embedding queue did not drain.
- `retrieval`: search failed or returned no results.
- `api_surface`: upload or chat HTTP/API failure.
- `scheduler_contract`: async job polling failed or returned terminal failure.
- `model_output`: missing expected citation, missing required term, or forbidden term appeared.

## Adding a Corpus

1. Add `tests/eval/corpora/<domain>/<corpus_id>.json`.
2. Set `schema_version` to `attune.eval.corpus.v1`.
3. Pin external data by URL plus commit/version/checksum, or mark generated data with a deterministic generator.
4. Declare `scale.tier`, document count, expected chunks, OCR ratio, languages, parser modes, and max pending seconds.
5. Run:

```bash
python3 scripts/eval/validate-manifests.py --root . --suite <suite_id>
```

## Adding a Scenario

1. Add `tests/eval/scenarios/<domain>/<scenario_id>.json`.
2. Set `schema_version` to `attune.eval.scenario.v1`.
3. Reference an existing `corpus_id`.
4. Each turn must include message, answer mode, citation requirement, expected sources, required terms, forbidden terms, and latency budget.
5. Add the scenario id to a suite.

## Ownership

Attune-owned failures:

- upload/index/search/chat API breakage.
- wrong or missing citations.
- answer not grounded in expected sources.
- web-demo upload/vector/chat/summary rendering.
- capability-pack or prompt/profile packaging regression.

Scheduler-owned failures:

- public scheduler contract schema break.
- scheduler job state inconsistency.
- worker timeout, quarantine, cold-start, or queue starvation.
- model runtime failure behind scheduler business tasks.

If a report includes scheduler-owned failures, the release note must reference
the scheduler version and link to scheduler logs or job ids.
