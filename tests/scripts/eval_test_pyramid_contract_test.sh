#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/test-pyramid.sh"

bash -n "$SCRIPT"

HELP="$(bash "$SCRIPT" --help)"
grep -q -- "--with-eval-smoke" <<<"$HELP"

grep -q "WITH_EVAL_SMOKE=false" "$SCRIPT"
grep -q -- "--with-eval-smoke" "$SCRIPT"
grep -q "Layer 6a: RAG Eval Smoke" "$SCRIPT"
grep -q "tests/scripts/eval_asset_registry_contract_test.sh" "$SCRIPT"
grep -q "tests/scripts/eval_metric_system_contract_test.sh" "$SCRIPT"
grep -q "tests/scripts/eval_single_industry_scale_contract_test.sh" "$SCRIPT"
grep -q "tests/scripts/eval_web_demo_frontend_contract_test.sh" "$SCRIPT"
grep -q "tests/scripts/eval_run_suite_generated_live_test.sh" "$SCRIPT"
grep -q "scripts/eval/validate-manifests.py --root" "$SCRIPT"
grep -q "scripts/eval/run-suite.py --root" "$SCRIPT"
grep -q "pr_rag_smoke" "$SCRIPT"
grep -q "eval_smoke" "$SCRIPT"
grep -q "With eval smoke:" "$SCRIPT"

echo "eval test-pyramid contract PASS"
