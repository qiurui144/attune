#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BASELINE="$(mktemp -t attune-eval-baseline-XXXXXX.json)"
CANDIDATE_GOOD="$(mktemp -t attune-eval-candidate-good-XXXXXX.json)"
CANDIDATE_BAD="$(mktemp -t attune-eval-candidate-bad-XXXXXX.json)"
OUT="$(mktemp -t attune-eval-diff-XXXXXX.json)"

cat > "$BASELINE" <<'JSON'
{
  "schema_version": "attune.eval.report.v1",
  "suite_id": "pr_rag_smoke",
  "summary": {"pass": true, "cases": 10, "failures": 0, "terminal_error_rate": 0.0},
  "metrics": {
    "retrieval": {"hit_at_5": 0.90},
    "answer": {"citation_hit_rate": 0.92, "answer_accuracy": 0.85},
    "performance": {"chat_p95_ms": 20000},
    "stability": {}
  },
  "failures": []
}
JSON

cat > "$CANDIDATE_GOOD" <<'JSON'
{
  "schema_version": "attune.eval.report.v1",
  "suite_id": "pr_rag_smoke",
  "summary": {"pass": true, "cases": 10, "failures": 0, "terminal_error_rate": 0.0},
  "metrics": {
    "retrieval": {"hit_at_5": 0.91},
    "answer": {"citation_hit_rate": 0.93, "answer_accuracy": 0.86},
    "performance": {"chat_p95_ms": 19000},
    "stability": {}
  },
  "failures": []
}
JSON

cat > "$CANDIDATE_BAD" <<'JSON'
{
  "schema_version": "attune.eval.report.v1",
  "suite_id": "pr_rag_smoke",
  "summary": {"pass": true, "cases": 10, "failures": 1, "terminal_error_rate": 0.1},
  "metrics": {
    "retrieval": {"hit_at_5": 0.80},
    "answer": {"citation_hit_rate": 0.75, "answer_accuracy": 0.70},
    "performance": {"chat_p95_ms": 35000},
    "stability": {}
  },
  "failures": [{"failure_layer": "retrieval"}]
}
JSON

python3 "$ROOT/scripts/eval/report-diff.py" \
  --baseline "$BASELINE" \
  --candidate "$CANDIDATE_GOOD" \
  --out "$OUT" \
  --fail-on-regression

if python3 "$ROOT/scripts/eval/report-diff.py" \
  --baseline "$BASELINE" \
  --candidate "$CANDIDATE_BAD" \
  --out "$OUT" \
  --fail-on-regression > /tmp/attune-eval-bad-diff.txt 2>&1; then
  echo "expected bad candidate to fail" >&2
  exit 1
fi

grep -q "hit_at_5" /tmp/attune-eval-bad-diff.txt
grep -q "citation_hit_rate" /tmp/attune-eval-bad-diff.txt
grep -q "chat_p95_ms" /tmp/attune-eval-bad-diff.txt

echo "eval report-diff regression contract PASS"
