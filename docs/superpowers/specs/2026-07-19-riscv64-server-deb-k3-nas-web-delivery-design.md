# riscv64 Server Deb and K3 NAS Web Delivery Design

Date: 2026-07-19

## Decision Summary

This delivery produces a `riscv64` headless Attune server `.deb` for K3/NAS Web use.
It does not produce a Tauri desktop `.deb`.

Attune owns the Web UI, HTTP API, vault/storage, knowledge-base import, retrieval
control, chat routing, plugin/WASM runtime, export endpoints, privacy gates, and
system service packaging. The scheduler owns ORT, Sherpa, model weights, inference
runtimes, worker lifecycle, hardware acceleration, and model lifecycle packages.

The K3/NAS user-visible target is: install the Attune server `.deb`, open the NAS/K3
IP in a browser, and complete chat plus knowledge-base workflows through the Web UI.

## Goals

- Build a reproducible `riscv64` Debian package for `attune-server-headless`.
- Use the SpacemiT private RVA23 toolchain by default:
  `/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2`.
- Keep all NAS Web interfaces complete after install:
  chat, vault setup/unlock, upload, folder bind/import, search, settings, scheduler
  configuration, export, plugin/WASM runtime, and status/diagnostics.
- Keep inference runtimes out of Attune's package. ORT, Sherpa, model files, and
  other inference-side runtime stacks are delivered and upgraded by the scheduler
  `.deb`, not by Attune.
- Standardize new release/test/maintenance script locations and output paths.
- Provide a conservative cleanup flow that audits existing sprawl first and only
  deletes ignored generated files when explicitly requested.

## Non-Goals

- No riscv64 Tauri desktop installer in this slice.
- No scheduler `.deb` implementation in this repository.
- No bundled ORT, Sherpa, local model weights, or local inference workers in the
  Attune server `.deb`.
- No destructive cleanup of historical reports, tracked benchmark evidence,
  screenshots, or user work-in-progress.
- No broad repo reshuffle that breaks existing documented entry points.

## Package Boundary

The Attune server `.deb` contains:

- `/usr/bin/attune-server-headless`
- systemd service file for the headless Web service
- `/etc/default/attune-server` runtime configuration
- license and basic package documentation
- the embedded Web UI already built into the Rust server artifact

The Attune server `.deb` depends on ordinary host tools needed by Attune control
paths, such as `curl`, `python3`, and `poppler-utils` where needed for document
handling. It must not depend on scheduler-owned inference packages.

The scheduler `.deb` owns:

- ORT / ONNX Runtime variants
- Sherpa / sherpa-onnx runtime
- embedding, rerank, OCR, ASR, LLM, VLM model weights
- RVV/IME/other hardware-specific worker runtimes
- scheduler worker services and model lifecycle

Attune connects to scheduler endpoints, probes their contract, and reports honest
degradation if scheduler capabilities are absent. Attune must not silently install
or mutate scheduler runtime state.

## Build Profile

The default delivery profile is "scheduler-owned inference + complete Web/API".

The build must not use the old minimal shape as a silent default if it removes
user-facing NAS Web features. Specifically:

- Disable Attune-owned local inference runtimes by avoiding `ort-bundled`,
  `ort-dynamic`, and `asr-sensevoice` in the Attune package build.
- Keep Web/API surface complete.
- Keep `artifact-export-rich` unless riscv64 compilation proves it is impossible.
- Keep `wasm-runtime` unless riscv64 compilation proves it is impossible.
- Keep `scheduler-runtime` as an explicit marker for this deployment class.

The intended command shape is:

```bash
ATTUNE_RVA23_TOOLCHAIN=/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2 \
  bash scripts/build-optimized.sh --profile rva23 \
    --package attune-server \
    --no-default-features \
    --features scheduler-runtime,artifact-export-rich,wasm-runtime
```

If `artifact-export-rich` or `wasm-runtime` fails on riscv64, the build is not
allowed to quietly drop it. The release report must classify the result as either:

- blocker: package cannot be accepted because the NAS Web contract is incomplete; or
- explicitly approved degradation: accepted with named missing capability and UI/API
  behavior documented.

## Script Layout

New script locations:

- `scripts/release/build-riscv64-server-deb.sh`
- `scripts/release/test-k3-nas-web-demo.sh`
- `scripts/maintenance/audit-scripts-and-outputs.sh`
- `scripts/maintenance/clean-workspace.sh`

Existing scripts remain callable from their current paths. If a new wrapper replaces
an older workflow, the old path must either stay as a thin compatibility wrapper or
be documented as deprecated before removal.

## Output Layout

New outputs use these locations:

- Build artifacts:
  `dist/release/riscv64-server-deb/`
- Release validation reports:
  `reports/release/`
- Maintenance and cleanup reports:
  `reports/maintenance/`

The build script writes at least:

- `attune-server_<version>_riscv64.deb`
- `attune-server_<version>_riscv64.deb.sha256`
- `build-riscv64-server-deb-<timestamp>.md`
- RVV/vectorization audit output
- package file listing and `dpkg-deb --info` output

The K3/NAS demo script writes at least:

- target host, OS, arch, kernel, and glibc summary
- installed package version and SHA256
- service status and journal excerpt
- scheduler contract probe result
- Web health/status diagnostics
- knowledge-base import/bind evidence
- chat evidence
- headed or browser-driven UI evidence when a browser driver is available

## K3/NAS Acceptance Gate

The K3/NAS gate validates the installed `.deb`, not a dev binary.

Required topology:

- Attune server runs on K3/NAS.
- Scheduler runs on K3/NAS or the configured scheduler host.
- Vault, vectors, Tantivy index, corpus, and bind directories are on the K3/NAS
  filesystem.
- A remote workstation may act only as browser/Playwright driver.
- `/api/v1/index/bind` paths must always be server-side K3/NAS paths.

Minimum gates:

1. Package gate:
   - `.deb` architecture is `riscv64`.
   - Package installs with `dpkg -i`.
   - Service starts and listens on the configured NAS Web bind address.

2. Web/API gate:
   - Browser can reach the K3/NAS Web UI.
   - Vault setup/unlock works.
   - Settings and scheduler endpoint configuration work.
   - `/api/v1/status/health` and diagnostics return valid responses.

3. Knowledge-base gate:
   - File upload works.
   - Folder bind/import works with K3/NAS-local paths.
   - Background bind returns quickly and remains visible in UI/status.
   - Search returns indexed content.

4. Chat gate:
   - Chat request reaches the configured cloud or scheduler route.
   - Answers include honest degradation/citation behavior according to existing
     long-text gates.
   - Scheduler metadata is visible when scheduler is used.

5. Regression gate:
   - Existing e2e scripts are reused where possible.
   - K3-specific headed UI topology from `tests/e2e/README.md` remains the source
     of truth for remote browser validation.

## Cleanup and Project Organization

The cleanup work is audit-first.

`audit-scripts-and-outputs.sh` inventories:

- script entry points under `scripts/`, `tests/e2e/`, `.github/scripts/`,
  `apps/attune-desktop/scripts/`, and `rust/scripts/`
- generated outputs under `reports/`, `reports/runs/`, `docs/reports/`,
  `docs/benchmarks/`, `tests/reports/`, `tmp/`, `.playwright-mcp/`, `.remember/`,
  `dist/`, and target/bundle directories
- tracked versus ignored status for each output root
- recommended canonical owner for each class of output

`clean-workspace.sh` defaults to dry run. In apply mode it may remove only ignored
generated files and directories. It must not remove tracked docs, benchmark evidence,
screenshots, source files, user-modified files, or untracked files that are not ignored.

Project cleanup must prefer compatibility wrappers and documentation updates over
renaming large swaths of the tree in this slice.

## Risks

- `wasm-runtime` or rich export may fail on riscv64 because of dependency/toolchain
  behavior. The build report must expose this as a release decision, not hide it.
- K3 demo validity depends on testing the installed `.deb`; reusing a dev binary would
  create false confidence.
- Existing repo output locations are messy. A hard cleanup could destroy evidence or
  user work, so cleanup must be explicit and conservative.
- Scheduler package availability is outside this Attune repository. Attune can probe
  scheduler contracts but cannot prove scheduler runtime packaging unless a scheduler
  `.deb` is supplied to the gate.

## Acceptance Criteria

- A single command can build the Attune riscv64 server `.deb` into
  `dist/release/riscv64-server-deb/`.
- The build report records toolchain path, feature set, commit SHA, package metadata,
  SHA256, and RVV audit result.
- A single command can validate an installed package on K3/NAS through the Web/API
  gate and write a report under `reports/release/`.
- Installing the package does not install ORT, Sherpa, model weights, or scheduler
  runtime packages.
- NAS Web workflows for chat and knowledge-base import are complete or any missing
  capability is reported as a blocker.
- Maintenance audit reports script/output sprawl and cleanup candidates without
  deleting anything by default.
