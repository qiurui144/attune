#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$(mktemp -d -t attune-scale-corpus-XXXXXX)"

python3 "$ROOT/scripts/eval/generate-scale-corpus.py" \
  --documents 100 \
  --domains networking,security,product \
  --out "$OUT"

COUNT="$(find "$OUT" -type f -name '*.md' | wc -l | tr -d ' ')"
test "$COUNT" = "100"

grep -R "ATTUNE_SCALE_DOMAIN=networking" "$OUT" >/dev/null
grep -R "ATTUNE_SCALE_DOMAIN=security" "$OUT" >/dev/null
grep -R "ATTUNE_SCALE_DOMAIN=product" "$OUT" >/dev/null
grep -R "near-duplicate distractor" "$OUT" >/dev/null
grep -R "support workflow" "$OUT" >/dev/null

python3 "$ROOT/scripts/eval/validate-manifests.py" --root "$ROOT" --suite k3_rag_scale_thousand >/tmp/attune-scale-thousand.txt
grep -q "k3_rag_scale_thousand" /tmp/attune-scale-thousand.txt
grep -q "generated_thousand_docs" /tmp/attune-scale-thousand.txt

python3 "$ROOT/scripts/eval/validate-manifests.py" --root "$ROOT" --suite k3_rag_scale_ten_thousand >/tmp/attune-scale-ten-thousand.txt
grep -q "k3_rag_scale_ten_thousand" /tmp/attune-scale-ten-thousand.txt
grep -q "generated_ten_thousand_docs" /tmp/attune-scale-ten-thousand.txt

echo "eval scale corpus contract PASS"
