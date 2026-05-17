#!/usr/bin/env bash
# law-pro 全量前端 E2E 编排 —— per plan tingly-knitting-zephyr 阶段 4。
#
# 跑 lawpro_ui_e2e.py（真 Chrome）覆盖 L0 Wizard / L1 Sidebar / L2 八视图 /
# L3 Settings / L4 模态 / L5 law-pro 接入。
#
# 密钥不入库：在本目录建 .env.local（已 gitignore）填：
#   ATTUNE_LLM_KEY=sk-...            # 云端 LLM token（hiapi.online 等）
#   PLUGINHUB_LICENSE=...            # pluginhub license key
# 可选覆盖：ATTUNE_BASE_URL / PLUGINHUB_URL / ATTUNE_LLM_URL / ATTUNE_HEADLESS
#
# 前置：attune-server 已起；law-pro 已在 ~/.local/share/attune/plugins/；
#       pluginhub 在 PLUGINHUB_URL 可达（自部署可经 SSH 隧道）。
# 用法：bash tests/e2e/playwright/run_ui_all.sh
set -euo pipefail
cd "$(dirname "$0")/../../.."   # → 仓库根

SELF_DIR="tests/e2e/playwright"
[ -f "$SELF_DIR/.env.local" ] && set -a && . "$SELF_DIR/.env.local" && set +a

BASE="${ATTUNE_BASE_URL:-http://127.0.0.1:18900}"
HUB="${PLUGINHUB_URL:-http://127.0.0.1:9100}"
PY="${ATTUNE_PYTHON:-.venv/bin/python}"

echo "── 前置检查 ──"
"$PY" -c 'import playwright' 2>/dev/null || { echo "playwright 未装：$PY -m pip install playwright"; exit 2; }
curl -sf -o /dev/null --max-time 5 "$BASE/" 2>/dev/null \
  || echo "⚠  $BASE 未响应 —— 先起 attune-server-headless --no-auth --port ${BASE##*:}"
curl -sf -o /dev/null --max-time 5 "$HUB/health" 2>/dev/null \
  || echo "⚠  pluginhub $HUB 未响应 —— L5 Marketplace 用例会 FAIL"
[ -n "${ATTUNE_LLM_KEY:-}" ] || { echo "ERROR: 未设 ATTUNE_LLM_KEY（见本脚本注释）"; exit 2; }
[ -n "${PLUGINHUB_LICENSE:-}" ] || { echo "ERROR: 未设 PLUGINHUB_LICENSE"; exit 2; }

echo "── 运行 lawpro_ui_e2e.py ──"
exec "$PY" "$SELF_DIR/lawpro_ui_e2e.py"
