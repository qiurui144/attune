# Scheduler RAG Stability Requirements

> Background: Attune `1.5.3` + k3-scheduler `0.8.4` + `oss-rag-default` E2E on K3 target `192.168.100.233` did not pass full release validation. Short Chat RAG can work, but high-complexity PDF RAG hits worker timeout, `llm-summary` can trigger accelerator quarantine, and quarantine currently impacts hot Chat/Embedding readiness.
>
> 2026-07-23 update: Attune `1.5.5` + k3-scheduler `0.8.6` passes the NAS Web API/web-demo contract for upload, index bind/rescan, vector drain, export, and Chat RAG through `/api/v1/chat -> /kb/tasks/kb.query.ask`. The remaining scheduler blockers are `llm-summary`/OCR accelerator quarantine blast radius and the loopback-only `/infer` performance-gate execution path.
>
> 2026-07-23 latest validation: Attune `1.5.6` + k3-scheduler `0.8.10+coldstart2` passes scheduler strict contract, NAS Web API contract, and kb-web-demo Playwright E2E on K3 target `192.168.100.233`. The previously observed job/readiness race was not reproduced on the latest scheduler: hot sync `kb.query.ask` and explicit async `kb.query.ask:async` both completed, and `/ready?hot=1` stayed healthy during concurrent polling. Cold-start semantics are improved enough for the validated hot demo path, but the scheduler contract still needs explicit ETA/phase/cancel semantics for cold or abandoned long-running 30B work.
>
> 2026-07-23 follow-up validation: k3-scheduler `0.8.11+eta1` preserves accepted async `eta_ms` on non-terminal `GET /jobs/{id}` and `GET /jobs` responses. K3 validation on `192.168.100.233` confirmed a cold async `kb.query.ask:async` returned `eta_ms=71100`, the immediate non-terminal job status retained the same ETA at top-level and `telemetry.eta_ms`, and the job later completed without error. This closes the accepted-ETA persistence gap; dynamic remaining ETA, long-running worker cancellation, and deferred-model blast-radius tests remain open.
>
> 2026-07-24 validation: Attune `1.5.12` + k3-scheduler `0.8.14+cma4` was tested on K3 target `192.168.100.233`. Attune fixed the `/api/v1/chat` behavior that previously exposed scheduler async job text as the user-visible answer: async `kb.query.ask` now polls within a bounded realtime budget and returns `local-scheduler-delayed` if the job does not finish. The remaining E2E blocker is scheduler/runtime-side: `llm-chat` 30B repeatedly oscillated between `WARMING`, `READY`, `FAILED/cold`, and `WARMING`; accepted `kb.query.ask` jobs stayed in `scheduler_queue` with `reason=cold_start_avoided`; and resident 30B could still be rejected with `K3 CMA free memory is below the scheduler minimum`. This blocks full deb E2E even when upload, bind scan, vector drain, export, scheduler contract, and RVV gate pass.
>
> 2026-07-24 release validation: Attune `1.5.18` + k3-scheduler `0.8.14+ragobs4` passes the K3 deb NAS Web API contract, kb-web-demo Playwright E2E, and single-industry RAG clean suites for networking, aviation, and mechanical design on `192.168.100.233`. Attune now sends recent chat history into scheduler-native `kb.query.ask`, disambiguates Chinese follow-up/source hints, separates manual/source lookup from diagnostic guardrails, and prepends a stable `引用来源：...` line to scheduler KB answers with citations so user-visible answers and eval output do not depend on the model restating source names. Scheduler `llm-summary` is present and direct operator infer works after cold start, but the stable application RAG task remains `kb.query.ask`; there is no separate `kb.query.summary` business task in the observed contract.
>
> 2026-07-24 planner validation: Attune `1.5.20` + k3-scheduler `0.8.14+ragobs4` passes K3 deb NAS Web API contract, kb-web-demo Playwright E2E, and an explicit multi-topic planner API E2E on `192.168.100.233`. The new Attune-owned `rag_intent_plan` appears in `/api/v1/chat`, scheduler `kb.query.ask` remains the execution task, and per-topic summary queries preserved all required topic representatives with `missing_topics=[]`.

## Goal

Make k3-scheduler reliably support Attune KB demo and edge KB deployment:

- PDF upload -> parse -> chunk -> embed -> vector/search ready.
- Normal Chat RAG uses 30B `llm-chat` as highest-priority foreground path.
- Summary RAG has a clearly defined and stable execution path.
- Failures in summary/OCR/rerank/deferred models must not break hot Chat RAG or embedding.
- Long 30B generation must complete through sync/async/streaming contract without false timeout or global quarantine.

## 2026-07-24 Blocking Requirements For Scheduler 30B Chat

These requirements are release-blocking for 30B Chat RAG on 32GB K3-class edge nodes.

1. Resident chat admission must not fail solely because `CmaFree` is below a startup threshold.
   - Observed failure: `POST /kb/tasks/kb.query.ask` returned HTTP 503 with `model_unavailable` and detail `K3 CMA free memory is below the scheduler minimum`.
   - At the same time, `llm-chat` could be reported as `READY/resident` and worker `/health` could return 200.
   - Required behavior: distinguish cold-start CMA preflight from resident inference admission. If the model is resident and worker health is OK, route the foreground chat request or return a precise worker-state error, not a generic CMA startup failure.

2. `llm-chat` warmup must converge to one stable terminal readiness state.
   - Observed sequence on `0.8.14+cma4`: `WARMING -> READY/resident -> WARMING -> FAILED/cold -> WARMING`.
   - Required behavior: after cold start, either `READY_FAST/FREE` is reached and remains stable for idle foreground chat, or the model is marked terminal failed with a stable failure reason and no hidden requeue loop.
   - The scheduler must not report stale `worker_exception` after a later successful worker health check without exposing a current-state timestamp/revision.

3. Accepted realtime chat jobs must leave `scheduler_queue`.
   - Observed job: `job_5fbef23106f7d5a4779484604156964e` remained `status=queued`, `phase=scheduler_queue`, `reason=cold_start_avoided`, `eta_ms=71100`, while `llm-chat` returned to `WARMING`.
   - Required behavior: a `kb.query.ask` job accepted with HTTP 202 must transition through `queued -> running -> done/error/canceled/expired` within the advertised ETA plus bounded slack.
   - If the worker cannot become ready, the job must become terminal `error` with `error_reason` such as `worker_start_failed`, `cma_preflight_failed`, or `worker_health_timeout`; it must not remain queued until client timeout.

4. ETA must reflect actual cold-start and queue state.
   - Observed ETA stayed `71100 ms`, but Attune realtime polling still timed out without job progress.
   - Required behavior: `GET /jobs/{id}` must expose dynamic `eta_ms`, `queue_wait_ms`, `startup_wait_ms`, `worker_pid`, and current worker state. If startup is retried, `startup_attempt` and `last_worker_error_reason` must be visible.

5. 32GB K3 default model profile must be internally consistent.
   - Tested mitigation attempts: reduced `embedding-int8` replicas from 4 to 1, reduced `llm-chat` process context to `-c 2048`, reduced warmup reps to 1, and lowered test CMA threshold to 0.
   - Result: worker still oscillated and jobs remained queued.
   - Required behavior: provide a scheduler-owned K3 profile for 30B Chat RAG that is stable by default, including embedding replicas, chat context, max output, warmup policy, CMA policy, and resource lease interactions. This must be delivered by scheduler/config package, not by hard-coded Attune server workarounds.

6. Telemetry must match runtime process configuration.
   - Observed `models/capacity` still showed `context_window_tokens=4096` while the actual worker command line used `-c 2048`.
   - Required behavior: `/models` and `/capacity` must report effective worker launch parameters, not stale or configured maxima that disagree with the running process.

Acceptance criteria:

- On a clean scheduler restart, `GET /ready?hot=1` reaches HTTP 200 and stays stable for at least 10 minutes with `embedding-int8` and `llm-chat`.
- A hot `POST /kb/tasks/kb.query.ask` using `llm-chat` completes sync or accepted-async-to-done within 30 seconds p95 for the release smoke prompt.
- A cold `POST /kb/tasks/kb.query.ask` either completes within advertised `eta_ms + 30s` or returns terminal job error with a precise reason.
- While 30B chat is resident, `CmaFree` may be low, but foreground chat must not be rejected solely by the cold-start CMA threshold.
- Repeated upload/index/search/chat E2E must pass three consecutive runs without manual scheduler restart.

## 2026-07-24 Final 1.5.18 Evidence

Target:

- Host: `192.168.100.233`
- Installed packages:
  - `attune-server 1.5.18`
  - `k3-scheduler 0.8.14+ragobs4`
- Scheduler hot readiness:
  - `/ready?hot=1` HTTP 200
  - hot models include `embedding-int8` and `llm-chat`

Attune package evidence:

- Deb: `dist/release/riscv64-server-deb-non-ocr-rag-1.5.18/attune-server_1.5.18_riscv64.deb`
- SHA256: `55959388f45fb7b9ddb90cb1f1dc48e87feaad45ab880a6ecbf6a6585c26feaa`
- Build report: `reports/release/build-riscv64-server-deb-20260724_124744.md`
- Package boundary audit: pass; no scheduler-owned inference runtime or model-looking files in the Attune deb.

K3/NAS Web API evidence:

- Report: `reports/release/k3-nas-web-demo-20260724_125359.md`
- API contract JSON: `reports/release/k3-nas-web-api-contract-20260724_125359.json`
- Result: pass.
- Covered gates: health, UI shell, vault, scheduler settings, scheduler probe, core reads, upload, bind scan, rescan, vector drain, export, scheduler-backed chat, cleanup, background bind smoke.
- Chat gate: `/api/v1/chat -> /kb/tasks/kb.query.ask`, `scheduled_as=sync`, `job_id=null`, latency about `5300 ms`.

kb-web-demo browser evidence:

- Playwright report: `reports/release/kb-web-demo-frontend-233-1.5.18-scheduler-ragobs4-20260724_125808.json`
- Result: pass.
- Covered checks: upload, vector chunk render, Chat RAG, Summary RAG, citation render, full-flow time render.
- Frontend pass rates: flow `1.0`, citation `1.0`, time `1.0`, vector chunk `1.0`.

Single-industry RAG evidence:

- Networking: `reports/release/k3-rag-release-smoke-networking-233-1.5.18-scheduler-ragobs4-clean-20260724_125614.json`
  - pass, `cases=2`, `failures=0`, retrieval/citation/answer all `1.0`.
- Aviation: `reports/release/k3-rag-release-smoke-aviation-233-1.5.18-scheduler-ragobs4-clean-20260724_125634.json`
  - pass, `cases=1`, `failures=0`, retrieval/citation/answer all `1.0`.
- Mechanical: `reports/release/k3-rag-release-smoke-mechanical-233-1.5.18-scheduler-ragobs4-clean-20260724_125638.json`
  - pass, `cases=2`, `failures=0`, retrieval/citation/answer all `1.0`, multiturn source continuity `1.0`.

Scheduler/runtime side evidence:

- Runner-side RVV perf gate cannot use SSH local forwarding on this target because sshd reports `administratively prohibited`; direct remote access to protected legacy `/infer` is also not an application route.
- Scheduler loopback gate was executed on the K3 host with worker subgate skipped because the target Python environment lacks `httpx`.
- Report copied back: `reports/release/k3-rvv-runtime-gate-20260724_124241.json`
- Result: pass for contract, acceleration metadata, and live latency evidence:
  - `acceleration_metadata=true`
  - `embedding-int8 p50_latency_ms=28.93`
  - `llm-chat p50_latency_ms=6142.98`
- Direct operator probe for `llm-summary`:
  - `/admit/llm-summary` admitted sync with cold ETA.
  - Deprecated-marker loopback `/infer/llm-summary` succeeded after cold start and returned a valid Chinese summary.
  - Application RAG still uses `kb.query.ask`; the observed scheduler contract does not expose `kb.query.summary` as a stable business task.

## 2026-07-24 Final 1.5.20 Planner Evidence

Target:

- Host: `192.168.100.233`
- Installed packages:
  - `attune-server 1.5.20`
  - `k3-scheduler 0.8.14+ragobs4`
- Service status:
  - `attune-server`: active
  - `k3-scheduler`: active
  - `attune-server-headless --version`: `1.5.20`
- Scheduler strict contract: pass, `contract_version=edge-scheduler-v1`, `models=9`, `failures=[]`.

Attune package evidence:

- Deb: `dist/release/riscv64-server-deb-rag-planner-1.5.20/attune-server_1.5.20_riscv64.deb`
- SHA256: `bf4e36b7f7a9f62a8fa91353033dec582c1ad89a69ee69261adc76eb932efa4a`
- Build report: `reports/release/build-riscv64-server-deb-20260724_145028.md`
- Package boundary audit: pass; no scheduler-owned inference runtime or model-looking files in the Attune deb.
- Package metadata: `Version: 1.5.20`, `Architecture: riscv64`.

K3/NAS Web API evidence:

- Report: `reports/release/k3-nas-web-demo-20260724_145751.md`
- API contract JSON: `reports/release/k3-nas-web-api-contract-20260724_145751.json`
- Result: pass.
- Covered gates: health, UI shell, vault, scheduler settings, scheduler probe, core reads, upload, bind scan, rescan, vector drain, export, scheduler-backed chat, cleanup, background bind smoke.
- Chat gate: `/api/v1/chat -> /kb/tasks/kb.query.ask`, `scheduled_as=sync`, `job_id=null`, latency about `7688 ms`.
- Response keys include `rag_intent_plan`, confirming the planner metadata is present in the real API surface.

kb-web-demo browser evidence:

- Playwright report: `reports/release/kb-web-demo-frontend-233-1.5.20-rag-planner-20260724_1500.json`
- Result: pass.
- Covered checks: upload, vector chunk render, Chat RAG, Summary RAG, citation render, full-flow time render.
- Frontend pass rates: flow `1.0`, citation `1.0`, time `1.0`, vector chunk `1.0`.
- Elapsed: `29453 ms`.

Planner-specific API evidence:

- Report: `reports/release/k3-rag-planner-multitopic-api-233-1.5.20-20260724_1502.json`
- Result: pass.
- Test shape: upload four independent markdown topics, then ask a single multi-topic summary question.
- `/api/v1/chat` response:
  - `local_scheduler.task=kb.query.ask`
  - `scheduled_as=sync`
  - `startup_state=hot_resident`
  - `knowledge_count=6`
  - `citations_count=6`
  - `rag_intent_plan.answer_mode=summary`
  - `rag_intent_plan.coverage_policy=per_topic`
  - `rag_intent_plan.selection.missing_topics=[]`
  - `selected_topic_hits` contains all four required topics: alpha control, beta evidence, gamma response, delta review.
- User-visible answer included a `引用来源：...` line covering four planner-selected representative topics.

## Historical Failing Evidence

Observed during E2E:

- Complex PDF Chat RAG via Attune no-auth backend `:18906` returned HTTP 500 after about `62s`.
- Error body: `local scheduler infer HTTP 500 Internal Server Error: {"detail":"worker response timeout","error":"worker_error"}`.
- Scheduler config has `http.request_timeout_ms=60000`.
- Scheduler-native `POST /kb/tasks/kb.query.ask` scheduled `model=llm-summary`, then job ended:
  - `status=error`
  - `detail=accelerator_quarantined`
  - `error_reason=accelerator_quarantined`
- After `llm-summary` failed, `/ready?hot=1` reported hot models not ready:
  - `embedding-int8`: `reason=resource_domain_quarantined`
  - `llm-chat`: `reason=resource_domain_quarantined`
- Manual `POST /admin/unquarantine/accelerator` restored hot readiness.

## 2026-07-23 E2E Update: Attune 1.5.5 + Scheduler 0.8.6

Target:

- Host: `192.168.100.233`
- Installed packages:
  - `attune-server 1.5.5`
  - `k3-scheduler 0.8.6`
- Final hot readiness:
  - `/ready?hot=1` HTTP 200
  - hot models: `embedding-int8`, `llm-chat`

Attune-side fix shipped in `1.5.5`:

- Root cause of previous `/api/v1/chat` 503: Attune used a `2s` control-plane submit timeout for synchronous `kb.query.ask`, while scheduler `0.8.6` can legitimately spend `5-11s` generating a sync Chat RAG answer.
- Fix: Chat RAG now uses a dedicated `kb.query.ask` submit timeout:
  - default: `30s`
  - override: `ATTUNE_CHAT_SCHEDULER_SUBMIT_TIMEOUT_MS`
  - hard clamp: `2s..120s`
- Control-plane and job-poll request timeouts remain short; only answer-generation submit gets the larger budget.

Web-demo/API contract evidence:

- Report: `reports/release/k3-nas-web-api-contract-final-20260723_0148.json`
- Overall result: pass.
- Passed gates:
  - `health`
  - `ui_shell`
  - `vault`
  - `settings_scheduler`
  - `scheduler_probe`
  - `core_reads`
  - `upload`
  - `index_bind`
  - `index_rescan`
  - `vector_indexing`
  - `export`
  - `chat_scheduler`
  - `cleanup`
- `chat_scheduler` result:
  - task: `kb.query.ask`
  - job: `null`
  - scheduled path: sync
  - latency: about `5445 ms` in final run

Additional successful observations:

- Scheduler strict contract probe passed with `failures: []`.
- `/api/v1/ai-stack` reported scheduler ready and listed both `llm-chat` and `llm-summary`.
- Summary intent in `/api/v1/chat` can complete through Attune extractive summary when evidence is short/high-confidence:
  - `answer_mode=extractive-summary`
  - `intent=summary`
  - `rag_profile=default_kb_summary`
  - citations returned.

Remaining failing evidence on scheduler `0.8.6`:

- Direct scheduler `POST /kb/tasks/kb.document.summary` accepted the task and returned:
  - `model=llm-summary`
  - `scheduled_as=async`
  - `job_id=job_7bdfe90e7f749a80fe7aed7093876179`
- Polling `/jobs/{id}` ended with:
  - `status=error`
  - `error=model_unavailable`
  - `error_reason=accelerator_quarantined`
  - `recoverable=true`
- The failed `llm-summary` job changed `/ready?hot=1` to HTTP 503 and made unrelated hot models not ready:
  - `embedding-int8`: `reason=resource_class_quarantined`
  - `llm-chat`: `reason=resource_class_quarantined`
- Long-text PDF E2E with OCR also hit repeated scheduler 503:
  - OCR task `kb.document.ocr_recognize:async` reported `lifecycle=FAILED`.
  - background embedding requests were rejected with `reason=resource_class_quarantined`.
  - the test was intentionally interrupted after confirming the quarantine condition, then scheduler was restarted and hot readiness recovered.

Interpretation:

- Attune/web-demo Chat RAG is now usable and release-pass for the short NAS Web demo flow.
- Scheduler `0.8.6` is not yet release-pass for the full P0 matrix because `llm-summary` and OCR failures still have accelerator/resource-class blast radius.
- Product summary mode should remain explicit:
  - normal web-demo summary can use Attune extractive summary metadata;
  - scheduler-backed `llm-summary` must not be treated as stable until R2/R4/T5/T6 pass.

## 2026-07-23 Latest E2E Update: Attune 1.5.6 + Scheduler 0.8.9+feedback1

Target:

- Host: `192.168.100.233`
- Installed packages:
  - `attune-server 1.5.6`
  - `k3-scheduler 0.8.9+feedback1`
- Scheduler strict contract:
  - `contract_version=edge-scheduler-v1`
  - models: `9`
  - failures: `[]`
- Final hot readiness:
  - `/ready?hot=1` HTTP 200
  - hot models: `embedding-int8`, `llm-chat`
  - `hot_models_ready=true`

Attune-side changes validated with this scheduler:

- Chat RAG and Summary RAG use the scheduler business task contract rather than deprecated direct infer paths.
- `rerank.enabled=true` from Attune settings is honored by search/chat routes.
- If scheduler reranker returns no actionable scores, Attune keeps RRF order instead of replacing all scores with zeros.
- Summary RAG is no longer tied to unstable `llm-summary` for the demo path:
  - web-demo defaults Summary RAG to `llm-chat` deep summary;
  - server allows summary-class prompts configured as `llm-summary` to fall back to scheduler-native `kb.query.ask -> llm-chat` when the scheduler contract maps `kb.query.ask` to `llm-chat`;
  - high-confidence summaries can still complete through `local.extractive.summary` with explicit metadata.
- web-demo proxy no longer cuts long RAG responses after `30s`; response idle wait is configurable and set to `600s` in the K3 launcher.
- web-demo no longer blocks a ready uploaded document only because unrelated global `pending_embeddings` is nonzero.

Package/build evidence:

- Built package: `dist/release/riscv64-server-deb-scheduler089-summary-fallback-1.5.6/attune-server_1.5.6_riscv64.deb`
- SHA256: `909975682ccf073c8748af17ceb5910949c796285e4758826afd6e5c3d0debcd`
- Build report: `reports/release/build-riscv64-server-deb-20260723_095315.md`

Hot-state API/Web E2E evidence:

- NAS Web API contract: `reports/release/k3-nas-web-api-contract-20260723_100355.json`
- NAS Web demo report: `reports/release/k3-nas-web-demo-20260723_100355.md`
- Result: pass.
- Passed gates:
  - `health`
  - `ui_shell`
  - `vault`
  - `settings_scheduler`
  - `scheduler_probe`
  - `core_reads`
  - `upload`
  - `index_bind`
  - `index_rescan`
  - `vector_indexing`
  - `export`
  - `chat_scheduler`
  - `cleanup`
- `chat_scheduler` hot-state result:
  - task: `kb.query.ask`
  - job: `null`
  - scheduled path: sync
  - latency: about `5232 ms`

Browser E2E evidence:

- Playwright report: `reports/release/playwright-kb-web-demo-scheduler089-20260723_100614.json`
- Screenshots: `reports/release/playwright-kb-web-demo-scheduler089-20260723_100614/`
- Result: pass.
- Covered:
  - upload
  - vector chunk display
  - Chat RAG
  - Summary RAG
- Chat RAG result:
  - `/api/v1/chat` HTTP 200
  - `answer_mode=llm-chat`
  - `local_scheduler.task=kb.query.ask`
  - `scheduled_as=sync`
  - `model=llm-chat`
  - `knowledge_count=8`
  - latency about `18.1s`
- Summary RAG result:
  - `/api/v1/chat` HTTP 200
  - `answer_mode=extractive-summary`
  - `local_scheduler.task=local.extractive.summary`
  - `scheduled_as=sync`
  - `model=local-extractive-source-answer`
  - `knowledge_count=12`
  - latency about `176ms`

Scheduler latest interface observed from `0.8.9+feedback1`:

- Health/readiness/capacity:
  - `GET /health`
  - `GET /ready?hot=1`
  - `GET /models`
  - `GET /capacity`
  - `GET /metrics`
- Contract:
  - `GET /benchmark/contract`
- Application KB tasks:
  - `POST /kb/tasks/kb.query.embed`
  - `POST /kb/tasks/kb.query.rerank`
  - `POST /kb/tasks/kb.query.ask`
  - `POST /kb/tasks/kb.query.ask:async`
  - `POST /kb/tasks/kb.query.ask_hq`
  - document async tasks such as `kb.document.summary`, `kb.document.long_summary`, OCR/VLM tasks
- Job lifecycle:
  - `GET /jobs/{job_id}`
  - cancellation route remains part of the Attune expected contract: `POST /jobs/{job_id}:cancel`

Important task contract values from `/benchmark/contract`:

- `kb.query.ask`
  - model: `llm-chat`
  - service_class: `realtime_answer`
  - `async_only=false`
  - `avoid_cold_start=true`
  - `timeout_ms=180000`
  - `deadline_ms=30000`
  - `context_tokens=4096`
  - `max_output_tokens=192`
  - `ttl_ms=900000`
- `kb.query.ask_hq`
  - model: `llm-chat`
  - service_class: `long_context`
  - `async_only=true`
  - `context_tokens=8192`
  - `max_output_tokens=512`
  - `timeout_ms=300000`
- `kb.document.long_summary`
  - model: `llm-chat`
  - service_class: `long_context`
  - `async_only=true`
  - `context_tokens=8192`
  - `max_output_tokens=1024`
  - `timeout_ms=600000`
- `kb.document.summary` / `doc.summarize`
  - model: `llm-summary`
  - `async_only=true`
  - still not considered stable for product demo because `llm-summary` has observed `worker_exception`.

Race validation result:

- The previously suspected scheduler race was not reproduced on `0.8.9+feedback1`.
- Sync `kb.query.ask` probe:
  - three consecutive hot-state requests returned HTTP 200.
  - all were `scheduled_as=sync`.
  - `startup_state=hot_resident`.
  - wall time about `3.5s..6.3s`.
- Explicit async `kb.query.ask:async` probe:
  - submit returned HTTP 202 in about `1.5ms`.
  - response included `scheduled_as=async`, `status=queued`, and a valid `job_id`.
  - observed job state sequence:
    - `queued / scheduler_queue`
    - `running / worker_infer`
    - `done / done`
  - final job:
    - `status=done`
    - `scheduled_as=async`
    - `latency_ms` about `3252ms`
    - `cold_start_wait_ms=0`
    - `model=llm-chat`
- Concurrent `/ready?hot=1` polling during async job:
  - 20 samples.
  - no errors.
  - all samples returned `status=ok` and `hot_models_ready=true`.
- Final async job pool:
  - `active=0`
  - `done_retained=1`

Cold-start boundary evidence:

- One cold-state API contract run failed only at `chat_scheduler` because the test used `60s` job timeout.
- Scheduler submitted `kb.query.ask` as async:
  - `scheduled_as=async`
  - `reason=cold_start_avoided`
  - `phase=worker_infer` when the test timed out
  - job later reached `status=done`
- Final completed job telemetry:
  - `cold_start_wait_ms` about `60905ms`
  - `startup_state=cold_start`
  - generation `latency_ms` about `9775ms`
  - model: `llm-chat`
- Interpretation:
  - This is no longer the earlier job/readiness race.
  - The remaining edge is scheduler cold-start contract semantics and upper-layer timeout policy.
- Fixed `60s` upper-layer job timeout is too close to a valid cold start plus inference.

## 2026-07-23 Latest Scheduler Update: Attune 1.5.6 + Scheduler 0.8.10+coldstart2

Target:

- Host: `192.168.100.233`
- Installed packages:
  - `attune-server 1.5.6`
  - `k3-scheduler 0.8.10+coldstart2`
- Scheduler strict contract:
  - command: `python3 scripts/probe-edge-scheduler-contract.py --base-url http://192.168.100.233:8090 --strict`
  - `contract_version=edge-scheduler-v1`
  - models: `9`
  - failures: `[]`
- Hot readiness:
  - `/ready?hot=1` HTTP 200
  - hot models: `embedding-int8`, `llm-chat`
  - `hot_models_ready=true`

Scheduler task contract observed from `0.8.10+coldstart2`:

- `kb.query.rerank`
  - model: `reranker-int8`
  - service_class: `realtime_retrieval`
  - `async_only=true`
  - `avoid_cold_start=true`
  - `deadline_ms=2000`
  - `timeout_ms=60000`
- `kb.query.ask`
  - model: `llm-chat`
  - service_class: `realtime_answer`
  - `async_only=false`
  - `avoid_cold_start=true`
  - `deadline_ms=30000`
  - `timeout_ms=180000`
  - `context_tokens=4096`
  - `max_output_tokens=192`
- `kb.query.ask_hq`
  - model: `llm-chat`
  - service_class: `long_context`
  - `async_only=true`
  - `timeout_ms=300000`
  - `context_tokens=8192`
  - `max_output_tokens=512`
- `kb.document.summary`
  - model: `llm-summary`
  - service_class: `user_async`
  - `async_only=true`
  - `timeout_ms=300000`
- `kb.document.long_summary`
  - model: `llm-chat`
  - service_class: `long_context`
  - `async_only=true`
  - `timeout_ms=600000`
  - `context_tokens=8192`
  - `max_output_tokens=1024`

Model state observed before E2E:

- `llm-chat`: `READY_FAST`, resident, not quarantined.
- `embedding-int8`: `READY_FAST`, resident, not quarantined.
- `llm-summary`: cold/displaced, not quarantined at observation time.
- `reranker-int8`: cold/displaced, not quarantined at observation time.
- `ocr-rec`: lifecycle `FAILED`, `quarantine_scope=resource_class`.

Race validation on `0.8.10+coldstart2`:

- Three consecutive hot sync `POST /kb/tasks/kb.query.ask` calls returned HTTP 200.
  - all `scheduled_as=sync`
  - all `startup_state=hot_resident`
  - observed wall time about `2.6s..6.9s`
- Explicit async `POST /kb/tasks/kb.query.ask:async` returned HTTP 202 in about `1.5ms`.
  - final status: `done`
  - `scheduled_as=async`
  - `latency_ms` about `2250ms`
  - `startup_wait_ms` about `0.47ms`
  - `cold_start_wait_ms=0`
  - model: `llm-chat`
- Concurrent `/ready?hot=1` polling during the async job had no errors.

NAS Web API contract evidence:

- First attempt: `reports/release/k3-nas-web-demo-20260723_120803.md`
  - Incomplete because the main Attune service was still processing old airplane/OCR background work.
  - Scheduler remained healthy; this was not a scheduler contract failure.
- Passing attempt after restarting only the Attune service:
  - NAS Web demo report: `reports/release/k3-nas-web-demo-20260723_121217.md`
  - API contract: `reports/release/k3-nas-web-api-contract-20260723_121217.json`
  - Result: pass.
  - Passed gates: `health`, `ui_shell`, `vault`, `settings_scheduler`, `scheduler_probe`, `core_reads`, `upload`, `index_bind`, `index_rescan`, `vector_indexing`, `export`, `chat_scheduler`, `cleanup`.
  - `chat_scheduler`: task `kb.query.ask`, sync path, latency about `7943ms`.

Browser E2E evidence:

- web-demo source was synced to `root@192.168.100.233:/tmp/kb-web-demo/`.
- web-demo was restarted on:
  - UI: `http://192.168.100.233:8888/`
  - proxy: `http://192.168.100.233:8889/`
  - Attune no-auth backend: `http://127.0.0.1:18906`
- Passing Playwright report: `reports/release/playwright-kb-web-demo-scheduler0810-20260723_122220.json`
- Screenshots: `reports/release/playwright-kb-web-demo-scheduler0810-20260723_122220/`
- Result: pass.
- Covered:
  - upload through `/api/v1/upload`
  - vector chunk display through `/api/v1/search`
  - Chat RAG through `/api/v1/chat`
  - Summary RAG through `/api/v1/chat`
- Browser test marker: `PX-0810-20260723_122220`
- Vector result:
  - `1 chunks`
  - query latency displayed as `1ms`
- Chat RAG result:
  - `/api/v1/chat` HTTP 200
  - `provider=local_scheduler`
  - `model=llm-chat`
  - `answer_mode=llm-chat`
  - `knowledge_count=8`
  - latency about `16781ms`
- Summary RAG result:
  - `/api/v1/chat` HTTP 200
  - `provider=local_scheduler`
  - `model=local-extractive-source-answer`
  - `answer_mode=extractive-summary`
  - `knowledge_count=12`
  - latency about `176ms`
- Final web-demo status:
  - `pending_embeddings=0`
  - `vector_index=true`
  - `embedding_available=true`

Interpretation for `0.8.10+coldstart2`:

- The latest scheduler is compatible with Attune `1.5.6` for the hot web-demo capability showcase.
- Normal Chat RAG remains the validated 30B foreground path through `kb.query.ask -> llm-chat`.
- Summary RAG remains product-stable through the explicit Attune extractive/source-grounded summary path; it must not depend on cold `llm-summary` until T5/T6 prove no blast radius.
- OCR remains the main unresolved deferred-model risk. Old airplane/OCR background work can still load the Attune process and produce noisy scheduler 503s for OCR tasks even while hot chat/embedding readiness stays healthy.

Layered resolution decision:

- Scheduler is the root-fix layer for cold-start semantics:
  - return accurate `eta_ms` for cold-start async jobs;
  - expose clear job phases such as `warming_model`, `worker_starting`, `worker_infer`, `done`;
  - ensure `/ready?hot=1` means the configured hot set is truly interaction-ready;
  - support cancellation or worker release when clients disconnect from long sync/async inference.
- Attune server is the compatibility layer:
  - use scheduler `eta_ms` plus cushion for job polling;
  - never treat a still-running async job as RAG evidence failure;
  - return job handle to UI when realtime polling budget expires;
  - keep summary/rerank fallbacks explicit in metadata.
- web-demo/test layer is not the root fix:
  - proxy must not disconnect long valid RAG responses early;
  - E2E must distinguish cold-state async completion from hot-state sync performance;
  - cold-state tests should either poll until scheduler ETA plus cushion, or assert job completion within the scheduler task `timeout_ms`, not a fixed `60s`.

## 2026-07-23 Scheduler Follow-up: Scheduler 0.8.11+eta1

Target:

- Host: `192.168.100.233`
- Installed scheduler package:
  - `k3-scheduler 0.8.11+eta1`
- Package artifact:
  - `dist/k3-scheduler_0.8.11+eta1_riscv64.deb`
  - SHA256: `9adb53d6d75d829c9093bf1cb4cbf2e40f94ed89bc62314d6ba843d2952907c5`

Scheduler-side fix:

- Async submit metadata now persists accepted `eta_ms` into the job tracker.
- While a job is non-terminal, both `GET /jobs/{job_id}` and `GET /jobs` return:
  - top-level `eta_ms`
  - `telemetry.eta_ms`
- This allows Attune UI/API polling to survive refreshes, worker handoff, or client process changes without relying on an in-memory copy of the original submit response.

K3 validation evidence:

- Install/daemon:
  - package upgraded from `0.8.10+coldstart2` to `0.8.11+eta1`;
  - `systemctl is-active k3-scheduler`: `active`;
  - `ActiveState=active`, `SubState=running`, `NRestarts=0`.
- Cold async ETA probe:
  - request: `POST /kb/tasks/kb.query.ask:async`
  - accepted response: `status=queued`, `model=llm-chat`, `eta_ms=71100`
  - immediate `GET /jobs/{job_id}`: `status=queued`, top-level `eta_ms=71100`, `telemetry.eta_ms=71100`
  - immediate `GET /jobs`: same job row retained `eta_ms=71100`
  - terminal status: `done`, `error=null`
- Final hot readiness after sample runs:
  - `/ready?hot=1` HTTP 200
  - hot models: `embedding-int8`, `llm-chat`
  - `failed=[]`, `quarantined=[]`
- Developer sample runner:
  - default safe mode: `passed=4`, `blocked=10`, `failed=0`
  - explicit ACCEL/risky mode: `passed=9`, `blocked=5`, `failed=0`
  - blocked samples are explicit model-policy blocks for currently `COLD/DISPLACED` VLM/OCR/rerank/TTS paths, not failures or quarantine.

Interpretation for `0.8.11+eta1`:

- The accepted ETA contract is now stable enough for Attune to use `eta_ms + cushion` from either the submit response or later job-status responses.
- Non-terminal job status now has enough metadata for UI feedback after cold async acceptance, including the case where the original submit response is no longer available to the client.
- This does not fully solve dynamic scheduling observability:
  - `eta_ms` is still the accepted estimate, not a continuously decreasing remaining-time counter;
  - long-running worker cancellation remains cooperative/deferred once `phase=worker_infer`;
  - deferred `llm-summary`, OCR, VLM, rerank, and TTS remain intentionally cold/displaced on the validated 32 GB K3 profile and still require deeper blast-radius testing before being treated as product-stable.

## P0 Requirements

### R1. Foreground Chat RAG Must Be Highest Priority

`llm-chat` is the primary product path for normal KB questions.

Required behavior:

- Hot-set must include `embedding-int8` and `llm-chat`.
- `embedding-int8` and `llm-chat` must be admitted before deferred tasks: `llm-summary`, `ocr-det`, `ocr-rec`, `reranker-int8`, VLM, ASR, TTS.
- Foreground Chat RAG requests must not wait behind background ingest, OCR, summary, rerank, or cold deferred warmup.
- If `llm-chat` is hot and idle, a foreground Chat RAG request must start generation immediately.
- If generation is expected to exceed sync timeout, scheduler must return a job id quickly and continue async, instead of blocking until HTTP 500.

Acceptance criteria:

- With `llm-chat` hot, short Chat RAG returns HTTP 200 within `30s`.
- With a complex PDF question, either:
  - sync returns HTTP 200 within configured long foreground deadline, or
  - async returns `job_id` within `2s`, and `/jobs/{id}` reaches `success` within `180s`.
- No foreground Chat RAG may end as `worker response timeout` solely because it exceeded `60000ms`.

### R2. Quarantine Must Be Scoped, Not Global

Current issue: a deferred `llm-summary` failure quarantines shared `ACCEL-SVC` and marks hot `embedding-int8` and `llm-chat` not ready.

Required behavior:

- Quarantine must be scoped to the failing model, worker, or resource class.
- A failed `llm-summary` warmup/job must not make `embedding-int8` or `llm-chat` not-ready.
- A failed OCR/rerank/VLM/deferred model must not make hot Chat RAG unavailable.
- `/ready?hot=1` must only fail when a required hot model is actually unavailable.
- Quarantine reason must identify exact scope:
  - `model_quarantined`
  - `worker_quarantined`
  - `resource_class_quarantined`
  - avoid generic `resource_domain_quarantined` for unrelated hot models.

Acceptance criteria:

- Force or simulate `llm-summary` failure.
- `/ready?hot=1` remains HTTP 200 if `embedding-int8` and `llm-chat` are healthy.
- `POST /kb/tasks/kb.query.embed` continues to work.
- Attune `/api/v1/chat` with `llm-chat` continues to work.
- No manual `/admin/unquarantine/accelerator` is required for unrelated hot models.

### R3. Worker Timeout Must Not Kill Product Requests Incorrectly

Current issue: 30B Chat RAG can need more than `60s`; scheduler returns worker timeout and can destabilize readiness.

Required behavior:

- Distinguish three timeouts:
  - HTTP read/request timeout.
  - worker health timeout.
  - model generation deadline.
- A long generation with a healthy worker must not be treated as worker death.
- Worker must emit heartbeat/progress while generating.
- Scheduler must expose progress via `/jobs/{id}` or streaming.
- Timeout must be configurable per service class:
  - `realtime_retrieval`: short, around `2s`.
  - `realtime_answer` short sync: around `30s`.
  - `realtime_answer` long/async 30B: at least `180s`.
  - background document tasks: task-specific, not blocking foreground.

Acceptance criteria:

- Complex 30B PDF RAG no longer returns `worker response timeout` at `60s`.
- If request exceeds sync budget, scheduler returns async job id before the sync budget expires.
- Worker remains healthy after timeout/async transition.
- Next short Chat RAG succeeds without manual recovery.

### R4. `llm-summary` Must Be Product-Defined

Current issue: Attune Summary RAG user flow passes through local extractive fallback, while scheduler `llm-summary` task fails.

The product must choose one of these two options:

Option A: Scheduler-backed summary

- `kb.query.ask` mapped to `llm-summary` must be stable.
- `llm-summary` may be cold/deferred, but cold start must not quarantine hot chat/embedding.
- If `llm-summary` cannot run due resource admission, scheduler must return async job id or explicit unavailable error without side effects.

Option B: Attune extractive summary

- Scheduler must not advertise `llm-summary` as required for Summary RAG.
- `kb.query.ask` should not default to an unstable `llm-summary`.
- Metadata must clearly say `model=local-extractive-source-answer` or equivalent.

Acceptance criteria:

- Summary RAG path must be explicit in API metadata.
- If configured as scheduler-backed, `POST /kb/tasks/kb.query.ask` reaches terminal success.
- If configured as extractive fallback, no `llm-summary` job is submitted for normal summary.

### R5. Embedding Path Must Survive Chat/Summary Failures

Embedding is required for upload/vector workflow.

Required behavior:

- `embedding-int8` must stay ready during `llm-chat` generation and deferred model failures.
- Background embedding batch failures must retry with bounded backoff.
- `pending_embeddings` must not remain stuck because of unrelated accelerator quarantine.
- Scheduler must provide an explicit `model_unavailable` reason only when `embedding-int8` itself cannot serve.

Acceptance criteria:

- Upload `rag-deck.pdf`.
- `pending_embeddings` reaches `0` within `120s` on healthy system.
- Then trigger a failed `llm-summary` task.
- Upload another small document.
- Embedding still completes without manual unquarantine.

### R6. Scheduler Business API Must Be the Only Application Contract

Attune and web-demo should not depend on deprecated `/infer/{model}`.

Required behavior:

- Application-facing APIs:
  - `POST /kb/tasks/kb.query.embed`
  - `POST /kb/tasks/kb.query.rerank`
  - `POST /kb/tasks/kb.query.ask`
  - `POST /kb/tasks/{task}:async`
  - `GET /jobs/{id}`
  - `POST /jobs/{id}:cancel`
  - `GET /ready?hot=1`
  - `GET /models`
- Deprecated infer guard can remain, but all required functionality must be available through task APIs.
- Task responses must contain enough metadata for Attune UI:
  - `job_id`
  - `model`
  - `service_class`
  - `scheduled_as`
  - `eta_ms`
  - `queue_wait_ms`
  - `latency_ms`
  - `error_code`
  - `error_reason`
  - `recoverable`

Acceptance criteria:

- No Attune product path requires `/infer/{model}`.
- `kb.query.ask` supports both short sync and long async foreground answer flows.

## P1 Requirements

### R7. Startup Must Be Deterministic

Required behavior:

- systemd service must enter `active (running)` without long `activating` state.
- Hot-set preload must be bounded and observable.
- Deferred model warmup must not block service readiness.

Acceptance criteria:

- After `systemctl restart k3-scheduler`, service becomes active within `30s`.
- `/ready?hot=1` becomes HTTP 200 within `180s`.
- If `llm-chat` warmup fails, error is scoped to `llm-chat`; embedding failure is reported independently.

### R8. Observability Must Explain Admission and Failure

Required behavior:

- `/ready?hot=1` must show hot-only readiness and exact not-ready reason.
- `/models` must show:
  - lifecycle
  - residency
  - queue depth
  - in-flight count
  - quarantine scope
  - last worker error
  - last successful inference timestamp
- `/jobs/{id}` must show:
  - terminal status
  - error code
  - error reason
  - retryable/recoverable flag
  - worker timeout vs generation deadline vs admission rejection.

Acceptance criteria:

- When a summary worker fails, operator can identify that only summary failed.
- When chat generation exceeds sync deadline, operator can see whether async continuation is running.

### R9. Recovery Must Be Automatic for Safe Cases

Required behavior:

- Safe transient worker failure should trigger model-scoped restart, not accelerator-wide quarantine.
- Automatic unquarantine can happen only after:
  - failed worker process exited,
  - memory governor reports ok,
  - hot model health check passes.
- Manual unquarantine endpoint remains for operator intervention.

Acceptance criteria:

- After one `llm-summary` failure, hot chat remains usable.
- After one 30B timeout, scheduler recovers without manual unquarantine and next short chat succeeds.

## Required E2E Test Matrix

Run these on K3 target after scheduler changes.

### T1. Fresh Boot Gate

Commands:

```bash
systemctl restart k3-scheduler
curl -sS http://127.0.0.1:8090/ready?hot=1
curl -sS http://127.0.0.1:8090/models
```

Expected:

- systemd active, not stuck activating.
- `/ready?hot=1` HTTP 200 within `180s`.
- hot models ready: `embedding-int8`, `llm-chat`.

### T2. PDF Upload and Vector Drain

Use Attune no-auth backend or web-demo proxy:

```bash
curl -sS -X POST http://192.168.100.233:8889/api/v1/upload \
  -F file=@rust/tests/corpora/openai-cookbook/examples/data/example_pdfs/rag-deck.pdf

curl -sS http://192.168.100.233:8889/api/v1/status
curl -sS 'http://192.168.100.233:8889/api/v1/search?q=Retrieval-Augmented%20Generation&top_k=5'
```

Expected:

- upload HTTP 200.
- `pending_embeddings=0` within `120s`.
- search returns `rag-deck - RAG` citation/content.

### T3. Short Chat RAG

```bash
curl -sS -X PATCH http://192.168.100.233:8889/api/v1/settings \
  -H 'content-type: application/json' \
  --data '{"llm":{"provider":"local_scheduler","endpoint":"http://127.0.0.1:8090","model":"llm-chat","api_key":"local-scheduler"}}'

curl -sS -w '\nHTTP %{http_code} time_total %{time_total}\n' \
  -X POST http://192.168.100.233:8889/api/v1/chat \
  -H 'content-type: application/json' \
  --data '{"message":"只基于知识库回答：RAG 是什么？用一句话回答。"}'
```

Expected:

- HTTP 200.
- `cost.provider=local_scheduler`.
- `cost.model=llm-chat`.
- `knowledge_count > 0`.
- citations include uploaded PDF.

### T4. High-Complexity PDF Chat RAG

Prompt:

```text
只基于已上传知识库中的 rag-deck.pdf 回答：RAG 的 data preparation、input processing、retrieval、answer generation 四个阶段分别有哪些关键做法和常见风险？请给出结构化要点，并引用文档证据。
```

Expected:

- No HTTP 500.
- No `worker response timeout`.
- Sync HTTP 200 or async `job_id` returned within `2s`.
- Final answer success within `180s`.
- Citations include `rag-deck - RAG`.
- After completion, `/ready?hot=1` remains HTTP 200.

### T5. Summary Path

If scheduler-backed:

```bash
curl -sS -X POST http://127.0.0.1:8090/kb/tasks/kb.query.ask \
  -H 'content-type: application/json' \
  --data '{"query":"总结 RAG 检索和安全检查建议","context":[{"title":"rag-deck","text":"Retrieval, re-ranking, context window and safety checks are discussed in the RAG deck."}],"max_output_tokens":160}'
```

Expected:

- sync success or async job success.
- No `accelerator_quarantined`.
- No impact to `embedding-int8` or `llm-chat`.

If Attune extractive:

- Attune summary API returns HTTP 200.
- Metadata clearly identifies extractive summary.
- No scheduler `llm-summary` job is submitted.

### T6. Quarantine Blast-Radius Test

Procedure:

1. Trigger or simulate `llm-summary` failure.
2. Query `/jobs/{id}` until terminal error.
3. Query `/ready?hot=1`.
4. Run short Chat RAG.
5. Upload a small document and wait for embedding drain.

Expected:

- Failure is scoped to `llm-summary`.
- `/ready?hot=1` remains HTTP 200.
- `llm-chat` still answers.
- `embedding-int8` still embeds.
- No manual unquarantine needed.

## Attune RAG Planner Requirements (1.5.20)

Scheduler remains the execution and model scheduling layer. Attune owns RAG planning
for knowledge-base chat and summary: intent classification, query decomposition,
evidence coverage, citation coverage, answer obligations, and response metadata.
These requirements are generic and must not encode industry facts such as TCP/IP,
aviation, mechanical design, security, legal, or medical rules in server code.

### P1. Generic Intent Plan

Attune `/api/v1/chat` must build a deterministic `RagIntentPlan` from the current
message and recent history before retrieval.

Required fields:

- `answer_mode`: `fact`, `how_to`, `troubleshooting`, `decision`, `summary`,
  `comparison`, `negative_evidence`, or `followup`.
- `coverage_policy`: `single_best`, `source_diverse`, `per_topic`, or
  `prior_sources`.
- `topics`: normalized topic phrases extracted from generic connectors and list
  delimiters.
- `sub_queries`: original query plus capped topic-focused retrieval queries.
- `obligations`: mode-specific answer requirements, for example evidence-first
  comparison, ordered troubleshooting steps, missing-evidence disclosure, and
  citation attachment.

The first implementation may use deterministic phrase extraction only. It must
not add a default LLM rewrite call before retrieval.

### P2. Coverage-Preserving Retrieval Fusion

For `per_topic` plans, Attune must retain one representative evidence chunk per
available topic before score-only truncation. It may then fill remaining slots by
score after deduplication. This protects multi-topic summary, decision, and
comparison answers from losing lower-scored but necessary evidence.

For `source_diverse` plans, Attune should preserve distinct source keys before
falling back to score order.

Fusion must run after initial retrieval, after recovery/topic sub-query retrieval,
and again after annotation/privacy filtering so final knowledge, citations, and
scheduler contexts reflect the same selected evidence.

### P3. Scheduler Admission Contract

When `kb.query.ask` is used, Attune must pass planner output to scheduler without
requiring scheduler to implement RAG-specific intent logic.

Required Attune behavior:

- Add planner obligations and required topics to the scheduler admission messages.
- Include `rag_intent_plan` in the scheduler task body.
- Keep `contexts` selected from planner-fused evidence.
- Keep `answer_budget` compatible with high-complexity, troubleshooting, decision,
  and summary prompts.

Scheduler only needs to accept the supplied task body/messages and execute the
selected model according to service class, priority, capacity, and job lifecycle.

### P4. Citation and Metadata Contract

Attune `/api/v1/chat` response must expose planner metadata for E2E diagnosis:

- `rag_intent_plan.answer_mode`
- `rag_intent_plan.coverage_policy`
- `rag_intent_plan.topics`
- `rag_intent_plan.sub_queries`
- `rag_intent_plan.obligations`
- `rag_intent_plan.selection.selected_topic_hits`
- `rag_intent_plan.selection.missing_topics`

For `summary`, `decision`, and `comparison`, the fallback `引用来源：...` line
should cover planner-selected representative sources, with a larger cap than the
simple three-source default when multiple topics are present.

### P5. Regression Tests

Attune server unit tests must cover:

- Decision topic detection without industry hardcoding.
- Summary topic-list detection without industry hardcoding.
- Per-topic fusion preserving representatives when high scores are concentrated
  in one topic.
- Scheduler admission prompt carrying answer obligations and required topics.
- Existing recent-history admission and source-summary behavior.

### Current Attune Planner Status

- Implemented in `attune-server` `1.5.20`.
- Local `cargo check -p attune-server`: pass.
- Targeted unit tests:
  - `rag_intent`: pass, 3 tests.
  - `rag_answer_obligations_are_added_to_admission_prompt`: pass.
  - `local_scheduler_admission_messages_include_recent_history`: pass.
  - `local_scheduler_source_summary_line_includes_readable_source_evidence`: pass.
- Remote 233 deb/web-demo E2E must be rerun after packaging `attune-server`
  `1.5.20` with the current scheduler build.

## Release Gate

Scheduler package is release-pass for Attune KB demo only when all P0 requirements and tests T1-T6 pass on a clean K3 target.

Current status:

- Scheduler `0.8.13+restore1` K3 deb E2E on `192.168.100.233`: pass.
  - deb: `dist/k3-scheduler_0.8.13+restore1_riscv64.deb`
  - sha256: `9cb98b3d8c57bf470a508240f57ca26ec3bb0daee30cd67f1b1d27952699d24a`
  - systemd: `Type=simple`, `ActiveState=active`, `SubState=running`, `NRestarts=0`.
  - `/benchmark/contract` exposes 19 runtime tasks; required KB tasks are present.
  - Resource domains: `embedding-int8=ACCEL.retrieval`, `llm-chat=ACCEL.chat`,
    `reranker-int8=ACCEL.aux`, `ocr-rec=ACCEL.aux`,
    `llm-summary/vlm=ACCEL.aux.session`.
  - Direct cold-start E2E: async ETA terminal pass, rerank scoped recovery pass,
    OCR-after-rerank no stale TCM suppression pass, VLM handoff hot-restore pass,
    final `/ready?hot=1` pass.
  - Developer sample runner with ACCEL/risky flags: 9 passed, 5 blocked by
    conservative cold/displaced availability gate, 0 failed.
  - 30s control-plane probe: 93 requests, 0 errors; `/health`, `/capacity`,
    `/metrics` p99 < 101ms.
- Scheduler `0.8.13+restore1` closes the prior K3 device gaps:
  - scoped TCM failure no longer upgrades explicit K3 domains into root ACCEL quarantine;
  - `reranker-int8` moved out of `ACCEL.retrieval` into `ACCEL.aux`;
  - scoped cooldown recovery clears same-domain TCM restart suppressions so OCR/rerank
    peers do not remain `model_restart_suppressed`;
  - terminal async errors keep `phase=done`, so polling clients do not wait until timeout;
  - auxiliary VLM/summary handoff can evict `llm-chat`, but hot-set auto-restart restores it.
- Scheduler `0.8.11+eta1` accepted-ETA persistence on non-terminal job status: pass.
- Attune `1.5.6` + scheduler `0.8.10+coldstart2` strict scheduler contract: pass.
- Attune `1.5.6` + scheduler `0.8.10+coldstart2` hot-state NAS Web API contract: pass.
- Attune `1.5.6` + scheduler `0.8.10+coldstart2` kb-web-demo Playwright E2E: pass.
- Attune `1.5.6` + scheduler `0.8.9+feedback1` hot-state NAS Web API contract: pass.
- Attune `1.5.6` + scheduler `0.8.9+feedback1` kb-web-demo Playwright E2E: pass.
- Scheduler job/readiness race validation: pass; the prior suspected race was not reproduced.
- Full scheduler RAG stability matrix: partially pass.
  - Hot-state Chat RAG path is release-usable.
  - Latest `0.8.11+eta1` preserves accepted ETA across submit and non-terminal job-status reads, but cold/deferred long-running jobs still need contract-level tightening around dynamic remaining ETA, cancellation, and upper-layer timeout expectations.
  - Scheduler-backed `llm-summary` remains not product-stable; Attune/web-demo currently use explicit fallback paths.

Known blockers:

- Cold/deferred long-running jobs still need a product decision for dynamic remaining ETA
  versus accepted cold-start ETA. `0.8.13+restore1` preserves accepted ETA and terminal
  phases, but does not yet expose a continuously recomputed remaining ETA.
- Client disconnect/cancel semantics for long 30B generation remain deferred: scheduler
  reports `cancel_requested` and uses `defer_until_worker_returns`; it does not preempt
  an active llama worker.
- `llm-summary` remains a deferred/cold auxiliary model. It is now blast-radius scoped,
  but product summary should continue using Attune/extractive fallback unless T5 quality
  and latency pass on a clean target.
- OCR/rerank/VLM scheduler reliability is blast-radius pass on `0.8.13+restore1`, but
  OCR/rerank model quality/functionality can still terminal-error on current K3 assets.
  The acceptance criterion for Attune should treat this as degraded auxiliary capability,
  not as hot RAG unavailability.
- RVV `/infer` performance gate is no longer an Attune product dependency, but any scheduler-owned performance gate must run through loopback/allowed scheduler paths or the business task API.
