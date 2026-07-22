# Edge Scheduler Delivery Plan

> Updated: 2026-07-22

This document defines the delivery boundary for stronger edge deployment:
Attune remains the product, retrieval, privacy, and citation plane; the edge
scheduler owns model runtime packaging, hardware acceleration, queueing, and
platform-specific service management.

## Package Split

| Package | Formats | Contains | Does not contain |
|---------|---------|----------|------------------|
| `attune-server` / Attune Desktop | `.deb`, `.rpm`, AppImage, NSIS `.exe`, MSI | UI, REST API, vault, ingest, parser, vector index, retrieval policy, citation policy, plugin registry, scheduler client | model weights, ONNX runtime, OCR/ASR/LLM workers, accelerator probes |
| `attune-edge-scheduler` | `.deb`, `.rpm`, container image, Windows `.exe`/MSI | scheduler API, worker manager, model registry, hardware backend selection, runtime benchmarks, job queue | Attune vault data, product policy, plugin business logic |
| Model packs | tar/zip or managed cache | optional 7B/14B/30B/70B weights and tokenizer/runtime metadata | Attune server binaries |

The release gate `probe-attune-package-boundary.sh` keeps the Attune package
free of inference runtime/model-looking artifacts. Scheduler packages should
have their own SBOM, hardware matrix, and runtime benchmark gate.

## Deployment Modes

| Mode | Use case | Attune behavior |
|------|----------|-----------------|
| `cloud-only` | Lowest operational burden, SaaS or BYOK cloud model | Attune uses cloud LLM settings; scheduler is not required. |
| `server-only` | Knowledge indexing without local generation | Attune serves upload/search/vector/citation flows; chat uses cloud or returns structured scheduler-unavailable errors. |
| `edge-scheduler` | Private/offline or low-latency enterprise/NAS/desktop | Attune calls scheduler `/kb/tasks/*` and `/jobs/{id}` only; scheduler chooses worker/runtime. |
| `hybrid` | Edge first with cloud fallback | Attune admission policy decides sync local, async local, refuse, or privacy-gated cloud fallback. |

Chat RAG is an interactive path and should receive the scheduler's highest
admission priority. If the scheduler returns an async job for chat, Attune polls
within the realtime SLA before exposing the job handle. Batch ingest, OCR, ASR,
and large summaries remain lower-priority async work.

## RAG Profile Contract

Plugins can declare `rag_profiles` to describe intent instead of hard-coding
RAG behavior in Attune server routes. Attune uses the profile to choose
retrieval scope, grounding policy, and answer budget. The scheduler uses the
same metadata to pick the best available worker.

Recommended profile classes:

| Intent | Model class | Preferred size | SLA | Behavior |
|--------|-------------|----------------|-----|----------|
| `chat.rag.lookup` | `rag-chat` | `30B` | 3-8s warm | grounded answer from cited chunks |
| `chat.rag.summary` | `rag-summary` | `30B` | 8-30s warm | cited multi-chunk summary |
| `chat.rag.synthesis` | `rag-synthesis` | `30B` fallback 70B/cloud | 30-60s | deeper cross-document synthesis |
| `ingest.embed` | `embedding` | small embedding model | async | vector generation and repair |
| `ingest.ocr` | `ocr` | layout OCR model | async | scanned PDF/image extraction |

For most knowledge-base conversations, a well-quantized 30B instruction model
is the target default because it can handle Chinese/English RAG, source-grounded
summaries, and moderate synthesis more reliably than 7B/14B while still fitting
common edge GPU or high-memory CPU deployments. The policy remains model-size
agnostic: if a platform only exposes 14B, Attune degrades budget and confidence;
if 70B or cloud is available, Attune can route high-complexity synthesis there.

## Multi-Platform Handling

### Linux `.deb` / `.rpm`

- Install `attune-server` as a systemd service or desktop app without model
  runtime dependencies.
- Install `attune-edge-scheduler` as a separate systemd service, for example
  listening on `http://127.0.0.1:8090`.
- Configure Attune with `ATTUNE_EDGE_SCHEDULER_URL` or settings wizard.
- Package maintainer scripts must only validate service wiring and never pull
  model weights implicitly.

### Windows `.exe` / MSI

- Install Attune Desktop through NSIS `.exe` or MSI.
- Install `attune-edge-scheduler` as a Windows Service with explicit user/admin
  consent.
- Use DirectML, CUDA, OpenVINO, ROCm-on-WSL, or CPU backend selection inside the
  scheduler package, not inside Attune.
- Store model packs under scheduler-owned directories and expose capability via
  `/benchmark/contract`.

### macOS

- Attune Desktop remains a normal app bundle.
- Scheduler can run as a user LaunchAgent or explicit CLI service.
- Metal/CoreML-specific runtime choices stay inside the scheduler.

### Container / NAS

- Run Attune and scheduler as separate containers or system services.
- Bind Attune vault/storage separately from scheduler model cache.
- Health checks should cover Attune API, scheduler contract, vector drain, and
  chat RAG answer/citation/latency metrics.

## Operational Contract

Attune sends scheduler tasks with:

- task name (`kb.query.ask`, `kb.query.embed`, `kb.query.rerank`,
  `kb.document.ocr_recognize`, `kb.meeting.asr_frontend`);
- model capability hints (`model_class`, `preferred_size`, `fallback_sizes`);
- latency/SLA hints (`sync_sla_ms`, realtime polling policy);
- cited evidence windows and answer budget metadata;
- privacy and grounding requirements.

Scheduler returns:

- sync result when admitted within SLA;
- async `job_id` plus `eta_ms` when queued;
- structured failure/admission codes;
- runtime metadata, cache state, timing, and worker information.

Attune must not depend on worker names, private ports, model file paths, or
accelerator-specific flags. New hardware support should be implemented by
shipping a new scheduler package or model pack, not by editing Attune server
routes.
