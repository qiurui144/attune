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
grep -q "/api/v1/demo/reset" "$DEMO"
grep -q "/api/v1/voice/transcribe-file" "$DEMO"
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
}.issubset(case_ids)
summary_case_ids = {case["case_id"] for case in report["artifacts"]["summary_cases"]}
assert {
    "summary_recent_core",
    "summary_folder_overview",
    "summary_compare_sources",
    "summary_risk_gap",
}.issubset(summary_case_ids)
assert "web_demo_complex_chat_pass_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_complex_chat_pass_rate"] == 1.0
assert "web_demo_summary_workflow_pass_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_summary_workflow_pass_rate"] == 1.0
assert "web_demo_model_switch_gate_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_model_switch_gate_rate"] == 1.0
assert "web_demo_clear_reset_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_clear_reset_rate"] == 1.0
assert "web_demo_voice_file_transcribe_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_voice_file_transcribe_rate"] == 1.0
assert "web_demo_webrtc_voice_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_webrtc_voice_rate"] == 1.0
assert "web_demo_attune_only_network_rate" in report["metrics"]["frontend"]
assert report["metrics"]["frontend"]["web_demo_attune_only_network_rate"] == 1.0
assert "clear_reset" in report["checks"]
assert "voice_file_transcribe" in report["checks"]
assert "webrtc_voice" in report["checks"]
assert "attune_only_network" in report["checks"]
for case in report["artifacts"]["summary_cases"]:
    assert {"select", "map", "synthesize", "audit"}.issubset(set(case["required_stages"]))
PY

python3 "$SCRIPT" --dry-run --profile rtos --out /tmp/attune-kb-web-demo-eval-frontend-rtos-dry-run.json >/tmp/attune-kb-web-demo-eval-frontend-rtos-dry-run.txt
python3 - /tmp/attune-kb-web-demo-eval-frontend-rtos-dry-run.json <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
assert report["profile"] == "rtos"
assert report["artifacts"]["rtos_case"]["question"] == "rtos开发中如何在ccu开发中查询时钟的type和id"
assert report["artifacts"]["rtos_case"]["source_file"] == "RTOS_CCU_开发指南.pdf"
assert report["artifacts"]["rtos_dmac_case"]["question"] == "给我rtos中dmac申请dma通道的函数接口"
assert report["artifacts"]["rtos_dmac_case"]["source_file"] == "RTOS_DMAC_开发指南.pdf"
assert "/mnt/hdd/allwinner/v821/tina-v821-release-v1.1/tina-v821-release/docs/pdf/其他文档/RTOS" in report["artifacts"]["rtos_case"]["corpus_dir"]
assert "web_demo_rtos_manual_howto_pass_rate" in report["metrics"]["frontend"]
assert "web_demo_rtos_dmac_howto_pass_rate" in report["metrics"]["frontend"]
required = report["artifacts"]["rtos_dmac_case"]["required_evidence_groups"]
assert "hal_dma_chan_request" in required["interface"]
assert "dma_request_chan" in required["forbidden_linux_interfaces"]
PY

echo "eval web-demo frontend contract PASS"
