#!/usr/bin/env bash
# v0.7 Memory Moat — E2E 套件统一 runner。
#
# 一键：编译 server → 起隔离 server → setup+unlock vault → 配 LLM（若 Ollama 可用）
# → 顺序跑全部 E2E 脚本 → 汇总 → 杀 server + 清理数据。
#
# 用法：bash tests/e2e/run_all.sh
# 退出码：0 = 全绿，非 0 = 有脚本 FAIL。
#
# 工作目录 /tmp/attune-e2e（各 E2E 脚本硬编码的 VAULT_DB 前缀，runner 与之对齐）。
# cleanup 只清 data/config/日志，不动该目录下其它文件。

set -u
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT=18905
DATA=/tmp/attune-e2e
PW=e2e-pass-2026
BIN="$REPO/rust/target/release/attune-server-headless"
SERVER_PID=""

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  rm -rf "$DATA/data" "$DATA/config" "$DATA/server.log"
}
trap cleanup EXIT

echo "=== v0.7 Memory Moat E2E 套件 ==="

# 1. 编译 server（产物已存在则跳过）
if [ ! -x "$BIN" ]; then
  echo "[1/5] 编译 attune-server-headless ..."
  ( cd "$REPO/rust" && cargo build --release -p attune-server --bin attune-server-headless ) || exit 1
else
  echo "[1/5] server 二进制已存在，跳过编译"
fi

# 2. 起隔离 server
echo "[2/5] 起隔离 server (port $PORT) ..."
rm -rf "$DATA/data" "$DATA/config" && mkdir -p "$DATA/data" "$DATA/config"
XDG_DATA_HOME="$DATA/data" XDG_CONFIG_HOME="$DATA/config" \
  "$BIN" --no-auth --port "$PORT" > "$DATA/server.log" 2>&1 &
SERVER_PID=$!
sleep 8
python3 -c "import urllib.request,sys
try: sys.exit(0 if urllib.request.urlopen('http://localhost:$PORT/health',timeout=3).status==200 else 1)
except Exception: sys.exit(1)" \
  || { echo "server 启动失败，见 $DATA/server.log"; exit 1; }

# 3+4. setup + unlock vault + 配 LLM（若 Ollama 可用）
echo "[3/5] setup + unlock vault ..."
HAS_LLM=$(python3 - "$PORT" "$PW" <<'PYEOF'
import json, sys, urllib.request, urllib.error
port, pw = sys.argv[1], sys.argv[2]
def call(method, path, body):
    r = urllib.request.Request(f"http://localhost:{port}{path}",
        data=json.dumps(body).encode(), headers={"Content-Type": "application/json"},
        method=method)
    try: urllib.request.urlopen(r, timeout=15).read()
    except urllib.error.HTTPError: pass
call("POST", "/api/v1/vault/setup", {"password": pw})
call("POST", "/api/v1/vault/unlock", {"password": pw})
has_llm = 0
try:
    tags = urllib.request.urlopen("http://localhost:11434/api/tags", timeout=3).read().decode()
    if "qwen2.5" in tags:
        call("PATCH", "/api/v1/settings", {"llm": {"provider": "openai_compat",
            "endpoint": "http://localhost:11434/v1", "model": "qwen2.5:3b",
            "api_key": "ollama"}})
        has_llm = 1
except Exception:
    pass
print(has_llm)
PYEOF
)
if [ "$HAS_LLM" = "1" ]; then
  echo "[4/5] Ollama 可用，已配 LLM provider"
else
  echo "[4/5] Ollama 不可用，跳过 chat E2E"
fi

# 5. 顺序跑 E2E 脚本
echo "[5/5] 跑 E2E 脚本 ..."
echo ""
SCRIPTS=(
  memory_moat_e2e.py
  memory_moat_signals_e2e.py
  memory_moat_stress_e2e.py
  memory_moat_fault_e2e.py
  memory_moat_annotation_e2e.py
  memory_moat_stress_loop_e2e.py
)
[ "$HAS_LLM" = "1" ] && SCRIPTS+=(memory_moat_chat_e2e.py)

TOTAL_FAIL=0
for s in "${SCRIPTS[@]}"; do
  echo "────── $s ──────"
  python3 "$REPO/tests/e2e/$s" 2>&1 | tail -2
  rc=${PIPESTATUS[0]}
  [ "$rc" -ne 0 ] && TOTAL_FAIL=$((TOTAL_FAIL + 1))
  echo ""
done

if [ "$TOTAL_FAIL" -eq 0 ]; then
  echo "=== E2E 套件全绿 (${#SCRIPTS[@]} 脚本) ==="
  exit 0
else
  echo "=== E2E 套件有 $TOTAL_FAIL 个脚本 FAIL ==="
  exit 1
fi
