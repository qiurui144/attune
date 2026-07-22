# Edge RAG Cross-Platform Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first production slice of a declarative edge RAG and packaging architecture for deb/exe cross-platform delivery.

**Architecture:** The server remains the control plane, the scheduler remains the inference plane, and RAG behavior is declared by plugin profiles instead of hard-coded in `routes/chat.rs`. Scheduler contracts grow capability metadata while retaining legacy fields.

**Tech Stack:** Rust, serde YAML/JSON, attune plugin loader/registry, local scheduler runtime profiles, shell packaging scripts, pytest/cargo tests.

## Global Constraints

- `attune-server` packages must not ship model weights, ORT/Sherpa worker runtimes, or concrete local inference workers.
- `attune-desktop` installers remain user-facing and may discover or configure a scheduler, but must support cloud-only mode.
- `attune-edge-scheduler` owns model lifecycle, worker lifecycle, hardware acceleration, and `/kb/tasks/*`.
- RAG policy must be declared through plugin metadata, not hard-coded by intent strings in `routes/chat.rs`.
- 30B is a preferred model class only when scheduler capacity proves it; fallback to 14B/7B must be expressible.

---

### Task 1: Declarative RAG Profiles

**Files:**
- Modify: `rust/crates/attune-core/src/plugin_loader.rs`
- Modify: `rust/crates/attune-core/src/plugin_registry.rs`
- Test: `rust/crates/attune-core/tests/plugin_protocol_e2e.rs`

**Interfaces:**
- Produces: `PluginManifest.rag_profiles: Vec<RagProfileSpec>`
- Produces: `PluginRegistry::list_rag_profiles(&self) -> Vec<(&str, &RagProfileSpec)>`

- [x] **Step 1: Write failing tests**

Add tests that parse a plugin YAML with one `rag_profiles` entry and assert registry aggregation returns it.

- [x] **Step 2: Verify RED**

Run: `cargo test -p attune-core plugin_loader_parses_rag_profiles --test plugin_protocol_e2e -- --nocapture`

Expected: compile failure or assertion failure because `rag_profiles` does not exist.

- [x] **Step 3: Implement schema and registry aggregation**

Add serde structs for `RagProfileSpec`, `RagRetrievalSpec`, `RagAnswerSpec`, and `RagGroundingSpec`. Add a defaulted `rag_profiles` field to `PluginManifest`. Add `PluginRegistry::list_rag_profiles`.

- [x] **Step 4: Verify GREEN**

Run: `cargo test -p attune-core plugin_loader_parses_rag_profiles --test plugin_protocol_e2e -- --nocapture`

Expected: one passing test.

### Task 2: Scheduler Capability Metadata

**Files:**
- Modify: `rust/crates/attune-core/src/edge_cloud/scheduler.rs`
- Modify: `rust/crates/attune-core/src/edge_cloud/runtime_profile.rs`
- Test: `rust/crates/attune-core/tests/local_scheduler_runtime_profile.rs`

**Interfaces:**
- Produces: optional task/model metadata fields `model_class`, `preferred_size`, `fallback_sizes`, and `sync_sla_ms`.
- Produces: runtime profile fields that expose capability metadata without breaking legacy scheduler contracts.

- [x] **Step 1: Write failing tests**

Add a contract fixture or inline DTO with `model_class: "local-answer"` and `preferred_size: "30b"`, then assert `RuntimeProfileResolver` preserves it.

- [x] **Step 2: Verify RED**

Run: `cargo test -p attune-core runtime_profile_preserves_scheduler_capability_metadata --test local_scheduler_runtime_profile -- --nocapture`

Expected: compile failure or assertion failure because metadata fields are missing.

- [x] **Step 3: Implement optional metadata**

Add serde-defaulted optional fields to scheduler DTOs and runtime profiles. Legacy contracts must deserialize unchanged.

- [x] **Step 4: Verify GREEN**

Run: `cargo test -p attune-core runtime_profile_preserves_scheduler_capability_metadata --test local_scheduler_runtime_profile -- --nocapture`

Expected: passing test plus existing scheduler profile tests unchanged.

### Task 3: Packaging Contract Checks

**Files:**
- Modify: `scripts/package-riscv64-deb.sh`
- Modify: `scripts/release/build-riscv64-server-deb.sh`
- Create: `scripts/release/probe-attune-package-boundary.sh`
- Test: `tests/scripts/release_scripts_test.sh`

**Interfaces:**
- Produces: one reusable script that checks server package staging does not contain model/runtime artifacts.
- Produces: release reports naming the scheduler as a separate package responsibility.

- [x] **Step 1: Write failing shell test**

Add a test that expects `scripts/release/probe-attune-package-boundary.sh` to exist and fail on files named like `model.onnx`, `libsherpa`, or `onnxruntime`.

- [x] **Step 2: Verify RED**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: failure because the new probe script is missing.

- [x] **Step 3: Implement probe script and wire packaging scripts**

Create the probe script and replace inline grep checks in the riscv64 deb staging script with the reusable probe.

- [x] **Step 4: Verify GREEN**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: passing release script tests.

### Task 4: First Orchestrator Extraction

**Files:**
- Create: `rust/crates/attune-server/src/rag_orchestrator.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`
- Modify: `rust/crates/attune-server/src/routes/chat.rs`
- Test: `rust/crates/attune-server/src/rag_orchestrator.rs`

**Interfaces:**
- Produces: pure helpers for answer budget, scheduler context building, source line extraction, and summary/source lookup detection.
- Keeps public `/api/v1/chat` response shape unchanged.

- [x] **Step 1: Write failing unit tests against `rag_orchestrator`**

Move or duplicate tests for local scheduler context budget, source lookup, and summary extraction to the new module first.

- [x] **Step 2: Verify RED**

Run: `cargo test -p attune-server rag_orchestrator -- --nocapture`

Expected: compile failure because the module does not exist.

- [x] **Step 3: Move pure helper code**

Move pure functions out of `routes/chat.rs` into `rag_orchestrator.rs` and re-export only the functions used by the route.

- [x] **Step 4: Verify GREEN**

Run: `cargo test -p attune-server rag_orchestrator -- --nocapture`

Expected: passing orchestrator tests and no public API change.

### Task 5: Cross-Platform Delivery Documentation and E2E Matrix

**Files:**
- Modify: `docs/plugin-protocol.md`
- Modify: `docs/local-llm-setup.md`
- Create: `docs/edge-scheduler-delivery.md`
- Modify: `tests/e2e/kb_longloop_windows.ps1`

**Interfaces:**
- Produces: operator-facing contract for deb/exe/msi scheduler deployment.
- Produces: shared E2E checklist for Linux, Windows, and RISC-V.

- [x] **Step 1: Write docs test or grep check**

Extend script tests to assert docs mention `attune-edge-scheduler`, `cloud-only`, `Windows Service`, and `systemd`.

- [x] **Step 2: Verify RED**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: failure if the docs do not contain the required contract terms.

- [x] **Step 3: Write delivery docs**

Document package roles, install directories, service names, scheduler URL configuration, and platform E2E checks.

- [x] **Step 4: Verify GREEN**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: passing docs/release checks.
