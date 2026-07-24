#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORKFLOW="$ROOT/.github/workflows/ci.yml"

test -f "$WORKFLOW"
grep -q "RAG Eval Smoke" "$WORKFLOW"
grep -q "tests/scripts/eval_asset_registry_contract_test.sh" "$WORKFLOW"
grep -q "tests/scripts/eval_metric_system_contract_test.sh" "$WORKFLOW"
grep -q "tests/scripts/eval_single_industry_scale_contract_test.sh" "$WORKFLOW"
grep -q "tests/scripts/eval_web_demo_frontend_contract_test.sh" "$WORKFLOW"
grep -q "scripts/eval/validate-manifests.py --root" "$WORKFLOW"
grep -q "scripts/eval/run-suite.py --root" "$WORKFLOW"
grep -q "pr_rag_smoke" "$WORKFLOW"
grep -q -- "--dry-run" "$WORKFLOW"

echo "eval CI workflow contract PASS"
