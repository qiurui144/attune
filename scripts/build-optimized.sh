#!/usr/bin/env bash
#
# Build Attune with an explicit CPU/ISA optimization profile.
#
# Default release builds stay portable. Use this script only for controlled
# artifacts where the deployment target is known, or for local benchmarking.

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST_PATH="$PROJECT_DIR/rust/Cargo.toml"

PROFILE="portable"
PACKAGE="attune-server"
FEATURES=""
TARGET=""
MODE="build"
RELEASE=1
EXTRA_ARGS=()
PROFILE_RUSTFLAGS=""
PROFILE_CFLAGS=""
RVA23_TOOLCHAIN=""
RVA23_TRIPLET=""

usage() {
  cat <<'USAGE'
Build Attune with an explicit CPU/ISA optimization profile.

Default release builds stay portable. Use this script only for controlled
artifacts where the deployment target is known, or for local benchmarking.

Usage:
  bash scripts/build-optimized.sh [options]

Options:
  --profile <name>       portable | native | x86_64-v3 | x86_64-v4 | rva23
  --package <pkg>        Cargo package, default: attune-server
  --features <list>      Cargo feature list passed to --features
  --target <triple>      Override target triple
  --check                Run cargo check instead of cargo build
  --debug                Build/check dev profile instead of --release
  -- <args>              Extra cargo args

Profiles:
  portable    No target-specific Rust flags. Use for default distribution.
  native      -C target-cpu=native. Local benchmark/dev only.
  x86_64-v3   AVX2/FMA/BMI2 class CPUs via -C target-cpu=x86-64-v3.
  x86_64-v4   AVX-512 class CPUs via -C target-cpu=x86-64-v4.
  rva23       riscv64gc target with RVV/bitmanip flags for local scheduler images.
              Uses ATTUNE_RVA23_TOOLCHAIN when set; otherwise prefers
              /data/RV/rv-spacemit-toolchain/...v1.2.2 when present.

Examples:
  bash scripts/build-optimized.sh --profile portable --package attune-server
  bash scripts/build-optimized.sh --profile x86_64-v3 --features cuda
  bash scripts/build-optimized.sh --profile native --package attune-core --check
  bash scripts/build-optimized.sh --profile rva23 --package attune-server --features ort-dynamic --no-default-features
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --profile)
      PROFILE="${2:-}"
      shift 2
      ;;
    --package|-p)
      PACKAGE="${2:-}"
      shift 2
      ;;
    --features)
      FEATURES="${2:-}"
      shift 2
      ;;
    --target)
      TARGET="${2:-}"
      shift 2
      ;;
    --check)
      MODE="check"
      shift
      ;;
    --debug)
      RELEASE=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      EXTRA_ARGS+=("$@")
      break
      ;;
    *)
      EXTRA_ARGS+=("$1")
      shift
      ;;
  esac
done

case "$PROFILE" in
  portable)
    PROFILE_RUSTFLAGS=""
    ;;
  native)
    PROFILE_RUSTFLAGS="-C target-cpu=native"
    ;;
  x86_64-v3|avx2)
    PROFILE="x86_64-v3"
    PROFILE_RUSTFLAGS="-C target-cpu=x86-64-v3"
    ;;
  x86_64-v4|avx512)
    PROFILE="x86_64-v4"
    PROFILE_RUSTFLAGS="-C target-cpu=x86-64-v4"
    ;;
  rva23)
    TARGET="${TARGET:-riscv64gc-unknown-linux-gnu}"
    # Keep RVV on native C/C++ kernels via CFLAGS. Rust `+v` is still unstable
    # and currently trips LLVM loop-vectorization during ThinLTO on the server
    # binary, so Rust codegen defaults to stable bitmanip extensions only.
    PROFILE_RUSTFLAGS="${ATTUNE_RVA23_RUSTFLAGS:--C target-cpu=generic-rv64 -C target-feature=+zba,+zbb,+zbs}"
    PROFILE_CFLAGS="-march=${ATTUNE_RVA23_MARCH:-rv64gcv_zba_zbb_zbs_zvfh_zvbb} -mabi=${ATTUNE_RVA23_ABI:-lp64d}"
    RVA23_TOOLCHAIN="${ATTUNE_RVA23_TOOLCHAIN:-/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2}"
    RVA23_TRIPLET="${ATTUNE_RVA23_TRIPLET:-riscv64-unknown-linux-gnu}"
    ;;
  *)
    echo "unknown optimization profile: $PROFILE" >&2
    usage >&2
    exit 2
    ;;
esac

if [ -n "$PROFILE_RUSTFLAGS" ]; then
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$PROFILE_RUSTFLAGS"
fi

if [ "$PROFILE" = "rva23" ]; then
  TOOLCHAIN_BIN="$RVA23_TOOLCHAIN/bin"
  TOOLCHAIN_PREFIX="$TOOLCHAIN_BIN/$RVA23_TRIPLET-"
  if [ -x "${TOOLCHAIN_PREFIX}gcc" ]; then
    export PATH="$TOOLCHAIN_BIN:$PATH"
    export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER:-${TOOLCHAIN_PREFIX}gcc}"
    export CC_riscv64gc_unknown_linux_gnu="${CC_riscv64gc_unknown_linux_gnu:-${TOOLCHAIN_PREFIX}gcc}"
    export CXX_riscv64gc_unknown_linux_gnu="${CXX_riscv64gc_unknown_linux_gnu:-${TOOLCHAIN_PREFIX}g++}"
    export AR_riscv64gc_unknown_linux_gnu="${AR_riscv64gc_unknown_linux_gnu:-${TOOLCHAIN_PREFIX}ar}"
    export RANLIB_riscv64gc_unknown_linux_gnu="${RANLIB_riscv64gc_unknown_linux_gnu:-${TOOLCHAIN_PREFIX}ranlib}"
    export CFLAGS_riscv64gc_unknown_linux_gnu="${CFLAGS_riscv64gc_unknown_linux_gnu:+$CFLAGS_riscv64gc_unknown_linux_gnu }$PROFILE_CFLAGS"
    export CXXFLAGS_riscv64gc_unknown_linux_gnu="${CXXFLAGS_riscv64gc_unknown_linux_gnu:+$CXXFLAGS_riscv64gc_unknown_linux_gnu }$PROFILE_CFLAGS"
  else
    echo "[build-optimized] warning: rva23 toolchain not found at ${TOOLCHAIN_PREFIX}gcc; falling back to PATH" >&2
  fi
fi

CMD=(cargo "$MODE" --manifest-path "$MANIFEST_PATH" -p "$PACKAGE")
if [ "$RELEASE" = "1" ]; then
  CMD+=(--release)
fi
if [ -n "$TARGET" ]; then
  CMD+=(--target "$TARGET")
fi
if [ -n "$FEATURES" ]; then
  CMD+=(--features "$FEATURES")
fi
CMD+=("${EXTRA_ARGS[@]}")

echo "[build-optimized] profile=$PROFILE package=$PACKAGE mode=$MODE release=$RELEASE"
echo "[build-optimized] target=${TARGET:-host} features=${FEATURES:-default}"
echo "[build-optimized] RUSTFLAGS=${RUSTFLAGS:-<none>}"
if [ "$PROFILE" = "rva23" ]; then
  echo "[build-optimized] RVA23_TOOLCHAIN=${RVA23_TOOLCHAIN:-<path>}"
  echo "[build-optimized] CC_riscv64gc_unknown_linux_gnu=${CC_riscv64gc_unknown_linux_gnu:-<PATH>}"
  echo "[build-optimized] CFLAGS_riscv64gc_unknown_linux_gnu=${CFLAGS_riscv64gc_unknown_linux_gnu:-<none>}"
fi
exec "${CMD[@]}"
