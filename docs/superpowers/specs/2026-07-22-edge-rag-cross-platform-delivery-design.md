# Edge RAG Cross-Platform Delivery Design

## Goal

Deliver Attune as separate control-plane, desktop, and edge-scheduler packages so knowledge-base chat can use local 30B-class models where hardware permits without hard-coding demo-specific behavior into the main server route.

## Package Boundaries

Attune ships three installable products:

- `attune-server`: Web/API/vault/plugin/RAG control plane. It does not ship model weights or concrete inference workers.
- `attune-desktop`: Tauri shell plus embedded server. It is the user-facing app and installer surface for Windows/Linux desktop.
- `attune-edge-scheduler`: inference plane. It owns embedding, rerank, OCR/ASR, LLM workers, model lifecycle, hardware acceleration, and runtime health.

The server package may recommend or probe a scheduler, but it must remain usable in cloud-only mode. Scheduler absence is a configuration/runtime state, not a server install failure.

## RAG Strategy Boundary

Knowledge-base chat must not encode special cases such as `origin`, `summary`, or demo-specific extractive answers in `routes/chat.rs`. Those policies belong in a declarative RAG profile loaded from OSS plugins.

The OSS distribution provides a default `edge-rag` capability with `rag_profiles`:

```yaml
rag_profiles:
  - id: default-kb-chat
    intents: [qa, summary, source_lookup, compare]
    retrieval:
      strategy: hybrid
      fallback_when_empty: recent_items
      top_k: adaptive
    answer:
      task: kb.rag.answer.v1
      model_class: local-answer
      preferred_size: 30b
      sync_sla_ms: 8000
      realtime_poll: eta_plus_margin
    grounding:
      min_citations: 1
      refuse_without_evidence: true
      allow_extractive_repair: true
```

`/api/v1/chat` selects a profile, invokes a RAG orchestrator, and returns the existing public response shape: `content`, `citations`, `knowledge_count`, `latency_ms`, `cost`, and `local_scheduler`.

## Scheduler Contract

Scheduler contracts must describe capabilities rather than product-specific model names. Fixed names such as `llm-chat`, `llm-summary`, and direct route logic remain supported for compatibility, but new planning uses:

- `task_name`: stable task id such as `kb.rag.answer.v1`.
- `model_class`: capability family such as `local-answer`, `local-summary`, `embedding`, `reranker`.
- `preferred_size`: `30b`, `14b`, `7b`, or `auto`.
- `latency_class`: `interactive` or `background`.
- `sync_sla_ms`: target synchronous delivery budget.
- `fallback_sizes`: smaller model sizes allowed by policy.

The scheduler maps these logical requirements to local hardware and model inventory. A 30B model is preferred only where memory and accelerator capacity make it realistic.

## Multi-Platform Delivery

Linux x86_64/aarch64:
- Server: deb/rpm/tar.
- Desktop: deb/rpm/AppImage.
- Scheduler: deb/rpm/systemd service.
- Acceleration: CUDA, OpenVINO, ROCm, CPU fallback.

Windows x86_64:
- Desktop: NSIS exe for users, MSI for enterprise.
- Scheduler: MSI that registers a Windows Service.
- Config: `C:\ProgramData\Attune\scheduler`.
- Models: `C:\ProgramData\Attune\models`.
- Acceleration: DirectML, CUDA, OpenVINO, CPU fallback.

RISC-V NAS/K3:
- Server: riscv64 deb.
- Scheduler: riscv64 deb supplied by device/runtime owner.
- Models: scheduler-owned, usually 7B/14B-class unless the appliance advertises enough memory for 30B.

macOS arm64:
- Desktop: dmg/App bundle in later packaging work.
- Scheduler: LaunchAgent or embedded helper.
- Acceleration: Metal/CPU; 30B requires explicit capacity proof.

## Acceptance Criteria

- Server packages never include model weights or concrete inference runtime artifacts.
- Desktop installers can run in cloud-only mode and can discover a local scheduler.
- Scheduler contracts expose capability-oriented task/model metadata.
- OSS plugin schema can declare RAG profiles.
- Chat route does not keep growing with fixed RAG intent branches.
- E2E validation for every platform checks upload, index readiness, chat RAG, summary RAG, citations, latency, and no async placeholder in normal interactive answers.

