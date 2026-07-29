#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCHEDULER_URL="${ATTUNE_K3_SCHEDULER_URL:-}"
REPORTS_DIR="$ROOT/reports/release"
SCHEDULER_ROOT="${ATTUNE_K3_SCHEDULER_ROOT:-/data/RV/k3-scheduler}"
TIMEOUT="${ATTUNE_K3_RVV_TIMEOUT:-10}"
REQUIRE_PERF="${ATTUNE_K3_RVV_REQUIRE_PERF:-1}"
SKIP_WORKER_GATE="${ATTUNE_K3_RVV_SKIP_WORKER_GATE:-0}"
DRY_RUN=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --scheduler-url)
      SCHEDULER_URL="${2:-}"
      shift 2
      ;;
    --reports-dir)
      REPORTS_DIR="${2:-}"
      shift 2
      ;;
    --scheduler-root)
      SCHEDULER_ROOT="${2:-}"
      shift 2
      ;;
    --timeout)
      TIMEOUT="${2:-}"
      shift 2
      ;;
    --require-perf)
      REQUIRE_PERF=1
      shift
      ;;
    --no-require-perf)
      REQUIRE_PERF=0
      shift
      ;;
    --skip-worker-gate)
      SKIP_WORKER_GATE=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      cat <<'HELP'
Validate K3 scheduler RVV/IME runtime performance evidence.

Usage:
  ATTUNE_K3_SCHEDULER_URL=http://<nas-ip>:8090 \
    bash scripts/release/test-k3-rvv-runtime-gate.sh

Options:
  --scheduler-url <url>  Scheduler base URL.
  --reports-dir <path>  Report output directory.
  --scheduler-root <p>   k3-scheduler checkout for worker_benchmark_gate.py.
  --timeout <seconds>    HTTP timeout. Defaults to 10.
  --no-require-perf      Only require acceleration metadata, not live latency evidence.
  --skip-worker-gate     Do not invoke k3-scheduler/tools/worker_benchmark_gate.py.
  --dry-run              Write planned report without network calls.

Environment thresholds:
  ATTUNE_K3_RVV_ACCELERATION_RE
  ATTUNE_K3_RVV_MAX_EMBED_P50_MS
  ATTUNE_K3_RVV_MAX_RERANK_P50_MS
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
REPORTS_DIR="$(cd "$REPORTS_DIR" && pwd -P)"
TS="$(date +%Y%m%d_%H%M%S)"
if [ "$DRY_RUN" = "1" ]; then
  REPORT="$REPORTS_DIR/k3-rvv-runtime-gate-dry-run.md"
  PERF_JSON="$REPORTS_DIR/k3-rvv-runtime-gate-dry-run.json"
  WORKER_JSON="$REPORTS_DIR/k3-worker-benchmark-gate-dry-run.json"
else
  REPORT="$REPORTS_DIR/k3-rvv-runtime-gate-$TS.md"
  PERF_JSON="$REPORTS_DIR/k3-rvv-runtime-gate-$TS.json"
  WORKER_JSON="$REPORTS_DIR/k3-worker-benchmark-gate-$TS.json"
fi
WORKER_GATE="$SCHEDULER_ROOT/tools/worker_benchmark_gate.py"

log() {
  printf '[k3-rvv-gate] %s\n' "$*"
}

scheduler_url_needs_loopback_hint() {
  case "$SCHEDULER_URL" in
    http://127.0.0.1:*|http://127.0.0.1/*|http://localhost:*|http://localhost/*) return 1 ;;
    http://*) return 0 ;;
    *) return 1 ;;
  esac
}

append_worker_loopback_hint() {
  if scheduler_url_needs_loopback_hint; then
    append_report "Worker gate diagnostic: scheduler direct /infer is commonly loopback-only; run this gate on the K3 host or expose it through an SSH tunnel whose base URL is 127.0.0.1 from the worker gate process."
  fi
}

append_report() {
  {
    echo
    echo "$@"
  } >> "$REPORT"
}

record_command() {
  {
    echo
    echo '```bash'
    printf '%q ' "$@"
    echo
    echo '```'
  } >> "$REPORT"
}

{
  echo "# K3 RVV Runtime Performance Gate"
  echo
  echo "- Timestamp: $(date -Iseconds)"
  echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "- Scheduler URL: ${SCHEDULER_URL:-<none>}"
  echo "- Scheduler root: $SCHEDULER_ROOT"
  echo "- Reports dir: $REPORTS_DIR"
  echo "- worker_benchmark_gate.py: $WORKER_GATE"
  echo "- Require performance evidence: $REQUIRE_PERF"
  echo "- Skip worker gate: $SKIP_WORKER_GATE"
  echo "- Dry run: $DRY_RUN"
  echo
  echo "This gate belongs to scheduler/runtime delivery. Attune uses it during K3/NAS Web validation so RVV/IME regressions are not hidden behind a successful Web package install."
} > "$REPORT"

if [ "$DRY_RUN" = "1" ]; then
  append_report "## Planned Gates"
  append_report "- Probe Attune-side scheduler contract with scripts/probe-edge-scheduler-contract.py."
  append_report "- Run k3-scheduler/tools/worker_benchmark_gate.py from cd \$SCHEDULER_ROOT so scheduler fixture paths resolve under the scheduler checkout."
  append_report "- Pass an absolute report path to worker_benchmark_gate.py so changing cwd does not redirect JSON output."
  append_worker_loopback_hint
  append_report "- Require scheduler /benchmark/contract, /models, or /capacity to advertise RVV/IME/SpacemiT acceleration metadata."
  append_report "- Require live p50/last latency evidence when ATTUNE_K3_RVV_REQUIRE_PERF=1."
  record_command python3 "$ROOT/scripts/probe-edge-scheduler-contract.py" --base-url "${SCHEDULER_URL:-http://<nas-ip>:8090}" --strict
  record_command bash -lc "cd \"\$SCHEDULER_ROOT\" && python3 tools/worker_benchmark_gate.py --base \"${SCHEDULER_URL:-http://<nas-ip>:8090}\" --out \"$WORKER_JSON\" --timeout \"$TIMEOUT\""
  log "dry-run report: $REPORT"
  exit 0
fi

case "$REQUIRE_PERF" in
  0|1) ;;
  *) echo "ATTUNE_K3_RVV_REQUIRE_PERF must be 0 or 1, got: $REQUIRE_PERF" >&2; exit 2 ;;
esac

if [ -z "$SCHEDULER_URL" ]; then
  echo "--scheduler-url or ATTUNE_K3_SCHEDULER_URL is required" >&2
  exit 2
fi

append_report "## Scheduler Contract Gate"
record_command python3 "$ROOT/scripts/probe-edge-scheduler-contract.py" --base-url "$SCHEDULER_URL" --strict
python3 "$ROOT/scripts/probe-edge-scheduler-contract.py" --base-url "$SCHEDULER_URL" --strict | tee -a "$REPORT"

append_report "## Scheduler Worker Benchmark Gate"
if [ "$SKIP_WORKER_GATE" = "1" ]; then
  append_report "Skipped by --skip-worker-gate."
elif [ -f "$WORKER_GATE" ]; then
  append_report "Worker gate cwd: $SCHEDULER_ROOT"
  append_report "Worker gate JSON output: $WORKER_JSON"
  record_command bash -lc "cd \"\$SCHEDULER_ROOT\" && python3 tools/worker_benchmark_gate.py --base \"$SCHEDULER_URL\" --out \"$WORKER_JSON\" --timeout \"$TIMEOUT\""
  set +e
  (
    cd "$SCHEDULER_ROOT"
    python3 tools/worker_benchmark_gate.py --base "$SCHEDULER_URL" --out "$WORKER_JSON" --timeout "$TIMEOUT"
  ) | tee -a "$REPORT"
  worker_status=${PIPESTATUS[0]}
  set -e
  if [ "$worker_status" -ne 0 ]; then
    append_worker_loopback_hint
    append_report "Worker gate failed with exit code $worker_status."
    exit "$worker_status"
  fi
else
  echo "missing scheduler worker benchmark gate: $WORKER_GATE" >&2
  exit 2
fi

append_report "## RVV/IME Metadata and Latency Gate"
record_command python3 - "$SCHEDULER_URL" "$TIMEOUT" "$REQUIRE_PERF" "$PERF_JSON"
python3 - "$SCHEDULER_URL" "$TIMEOUT" "$REQUIRE_PERF" "$PERF_JSON" <<'PY' | tee -a "$REPORT"
from __future__ import annotations

import json
import math
import os
import re
import sys
import urllib.error
import urllib.request
from typing import Any

base, timeout_s, require_perf_s, out_path = sys.argv[1:5]
base = base.rstrip("/")
timeout = float(timeout_s)
require_perf = require_perf_s == "1"
accel_re = re.compile(
    os.environ.get(
        "ATTUNE_K3_RVV_ACCELERATION_RE",
        r"rvv|ime|spacemit|x100|xsmtv|risc-v|riscv|SpaceMITExecutionProvider|spacemit-onnxruntime",
    ),
    re.IGNORECASE,
)
embed_max = float(os.environ.get("ATTUNE_K3_RVV_MAX_EMBED_P50_MS", "200"))
rerank_max = float(os.environ.get("ATTUNE_K3_RVV_MAX_RERANK_P50_MS", "300"))


def get_json(path: str) -> tuple[int, Any]:
    req = urllib.request.Request(base + path, method="GET", headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode(errors="replace")
            return resp.status, json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode(errors="replace")
        try:
            return exc.code, json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            return exc.code, {"raw": raw[:512]}


def flatten(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


def walk(value: Any, path: str = "$") -> list[tuple[str, dict[str, Any]]]:
    rows: list[tuple[str, dict[str, Any]]] = []
    if isinstance(value, dict):
        if any(key in value for key in ("last_latency_ms", "p50_latency_ms", "p99_latency_ms", "latency_ms")):
            rows.append((path, value))
        for key, child in value.items():
            rows.extend(walk(child, f"{path}.{key}"))
    elif isinstance(value, list):
        for idx, child in enumerate(value):
            rows.extend(walk(child, f"{path}[{idx}]"))
    return rows


payloads: dict[str, Any] = {}
statuses: dict[str, int] = {}
for path in ("/benchmark/contract", "/models", "/capacity", "/health", "/healthz"):
    status, payload = get_json(path)
    statuses[path] = status
    payloads[path] = payload

combined = flatten(payloads)
acceleration_metadata = bool(accel_re.search(combined))
latency_rows: list[dict[str, Any]] = []
for root, payload in payloads.items():
    for path, row in walk(payload, root):
        text = flatten(row)
        if not accel_re.search(text) and not re.search(r"embed|rerank|ocr|onnx|ort", text, re.IGNORECASE):
            continue
        latency: float | None = None
        latency_field = ""
        for field in ("p50_latency_ms", "last_latency_ms", "latency_ms"):
            raw = row.get(field)
            if isinstance(raw, (int, float)) and not isinstance(raw, bool) and math.isfinite(float(raw)) and raw >= 0:
                latency = float(raw)
                latency_field = field
                break
        if latency is None:
            continue
        latency_rows.append(
            {
                "path": path,
                "field": latency_field,
                "latency_ms": latency,
                "name": row.get("name") or row.get("model") or row.get("task"),
                "worker_kind": row.get("worker_kind"),
                "runtime_adapter": row.get("runtime_adapter"),
                "backend": row.get("backend"),
            }
        )

embed_rows = [row for row in latency_rows if re.search(r"embed", flatten(row), re.IGNORECASE)]
rerank_rows = [row for row in latency_rows if re.search(r"rerank", flatten(row), re.IGNORECASE)]
embed_ok = not embed_rows or min(row["latency_ms"] for row in embed_rows) <= embed_max
rerank_ok = not rerank_rows or min(row["latency_ms"] for row in rerank_rows) <= rerank_max
perf_evidence = bool(latency_rows)
pass_gate = all(200 <= status < 300 for status in statuses.values())
pass_gate = pass_gate and acceleration_metadata
if require_perf:
    pass_gate = pass_gate and perf_evidence and embed_ok and rerank_ok

report = {
    "base": base,
    "statuses": statuses,
    "acceleration_regex": accel_re.pattern,
    "acceleration_metadata": acceleration_metadata,
    "require_perf": require_perf,
    "perf_evidence": perf_evidence,
    "latency_rows": latency_rows[:20],
    "thresholds": {
        "embed_p50_or_last_ms_max": embed_max,
        "rerank_p50_or_last_ms_max": rerank_max,
        "embed_ok": embed_ok,
        "rerank_ok": rerank_ok,
    },
    "pass": pass_gate,
}
with open(out_path, "w", encoding="utf-8") as fh:
    json.dump(report, fh, ensure_ascii=False, indent=2)
print(json.dumps(report, ensure_ascii=False, indent=2))
raise SystemExit(0 if pass_gate else 3)
PY

append_report "## Result"
append_report "K3 RVV runtime performance gate complete."
log "report: $REPORT"
log "json: $PERF_JSON"
