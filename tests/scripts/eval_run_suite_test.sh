#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$(mktemp -t attune-pr-rag-smoke-XXXXXX.json)"

python3 "$ROOT/scripts/eval/run-suite.py" \
  --root "$ROOT" \
  --suite pr_rag_smoke \
  --base-url http://127.0.0.1:18905 \
  --out "$OUT" \
  --dry-run

python3 - "$OUT" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
assert report["schema_version"] == "attune.eval.report.v1"
assert report["suite_id"] == "pr_rag_smoke"
assert report["summary"]["pass"] is True
assert report["summary"]["cases"] == 3
assert report["summary"]["failures"] == 0
assert report["target"]["base_url"] == "http://127.0.0.1:18905"
assert report["metrics"]["manifest"]["corpora"] == 1
assert report["metrics"]["manifest"]["scenarios"] == 2
assert report["metrics"]["manifest"]["turns"] == 3
assert any(
    row["scenario_id"] == "networking_tcpip_summary"
    and row["scenario_type"] == "summary"
    for row in report["resolved"]["scenarios"]
)
assert report["artifacts"]["mode"] == "dry_run"
PY

echo "eval run-suite dry-run contract PASS"
