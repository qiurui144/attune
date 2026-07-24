# Attune RAG Evaluation Framework Implementation Plan

Date: 2026-07-23

Status: Draft for execution

Source spec: `docs/specs/2026-07-23-attune-rag-evaluation-framework.md`

## Goal

Build a durable Attune RAG evaluation framework that validates industry scenarios, thousand/ten-thousand-document knowledge bases, Chat RAG, Summary RAG, retrieval, web-demo, scheduler integration, and CI regression gates.

## Architecture

The framework is manifest-driven. JSON schemas define corpora, scenarios, suites, and reports; Python runners execute existing Attune API/web/scheduler gates plus new generic scenario suites; CI consumes a single report contract and archives artifacts.

## Global Constraints

- Do not hard-code industry cases in Attune server business logic.
- Reuse existing `tests/e2e`, `scripts/eval-*`, `scripts/release/*`, and `reports/release` patterns.
- Every release-blocking test must have a runner, report, threshold, and failure attribution.
- Thousand-document and ten-thousand-document suites must be explicit scale tiers, not accidental stress tests.
- Scheduler worker correctness remains scheduler-owned; Attune validates product paths and public scheduler contracts.
- All public corpora must be pinned by URL plus commit, version, checksum, or official snapshot.
- Reports must identify target Attune version, scheduler version, platform, corpus, scenario, metrics, artifacts, and failure layer.

## Files

Create:

- `tests/eval/schemas/corpus.schema.json`
- `tests/eval/schemas/scenario.schema.json`
- `tests/eval/schemas/suite.schema.json`
- `tests/eval/schemas/report.schema.json`
- `tests/eval/assets/public_knowledge_assets.json`
- `tests/eval/corpora/networking/tcpip_smoke.json`
- `tests/eval/scenarios/networking/tcpip_troubleshooting.json`
- `tests/eval/suites/pr_rag_smoke.json`
- `tests/eval/suites/k3_rag_release_smoke.json`
- `tests/eval/suites/k3_rag_scale_thousand.json`
- `scripts/eval/validate-manifests.py`
- `scripts/eval/run-suite.py`
- `scripts/eval/report-diff.py`
- `scripts/eval/generate-scale-corpus.py`
- `docs/specs/2026-07-23-attune-rag-evaluation-assets.md`
- `docs/testing/attune-rag-evaluation-framework.md`

Modify:

- `scripts/test-pyramid.sh`
- `docs/testing/k3-nas-web-remote-ci.md`
- `.github/workflows/ci.yml`
- `scripts/release/test-k3-nas-web-demo.sh`
- `scripts/release/probe-nas-web-api-contract.py`
- `scripts/eval-longtext-corpora-suite.py`
- `docs/specs/2026-07-22-scheduler-rag-stability-requirements.md`

## Task 1: Schemas and Manifest Validator

Deliverable:

- JSON schemas for corpus, scenario, suite, and report.
- `scripts/eval/validate-manifests.py` validates required fields, type basics, suite-to-corpus references, suite-to-scenario references, and scenario-to-corpus references.

Tests:

- `tests/scripts/eval_manifest_validator_test.sh`

Commands:

```bash
bash tests/scripts/eval_manifest_validator_test.sh
python3 scripts/eval/validate-manifests.py --root . --suite pr_rag_smoke
```

Acceptance:

- invalid manifests fail with actionable messages.
- valid `pr_rag_smoke` and `k3_rag_release_smoke` pass.

## Task 2: Initial Industry Manifests

Deliverable:

- networking TCP/IP generated corpus.
- operation-guidance TCP/IP troubleshooting scenario.
- PR smoke suite.
- K3 release smoke suite.

Tests:

```bash
python3 scripts/eval/validate-manifests.py --root . --suite pr_rag_smoke
python3 scripts/eval/validate-manifests.py --root . --suite k3_rag_release_smoke
```

Acceptance:

- PR suite does not require K3 hardware.
- K3 suite references existing airplane/mechanical-design coverage plus networking smoke.
- all scenarios include citations, expected sources, required terms, forbidden terms, latency budget, and scheduler expectations.

## Task 3: Generic Suite Runner

Deliverable:

- `scripts/eval/run-suite.py`
- dry-run mode resolves all cases and emits an eval report.
- live mode can upload fixtures, wait for embeddings, run search, run chat, poll scheduler jobs, and classify failures.
- Current implementation status: dry-run mode and basic live API mode are implemented for generated Markdown corpora. Live mode covers upload, status drain, search, chat, async scheduler job polling, citation/source checks, required/forbidden term checks, latency extraction, retrieval Hit@5, API counters, scheduler queue/generation/cold-start metrics, suite threshold enforcement, terminal error rate by failed turn, and `failure_layer` reporting.

Tests:

- `tests/scripts/eval_run_suite_test.sh`
- `tests/scripts/eval_run_suite_live_test.sh`
- `tests/scripts/eval_run_suite_live_async_job_test.sh`
- `tests/scripts/eval_run_suite_live_failure_test.sh`
- `tests/scripts/eval_run_suite_live_threshold_test.sh`

Commands:

```bash
python3 scripts/eval/run-suite.py \
  --root . \
  --suite pr_rag_smoke \
  --base-url http://127.0.0.1:18905 \
  --out /tmp/attune-pr-rag-smoke.json \
  --dry-run
```

Acceptance:

- report matches `attune.eval.report.v1`.
- dry-run requires no server.
- live mode can fail cases with explicit `failure_layer`.
- live mode enforces suite thresholds as release-blocking `failure_layer=threshold` failures.

## Task 4: Report Diff and Regression Gate

Deliverable:

- `scripts/eval/report-diff.py`

Tests:

- `tests/scripts/eval_report_diff_test.sh`

Metrics compared:

- pass/fail state.
- terminal error rate.
- retrieval Hit@5.
- citation hit rate.
- answer accuracy.
- p95 latency.
- failure layer count.

Acceptance:

- worse candidate exits nonzero with `--fail-on-regression`.
- better or equivalent candidate exits 0.
- report explains which metrics regressed.

## Task 5: Thousand-Document Scale Suite

Deliverable:

- deterministic scale corpus generator.
- `k3_rag_scale_thousand` suite.

Files:

- `scripts/eval/generate-scale-corpus.py`
- `tests/eval/corpora/security/security_generated_thousand_docs.json`
- `tests/eval/scenarios/security/security_scale_coverage.json`
- `tests/eval/suites/k3_rag_scale_thousand.json`

Command:

```bash
python3 scripts/eval/generate-scale-corpus.py \
  --documents 1000 \
  --domains security \
  --out /tmp/attune-scale-corpus
```

Acceptance:

- generated documents contain domain markers, document ids, topic terms, near-duplicate distractors, and support workflow snippets.
- suite declares scale tier `T2` and a single industry domain.
- the scale corpus must not use `mixed_enterprise` or cross-industry documents to satisfy the 1,000 document count.
- scenario coverage includes fact lookup, operation guidance, decision assistance, summary, multiturn, and negative evidence.
- thresholds include Hit@5 >= 0.85, citation hit rate >= 0.90, summary coverage >= 0.80, multiturn source continuity >= 0.85, negative evidence refusal rate >= 0.90, terminal error rate <= 0.02, search p95 <= 5000ms, hot chat p95 <= 30000ms, and summary p95 <= 45000ms.

## Task 6: Ten-Thousand-Document Suite Contract

Deliverable:

- `k3_rag_scale_ten_thousand` suite contract.
- generator profile for 10,000 single-industry security documents.
- report storage convention for weekly/release scale runs.

Acceptance:

- suite declares scale tier `T3` and a single industry domain.
- the scale corpus must not use `mixed_enterprise` or cross-industry documents to satisfy the 10,000 document count.
- at least 20% same-industry generated noise or near-duplicate documents.
- at least 500 search cases and 150 chat/summary/multiturn cases in the final filled suite.
- delete/rescan convergence and concurrent read/write smoke are required gates.

## Task 7: K3/Nightly CI Wiring

Deliverable:

- `--with-eval-smoke` switch in `scripts/test-pyramid.sh`.
- CI validates schemas and dry-run suites.
- remote CI docs define PR, nightly hardware, weekly scale, and release gate ownership.
- Current implementation status: `scripts/test-pyramid.sh --with-eval-smoke` is implemented for local/PR manifest validation and `pr_rag_smoke` dry-run. `.github/workflows/ci.yml` has a hardware-free `RAG Eval Smoke` job that validates manifests, runs the PR suite dry-run, and uploads the report artifact. Eval smoke also validates the asset registry, single-industry scale contract, metric-system contract, and `kb-web-demo` frontend contract. Remote CI documentation references the eval framework guide and suite ids.

Commands:

```bash
bash scripts/test-pyramid.sh --with-eval-smoke
python3 scripts/eval/validate-manifests.py --root . --suite pr_rag_smoke
python3 scripts/eval/run-suite.py --root . --suite pr_rag_smoke --dry-run --out /tmp/attune-pr-rag-smoke.json
```

Acceptance:

- PR CI stays hardware-free.
- nightly hardware CI owns K3 live suites.
- weekly CI owns T2/T3 scale runs.
- `kb-web-demo` is the standard frontend simulation surface for upload,
  vector chunk display, Chat RAG, Summary RAG, citations, and timing display.

## Task 8: Existing Long-Text Migration

Deliverable:

- compatibility manifests for existing airplane and mechanical-design corpora.
- optional `--report-schema-v1` output in `scripts/eval-longtext-corpora-suite.py`.

Files:

- `tests/eval/corpora/aviation/airplane_manuals.json`
- `tests/eval/corpora/mechanical/mechanical_design_handbook.json`
- `tests/eval/scenarios/aviation/airplane_operation_guidance.json`
- `tests/eval/scenarios/mechanical/mechanical_design_multiturn.json`

Acceptance:

- existing long-text scripts keep working.
- new report schema can include compatibility summaries.
- no loss of current airplane/mechanical coverage dimensions.

## Task 9: Release Gate Adoption

Deliverable:

- optional eval suite execution in `scripts/release/test-k3-nas-web-demo.sh`.
- release report archives eval JSON/Markdown.

New env:

```bash
ATTUNE_K3_EVAL_SUITE=k3_rag_release_smoke
ATTUNE_K3_EVAL_OUT=/tmp/attune-k3-rag-release-smoke.json
```

Acceptance:

- current release script behavior is unchanged when env is unset.
- when env is set, failure in required eval suite blocks release.
- release report names scheduler-owned exceptions separately from Attune product failures.

## Task 10: Operator Guide

Deliverable:

- `docs/testing/attune-rag-evaluation-framework.md`

Current implementation status: initial operator guide is implemented with local PR smoke, live API smoke, K3 release smoke, scale suites, report diff, failure attribution, corpus/scenario authoring, and ownership guidance.

Required sections:

- local PR smoke.
- K3 nightly.
- weekly thousand/ten-thousand document runs.
- report paths.
- failure attribution.
- how to add a new industry corpus.
- how to add a new scenario.
- how to update thresholds.
- how to decide Attune-owned vs scheduler-owned failure.

Acceptance:

- a new engineer can run PR dry-run and read a report from the guide.
- K3 operator can run the hardware suite and archive artifacts from the guide.

## Execution Order

1. Task 1: schemas and validator.
2. Task 2: initial manifests.
3. Task 3: generic runner dry-run, then live mode.
4. Task 4: report diff.
5. Task 5: thousand-document suite.
6. Task 7: PR/nightly CI wiring.
7. Task 8: long-text migration.
8. Task 9: release gate adoption.
9. Task 6: ten-thousand-document suite.
10. Task 10: operator guide, updated as each task lands.

## Review Gates

Each task must pass:

- schema or script tests added in that task.
- `python3 scripts/eval/validate-manifests.py --root . --suite pr_rag_smoke` after Task 2.
- no change to existing K3 release behavior unless `ATTUNE_K3_EVAL_SUITE` is set.
- no server hard-coding of industry-specific cases.
