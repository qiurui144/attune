#!/usr/bin/env bash
# Attune E2E canonical runner.
#
# E2E for this project means K3 physical-device validation only. This wrapper
# intentionally does not build or start a local attune-server, and it refuses
# loopback targets so a workstation run cannot be mistaken for K3 evidence.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

usage() {
  cat <<'HELP'
Run the canonical Attune E2E gate on a K3 physical device.

Required for live E2E:
  ATTUNE_K3_HOST                 K3 host/IP reachable by SSH and browser.
  ATTUNE_K3_BASE_URL             Attune URL on the K3 device, e.g. http://192.168.x.y:18900.
  ATTUNE_K3_E2E_PASSWORD         Test vault password for setup/unlock.
  ATTUNE_K3_DEB or --deb         attune-server riscv64 .deb.

Common optional inputs:
  ATTUNE_K3_WEB_DEMO_DEB         attune-web-demo companion .deb.
  ATTUNE_K3_SCHEDULER_URL        Scheduler URL as seen by the runner.
  ATTUNE_K3_SERVER_SCHEDULER_BASE
                                  Scheduler URL as seen by Attune on K3.
  ATTUNE_K3_EVAL_SUITE           e.g. k3_rag_release_smoke.
  ATTUNE_K3_LONGTEXT_E2E=1       Run K3-hosted long-text corpus gate.
  ATTUNE_K3_WEB_DEMO_BASE_URL    K3 web-demo URL, e.g. http://192.168.x.y:8968.

Example:
  ATTUNE_K3_HOST=192.168.100.233 \
  ATTUNE_K3_BASE_URL=http://192.168.100.233:18900 \
  ATTUNE_K3_SCHEDULER_URL=http://192.168.100.233:8090 \
  ATTUNE_K3_SERVER_SCHEDULER_BASE=http://127.0.0.1:8090 \
  ATTUNE_K3_E2E_PASSWORD='...' \
  ATTUNE_K3_EVAL_SUITE=k3_rag_release_smoke \
  bash tests/e2e/run_all.sh --deb dist/release/riscv64-server-deb/attune-server_..._riscv64.deb

For non-E2E local integration/debug runs, use targeted unit/integration tests or
explicit developer scripts. Do not use this E2E entrypoint for localhost.
HELP
}

is_loopback_or_local() {
  local value="${1:-}"
  case "$value" in
    ""|localhost|localhost:*|127.*|127.*:*) return 0 ;;
    http://localhost|http://localhost:*|https://localhost|https://localhost:*) return 0 ;;
    http://127.*|http://127.*:*|https://127.*|https://127.*:*) return 0 ;;
    http://\[::1\]*|https://\[::1\]*|\[::1\]|\[::1\]:*) return 0 ;;
    *) return 1 ;;
  esac
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

HOST="${ATTUNE_K3_HOST:-}"
BASE_URL="${ATTUNE_K3_BASE_URL:-}"
if [ -z "$BASE_URL" ] && [ -n "$HOST" ]; then
  BASE_URL="http://$HOST:18900"
  export ATTUNE_K3_BASE_URL="$BASE_URL"
fi

if [ -z "$HOST" ]; then
  echo "E2E requires ATTUNE_K3_HOST; local/localhost E2E is not a valid project gate." >&2
  usage >&2
  exit 2
fi
if is_loopback_or_local "$HOST"; then
  echo "E2E requires a K3 physical-device host, got ATTUNE_K3_HOST=$HOST" >&2
  exit 2
fi
if [ -z "$BASE_URL" ]; then
  echo "E2E requires ATTUNE_K3_BASE_URL or a non-local ATTUNE_K3_HOST." >&2
  exit 2
fi
if is_loopback_or_local "$BASE_URL"; then
  echo "E2E requires a K3 physical-device base URL, got ATTUNE_K3_BASE_URL=$BASE_URL" >&2
  exit 2
fi

exec bash "$ROOT/scripts/release/test-k3-nas-web-demo.sh" "$@"
