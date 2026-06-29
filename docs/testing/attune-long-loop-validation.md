# Attune Long Loop Validation

This plan covers the longer validation loop for Attune OSS, Pro, and cloud-backed
membership flows. It intentionally uses real benchmark reports as knowledge-base
material instead of synthetic one-line notes.

## Hardware Strategy Source

Strategy source: `/data/company/project/vlm-llm-benchmark/reports/`.

Platform policy:

- Intel Windows:
  - General ONNX acceleration: OpenVINO first, CPU fallback.
  - OCR: OpenVINO required; DirectML is rejected because the benchmark recorded
    unusable OCR quality on Intel DirectML.
  - ASR: SenseVoice DirectML remains a separately validated path and should get
    its own task-specific rule if routed through the shared EP selector later.
- AMD Windows:
  - LLM and embedding: Ollama Vulkan on Radeon iGPU.
  - OCR interactive: DirectML on Radeon iGPU.
  - OCR background batch: VitisAI NPU only when the runtime is present; otherwise
    DirectML/CPU fallback.
  - Reranker default: CPU ONNX remains acceptable; DirectML is not the default
    latency winner in current measurements.

## Knowledge-Base Material

Primary documents:

- `intel-windows.en.md`
- `intel-windows-igpu.en.md`
- `amd-windows.en.md`
- `amd-windows-igpu.en.md`
- `amd-windows-npu.en.md`

Import tags:

- `kb-longloop`
- `benchmark`
- `intel` or `amd`

Queries that must hit the expected platform:

- `Intel DirectML OCR CER 202 OpenVINO`
- `Intel Arc iGPU OpenVINO embedding reranker latency`
- `AMD Radeon 780M OCR DirectML fastest path`
- `AMD XDNA 1 NPU LLM not supported 8845H`
- `qwen2.5 7b amd win translation fail en zh`

Expected checks:

- Search HTTP 200 for every query.
- Top 5 results include at least one expected vendor/platform document.
- Repeated query cache path remains stable.
- `/api/v1/items` count increases or duplicate status is returned without error.
- `/api/v1/ai_stack` remains responsive during import/search loop.

## Long Loop Schedule

Minimum loop for Intel Windows E2E:

- 30 import/search/chat cycles.
- Every 5 cycles: poll `/api/v1/ai_stack`, `/api/v1/background/status`, and
  process RSS/CPU/thread count.
- Every 10 cycles: restart the desktop executable and verify vault unlock plus
  search still works.
- End condition: no hung request, no 5xx on ingest/search/status, and memory
  growth below 20 percent after warm-up.

Extended soak:

- 2 hours mixed workload: 60 percent search, 25 percent ingest/update, 15 percent
  chat with cloud gateway.
- Keep the same test account so quota and gateway state are exercised.
- Save all raw reports under `/tmp/attune-longloop-*`.

## Current Windows E2E Result

Intel Windows host result on 2026-06-29:

- GitHub package installation directory exists at `%LOCALAPPDATA%\Attune`.
- Runtime stack/model directories were deployed:
  - `models/asr`
  - `models/ppocr`
  - `models/BAAI_bge-reranker-base`
  - `models/Xenova_bge-m3`
  - `models/ep-stacks`
  - `lib/windows`
- Health endpoint was unreachable at `http://127.0.0.1:28630/api/v1/status/health`.
- `attune-desktop.exe`/`attune-server` were not running after SSH-side start.
- Application log file existed but was empty: `%LOCALAPPDATA%\Attune\logs\attune-server.2026-06-29`.
- Windows Event Log recorded `attune-desktop.exe 1.5.0.0` with
  `RADAR_PRE_LEAK_64` earlier in the same package session.
- KB long-loop runner generated a blocked report instead of hanging:
  `C:\attune-e2e\kb-longloop-report-20260629-153101.json`.

E2E blocker: the current Windows package can deploy files/models, but the desktop
service did not stay up, so knowledge import/search and Pro login validation
cannot proceed from the GitHub exe until package startup is fixed.

## UI Entry Audit

Checked scope:

- Sidebar primary entries: chat, workbench, items, projects, knowledge.
- More group entries: remote sources, skills, office, doc intelligence, writing,
  skill runner, monitoring, marketplace, privacy, quota.
- Command palette view targets.
- Empty/dead-link scan for `href="#"`, `javascript:` links, placeholder copy,
  unimplemented markers, and empty click handlers.

Fixes applied:

- Temporary chat is now explicitly labeled as `Temporary chat` / `临时对话`.
- Knowledge is no longer an empty placeholder route; it now shows item/source
  metrics, recent knowledge, and direct actions to items, sources, and projects.
- Drag-and-drop copy no longer says placeholder.

## Pro Blocked Until Windows Package Exists

Current blocker: cloud entitlement installed a `law-pro` package containing a
Linux ELF binary on Windows. Client-side validation now rejects incompatible
packages, but the full Pro industry workflow requires a Windows `law-pro`
package from PluginHub.

Prepared industry test material:

- Civil loan:
  - loan principal, term, annual rate, repayment ledger, partial payments.
  - Expected: principal/interest table, red-line checks, missing evidence list.
- Labor dispute:
  - employment period, salary records, termination notice.
  - Expected: compensation calculation and statute limitation checks.
- Traffic accident:
  - accident responsibility ratio, medical expense list, disability evidence.
  - Expected: liability split and compensation itemization.
- Defamation:
  - publication text, spread evidence, identity mapping, damages evidence.
  - Expected: fact extraction, red-line proof requirements, litigation checklist.

Pro acceptance after package fix:

- Plugin install succeeds on Windows with PE binaries or WASM where applicable.
- `/api/v1/plugins` lists `law-pro` enabled.
- `/api/v1/agents/civil_loan_agent/run` returns structured result, not 500.
- Exportable xlsx/csv/md output works for civil-loan calculation.
- Industry loop runs 20 cases without crashing or corrupting vault state.
