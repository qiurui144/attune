# Build Optimization Profiles

Attune's runtime path is moving toward a unified scheduler contract:

- Attune owns policy, privacy, retrieval, context admission, and cloud fallback.
- The local scheduler owns local worker lifecycle, hardware arbitration, and
  device-specific inference acceleration.
- Cloud providers remain an Attune-controlled fallback behind `OutboundGate`.

This means hardware-specific acceleration should live behind the scheduler
service, not as product-level code branches in Attune.

## Build Profiles

Use `scripts/build-optimized.sh` when producing a target-specific binary:

```bash
bash scripts/build-optimized.sh --profile portable --package attune-server
bash scripts/build-optimized.sh --profile x86_64-v3 --package attune-server
bash scripts/build-optimized.sh --profile x86_64-v4 --package attune-server
bash scripts/build-optimized.sh --profile native --package attune-server --check
bash scripts/build-optimized.sh --profile rva23 --package attune-server --features scheduler-runtime,artifact-export-rich,wasm-runtime -- --no-default-features
```

Profile intent:

| Profile | Rust flags | Use |
|:---|:---|:---|
| `portable` | none | Default distribution and CI. |
| `native` | `-C target-cpu=native` | Local benchmark/dev only. Do not ship broadly. |
| `x86_64-v3` | `-C target-cpu=x86-64-v3` | AVX2/FMA/BMI2 class x86_64 systems. |
| `x86_64-v4` | `-C target-cpu=x86-64-v4` | AVX-512 class x86_64 systems. |
| `rva23` | `riscv64gc` target plus RVV/bitmanip flags | Local scheduler images on RVA23-class platforms. |

The `rva23` profile can be overridden without editing the repo:

```bash
ATTUNE_RVA23_RUSTFLAGS='-C target-cpu=generic-rv64 -C target-feature=+v,+zba,+zbb,+zbs' \
  bash scripts/build-optimized.sh --profile rva23 --package attune-server
```

## riscv64 NAS Web Server Deb

Use the release wrapper for K3/NAS Web delivery:

```bash
bash scripts/package-riscv64-deb.sh
```

This is the ordinary-user one-key entrypoint. It uses the SpacemiT private
toolchain by default:

```text
/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2
```

The Attune package profile is scheduler-owned inference plus complete Web/API:

```bash
--no-default-features --features scheduler-runtime,artifact-export-rich,wasm-runtime
```

This keeps NAS Web workflows complete while excluding Attune-owned local
inference runtimes. ORT, Sherpa, model weights, RVV/IME workers, and model
lifecycle are scheduler `.deb` responsibilities, not Attune `.deb`
responsibilities. The output roots are:

```text
dist/release/riscv64-server-deb/
reports/release/
```

Use the lower-level script only when debugging package internals:

```bash
bash scripts/release/build-riscv64-server-deb.sh
```

## Artifact Audit

After an RVA23 build, verify the shipped binary instead of trusting build flags:

```bash
bash scripts/audit-rvv-vectorization.sh \
  rust/target/riscv64gc-unknown-linux-gnu/release/attune-server-headless
```

The audit checks:

- RISC-V ELF attributes such as `v1p0`, `zve*`, `zvfh`, and `zvbb`.
- RVV instruction mnemonics in the main Attune server binary.
- RVV instruction mnemonics in core vector-search libraries
  `libnumkong*.rlib` and `libusearch*.rlib`.

Release builds run this audit in strict mode by default:

```text
ATTUNE_RVV_AUDIT_STRICT=1
ATTUNE_RVV_AUDIT_MIN_MAIN_LINES=1
ATTUNE_RVV_AUDIT_MIN_CORE_LINES=1
```

This prevents a package from passing only because the ELF attribute mentions
RVV. It still does not prove K3 inference performance, because ORT, Sherpa,
model workers, and execution-provider dispatch live in the scheduler package.

Set `ATTUNE_RVV_AUDIT_SCAN_NATIVE=1` to inspect all native `.a` archives. Keep
that off for routine checks because large native dependency trees produce noisy
output.

## K3 Runtime Performance Gate

For K3/NAS acceptance, run the scheduler runtime gate against the installed
scheduler service:

```bash
ATTUNE_K3_SCHEDULER_URL=http://<nas-ip>:8090 \
  bash scripts/release/test-k3-rvv-runtime-gate.sh
```

The full NAS Web demo invokes this gate automatically when
`ATTUNE_K3_SCHEDULER_URL` or `--scheduler-url` is provided. The gate requires
scheduler RVV/IME/SpacemiT metadata and live latency evidence from
`/benchmark/contract`, `/models`, `/capacity`, and
`/data/RV/k3-scheduler/tools/worker_benchmark_gate.py`. Thresholds can be tuned
without editing the repository:

```bash
ATTUNE_K3_RVV_MAX_EMBED_P50_MS=200 \
ATTUNE_K3_RVV_MAX_RERANK_P50_MS=300 \
  bash scripts/release/test-k3-rvv-runtime-gate.sh \
    --scheduler-url http://<nas-ip>:8090
```

If this gate fails, fix the scheduler runtime/provider/model `.deb` pipeline.
Do not move ORT, Sherpa, model weights, or inference runtime ownership into the
Attune `.deb`.

## Worker Acceleration Classes

Scheduler worker builds may use different execution-provider artifacts:

| Platform | Feature path |
|:---|:---|
| NVIDIA Linux | `--features cuda` |
| Windows high-performance GPU | `--features directml` |
| Intel Windows/Linux | `--features openvino` with `ort-dynamic` bundles where needed |
| AMD Linux | `--features rocm` |

These provider choices must be reported by the scheduler through
`/benchmark/contract`, `/models`, and `/capacity`. Attune consumes capability
and runtime metadata instead of branching on vendor hardware directly.

## Rules

- Do not put `target-cpu=native` into default cargo config. It can produce
  binaries that crash on older CPUs.
- Keep `portable` as the CI and generic release default.
- Use target-specific profiles only for controlled artifacts with a known
  deployment class.
- Treat AVX/RVV/GPU acceleration as a worker/scheduler capability. Product
  behavior should stay behind the same local scheduler and cloud routing APIs.
