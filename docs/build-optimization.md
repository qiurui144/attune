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
bash scripts/release/build-riscv64-server-deb.sh
```

The wrapper uses the SpacemiT private toolchain by default:

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

Set `ATTUNE_RVV_AUDIT_SCAN_NATIVE=1` to inspect all native `.a` archives. Keep
that off for routine checks because large native dependency trees produce noisy
output.

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
