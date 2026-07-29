#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d -t attune-longtext-corpora-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

python3 -m py_compile \
  "$ROOT/scripts/build-mechanical-design-longtext-dataset.py" \
  "$ROOT/scripts/eval-airplane-manual-longtext-multiturn.py" \
  "$ROOT/tests/e2e/mechanical_design_longtext_e2e.py" \
  "$ROOT/tests/e2e/longtext_corpora_e2e.py" \
  "$ROOT/scripts/eval-longtext-corpora-suite.py"

python3 "$ROOT/scripts/build-mechanical-design-longtext-dataset.py" \
  --repo-dir "$TMP/mechanical-design-repo" \
  --out "$TMP/mechanical-design-cases.json" \
  --golden-out "$TMP/mechanical-design-golden.json" \
  --no-github-api
test -f "$TMP/mechanical-design-cases.json"
test -f "$TMP/mechanical-design-golden.json"
grep -q "https://github.com/GEQfa/handbook-of-mechanical-design.git" "$TMP/mechanical-design-cases.json"
grep -q "86832fd643cb1f9cfa1188d242d34b62dd52e41f" "$TMP/mechanical-design-cases.json"
grep -q "mechanical_design_volume_1" "$TMP/mechanical-design-cases.json"
grep -q "mixed_difficulty_chat" "$TMP/mechanical-design-cases.json"
grep -q '"multiturn"' "$TMP/mechanical-design-cases.json"
grep -q "MECHANICAL_DESIGN_HANDBOOK_ROOT" "$TMP/mechanical-design-cases.json"
grep -q "mechanical-design-handbook" "$TMP/mechanical-design-golden.json"

ATTUNE_LONGTEXT_DRY_RUN=1 \
ATTUNE_LONGTEXT_UI=0 \
ATTUNE_LONGTEXT_CORPORA=airplane,mechanical_design \
ATTUNE_LONGTEXT_RESULTS_DIR="$TMP/results" \
  python3 "$ROOT/tests/e2e/longtext_corpora_e2e.py" > "$TMP/combined-dry-run.txt"
grep -q "airplane manual longtext E2E" "$TMP/combined-dry-run.txt"
grep -q "mechanical design handbook longtext E2E" "$TMP/combined-dry-run.txt"
grep -q "attune-airplane-longtext-edge_scheduler_comprehensive-chat.json" "$TMP/combined-dry-run.txt"
grep -q "attune-mechanical-design-longtext-edge_scheduler_comprehensive-chat.json" "$TMP/combined-dry-run.txt"

ATTUNE_LONGTEXT_DRY_RUN=1 \
ATTUNE_LONGTEXT_REPEAT_CHAT=2 \
ATTUNE_LONGTEXT_CORPORA=airplane,mechanical_design \
ATTUNE_LONGTEXT_RESULTS_DIR="$TMP/suite" \
  python3 "$ROOT/scripts/eval-longtext-corpora-suite.py" \
    --base-url http://127.0.0.1:18905 \
    --profile edge_scheduler_comprehensive \
    --out "$TMP/suite/summary.json" > "$TMP/suite-dry-run.txt"
grep -q "mechanical_design" "$TMP/suite-dry-run.txt"
grep -q "chat-repeat-02" "$TMP/suite-dry-run.txt"
grep -q "multiturn" "$TMP/suite-dry-run.txt"

bash "$ROOT/scripts/release/test-k3-nas-web-demo.sh" \
  --dry-run \
  --skip-deb-check \
  --deb "$TMP/fake.deb" \
  --base-url http://127.0.0.1:18900 \
  --reports-dir "$TMP/reports"
grep -q "Airplane GitHub Longtext Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Mechanical Design GitHub Longtext Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "https://github.com/shiroinekotfs/airplane-manual-collection.git" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "https://github.com/GEQfa/handbook-of-mechanical-design.git" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_LONGTEXT_CORPORA" "$TMP/reports/k3-nas-web-demo-dry-run.md"

test -f "$ROOT/docs/benchmarks/2026-07-20-longtext-corpora-e2e.md"
grep -q "handbook-of-mechanical-design" "$ROOT/docs/benchmarks/2026-07-20-longtext-corpora-e2e.md"
grep -q "ATTUNE_LONGTEXT_REPEAT_CHAT" "$ROOT/docs/benchmarks/2026-07-20-longtext-corpora-e2e.md"
grep -q "https://github.com/GEQfa/handbook-of-mechanical-design.git" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"

echo "longtext corpora script contracts PASS"
