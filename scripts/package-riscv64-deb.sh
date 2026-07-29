#!/usr/bin/env bash
# One-key riscv64 Debian package entrypoint for ordinary users.
#
# The lower-level release script remains the source of truth for package
# staging. This wrapper keeps the common K3/NAS path to one command:
#
#   bash scripts/package-riscv64-deb.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_TOOLCHAIN="/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2"

VERSION=""
TOOLCHAIN="${ATTUNE_RVA23_TOOLCHAIN:-$DEFAULT_TOOLCHAIN}"
OUT_DIR="$ROOT/dist/release/riscv64-server-deb"
REPORTS_DIR="$ROOT/reports/release"
SKIP_FRONTEND="${ATTUNE_PACKAGE_SKIP_FRONTEND:-0}"
SKIP_BUILD="${ATTUNE_PACKAGE_SKIP_BUILD:-0}"
SKIP_RVV_AUDIT="${ATTUNE_PACKAGE_SKIP_RVV_AUDIT:-0}"
INCLUDE_WEB_DEMO="${ATTUNE_PACKAGE_INCLUDE_WEB_DEMO:-1}"
INCLUDE_OSS_COMPANION="${ATTUNE_PACKAGE_INCLUDE_OSS_COMPANION:-1}"
STRICT_RVV="${ATTUNE_PACKAGE_STRICT_RVV:-1}"
DRY_RUN=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --toolchain)
      TOOLCHAIN="${2:-}"
      shift 2
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
      shift 2
      ;;
    --reports-dir)
      REPORTS_DIR="${2:-}"
      shift 2
      ;;
    --skip-frontend)
      SKIP_FRONTEND=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --skip-rvv-audit)
      SKIP_RVV_AUDIT=1
      shift
      ;;
    --skip-web-demo)
      INCLUDE_WEB_DEMO=0
      shift
      ;;
    --skip-oss-companion)
      INCLUDE_OSS_COMPANION=0
      shift
      ;;
    --strict-rvv)
      STRICT_RVV=1
      shift
      ;;
    --no-strict-rvv)
      STRICT_RVV=0
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      cat <<'HELP'
One-key riscv64 Debian package build for K3/NAS Web delivery.

Usage:
  bash scripts/package-riscv64-deb.sh

Common options:
  --toolchain <path>    SpacemiT toolchain root.
  --version <value>     Override package version.
  --out-dir <path>      Output directory for .deb and package metadata.
  --reports-dir <path>  Output directory for build reports.
  --skip-frontend       Reuse existing attune-server UI dist.
  --skip-build          Reuse existing riscv64 binary.
  --skip-web-demo       Do not build the attune-web-demo companion package.
  --skip-oss-companion  Do not build the attune-oss-companion package.
  --no-strict-rvv       Do not require RVV instruction thresholds.
  --dry-run             Write reports without building.

Environment shortcuts:
  ATTUNE_PACKAGE_SKIP_FRONTEND=1
  ATTUNE_PACKAGE_SKIP_BUILD=1
  ATTUNE_PACKAGE_INCLUDE_WEB_DEMO=0
  ATTUNE_PACKAGE_INCLUDE_OSS_COMPANION=0
  ATTUNE_RVA23_TOOLCHAIN=/path/to/spacemit-toolchain
HELP
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

mkdir -p "$REPORTS_DIR"
TS="$(date +%Y%m%d_%H%M%S)"
if [ "$DRY_RUN" = "1" ]; then
  REPORT="$REPORTS_DIR/package-riscv64-deb-dry-run.md"
else
  REPORT="$REPORTS_DIR/package-riscv64-deb-$TS.md"
fi

log() {
  printf '[package-riscv64-deb] %s\n' "$*"
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 2
  fi
}

validate_switch() {
  case "$2" in
    0|1) ;;
    *) echo "$1 must be 0 or 1, got: $2" >&2; exit 2 ;;
  esac
}

validate_switch ATTUNE_PACKAGE_SKIP_FRONTEND "$SKIP_FRONTEND"
validate_switch ATTUNE_PACKAGE_SKIP_BUILD "$SKIP_BUILD"
validate_switch ATTUNE_PACKAGE_SKIP_RVV_AUDIT "$SKIP_RVV_AUDIT"
validate_switch ATTUNE_PACKAGE_INCLUDE_WEB_DEMO "$INCLUDE_WEB_DEMO"
validate_switch ATTUNE_PACKAGE_INCLUDE_OSS_COMPANION "$INCLUDE_OSS_COMPANION"
validate_switch ATTUNE_PACKAGE_STRICT_RVV "$STRICT_RVV"

if [ "$DRY_RUN" != "1" ]; then
  require_cmd bash
  require_cmd cargo
  require_cmd dpkg-deb
  require_cmd python3
  require_cmd sha256sum
  if [ "$SKIP_FRONTEND" != "1" ]; then
    require_cmd npm
  fi
  if [ "$SKIP_BUILD" != "1" ] && [ ! -x "$TOOLCHAIN/bin/riscv64-unknown-linux-gnu-gcc" ]; then
    echo "SpacemiT toolchain not found: $TOOLCHAIN" >&2
    echo "Set ATTUNE_RVA23_TOOLCHAIN or pass --toolchain." >&2
    exit 2
  fi
fi

BUILD_ARGS=(
  --toolchain "$TOOLCHAIN"
  --out-dir "$OUT_DIR"
  --reports-dir "$REPORTS_DIR"
)
if [ -n "$VERSION" ]; then
  BUILD_ARGS+=(--version "$VERSION")
fi
if [ "$SKIP_FRONTEND" = "1" ]; then
  BUILD_ARGS+=(--skip-frontend)
fi
if [ "$SKIP_BUILD" = "1" ]; then
  BUILD_ARGS+=(--skip-build)
fi
if [ "$SKIP_RVV_AUDIT" = "1" ]; then
  BUILD_ARGS+=(--skip-rvv-audit)
fi
if [ "$DRY_RUN" = "1" ]; then
  BUILD_ARGS+=(--dry-run)
fi

{
  echo "# One-key riscv64 Debian Package"
  echo
  echo "- Timestamp: $(date -Iseconds)"
  echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "- Toolchain: $TOOLCHAIN"
  echo "- Output dir: $OUT_DIR"
  echo "- Reports dir: $REPORTS_DIR"
  echo "- Skip frontend: $SKIP_FRONTEND"
  echo "- Skip build: $SKIP_BUILD"
  echo "- Skip RVV audit: $SKIP_RVV_AUDIT"
  echo "- Include web demo package: $INCLUDE_WEB_DEMO"
  echo "- Include OSS companion package: $INCLUDE_OSS_COMPANION"
  echo "- Strict RVV audit: $STRICT_RVV"
  echo "- Dry run: $DRY_RUN"
  echo
  echo "## Package Boundary"
  echo
  echo "This command builds the Attune NAS Web/API/control-plane package only. ORT, Sherpa, models, IME/RVV workers, and inference runtimes remain scheduler package responsibilities."
  echo
  echo "## Underlying Command"
  echo
  echo '```bash'
  printf '%q ' bash "$ROOT/scripts/release/build-riscv64-server-deb.sh" "${BUILD_ARGS[@]}"
  echo
  echo '```'
} > "$REPORT"

log "report: $REPORT"
if [ "$STRICT_RVV" = "1" ]; then
  export ATTUNE_RVV_AUDIT_STRICT="${ATTUNE_RVV_AUDIT_STRICT:-1}"
  export ATTUNE_RVV_AUDIT_MIN_MAIN_LINES="${ATTUNE_RVV_AUDIT_MIN_MAIN_LINES:-1}"
  export ATTUNE_RVV_AUDIT_MIN_CORE_LINES="${ATTUNE_RVV_AUDIT_MIN_CORE_LINES:-1}"
fi

bash "$ROOT/scripts/release/build-riscv64-server-deb.sh" "${BUILD_ARGS[@]}"

if [ "$INCLUDE_WEB_DEMO" = "1" ]; then
  WEB_DEMO_ARGS=(
    --out-dir "$OUT_DIR"
    --reports-dir "$REPORTS_DIR"
  )
  if [ -n "$VERSION" ]; then
    WEB_DEMO_ARGS+=(--version "$VERSION")
  fi
  if [ "$DRY_RUN" = "1" ]; then
    WEB_DEMO_ARGS+=(--dry-run)
  fi
  bash "$ROOT/scripts/release/build-riscv64-web-demo-deb.sh" "${WEB_DEMO_ARGS[@]}"
fi

if [ "$INCLUDE_OSS_COMPANION" = "1" ]; then
  OSS_COMPANION_ARGS=(
    --out-dir "$OUT_DIR"
    --reports-dir "$REPORTS_DIR"
  )
  if [ -n "$VERSION" ]; then
    OSS_COMPANION_ARGS+=(--version "$VERSION")
  fi
  if [ "$DRY_RUN" = "1" ]; then
    OSS_COMPANION_ARGS+=(--dry-run)
  fi
  bash "$ROOT/scripts/release/build-riscv64-oss-companion-deb.sh" "${OSS_COMPANION_ARGS[@]}"
fi
