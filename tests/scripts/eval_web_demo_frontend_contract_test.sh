#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEMO="$ROOT/kb-web-demo/index.html"
SCRIPT="$ROOT/tests/e2e/playwright/kb_web_demo_eval_frontend_e2e.py"

test -f "$DEMO"
test -f "$SCRIPT"
python3 -m py_compile "$SCRIPT"

grep -q "上传 & 管理" "$DEMO"
grep -q "向量库" "$DEMO"
grep -q "Chat RAG" "$DEMO"
grep -q "Summary RAG" "$DEMO"
grep -q "全流程时间" "$DEMO"
grep -q "向量块显示" "$DEMO"
grep -q "/api/v1/upload" "$DEMO"
grep -q "/api/v1/search" "$DEMO"
grep -q "/api/v1/chat" "$DEMO"
grep -q "18906/tcp" "$ROOT/kb-web-demo/start_k3.sh"

grep -q "web_demo_flow_pass_rate" "$SCRIPT"
grep -q "web_demo_citation_render_rate" "$SCRIPT"
grep -q "web_demo_time_render_rate" "$SCRIPT"
grep -q "web_demo_vector_chunk_render_rate" "$SCRIPT"
grep -q "upload" "$SCRIPT"
grep -q "vector" "$SCRIPT"
grep -q "chat" "$SCRIPT"
grep -q "summary" "$SCRIPT"

python3 "$SCRIPT" --dry-run --out /tmp/attune-kb-web-demo-eval-frontend-dry-run.json >/tmp/attune-kb-web-demo-eval-frontend-dry-run.txt
grep -q "kb-web-demo eval frontend dry-run PASS" /tmp/attune-kb-web-demo-eval-frontend-dry-run.txt

python3 "$SCRIPT" --dry-run --profile deep --out /tmp/attune-kb-web-demo-eval-frontend-deep-dry-run.json >/tmp/attune-kb-web-demo-eval-frontend-deep-dry-run.txt
python3 - /tmp/attune-kb-web-demo-eval-frontend-deep-dry-run.json <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
assert report["profile"] == "deep"
case_ids = {case["case_id"] for case in report["artifacts"]["chat_cases"]}
assert {
    "fact_origin",
    "operation_troubleshooting",
    "multi_intent_decision",
    "negative_evidence_boundary",
    "out_of_manual_industry_general",
    "summary_citation_coverage",
}.issubset(case_ids)
assert "web_demo_complex_chat_pass_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_complex_chat_pass_rate"] == 1.0
PY

echo "eval web-demo frontend contract PASS"
