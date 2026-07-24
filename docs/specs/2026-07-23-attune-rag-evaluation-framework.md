# Attune RAG Evaluation Framework

Date: 2026-07-23

Status: Draft for implementation

## Purpose

Attune's RAG, web-demo, scheduler integration, and capability-pack delivery must be validated by a durable evaluation framework, not by one-off manual E2E runs. The framework is a release-quality acceptance system for high-frequency iteration, large upgrades, scheduler changes, model changes, prompt/plugin changes, and multi-platform edge delivery.

The framework must answer five questions for every release candidate:

1. Can Attune ingest and index real-world knowledge bases at small, thousand-document, and ten-thousand-document scale?
2. Can users ask normal, practical, and high-complexity questions and receive grounded answers with citations?
3. Can Chat RAG, Summary RAG, retrieval, rerank, OCR, and scheduler jobs be isolated and diagnosed when one layer fails?
4. Can the same test intent run locally, in PR CI, in nightly hardware CI, and in release gates without rewriting cases?
5. Can every failure be traced to data, parser/OCR, retrieval, rerank, prompt/profile, scheduler, model, UI/proxy, or packaging?

## Existing Assets

The framework must build on the existing Attune test assets:

- `scripts/test-pyramid.sh`: current unit, integration, smoke, quality, corpus, and E2E test pyramid.
- `scripts/release/test-k3-nas-web-demo.sh`: K3 `.deb` remote release gate and NAS Web API contract runner.
- `scripts/release/probe-nas-web-api-contract.py`: upload, bind/rescan, vector, export, and chat scheduler API gate.
- `scripts/probe-edge-scheduler-contract.py`: strict scheduler business-contract probe.
- `tests/e2e/longtext_corpora_e2e.py`: standard long-text corpus runner.
- `scripts/eval-longtext-corpora-suite.py`: repeated search/chat/multiturn stability aggregation.
- `tests/e2e/airplane_manual_longtext_cases.json`: airplane long-text manifest.
- `tests/e2e/mechanical_design_longtext_cases.json`: Chinese mechanical-design long-text manifest.
- `docs/testing/k3-nas-web-remote-ci.md`: remote K3 CI topology and scheduler/Attune boundary.
- `docs/specs/2026-07-22-scheduler-rag-stability-requirements.md`: scheduler RAG stability requirements.

No new framework should bypass these. New pieces should generalize the current manifest/evaluator pattern.

## Design Principles

- Manifest-driven: industry, corpus, scenario, golden expectation, and gate thresholds live in data files, not hard-coded server logic.
- Layered gates: PR CI, nightly hardware CI, and release gates have different cost and coverage.
- Source-grounded by default: answer quality is not measured without citation and evidence checks.
- Scale is explicit: tests must declare document count, total bytes, chunk count, expected index size, and latency budget.
- Scheduler-aware but not scheduler-owned: Attune validates product paths; scheduler owns worker correctness and low-level performance.
- Failure attribution is mandatory: every failed case must classify the failure layer.
- Reproducible data: external corpora are pinned by URL, commit, version, checksum, or official source snapshot.
- No hidden manual gate: if a test is required for release, it must have a runner, a report, and a CI owner.

## Industry Scenario Taxonomy

### S1. Real-Time Knowledge Conversation

User asks direct factual or conceptual questions against uploaded/internal documents.

Examples:

- "TCP/IP originated where?"
- "What does this product API parameter mean?"
- "Which section defines access control?"

Acceptance:

- Hot `llm-chat` path returns within the configured interactive budget.
- Answer cites at least one relevant source.
- No unsupported answer when retrieval returns no evidence.

### S2. Operation Guidance

User asks how to perform or troubleshoot an action, using theoretical knowledge plus procedural evidence.

Examples:

- "How should I troubleshoot TCP/IP connectivity?"
- "How should I inspect hydraulic failure symptoms from the manual?"
- "How should I find gear transmission design checks in the mechanical handbook?"

Acceptance:

- Answer contains ordered steps.
- Steps are grounded in retrieved sources.
- Safety-critical or engineering-signoff claims are bounded by a caution/refusal when the corpus is not authoritative.

### S3. Decision Assistance

User asks for analysis, comparison, risk, or recommendation.

Examples:

- SEC filing risk comparison.
- NIST control mapping.
- Component trade-off summary from engineering manuals.

Acceptance:

- Answer separates evidence, inference, uncertainty, and recommendation.
- Evidence table or bullet references must map back to citations.
- The model must not invent quantitative facts absent from source material.

### S4. Summary RAG

User asks for document, corpus, or query-focused summaries.

Examples:

- "Summarize the uploaded PDF."
- "Summarize TCP/IP troubleshooting and airplane mechanical design."
- "Summarize risks across these filings."

Acceptance:

- Metadata must identify the path: `extractive-summary`, `llm-chat`, `llm-summary`, or other explicit mode.
- Summary cites evidence unless explicitly configured as metadata-only.
- If scheduler-backed `llm-summary` is unavailable, fallback must be explicit and must not poison Chat RAG readiness.

### S5. Multi-Turn Source Continuity

User asks follow-up questions where the source scope from prior turns matters.

Examples:

- "Continue based on the previous cited mechanical-design volume."
- "Now give the retrieval path for that same topic."

Acceptance:

- Follow-up does not drift to unrelated corpora.
- Citations remain consistent with the intended source family.
- Forbidden source/domain terms are checked.

### S6. Negative, Sparse, and Ambiguous Evidence

User asks questions that are outside the corpus, ambiguous, or under-supported.

Acceptance:

- The answer refuses or asks for more context.
- It must not backfill from model prior knowledge as if it came from the knowledge base.
- It must report `knowledge_count=0` or low coverage metadata when applicable.

### S7. High-Complexity Long-Text RAG

User asks across long PDFs, OCR-heavy documents, tables, formulas, multiple vendors, or multiple volumes.

Acceptance:

- Search hit/citation remains stable under large context pressure.
- Long answer either completes sync within budget or returns/polls async job without false evidence failure.
- `/ready?hot=1` remains healthy after the query.

### S8. Concurrency and High-Frequency Regression

Multiple users or CI jobs upload, search, chat, poll, and cancel concurrently.

Acceptance:

- Foreground Chat RAG is not blocked by background OCR/summary/rerank work.
- Scheduler job states remain consistent under submit/poll/cancel races.
- Attune reports queue/cold-start/failure telemetry.

## Corpus Strategy

### Scale Tiers

| Tier | Name | Documents | Target chunks | Purpose | Required in |
|---|---:|---:|---:|---|---|
| T0 | fixture | 1-10 | 10-500 | unit/integration deterministic checks | PR CI |
| T1 | smoke corpus | 10-100 | 500-10,000 | fast real parser/search/chat gate | PR optional, nightly required |
| T2 | single-industry thousand-doc corpus | 1,000-5,000 | 50,000-500,000 | realistic team/department KB in one industry | nightly hardware |
| T3 | single-industry ten-thousand-doc corpus | 10,000-50,000 | 500,000-5,000,000 | enterprise/NAS scale and regression in one industry | weekly/release gate |
| T4 | huge corpus / routing drift corpus | 50,000+ | 5,000,000+ | soak, deletion/rescan, index compaction, optional cross-industry routing drift | scheduled soak only |

Every corpus manifest must include estimated document count, byte size, parser mix, OCR ratio, language mix, expected chunks, expected index drain time, and storage footprint.

### Required Corpus Families

| Family | Examples | Purpose | Initial source |
|---|---|---|---|
| Aviation/operation | airplane manuals, FAA handbooks | operation guidance, safety refusal, long PDFs | existing airplane corpus, FAA aviation handbooks |
| Mechanical engineering | mechanical design handbook | Chinese OCR, tables/formulas, engineering retrieval | existing mechanical-design corpus |
| Networking/software | TCP/IP docs, Kubernetes docs, API docs | real-time Q&A, troubleshooting, code/API explanation | pinned public docs repos |
| Security/compliance | NIST CSF, NIST SP documents | control mapping, risk reasoning, citation precision | NIST official documents |
| Finance/filings | SEC EDGAR submissions/company facts | decision assistance, numeric evidence boundaries | SEC EDGAR APIs |
| Legal/policy | contracts, policies, regulations | clause retrieval, ambiguity, refusal | public legal/policy datasets where license permits |
| Product/support | synthetic support KB plus real docs | support workflows, versioned docs, stale docs | generated fixtures plus pinned OSS docs |
| Mixed enterprise drift | all above plus generated noise docs | cross-domain routing and source drift resistance only; not a scale acceptance corpus | generated from pinned corpora |

External sources that are acceptable as reference inputs:

- BEIR retrieval benchmark: <https://github.com/beir-cellar/beir>
- MIRACL multilingual retrieval benchmark: <https://project-miracl.github.io/>
- TREC RAG Track reference tasks: <https://trec.nist.gov/data/rag.html>
- SEC EDGAR APIs: <https://www.sec.gov/search-filings/edgar-application-programming-interfaces>
- FAA aviation handbooks/manuals: <https://www.faa.gov/regulations_policies/handbooks_manuals/aviation>
- NIST Cybersecurity Framework: <https://www.nist.gov/cyberframework>
- Ragas metric taxonomy reference: <https://docs.ragas.io/>

## Manifest Schema

The framework standardizes three manifest types.

### Corpus Manifest

Path convention:

- `tests/eval/corpora/<domain>/<corpus_id>.json`

Required fields:

```json
{
  "schema_version": "attune.eval.corpus.v1",
  "corpus_id": "networking_tcpip_smoke",
  "domain": "networking",
  "license": "public-docs",
  "source": {
    "type": "git|http|generated|local",
    "url": "https://example.invalid/repo.git",
    "commit": "optional-pinned-commit",
    "checksums": [{"path": "doc.md", "sha256": "hex"}]
  },
  "scale": {
    "tier": "T1",
    "documents": 50,
    "bytes": 10000000,
    "expected_chunks": 5000,
    "ocr_ratio": 0.0,
    "languages": ["zh", "en"]
  },
  "profiles": {
    "smoke": {"documents": ["doc-001"]},
    "comprehensive": {"documents": ["doc-001", "doc-002"]}
  },
  "indexing": {
    "parser_modes": ["text", "pdf", "ocr"],
    "max_pending_seconds": 120,
    "delete_rescan_required": true
  }
}
```

### Scenario Manifest

Path convention:

- `tests/eval/scenarios/<domain>/<scenario_id>.json`

Required fields:

```json
{
  "schema_version": "attune.eval.scenario.v1",
  "scenario_id": "networking_tcpip_troubleshooting",
  "domain": "networking",
  "scenario_type": "operation_guidance",
  "difficulty": "medium",
  "corpus_id": "networking_tcpip_smoke",
  "turns": [
    {
      "turn_id": "initial",
      "message": "如何排查 TCP/IP 连接失败？",
      "answer_mode": "grounded_steps",
      "requires_citations": true,
      "expected_sources": ["tcpip_overview"],
      "must_include": ["物理链路", "IP", "路由", "DNS", "抓包"],
      "must_not_include": ["编造引用", "无需证据"],
      "latency_budget_ms": 30000
    }
  ],
  "scheduler": {
    "expected_task": "kb.query.ask",
    "allow_async": true,
    "require_local_scheduler_metadata": true
  }
}
```

### Suite Manifest

Path convention:

- `tests/eval/suites/<suite_id>.json`

Required fields:

```json
{
  "schema_version": "attune.eval.suite.v1",
  "suite_id": "k3_release_rag_comprehensive",
  "purpose": "K3 release RAG validation",
  "corpora": ["airplane_manuals", "mechanical_design", "networking_tcpip"],
  "scenarios": ["airplane_operation_guidance", "mdh_multiturn", "network_tcpip_troubleshooting"],
  "gates": ["ingest", "search", "chat", "summary", "multiturn", "web_demo", "scheduler_contract"],
  "thresholds": {
    "retrieval_hit_at_5_min": 0.85,
    "citation_hit_rate_min": 0.90,
    "answer_accuracy_min": 0.80,
    "terminal_error_rate_max": 0.02,
    "hot_chat_p95_ms_max": 30000
  }
}
```

## Metrics

### Retrieval Metrics

- Hit@K
- Recall@K
- MRR@K
- nDCG@K when graded relevance exists
- partition/domain hit rate
- source diversity
- rerank delta
- zero-result correctness
- query rewrite correctness when rewrite is enabled

### Answer Metrics

- answer accuracy rate
- citation hit rate
- citation span validity
- groundedness/faithfulness
- required-term coverage
- forbidden-term violation rate
- unsafe operational advice rate
- unsupported numeric claim rate
- answer completeness class: `complete`, `partial`, `refusal`, `hallucinated`, `wrong_source`

### Interaction Metrics

- multi-turn source continuity
- follow-up intent preservation
- source drift rate
- user constraint preservation
- summary mode correctness
- refusal correctness

### Performance Metrics

- upload latency
- parse latency
- OCR latency
- chunk count
- embedding drain latency
- search p50/p95/p99
- rerank p50/p95
- chat p50/p95/p99
- scheduler queue wait
- scheduler cold-start wait
- scheduler generation latency
- proxy/UI wall time

### Stability Metrics

- terminal error rate
- timeout rate
- retry count
- job status miss rate
- cancel success rate
- hot readiness failure count
- quarantine event count
- Attune process restart count
- scheduler restart count

## Failure Attribution

Every failed case must emit one primary `failure_layer` and optional secondary layers:

- `data_source`: source download, checksum, LFS, license, missing file.
- `parser_ocr`: PDF extraction, OCR failure, corrupt text, table/formula degradation.
- `indexing`: chunk creation, embedding, pending queue, delete/rescan, duplicate/stale item.
- `retrieval`: search miss, wrong domain, poor rerank, vector disabled.
- `prompt_profile`: wrong intent, wrong answer mode, missing fallback, bad answer budget.
- `scheduler_contract`: `/benchmark/contract`, task schema, job status, timeout/cancel semantics.
- `scheduler_runtime`: model unavailable, quarantine, cold start, queue starvation, worker crash.
- `model_output`: hallucination, unsupported claim, incomplete answer, unsafe guidance.
- `api_surface`: HTTP status, schema break, missing metadata.
- `ui_proxy`: CORS/proxy timeout, missing latency/citations/vector chunk display.
- `packaging_config`: missing capability pack, wrong prompt/plugin/settings in `.deb` or `.exe`.

Reports must include raw API response snippets, scheduler job id, model, task, request id if available, and relevant log pointers.

## CI/CD Gates

### PR CI

Purpose: fast regression guard.

Required gates:

- Rust unit/integration tests relevant to modified crates.
- manifest schema validation.
- scenario schema validation.
- deterministic fixture search/chat tests.
- web-demo static contract tests.
- dry-run of suite planner.

Maximum target duration: 10-15 minutes.

No real K3 or huge corpus dependency.

### Nightly Hardware CI

Purpose: validate real scheduler, K3, and medium-size corpora.

Required gates:

- `.deb` install on K3.
- scheduler strict contract.
- NAS Web API contract.
- web-demo Playwright smoke.
- T1/T2 corpus ingest/search/chat/summary/multiturn.
- repeated chat and multiturn stability with at least 3 repeats.
- upload/delete/rescan convergence.

Reports:

- `reports/eval/nightly/<date>/<suite>.json`
- `reports/eval/nightly/<date>/<suite>.md`
- screenshots for failed UI cases.
- scheduler model/job snapshots.

### Weekly Scale CI

Purpose: validate thousand/ten-thousand document behavior.

Required gates:

- T2 and T3 corpus materialization.
- 1,000+ document ingest.
- 10,000+ document mixed-domain corpus when hardware budget permits.
- search regression suite.
- chat subset with domain routing.
- summary subset.
- multi-turn subset.
- index delete/rescan and rebind.
- concurrency submit/search/chat.

Reports:

- index size, chunk count, pending drain time.
- p50/p95/p99 for search/chat.
- retrieval and answer quality trend.
- failure attribution trend.

### Release Gate

Purpose: block product release when RAG capability regresses.

Required gates:

- PR CI pass.
- latest nightly hardware CI pass.
- scheduler strict contract pass.
- K3 `.deb` install and web-demo pass.
- capability pack packaging boundary pass.
- selected T2/T3 corpus pass.
- all P0 scenarios pass.
- no unclassified failures.

Release gate may allow known scheduler-deferred issues only if:

- the product path is explicitly using a stable fallback,
- metadata exposes the fallback,
- the known issue is documented in `docs/specs/2026-07-22-scheduler-rag-stability-requirements.md`,
- the release report names the exception.

## Multi-Document Scale Validation

The scale framework must validate both information lookup and support workflows.

### Thousand-Document Gate

Minimum:

- 1,000 documents in one industry domain.
- Cross-industry documents are not allowed to satisfy the document count.
- At least 2 languages.
- At least 5% PDF or OCR documents.
- At least 100 golden search queries.
- At least 50 golden chat/summary/multiturn cases covering fact lookup, operation guidance, decision assistance, summary, multiturn, and negative evidence.

Pass criteria:

- pending embeddings drain to 0.
- Hit@5 >= 0.85.
- citation hit rate >= 0.90.
- terminal error rate <= 0.02.
- hot Chat RAG p95 <= 30s on K3 hot path unless suite declares async expected.

### Ten-Thousand-Document Gate

Minimum:

- 10,000 documents in one industry domain.
- Cross-industry documents are not allowed to satisfy the document count.
- At least 20% same-industry generated noise or near-duplicate documents.
- At least 500 golden search queries.
- At least 150 chat/summary/multiturn cases covering fact lookup, operation guidance, decision assistance, summary, multiturn, negative evidence, citation stability, and long-context reasoning.
- delete/rescan convergence test.
- concurrent read/write smoke.

Pass criteria:

- no stuck pending embedding queue.
- no unbounded index growth after delete/rescan.
- wrong-domain citation rate <= 5%.
- source drift rate <= 3%.
- terminal error rate <= 3%.
- all P0 product scenarios pass.

### Support Workflow Gate

Each support workflow must include:

- user symptom.
- retrieval target.
- step-by-step guidance expectation.
- escalation/refusal boundary.
- citation expectation.
- follow-up turn.
- final answer classification.

Example:

```json
{
  "workflow_id": "support_network_tcpip_timeout",
  "symptom": "用户无法访问内网服务，ping 网关成功但服务端口超时。",
  "expected_steps": ["确认服务监听", "检查路由", "检查防火墙", "DNS/端口测试", "抓包"],
  "expected_escalation": "如果证据不足，要求用户补充日志、拓扑或抓包。",
  "forbidden": ["断言硬件故障", "编造设备配置"]
}
```

## Report Contract

Every runner emits:

```json
{
  "schema_version": "attune.eval.report.v1",
  "suite_id": "k3_release_rag_comprehensive",
  "run_id": "20260723_150000_k3",
  "target": {
    "attune_version": "1.5.6",
    "scheduler_version": "0.8.10+coldstart2",
    "platform": "k3-riscv64"
  },
  "summary": {
    "pass": true,
    "cases": 100,
    "failures": 0,
    "terminal_error_rate": 0.0
  },
  "metrics": {
    "retrieval": {},
    "answer": {},
    "performance": {},
    "stability": {}
  },
  "failures": [],
  "artifacts": {
    "markdown": "reports/eval/nightly/20260723/suite.md",
    "screenshots": [],
    "raw_logs": []
  }
}
```

Markdown report must include:

- target versions and commit.
- suite/corpus/scenario summary.
- pass/fail table.
- top regressions.
- failure attribution.
- metric trend versus previous baseline.
- operator notes for scheduler/Attune ownership.

## Required Repository Additions

The implementation should add these stable paths:

- `tests/eval/schemas/`: JSON schemas for corpus, scenario, suite, and report.
- `tests/eval/corpora/`: corpus manifests.
- `tests/eval/scenarios/`: scenario manifests.
- `tests/eval/suites/`: suite manifests.
- `tests/eval/fixtures/`: deterministic tiny fixtures for PR CI.
- `scripts/eval/validate-manifests.py`: schema validator.
- `scripts/eval/run-suite.py`: generic suite runner.
- `scripts/eval/report-diff.py`: baseline/regression diff.
- `docs/testing/attune-rag-evaluation-framework.md`: operator-facing test framework guide.

Existing long-text scripts may remain during migration, but new suites should converge on the generic runner.

## Acceptance Criteria

P0:

- The new framework can express existing airplane and mechanical-design tests without losing current checks.
- PR CI can validate schemas and run deterministic fixtures.
- K3 nightly can run scheduler contract, NAS Web API contract, web-demo Playwright, and at least one long-text suite.
- Reports classify failures into the defined layers.
- Summary, chat, retrieval, and web-demo metrics are emitted in one report schema.

P1:

- Networking, security/compliance, finance, and product-support corpora are added.
- Thousand-document suite is automated and repeatable.
- Regression diff can compare two reports and fail on metric degradation.
- CI artifacts are archived with screenshots and raw response snippets.

P2:

- Ten-thousand-document suite is automated.
- Soak/concurrency suite is scheduled.
- Trend dashboard can be generated from report JSON files.
