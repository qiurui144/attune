#!/usr/bin/env bash
#
# Attune Linux host preflight.
#
# This script intentionally does not install Ollama, pull model weights, or
# start local inference workers. Attune production paths use either a cloud LLM
# configured by the user or an edge scheduler service supplied by the host.
#
# Usage:
#   ./scripts/deploy-linux.sh --scheduler-url http://127.0.0.1:8090
#   ATTUNE_EDGE_SCHEDULER_URL=http://127.0.0.1:8090 ./scripts/deploy-linux.sh
#   ./scripts/deploy-linux.sh --cloud-only
#   ./scripts/deploy-linux.sh --dry-run
#
# Exit codes:
#   0 = host preflight passed
#   2 = unsupported platform or bad arguments
#   5 = edge scheduler contract probe failed

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCHEDULER_URL="${ATTUNE_EDGE_SCHEDULER_URL:-${ATTUNE_LOCAL_SCHEDULER_BASE:-}}"
CLOUD_ONLY=0
DRY_RUN=0
STRICT="${ATTUNE_EDGE_SCHEDULER_STRICT:-1}"

usage() {
  sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --scheduler-url)
      [ "$#" -ge 2 ] || { echo "missing value for --scheduler-url" >&2; exit 2; }
      SCHEDULER_URL="$2"
      shift 2
      ;;
    --cloud-only)
      CLOUD_ONLY=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --no-strict-scheduler)
      STRICT=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

log() { printf "\033[1;36m[deploy]\033[0m %s\n" "$*"; }
warn() { printf "\033[1;33m[warn]\033[0m %s\n" "$*"; }
err() { printf "\033[1;31m[err]\033[0m %s\n" "$*" >&2; }

run() {
  if [ "$DRY_RUN" = "1" ]; then
    log "[dry-run] $*"
  else
    "$@"
  fi
}

log "step 1/4: platform check"
if [ "$(uname -s)" != "Linux" ]; then
  err "this script is Linux-only (got $(uname -s))"
  exit 2
fi
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|aarch64|riscv64) ;;
  *)
    err "unsupported arch: $ARCH"
    exit 2
    ;;
esac
RAM_GB="$(free -g | awk '/^Mem:/{print $2}')"
log "  Linux $ARCH | RAM ${RAM_GB:-unknown} GB"

log "step 2/4: dependency visibility"
for bin in curl python3; do
  if command -v "$bin" >/dev/null 2>&1; then
    log "  $bin: $(command -v "$bin")"
  else
    warn "  $bin not found"
  fi
done

log "step 3/4: AI execution path"
if [ "$CLOUD_ONLY" = "1" ]; then
  log "  cloud-only mode selected; configure LLM endpoint/key in Attune wizard or settings"
elif [ -n "$SCHEDULER_URL" ]; then
  log "  probing edge scheduler: $SCHEDULER_URL"
  PROBE_ARGS=(--base-url "$SCHEDULER_URL")
  if [ "$STRICT" = "0" ]; then
    PROBE_ARGS+=(--no-strict)
  else
    PROBE_ARGS+=(--strict)
  fi
  run python3 "$ROOT/scripts/probe-edge-scheduler-contract.py" "${PROBE_ARGS[@]}" || exit 5
else
  warn "  no scheduler URL supplied; Attune will require cloud LLM settings for chat"
  warn "  pass --scheduler-url or set ATTUNE_EDGE_SCHEDULER_URL to validate an edge scheduler"
fi

log "step 4/4: Attune runtime hints"
log "  Attune does not install or manage concrete local inference workers."
log "  Local embedding/rerank/OCR/ASR/LLM acceleration is owned by the edge scheduler service."
log "  For cloud mode, configure OpenAI-compatible endpoint, model, and key in the wizard."
log "  For edge mode, use scheduler endpoint ${SCHEDULER_URL:-'(not configured)'}."

log "deploy-linux.sh: host preflight done."
