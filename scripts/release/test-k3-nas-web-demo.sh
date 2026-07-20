#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

DEB="${ATTUNE_K3_DEB:-}"
HOST="${ATTUNE_K3_HOST:-}"
SSH_USER="${ATTUNE_K3_SSH_USER:-root}"
SSH_OPTS_RAW="${ATTUNE_K3_SSH_OPTS:--o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null}"
SSH_PASSWORD="${ATTUNE_K3_SSH_PASSWORD:-}"
BASE_URL="${ATTUNE_K3_BASE_URL:-}"
SCHEDULER_URL="${ATTUNE_K3_SCHEDULER_URL:-}"
REPORTS_DIR="$ROOT/reports/release"
REMOTE_TMP="${ATTUNE_K3_REMOTE_TMP:-}"
PASSWORD="${ATTUNE_K3_E2E_PASSWORD:-e2e-pass-2026}"
BIND_DIR="${ATTUNE_K3_BACKGROUND_BIND_DIR:-}"
LONGTEXT_MANIFEST="${ATTUNE_K3_LONGTEXT_MANIFEST:-}"
LONGTEXT_E2E="${ATTUNE_K3_LONGTEXT_E2E:-0}"
LONGTEXT_PROFILE="${ATTUNE_K3_LONGTEXT_PROFILE:-edge_scheduler_comprehensive}"
LONGTEXT_LIMIT_DOCS="${ATTUNE_K3_LONGTEXT_LIMIT_DOCS:-}"
LONGTEXT_CORPUS_DIR="${ATTUNE_K3_LONGTEXT_CORPUS_DIR:-}"
LONGTEXT_REMOTE_RUNNER="${ATTUNE_K3_LONGTEXT_REMOTE_RUNNER:-}"
LONGTEXT_REMOTE_MANIFEST="${ATTUNE_K3_LONGTEXT_REMOTE_MANIFEST:-}"
LONGTEXT_REMOTE_RESULTS="${ATTUNE_K3_LONGTEXT_REMOTE_RESULTS:-}"
LONGTEXT_REQUIRE_SCHEDULER_GENERATION="${ATTUNE_K3_LONGTEXT_REQUIRE_SCHEDULER_GENERATION:-0}"
LONGTEXT_PDF_OCR="${ATTUNE_K3_LONGTEXT_PDF_OCR:-0}"
AIRPLANE_LONGTEXT_REPO_URL="https://github.com/shiroinekotfs/airplane-manual-collection.git"
SERVER_SCHEDULER_BASE="${ATTUNE_K3_SERVER_SCHEDULER_BASE:-http://127.0.0.1:8090}"
SCHEDULER_CHAT_MODEL="${ATTUNE_K3_SCHEDULER_CHAT_MODEL:-llm-summary}"
CONFIGURE_SCHEDULER_AI="${ATTUNE_K3_CONFIGURE_SCHEDULER_AI:-}"
REQUIRE_SCHEDULER_CHAT="${ATTUNE_K3_REQUIRE_SCHEDULER_CHAT:-}"
CHAT_JOB_TIMEOUT="${ATTUNE_K3_CHAT_JOB_TIMEOUT:-60}"
RVV_REQUIRE_PERF="${ATTUNE_K3_RVV_REQUIRE_PERF:-0}"
API_CONTRACT="${ATTUNE_K3_API_CONTRACT:-1}"
API_CONTRACT_BIND_DIR="${ATTUNE_K3_API_CONTRACT_BIND_DIR:-}"
SKIP_DEB_CHECK=0
SKIP_INSTALL=0
SKIP_UI=0
SKIP_RVV_PERFORMANCE=0
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
    --skip-rvv-performance)
      SKIP_RVV_PERFORMANCE=1
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
  --skip-rvv-performance Skip scheduler RVV/IME runtime performance gate.
  --dry-run              Write planned report without touching the target.

Environment:
  ATTUNE_K3_RVV_REQUIRE_PERF=1  Make scheduler latency thresholds block this Attune demo.
                                Defaults to 0 because scheduler performance is validated by scheduler.
  ATTUNE_K3_SERVER_SCHEDULER_BASE
                                Scheduler URL as seen by attune-server on the NAS host.
                                Usually http://127.0.0.1:8090 when scheduler is co-located.
  ATTUNE_K3_SCHEDULER_URL       Scheduler URL as seen by this CI runner.
                                Use a runner-side SSH tunnel when scheduler is loopback-only.
  ATTUNE_K3_SSH_OPTS            Extra ssh/scp options. Defaults to disabling runner
                                known_hosts writes/checks for disposable K3 CI targets.
  ATTUNE_K3_SSH_PASSWORD        Password for non-interactive SSH/scp via sshpass -e.
                                Omit when key-based SSH is configured.
  ATTUNE_K3_API_CONTRACT=0      Skip the strict NAS Web API contract probe.
  ATTUNE_K3_LONGTEXT_E2E=1      Run the full airplane GitHub long-text API gate
                                on the NAS host before the optional UI gate.
  ATTUNE_K3_LONGTEXT_PROFILE    Long-text profile. Defaults to edge_scheduler_comprehensive.
  ATTUNE_K3_LONGTEXT_REQUIRE_SCHEDULER_GENERATION=1
                                Require every non-safety long-text chat row to use
                                scheduler answer generation. Defaults to 0; scheduler
                                generation coverage is reported but does not block Attune.
  ATTUNE_K3_LONGTEXT_PDF_OCR=1  Keep server-side PDF OCR enabled for the airplane long-text
                                bind. Defaults to 0 so scanned manuals degrade to metadata
                                instead of blocking vector/search/chat validation.
  ATTUNE_K3_LONGTEXT_MANIFEST   Local JSON manifest for the optional long-text UI gate.
                                The corpus must already be materialized and indexed
                                on the NAS host before the UI-only gate runs.
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
if [ -z "$REMOTE_TMP" ]; then
  if [ "$SSH_USER" = "root" ]; then
    REMOTE_TMP="/root/attune-k3-release"
  else
    REMOTE_TMP="/home/$SSH_USER/attune-k3-release"
  fi
fi
if [ -z "$BIND_DIR" ]; then
  BIND_DIR="$REMOTE_TMP/background-bind-smoke"
fi
if [ -z "$API_CONTRACT_BIND_DIR" ]; then
  API_CONTRACT_BIND_DIR="$REMOTE_TMP/api-contract-bind"
fi
if [ -z "$LONGTEXT_CORPUS_DIR" ]; then
  LONGTEXT_CORPUS_DIR="$REMOTE_TMP/airplane-manual-collection"
fi
if [ -z "$LONGTEXT_REMOTE_RUNNER" ]; then
  LONGTEXT_REMOTE_RUNNER="$REMOTE_TMP/airplane-longtext-runner"
fi
if [ -z "$LONGTEXT_REMOTE_MANIFEST" ]; then
  LONGTEXT_REMOTE_MANIFEST="$REMOTE_TMP/attune-airplane-longtext-$LONGTEXT_PROFILE.json"
fi
if [ -z "$LONGTEXT_REMOTE_RESULTS" ]; then
  LONGTEXT_REMOTE_RESULTS="$REMOTE_TMP/airplane-longtext-results"
fi
if [ -z "$CONFIGURE_SCHEDULER_AI" ]; then
  if [ -n "$SCHEDULER_URL" ]; then
    CONFIGURE_SCHEDULER_AI=1
  else
    CONFIGURE_SCHEDULER_AI=0
  fi
fi
if [ -z "$REQUIRE_SCHEDULER_CHAT" ]; then
  REQUIRE_SCHEDULER_CHAT="$CONFIGURE_SCHEDULER_AI"
fi
case "$CONFIGURE_SCHEDULER_AI" in
  0|1) ;;
  *) echo "ATTUNE_K3_CONFIGURE_SCHEDULER_AI must be 0 or 1, got: $CONFIGURE_SCHEDULER_AI" >&2; exit 2 ;;
esac
case "$REQUIRE_SCHEDULER_CHAT" in
  0|1) ;;
  *) echo "ATTUNE_K3_REQUIRE_SCHEDULER_CHAT must be 0 or 1, got: $REQUIRE_SCHEDULER_CHAT" >&2; exit 2 ;;
esac
case "$RVV_REQUIRE_PERF" in
  0|1) ;;
  *) echo "ATTUNE_K3_RVV_REQUIRE_PERF must be 0 or 1, got: $RVV_REQUIRE_PERF" >&2; exit 2 ;;
esac
case "$API_CONTRACT" in
  0|1) ;;
  *) echo "ATTUNE_K3_API_CONTRACT must be 0 or 1, got: $API_CONTRACT" >&2; exit 2 ;;
esac
case "$LONGTEXT_E2E" in
  0|1) ;;
  *) echo "ATTUNE_K3_LONGTEXT_E2E must be 0 or 1, got: $LONGTEXT_E2E" >&2; exit 2 ;;
esac
case "$LONGTEXT_REQUIRE_SCHEDULER_GENERATION" in
  0|1) ;;
  *) echo "ATTUNE_K3_LONGTEXT_REQUIRE_SCHEDULER_GENERATION must be 0 or 1, got: $LONGTEXT_REQUIRE_SCHEDULER_GENERATION" >&2; exit 2 ;;
esac
case "$LONGTEXT_PDF_OCR" in
  0|1) ;;
  *) echo "ATTUNE_K3_LONGTEXT_PDF_OCR must be 0 or 1, got: $LONGTEXT_PDF_OCR" >&2; exit 2 ;;
esac

mkdir -p "$REPORTS_DIR"
TS="$(date +%Y%m%d_%H%M%S)"
if [ "$DRY_RUN" = "1" ]; then
  REPORT="$REPORTS_DIR/k3-nas-web-demo-dry-run.md"
else
  REPORT="$REPORTS_DIR/k3-nas-web-demo-$TS.md"
fi

SSH_TARGET="$SSH_USER@$HOST"
SSH_OPTS=()
if [ -n "$SSH_OPTS_RAW" ]; then
  read -r -a SSH_OPTS <<< "$SSH_OPTS_RAW"
fi

if [ -n "$SSH_PASSWORD" ] && [ "$DRY_RUN" != "1" ] && ! command -v sshpass >/dev/null 2>&1; then
  echo "ATTUNE_K3_SSH_PASSWORD requires sshpass on the CI runner" >&2
  exit 2
fi

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
    echo "- SSH opts: ${SSH_OPTS_RAW:-<none>}"
    if [ -n "$SSH_PASSWORD" ]; then
      echo "- SSH auth: password via sshpass -e"
    else
      echo "- SSH auth: default ssh client credentials"
    fi
    echo "- Base URL: ${BASE_URL:-<none>}"
    echo "- Scheduler URL: ${SCHEDULER_URL:-<none>}"
    echo "- Server scheduler base: $SERVER_SCHEDULER_BASE"
    echo "- Scheduler chat model: $SCHEDULER_CHAT_MODEL"
    echo "- Remote tmp: $REMOTE_TMP"
    echo "- Server-side bind dir: $BIND_DIR"
    echo "- API contract bind dir: $API_CONTRACT_BIND_DIR"
    echo "- Configure scheduler AI: $CONFIGURE_SCHEDULER_AI"
    echo "- Require scheduler chat metadata: $REQUIRE_SCHEDULER_CHAT"
    echo "- Require scheduler performance thresholds: $RVV_REQUIRE_PERF"
    echo "- Run NAS Web API contract: $API_CONTRACT"
    echo "- Long-text manifest: ${LONGTEXT_MANIFEST:-<none>}"
    echo "- Long-text E2E: $LONGTEXT_E2E"
    echo "- Airplane long-text repo: $AIRPLANE_LONGTEXT_REPO_URL"
    echo "- Long-text profile: $LONGTEXT_PROFILE"
    echo "- Long-text corpus dir: $LONGTEXT_CORPUS_DIR"
    echo "- Long-text remote runner: $LONGTEXT_REMOTE_RUNNER"
    echo "- Long-text remote manifest: $LONGTEXT_REMOTE_MANIFEST"
    echo "- Long-text remote results: $LONGTEXT_REMOTE_RESULTS"
    echo "- Require long-text scheduler generation: $LONGTEXT_REQUIRE_SCHEDULER_GENERATION"
    echo "- Long-text PDF OCR guard: ATTUNE_K3_LONGTEXT_PDF_OCR=$LONGTEXT_PDF_OCR"
    echo "- Skip RVV performance gate: $SKIP_RVV_PERFORMANCE"
    echo "- Dry run: $DRY_RUN"
    echo
    echo "## Package Boundary"
    echo
    echo "This validation expects Attune to provide NAS Web/API/control-plane behavior. ORT, Sherpa, model weights, and inference runtimes are scheduler package responsibilities."
    echo
    echo "## Remote CI topology"
    echo
    echo "- Base URL is reached by the CI runner/browser."
    echo "- Scheduler URL is reached by the CI runner; use an SSH tunnel for loopback-only scheduler endpoints."
    echo "- Server scheduler base is persisted into Attune settings and is reached by attune-server on the NAS host."
    echo "- Bind directories are server-side paths on the NAS host, never runner-local paths."
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

run_display() {
  local display="$1"
  shift
  log "+ $display"
  {
    echo
    echo '```bash'
    echo "$display"
    echo '```'
  } >> "$REPORT"
  if [ "$DRY_RUN" != "1" ]; then
    "$@"
  fi
}

run_ssh() {
  local cmd="$1"
  if [ -n "$SSH_PASSWORD" ]; then
    run_display \
      "sshpass -e ssh ${SSH_OPTS_RAW:-} $SSH_TARGET $(printf '%q' "$cmd")" \
      env SSHPASS="$SSH_PASSWORD" sshpass -e ssh "${SSH_OPTS[@]}" "$SSH_TARGET" "$cmd"
  else
    run ssh "${SSH_OPTS[@]}" "$SSH_TARGET" "$cmd"
  fi
}

run_scp() {
  if [ -n "$SSH_PASSWORD" ]; then
    local rendered=()
    local arg
    for arg in "$@"; do
      rendered+=("$(printf '%q' "$arg")")
    done
    run_display \
      "sshpass -e scp ${SSH_OPTS_RAW:-} ${rendered[*]}" \
      env SSHPASS="$SSH_PASSWORD" sshpass -e scp "${SSH_OPTS[@]}" "$@"
  else
    run scp "${SSH_OPTS[@]}" "$@"
  fi
}

remote() {
  local cmd="$1"
  run_ssh "$cmd"
}

configure_longtext_pdf_ocr_guard() {
  if [ "$LONGTEXT_E2E" != "1" ]; then
    return
  fi
  if [ -z "$HOST" ]; then
    return
  fi

  append_report "## Long-text PDF OCR Guard"
  if [ "$LONGTEXT_PDF_OCR" = "1" ]; then
    append_report "Server-side PDF OCR remains enabled for airplane long-text bind."
    remote "rm -f /etc/systemd/system/attune-server.service.d/90-attune-k3-longtext-pdf-ocr.conf && systemctl daemon-reload && systemctl restart attune-server.service && systemctl is-active attune-server.service"
    return
  fi

  append_report "Server-side PDF OCR disabled for airplane long-text bind (ATTUNE_K3_LONGTEXT_PDF_OCR=0). This keeps the Attune vector/search/chat gate focused on indexing and retrieval; OCR-specific coverage belongs to scheduler/attune OCR gates."
  remote "mkdir -p /etc/systemd/system/attune-server.service.d && printf '%s\n' '[Service]' 'Environment=ATTUNE_SCHEDULER_PDF_OCR_ENABLED=0' 'Environment=ATTUNE_LOCAL_SCHEDULER_PDF_OCR_ENABLED=0' 'Environment=ATTUNE_PDF_OCR_ENABLED=0' > /etc/systemd/system/attune-server.service.d/90-attune-k3-longtext-pdf-ocr.conf && systemctl daemon-reload && systemctl restart attune-server.service && systemctl is-active attune-server.service"
}

report_header

if [ "$DRY_RUN" = "1" ]; then
  append_report "## Planned Gates"
  append_report "- Check .deb architecture is riscv64 unless --skip-deb-check is set."
  append_report "- Install package on K3/NAS over SSH unless --skip-install is set."
  append_report "- Restart attune-server.service and check Web health."
  append_report "- Probe scheduler contract when scheduler URL is provided."
  append_report "- K3 RVV Runtime Performance Gate: run worker_benchmark_gate.py and require scheduler RVV/IME metadata when scheduler URL is provided; live scheduler latency thresholds block only when ATTUNE_K3_RVV_REQUIRE_PERF=1."
  append_report "- Configure Attune scheduler-native AI settings when scheduler URL is provided."
  append_report "- NAS Web API Contract Gate: probe health, vault, settings, scheduler config, UI read endpoints, upload, server-side index bind/search, embedding/vector queue drain, export, and chat scheduler metadata."
  append_report "- Long-text PDF OCR guard: default ATTUNE_K3_LONGTEXT_PDF_OCR=0 restarts attune-server with PDF OCR disabled before airplane bind; set ATTUNE_K3_LONGTEXT_PDF_OCR=1 only for OCR-specific validation."
  append_report "- Airplane GitHub Longtext Gate: when ATTUNE_K3_LONGTEXT_E2E=1, run tests/e2e/airplane_manual_longtext_e2e.py on the NAS host with source repo $AIRPLANE_LONGTEXT_REPO_URL, profile $LONGTEXT_PROFILE, materialized corpus under $LONGTEXT_CORPUS_DIR, then copy the generated manifest back for the optional UI gate; scheduler generation coverage is reported and only blocks when ATTUNE_K3_LONGTEXT_REQUIRE_SCHEDULER_GENERATION=1."
  append_report "- Use K3/NAS-local bind path for knowledge-base import."
  append_report "- Require local scheduler chat metadata and poll async answer jobs when scheduler chat is required."
  append_report "- Run optional Playwright UI gate when ATTUNE_K3_LONGTEXT_MANIFEST points to a local long-text manifest and the NAS-side corpus is already indexed."
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
  run_ssh "uname -m && sed -n '1,8p' /etc/os-release"
  run_scp "$DEB" "$SSH_TARGET:$REMOTE_TMP/"
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
append_report "- Diagnostics is authenticated and is checked after vault unlock."

if [ -n "$SCHEDULER_URL" ]; then
  append_report "## Scheduler Contract"
  run python3 "$ROOT/scripts/probe-edge-scheduler-contract.py" --base-url "$SCHEDULER_URL" --strict
  append_report "## K3 RVV Runtime Performance Gate"
  if [ "$SKIP_RVV_PERFORMANCE" = "1" ]; then
    append_report "Skipped by --skip-rvv-performance."
  else
    RVV_GATE_ARGS=(
      "$ROOT/scripts/release/test-k3-rvv-runtime-gate.sh"
      --scheduler-url "$SCHEDULER_URL" \
      --reports-dir "$REPORTS_DIR"
    )
    if [ "$RVV_REQUIRE_PERF" = "1" ]; then
      RVV_GATE_ARGS+=(--require-perf)
    else
      RVV_GATE_ARGS+=(--no-require-perf)
    fi
    run bash "${RVV_GATE_ARGS[@]}"
  fi
else
  append_report "## Scheduler Contract"
  append_report "Skipped because no scheduler URL was provided."
  append_report "## K3 RVV Runtime Performance Gate"
  append_report "Skipped because no scheduler URL was provided. worker_benchmark_gate.py will run when ATTUNE_K3_SCHEDULER_URL or --scheduler-url is set."
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

append_report "## Scheduler-backed AI Settings Gate"
if [ "$CONFIGURE_SCHEDULER_AI" = "1" ]; then
  python3 - "$BASE_URL" "$TOKEN" "$SERVER_SCHEDULER_BASE" "$SCHEDULER_CHAT_MODEL" <<'PY' | tee -a "$REPORT"
import json
import sys
import urllib.request

base, token, scheduler_base, chat_model = sys.argv[1:5]
base = base.rstrip("/")
scheduler_base = scheduler_base.rstrip("/")
if scheduler_base.endswith("/v1"):
    scheduler_base = scheduler_base[:-3].rstrip("/")
if not scheduler_base:
    raise SystemExit("server scheduler base is empty")
if not chat_model.strip():
    raise SystemExit("scheduler chat model is empty")

body = {
    "llm": {
        "provider": "local_scheduler",
        "endpoint": scheduler_base,
        "model": chat_model,
        "api_key": "local-scheduler",
    },
    "embedding": {
        "provider": "local_scheduler",
        "endpoint": scheduler_base,
        "model": "embedding-int8",
        "task": "kb.query.embed",
        "dims": 512,
    },
}
req = urllib.request.Request(
    base + "/api/v1/settings",
    data=json.dumps(body).encode(),
    headers={"Content-Type": "application/json", "Authorization": f"Bearer {token}"},
    method="PATCH",
)
with urllib.request.urlopen(req, timeout=60) as resp:
    data = json.loads(resp.read().decode())

llm = data.get("llm") if isinstance(data, dict) else None
embedding = data.get("embedding") if isinstance(data, dict) else None
if not isinstance(llm, dict) or llm.get("provider") != "local_scheduler":
    raise SystemExit(f"llm settings did not switch to local_scheduler: {llm}")
if llm.get("endpoint") != scheduler_base:
    raise SystemExit(f"llm endpoint mismatch: {llm.get('endpoint')} != {scheduler_base}")
if llm.get("model") != chat_model:
    raise SystemExit(f"llm model mismatch: {llm.get('model')} != {chat_model}")
if not isinstance(embedding, dict) or embedding.get("provider") != "local_scheduler":
    raise SystemExit(f"embedding settings did not switch to local_scheduler: {embedding}")
print(
    "scheduler ai settings status=200 "
    f"llm_provider={llm.get('provider')} "
    f"llm_endpoint={llm.get('endpoint')} "
    f"llm_model={llm.get('model')} "
    f"embedding_model={embedding.get('model')}"
)
PY
else
  append_report "Skipped because ATTUNE_K3_CONFIGURE_SCHEDULER_AI=0."
fi

python3 - "$BASE_URL" "$TOKEN" <<'PY' | tee -a "$REPORT"
import json
import sys
import urllib.request

base, token = sys.argv[1].rstrip("/"), sys.argv[2]
req = urllib.request.Request(
    base + "/api/v1/status/diagnostics",
    headers={"Authorization": f"Bearer {token}"},
    method="GET",
)
with urllib.request.urlopen(req, timeout=60) as resp:
    data = json.loads(resp.read().decode())
print(f"diagnostics status=200 keys={sorted(data.keys())}")
PY

append_report "## NAS Web API Contract Gate"
if [ "$API_CONTRACT" = "1" ]; then
  API_CONTRACT_JSON="$REPORTS_DIR/k3-nas-web-api-contract-$TS.json"
  if [ -n "$HOST" ]; then
    remote "rm -rf '$API_CONTRACT_BIND_DIR' && mkdir -p '$API_CONTRACT_BIND_DIR' && printf '# Attune NAS Web API contract\n\nattune-nas-web-api-bind-token\n' > '$API_CONTRACT_BIND_DIR/nas-web-api-contract.md'"
  else
    rm -rf "$API_CONTRACT_BIND_DIR"
    mkdir -p "$API_CONTRACT_BIND_DIR"
    printf '# Attune NAS Web API contract\n\nattune-nas-web-api-bind-token\n' > "$API_CONTRACT_BIND_DIR/nas-web-api-contract.md"
  fi
  API_CONTRACT_ARGS=(
    "$ROOT/scripts/release/probe-nas-web-api-contract.py"
    --base-url "$BASE_URL"
    --password "$PASSWORD"
    --token "$TOKEN"
    --bind-dir "$API_CONTRACT_BIND_DIR"
    --server-scheduler-base "$SERVER_SCHEDULER_BASE"
    --scheduler-chat-model "$SCHEDULER_CHAT_MODEL"
    --job-timeout "$CHAT_JOB_TIMEOUT"
    --out "$API_CONTRACT_JSON"
  )
  if [ -n "$SCHEDULER_URL" ]; then
    API_CONTRACT_ARGS+=(--scheduler-url "$SCHEDULER_URL")
  fi
  if [ "$REQUIRE_SCHEDULER_CHAT" = "1" ]; then
    API_CONTRACT_ARGS+=(--require-scheduler-chat)
  fi
  run python3 "${API_CONTRACT_ARGS[@]}"
  append_report "- API contract JSON: $API_CONTRACT_JSON"
else
  append_report "Skipped because ATTUNE_K3_API_CONTRACT=0."
fi

if [ -n "$HOST" ]; then
  remote "rm -rf '$BIND_DIR' && mkdir -p '$BIND_DIR' && printf '# Attune K3 NAS Web gate\n\nattune-k3-nas-web-bind-token\n' > '$BIND_DIR/k3-nas-web-gate.md'"
else
  rm -rf "$BIND_DIR"
  mkdir -p "$BIND_DIR"
  printf '# Attune K3 NAS Web gate\n\nattune-k3-nas-web-bind-token\n' > "$BIND_DIR/k3-nas-web-gate.md"
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
python3 - "$BASE_URL" "$TOKEN" "$REQUIRE_SCHEDULER_CHAT" "$CHAT_JOB_TIMEOUT" "${SCHEDULER_URL:-}" <<'PY' | tee -a "$REPORT"
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

base, token, require_scheduler_s, job_timeout_s, scheduler_url = sys.argv[1:6]
base = base.rstrip("/")
require_scheduler = require_scheduler_s == "1"
job_timeout = float(job_timeout_s)
scheduler_url = scheduler_url.rstrip("/")


def request_json(url: str, *, method: str = "GET", body=None, timeout: float = 180):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Authorization": f"Bearer {token}"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode()
            return resp.status, json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode(errors="replace")
        try:
            payload = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            payload = {"raw": raw[:1000]}
        raise SystemExit(f"{method} {url} failed status={exc.code} body={payload}") from exc


def poll_scheduler_job(job_id: str):
    quoted = urllib.parse.quote(job_id, safe="")
    candidates = [base + f"/api/v1/chat/local-scheduler/jobs/{quoted}"]
    if scheduler_url:
        candidates.append(scheduler_url + f"/jobs/{quoted}")
    deadline = time.monotonic() + job_timeout
    last = None
    while time.monotonic() < deadline:
        for url in candidates:
            try:
                _, payload = request_json(url, timeout=30)
            except SystemExit as exc:
                last = str(exc)
                continue
            job = payload.get("job") if isinstance(payload, dict) else None
            if not isinstance(job, dict):
                job = payload if isinstance(payload, dict) else {}
            status = str(job.get("status") or job.get("phase") or "").lower()
            last = job
            if status in {"done", "failed", "cancelled", "canceled", "expired"}:
                return job
        time.sleep(1)
    raise SystemExit(f"local scheduler chat job did not finish within {job_timeout}s: {last}")


body = {"message": "用一句话说明 attune-k3-nas-web-bind-token 这个测试文档是否在知识库里。"}
_, data = request_json(base + "/api/v1/chat", method="POST", body=body, timeout=180)
print(f"chat status=200 keys={sorted(data.keys())}")
text = json.dumps(data, ensure_ascii=False)
answer = data.get("answer") or data.get("content") or ""
if "error" in data and not answer:
    raise SystemExit(f"chat returned error without answer: {data}")
if not isinstance(answer, str) or not answer.strip():
    raise SystemExit(f"chat returned empty answer/content: {data}")
local_scheduler = data.get("local_scheduler")
if require_scheduler:
    if not isinstance(local_scheduler, dict):
        raise SystemExit(f"chat did not return local_scheduler metadata: {data}")
    task = local_scheduler.get("task")
    if task not in {"kb.query.ask", "local.extractive.answer", "local.safety.refusal"}:
        raise SystemExit(f"unexpected local scheduler chat task: {task}")
    print(
        "local_scheduler_chat "
        f"task={task} "
        f"scheduled_as={local_scheduler.get('scheduled_as')} "
        f"status={local_scheduler.get('status')} "
        f"job_id={local_scheduler.get('job_id')}"
    )
    job_id = local_scheduler.get("job_id")
    if isinstance(job_id, str) and job_id:
        job = poll_scheduler_job(job_id)
        status = str(job.get("status") or job.get("phase") or "").lower()
        if status != "done":
            raise SystemExit(f"local scheduler chat job ended without done status: {job}")
        print(
            "local_scheduler_chat_job "
            f"status={status} "
            f"model={job.get('model')} "
            f"latency_ms={job.get('latency_ms')} "
            f"queue_wait_ms={job.get('queue_wait_ms')} "
            f"cache_hit={job.get('cache_hit')}"
        )
print(text[:500])
PY

LONGTEXT_UI_MANIFEST="$LONGTEXT_MANIFEST"
append_report "## Airplane GitHub Longtext Gate"
if [ "$LONGTEXT_E2E" = "1" ]; then
  if [ -z "$HOST" ]; then
    echo "--host or ATTUNE_K3_HOST is required for ATTUNE_K3_LONGTEXT_E2E=1" >&2
    exit 2
  fi
  configure_longtext_pdf_ocr_guard
  LONGTEXT_LOCAL_MANIFEST="$REPORTS_DIR/k3-airplane-longtext-$LONGTEXT_PROFILE-$TS.json"
  LONGTEXT_RESULTS_TGZ="$REPORTS_DIR/k3-airplane-longtext-results-$LONGTEXT_PROFILE-$TS.tgz"
  remote "command -v git >/dev/null || { echo 'git is required on the NAS host for airplane GitHub long-text materialization' >&2; exit 2; }"
  remote "mkdir -p '$LONGTEXT_REMOTE_RUNNER/scripts' '$LONGTEXT_REMOTE_RUNNER/tests/e2e' '$LONGTEXT_REMOTE_RESULTS'"
  run_scp \
    "$ROOT/scripts/build-airplane-manual-longtext-dataset.py" \
    "$ROOT/scripts/eval-airplane-manual-longtext-search.py" \
    "$ROOT/scripts/eval-airplane-manual-longtext-chat.py" \
    "$ROOT/scripts/eval-airplane-manual-longtext-multiturn.py" \
    "$SSH_TARGET:$LONGTEXT_REMOTE_RUNNER/scripts/"
  run_scp \
    "$ROOT/tests/e2e/airplane_longtext_support.py" \
    "$ROOT/tests/e2e/airplane_manual_longtext_e2e.py" \
    "$SSH_TARGET:$LONGTEXT_REMOTE_RUNNER/tests/e2e/"
  LONGTEXT_REMOTE_CMD="ATTUNE_BASE_URL='http://127.0.0.1:18900' ATTUNE_E2E_PASSWORD='$PASSWORD' ATTUNE_LONGTEXT_PROFILE='$LONGTEXT_PROFILE' ATTUNE_LONGTEXT_CORPUS_DIR='$LONGTEXT_CORPUS_DIR' ATTUNE_LONGTEXT_MANIFEST='$LONGTEXT_REMOTE_MANIFEST' ATTUNE_LONGTEXT_GOLDEN='$LONGTEXT_REMOTE_RESULTS/attune-airplane-longtext-$LONGTEXT_PROFILE-golden.json' ATTUNE_LONGTEXT_RESULTS_DIR='$LONGTEXT_REMOTE_RESULTS' ATTUNE_LONGTEXT_UI=0 ATTUNE_LONGTEXT_FAIL_ON_TARGETS=1 ATTUNE_LONGTEXT_BACKGROUND_BIND_UX=1 ATTUNE_LONGTEXT_MULTITURN=1 ATTUNE_LONGTEXT_REQUIRE_SCHEDULER_GENERATION='$LONGTEXT_REQUIRE_SCHEDULER_GENERATION'"
  if [ -n "$LONGTEXT_LIMIT_DOCS" ]; then
    LONGTEXT_REMOTE_CMD="$LONGTEXT_REMOTE_CMD ATTUNE_LONGTEXT_LIMIT_DOCS='$LONGTEXT_LIMIT_DOCS'"
  fi
  remote "$LONGTEXT_REMOTE_CMD python3 '$LONGTEXT_REMOTE_RUNNER/tests/e2e/airplane_manual_longtext_e2e.py'"
  run_scp "$SSH_TARGET:$LONGTEXT_REMOTE_MANIFEST" "$LONGTEXT_LOCAL_MANIFEST"
  remote "tar -C '$LONGTEXT_REMOTE_RESULTS' -czf '$REMOTE_TMP/airplane-longtext-results-$LONGTEXT_PROFILE-$TS.tgz' ."
  run_scp "$SSH_TARGET:$REMOTE_TMP/airplane-longtext-results-$LONGTEXT_PROFILE-$TS.tgz" "$LONGTEXT_RESULTS_TGZ"
  LONGTEXT_UI_MANIFEST="$LONGTEXT_LOCAL_MANIFEST"
  append_report "- Airplane GitHub repo: $AIRPLANE_LONGTEXT_REPO_URL"
  append_report "- Long-text profile: $LONGTEXT_PROFILE"
  append_report "- Local manifest copy: $LONGTEXT_LOCAL_MANIFEST"
  append_report "- Results archive: $LONGTEXT_RESULTS_TGZ"
  append_report "- Require scheduler generation: $LONGTEXT_REQUIRE_SCHEDULER_GENERATION"
else
  append_report "Skipped because ATTUNE_K3_LONGTEXT_E2E=0."
fi

append_report "## Optional UI Gate"
if [ "$SKIP_UI" = "1" ]; then
  append_report "Skipped by --skip-ui."
elif [ -n "$LONGTEXT_UI_MANIFEST" ] && [ -f "$LONGTEXT_UI_MANIFEST" ]; then
  run env \
      "ATTUNE_HEADLESS=${ATTUNE_HEADLESS:-0}" \
      "ATTUNE_BASE_URL=$BASE_URL" \
      "ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_CREATE=0" \
      "ATTUNE_LONGTEXT_UI_BACKGROUND_BIND_DIR=$BIND_DIR" \
      python3 "$ROOT/tests/e2e/playwright/airplane_manual_longtext_ui_e2e.py" \
      --manifest "$LONGTEXT_UI_MANIFEST" \
      --base-url "$BASE_URL" \
      --profile "$LONGTEXT_PROFILE" \
      --background-bind-create 0 \
      --background-bind-dir "$BIND_DIR"
else
  append_report "Skipped because ATTUNE_K3_LONGTEXT_MANIFEST is not set to a local manifest file and ATTUNE_K3_LONGTEXT_E2E did not generate one."
fi

append_report "## Result"
append_report "K3/NAS Web demo validation complete."
log "report: $REPORT"
