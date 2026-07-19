#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

DEB="${ATTUNE_K3_DEB:-}"
HOST="${ATTUNE_K3_HOST:-}"
SSH_USER="${ATTUNE_K3_SSH_USER:-root}"
BASE_URL="${ATTUNE_K3_BASE_URL:-}"
SCHEDULER_URL="${ATTUNE_K3_SCHEDULER_URL:-}"
REPORTS_DIR="$ROOT/reports/release"
REMOTE_TMP="${ATTUNE_K3_REMOTE_TMP:-/tmp/attune-k3-release}"
PASSWORD="${ATTUNE_K3_E2E_PASSWORD:-e2e-pass-2026}"
BIND_DIR="${ATTUNE_K3_BACKGROUND_BIND_DIR:-$REMOTE_TMP/background-bind-smoke}"
LONGTEXT_MANIFEST="${ATTUNE_K3_LONGTEXT_MANIFEST:-}"
SKIP_DEB_CHECK=0
SKIP_INSTALL=0
SKIP_UI=0
DRY_RUN=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --deb)
      DEB="${2:-}"
      shift 2
      ;;
    --host)
      HOST="${2:-}"
      shift 2
      ;;
    --ssh-user)
      SSH_USER="${2:-}"
      shift 2
      ;;
    --base-url)
      BASE_URL="${2:-}"
      shift 2
      ;;
    --scheduler-url)
      SCHEDULER_URL="${2:-}"
      shift 2
      ;;
    --reports-dir)
      REPORTS_DIR="${2:-}"
      shift 2
      ;;
    --skip-deb-check)
      SKIP_DEB_CHECK=1
      shift
      ;;
    --skip-install)
      SKIP_INSTALL=1
      shift
      ;;
    --skip-ui)
      SKIP_UI=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      cat <<'HELP'
Validate a riscv64 Attune server .deb on a K3/NAS Web target.

Usage:
  ATTUNE_K3_HOST=192.168.x.y \
  ATTUNE_K3_BASE_URL=http://192.168.x.y:18900 \
  ATTUNE_K3_SCHEDULER_URL=http://192.168.x.y:8090 \
    bash scripts/release/test-k3-nas-web-demo.sh --deb dist/release/riscv64-server-deb/attune-server_1.5.0_riscv64.deb

Options:
  --deb <path>           Debian package to install/validate.
  --host <ssh-host>      SSH host for install and server-side fixture setup.
  --ssh-user <user>      SSH user. Defaults to root.
  --base-url <url>       Attune Web/API base URL. Defaults to http://<host>:18900.
  --scheduler-url <url>  Scheduler contract URL.
  --reports-dir <path>   Report output directory.
  --skip-deb-check       Skip local dpkg-deb architecture checks.
  --skip-install         Do not scp/dpkg install over SSH.
  --skip-ui              Skip optional Playwright headed/browser gate.
  --dry-run              Write planned report without touching the target.
HELP
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

if [ -z "$BASE_URL" ] && [ -n "$HOST" ]; then
  BASE_URL="http://$HOST:18900"
fi

mkdir -p "$REPORTS_DIR"
TS="$(date +%Y%m%d_%H%M%S)"
if [ "$DRY_RUN" = "1" ]; then
  REPORT="$REPORTS_DIR/k3-nas-web-demo-dry-run.md"
else
  REPORT="$REPORTS_DIR/k3-nas-web-demo-$TS.md"
fi

SSH_TARGET="$SSH_USER@$HOST"

log() {
  printf '[k3-demo] %s\n' "$*"
}

report_header() {
  {
    echo "# K3/NAS Web Demo Validation Report"
    echo
    echo "- Timestamp: $(date -Iseconds)"
    echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "- Deb: ${DEB:-<none>}"
    echo "- Host: ${HOST:-<none>}"
    echo "- SSH user: $SSH_USER"
    echo "- Base URL: ${BASE_URL:-<none>}"
    echo "- Scheduler URL: ${SCHEDULER_URL:-<none>}"
    echo "- Remote tmp: $REMOTE_TMP"
    echo "- Server-side bind dir: $BIND_DIR"
    echo "- Dry run: $DRY_RUN"
    echo
    echo "## Package Boundary"
    echo
    echo "This validation expects Attune to provide NAS Web/API/control-plane behavior. ORT, Sherpa, model weights, and inference runtimes are scheduler package responsibilities."
  } > "$REPORT"
}

append_report() {
  {
    echo
    echo "$@"
  } >> "$REPORT"
}

run() {
  log "+ $*"
  {
    echo
    echo '```bash'
    printf '%q ' "$@"
    echo
    echo '```'
  } >> "$REPORT"
  if [ "$DRY_RUN" != "1" ]; then
    "$@"
  fi
}

remote() {
  local cmd="$1"
  run ssh "$SSH_TARGET" "$cmd"
}

report_header

if [ "$DRY_RUN" = "1" ]; then
  append_report "## Planned Gates"
  append_report "- Check .deb architecture is riscv64 unless --skip-deb-check is set."
  append_report "- Install package on K3/NAS over SSH unless --skip-install is set."
  append_report "- Restart attune-server.service and check Web health."
  append_report "- Probe scheduler contract when scheduler URL is provided."
  append_report "- Use K3/NAS-local bind path for knowledge-base import."
  append_report "- Run optional Playwright UI gate when manifest and driver are available."
  log "dry-run report: $REPORT"
  exit 0
fi

append_report "## Package Gate"
if [ "$SKIP_DEB_CHECK" != "1" ]; then
  if [ -z "$DEB" ] || [ ! -f "$DEB" ]; then
    echo "missing --deb package path" >&2
    exit 2
  fi
  ARCH="$(dpkg-deb -f "$DEB" Architecture)"
  echo "Architecture: $ARCH" >> "$REPORT"
  if [ "$ARCH" != "riscv64" ]; then
    echo "expected riscv64 package, got $ARCH" >&2
    exit 1
  fi
  sha256sum "$DEB" | tee -a "$REPORT"
else
  append_report "Skipped by --skip-deb-check."
fi

if [ "$SKIP_INSTALL" != "1" ]; then
  if [ -z "$HOST" ]; then
    echo "--host or ATTUNE_K3_HOST is required unless --skip-install is set" >&2
    exit 2
  fi
  append_report "## Remote Install"
  remote "mkdir -p '$REMOTE_TMP'"
  run ssh "$SSH_TARGET" "uname -m && sed -n '1,8p' /etc/os-release"
  run scp "$DEB" "$SSH_TARGET:$REMOTE_TMP/"
  remote "dpkg -i '$REMOTE_TMP/$(basename "$DEB")' || apt-get -f install -y"
  remote "systemctl restart attune-server.service && systemctl is-active attune-server.service"
  remote "systemctl status attune-server.service --no-pager | sed -n '1,20p'"
else
  append_report "## Remote Install"
  append_report "Skipped by --skip-install."
fi

if [ -z "$BASE_URL" ]; then
  echo "--base-url or ATTUNE_K3_BASE_URL is required for Web/API validation" >&2
  exit 2
fi

append_report "## Web Health"
run curl -fsS "$BASE_URL/api/v1/status/health"
run curl -fsS "$BASE_URL/api/v1/status/diagnostics"

if [ -n "$SCHEDULER_URL" ]; then
  append_report "## Scheduler Contract"
  run python3 "$ROOT/scripts/probe-edge-scheduler-contract.py" --base-url "$SCHEDULER_URL" --strict
else
  append_report "## Scheduler Contract"
  append_report "Skipped because no scheduler URL was provided."
fi

append_report "## Vault and Knowledge Base Gate"
TOKEN="$(python3 - "$BASE_URL" "$PASSWORD" <<'PY'
import json
import sys
import urllib.error
import urllib.request

base, password = sys.argv[1].rstrip("/"), sys.argv[2]

def call(method, path, body=None, token="", allow=()):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            raw = resp.read().decode()
            return resp.status, json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        if exc.code in allow:
            return exc.code, {}
        raise

call("POST", "/api/v1/vault/setup", {"password": password}, allow={400, 409})
_, unlocked = call("POST", "/api/v1/vault/unlock", {"password": password})
token = unlocked.get("token")
if not isinstance(token, str) or not token:
    raise SystemExit("vault unlock did not return token")
print(token)
PY
)"
echo "Vault unlock: token length ${#TOKEN}" >> "$REPORT"

if [ -n "$HOST" ] && [ "$SKIP_INSTALL" != "1" ]; then
  remote "rm -rf '$BIND_DIR' && mkdir -p '$BIND_DIR' && printf '# Attune K3 NAS Web gate\n\nattune-k3-nas-web-bind-token\n' > '$BIND_DIR/k3-nas-web-gate.md'"
else
  append_report "Server-side fixture creation skipped; provide --host without --skip-install for full bind setup."
fi

python3 - "$BASE_URL" "$TOKEN" "$BIND_DIR" <<'PY' | tee -a "$REPORT"
import json
import sys
import time
import urllib.parse
import urllib.request

base, token, bind_dir = sys.argv[1].rstrip("/"), sys.argv[2], sys.argv[3]

def call(method, path, body=None, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        raw = resp.read().decode()
        return resp.status, json.loads(raw) if raw else {}

body = {
    "path": bind_dir,
    "recursive": True,
    "file_types": ["md", "txt", "pdf"],
    "corpus_domain": "k3-nas-web",
}
status, data = call("POST", "/api/v1/index/bind", body, timeout=120)
print(f"bind status={status} response_keys={sorted(data.keys())}")
time.sleep(2)
query = urllib.parse.quote("attune-k3-nas-web-bind-token")
status, data = call("GET", f"/api/v1/search?q={query}", timeout=60)
results = data.get("results", [])
print(f"search status={status} results={len(results)}")
if not results:
    raise SystemExit("search did not return the K3/NAS bind fixture")
PY

append_report "## Chat Gate"
python3 - "$BASE_URL" "$TOKEN" <<'PY' | tee -a "$REPORT"
import json
import sys
import urllib.request

base, token = sys.argv[1].rstrip("/"), sys.argv[2]
body = {"message": "用一句话说明 attune-k3-nas-web-bind-token 这个测试文档是否在知识库里。"}
req = urllib.request.Request(
    base + "/api/v1/chat",
    data=json.dumps(body).encode(),
    headers={"Content-Type": "application/json", "Authorization": f"Bearer {token}"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=180) as resp:
    data = json.loads(resp.read().decode())
print(f"chat status=200 keys={sorted(data.keys())}")
text = json.dumps(data, ensure_ascii=False)
if "error" in data and not data.get("answer"):
    raise SystemExit(f"chat returned error without answer: {data}")
print(text[:500])
PY

append_report "## Optional UI Gate"
if [ "$SKIP_UI" = "1" ]; then
  append_report "Skipped by --skip-ui."
elif [ -n "$LONGTEXT_MANIFEST" ] && [ -f "$LONGTEXT_MANIFEST" ]; then
  run env \
      "ATTUNE_HEADLESS=${ATTUNE_HEADLESS:-0}" \
      "ATTUNE_BASE_URL=$BASE_URL" \
      "ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_CREATE=0" \
      "ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_DIR=$BIND_DIR" \
      python3 "$ROOT/tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py" \
      --manifest "$LONGTEXT_MANIFEST" \
      --base-url "$BASE_URL" \
      --profile local_scheduler_comprehensive \
      --background-bind-create 0 \
      --background-bind-dir "$BIND_DIR"
else
  append_report "Skipped because ATTUNE_K3_LONGTEXT_MANIFEST is not set to a local manifest file."
fi

append_report "## Result"
append_report "K3/NAS Web demo validation complete."
log "report: $REPORT"
