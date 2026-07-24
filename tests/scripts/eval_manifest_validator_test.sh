#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$(mktemp -t attune-eval-validator-XXXXXX.txt)"

python3 "$ROOT/scripts/eval/validate-manifests.py" \
  --root "$ROOT" \
  --suite pr_rag_smoke \
  --dry-run > "$OUT"

grep -q "schema_version" "$OUT"
grep -q "pr_rag_smoke" "$OUT"
grep -q "networking_tcpip_smoke" "$OUT"

python3 "$ROOT/scripts/eval/validate-manifests.py" \
  --root "$ROOT" \
  --suite k3_rag_release_smoke \
  --dry-run > "$OUT"

grep -q "k3_rag_release_smoke" "$OUT"
grep -q "airplane_manuals" "$OUT"
grep -q "mechanical_design_handbook" "$OUT"

TMP_ROOT="$(mktemp -d -t attune-eval-validator-invalid-XXXXXX)"
mkdir -p "$TMP_ROOT/tests"
cp -R "$ROOT/tests/eval" "$TMP_ROOT/tests/eval"
mkdir -p "$TMP_ROOT/scripts/eval"
cp "$ROOT/scripts/eval/validate-manifests.py" "$TMP_ROOT/scripts/eval/validate-manifests.py"
python3 - "$TMP_ROOT/tests/eval/suites/k3_rag_scale_thousand.json" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
data = json.loads(path.read_text(encoding="utf-8"))
data["corpora"] = ["generated_thousand_docs"]
data["scenarios"] = ["mixed_enterprise_support"]
path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
if python3 "$TMP_ROOT/scripts/eval/validate-manifests.py" \
  --root "$TMP_ROOT" \
  --suite k3_rag_scale_thousand \
  --dry-run > "$OUT" 2>&1; then
  echo "mixed_enterprise scale suite unexpectedly passed validation" >&2
  exit 1
fi
grep -q "single industry" "$OUT"

echo "eval manifest validator contract PASS"
