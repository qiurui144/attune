#!/usr/bin/env bash
# v0.7 Memory Moat — E2E 套件统一 runner。
#
# 一键：编译 server → 起隔离 server → setup+unlock vault → 配 cloud/scheduler LLM（若显式配置）
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
TARGET_DIR="${CARGO_TARGET_DIR:-}"
if [ -z "$TARGET_DIR" ]; then
  TARGET_DIR="$(cd "$REPO/rust" && cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
fi
BIN="$TARGET_DIR/release/attune-server-headless"
BIN_DIR="$(dirname "$BIN")"
SERVER_PID=""
WITH_LONGTEXT="${ATTUNE_E2E_LONGTEXT:-0}"
LOCAL_SCHEDULER="${ATTUNE_E2E_LOCAL_SCHEDULER:-}"
[ -z "$LOCAL_SCHEDULER" ] && LOCAL_SCHEDULER="${ATTUNE_E2E_SCHEDULER_ENDPOINT:-}"
LOCAL_SCHEDULER="${LOCAL_SCHEDULER%/}"
SCHEDULER_STRICT="${ATTUNE_E2E_SCHEDULER_STRICT:-1}"

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  rm -rf "$DATA/data" "$DATA/config" "$DATA/server.log"
}
trap cleanup EXIT

echo "=== v0.7 Memory Moat E2E 套件 ==="

# 1. 编译 server（产物不存在、显式强制、或源码比二进制新时重编译）
NEEDS_BUILD=0
if [ ! -x "$BIN" ]; then
  NEEDS_BUILD=1
elif [ "${ATTUNE_E2E_FORCE_BUILD:-0}" = "1" ]; then
  NEEDS_BUILD=1
elif [ -n "$(find "$REPO/rust" -type f \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' \) -newer "$BIN" -print -quit)" ]; then
  NEEDS_BUILD=1
fi
if [ "$NEEDS_BUILD" = "1" ]; then
  echo "[1/5] 编译 attune-server-headless ..."
  ( cd "$REPO/rust" && cargo build --release -p attune-server --bin attune-server-headless ) || exit 1
else
  echo "[1/5] server 二进制已是最新，跳过编译"
fi

# 2. 起隔离 server
echo "[2/5] 起隔离 server (port $PORT) ..."
rm -rf "$DATA/data" "$DATA/config" && mkdir -p "$DATA/data" "$DATA/config"
SERVER_ENV=(
  "LD_LIBRARY_PATH=$BIN_DIR:$BIN_DIR/deps:${LD_LIBRARY_PATH:-}"
  "XDG_DATA_HOME=$DATA/data"
  "XDG_CONFIG_HOME=$DATA/config"
)
if [ -n "$LOCAL_SCHEDULER" ]; then
  SERVER_ENV+=(
    "ATTUNE_ENABLE_OCRMYPDF_FALLBACK=${ATTUNE_ENABLE_OCRMYPDF_FALLBACK:-0}"
    "ATTUNE_SCHEDULER_OCR_ENABLED=${ATTUNE_SCHEDULER_OCR_ENABLED:-0}"
    "ATTUNE_EMBED_QUEUE_BATCH_SIZE=${ATTUNE_EMBED_QUEUE_BATCH_SIZE:-${ATTUNE_SCHEDULER_EMBED_QUEUE_BATCH_SIZE:-32}}"
    "ATTUNE_SCHEDULER_EMBED_MAX_INPUT_CHARS=${ATTUNE_SCHEDULER_EMBED_MAX_INPUT_CHARS:-${ATTUNE_LOCAL_EMBED_MAX_INPUT_CHARS:-512}}"
    "ATTUNE_SCHEDULER_EMBED_MAX_INPUT_TOKENS=${ATTUNE_SCHEDULER_EMBED_MAX_INPUT_TOKENS:-${ATTUNE_LOCAL_EMBED_MAX_INPUT_TOKENS:-256}}"
    "ATTUNE_SCHEDULER_CONTEXT_CHUNK_MAX_CHARS=${ATTUNE_SCHEDULER_CONTEXT_CHUNK_MAX_CHARS:-${ATTUNE_LOCAL_CONTEXT_CHUNK_MAX_CHARS:-96}}"
    "ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K=${ATTUNE_SCHEDULER_ASK_CONTEXT_TOP_K:-${ATTUNE_LOCAL_ASK_CONTEXT_TOP_K:-3}}"
    "ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS=${ATTUNE_SCHEDULER_ASK_MAX_OUTPUT_TOKENS:-${ATTUNE_LOCAL_ASK_MAX_OUTPUT_TOKENS:-24}}"
    "ATTUNE_SCHEDULER_NATIVE_KB=${ATTUNE_SCHEDULER_NATIVE_KB:-1}"
    "ATTUNE_SCHEDULER_INGEST_CHUNK_SIZE=${ATTUNE_SCHEDULER_INGEST_CHUNK_SIZE:-4096}"
    "ATTUNE_SCHEDULER_INGEST_CHUNK_OVERLAP=${ATTUNE_SCHEDULER_INGEST_CHUNK_OVERLAP:-256}"
    "ATTUNE_SCHEDULER_INGEST_INCLUDE_LEVEL1=${ATTUNE_SCHEDULER_INGEST_INCLUDE_LEVEL1:-0}"
    "ATTUNE_SCHEDULER_INGEST_INCLUDE_LEVEL2=${ATTUNE_SCHEDULER_INGEST_INCLUDE_LEVEL2:-1}"
    "ATTUNE_OCRMYPDF_MAX_BYTES=${ATTUNE_OCRMYPDF_MAX_BYTES:-16777216}"
  )
  if [ "$WITH_LONGTEXT" = "1" ] && [ "$SCHEDULER_STRICT" != "0" ]; then
    SERVER_ENV+=(
      "ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER=${ATTUNE_SCHEDULER_EXTRACTIVE_ANSWER:-0}"
    )
  fi
fi

if [ -n "$LOCAL_SCHEDULER" ]; then
  echo "[preflight] probe edge scheduler contract ..."
  PROBE_ARGS=(--base-url "$LOCAL_SCHEDULER")
  if [ "$SCHEDULER_STRICT" = "0" ]; then
    PROBE_ARGS+=(--no-strict)
  else
    PROBE_ARGS+=(--strict)
  fi
  python3 "$REPO/scripts/probe-edge-scheduler-contract.py" "${PROBE_ARGS[@]}" || exit 1
fi
env "${SERVER_ENV[@]}" "$BIN" --no-auth --port "$PORT" > "$DATA/server.log" 2>&1 &
SERVER_PID=$!
sleep 8
python3 -c "import urllib.request,sys
try: sys.exit(0 if urllib.request.urlopen('http://localhost:$PORT/health',timeout=3).status==200 else 1)
except Exception: sys.exit(1)" \
  || { echo "server 启动失败，见 $DATA/server.log"; exit 1; }

# 3+4. setup + unlock vault + 配 LLM / embedding（cloud / scheduler）
echo "[3/5] setup + unlock vault ..."
SETUP_RESULT=$(python3 - "$PORT" "$PW" <<'PYEOF'
import json, os, sys, urllib.request, urllib.error
port, pw = sys.argv[1], sys.argv[2]
def call(method, path, body):
    r = urllib.request.Request(f"http://localhost:{port}{path}",
        data=json.dumps(body).encode(), headers={"Content-Type": "application/json"},
        method=method)
    try:
        urllib.request.urlopen(r, timeout=15).read()
        return True
    except urllib.error.HTTPError:
        return False
call("POST", "/api/v1/vault/setup", {"password": pw})
call("POST", "/api/v1/vault/unlock", {"password": pw})
has_llm = 0
scheduler = (
    os.environ.get("ATTUNE_E2E_LOCAL_SCHEDULER", "")
    or os.environ.get("ATTUNE_E2E_SCHEDULER_ENDPOINT", "")
).strip().rstrip("/")
endpoint = os.environ.get("ATTUNE_E2E_LLM_ENDPOINT", "").strip()
if not endpoint and scheduler:
    endpoint = f"{scheduler}/v1"
if endpoint:
    llm = {
        "provider": os.environ.get("ATTUNE_E2E_LLM_PROVIDER", "openai_compat"),
        "endpoint": endpoint,
        "api_key": os.environ.get("ATTUNE_E2E_LLM_API_KEY", ""),
    }
    model = os.environ.get("ATTUNE_E2E_LLM_MODEL", "").strip()
    if not model and scheduler:
        model = "llm-summary"
    if model:
        llm["model"] = model
    if call("PATCH", "/api/v1/settings", {"llm": llm}):
        has_llm = 1
has_embedding = 0
embedding_endpoint = os.environ.get("ATTUNE_E2E_EMBEDDING_ENDPOINT", "").strip()
if not embedding_endpoint and scheduler:
    embedding_endpoint = scheduler
if embedding_endpoint:
    default_embedding_provider = "local_scheduler" if scheduler else "openai_compat"
    embedding = {
        "provider": os.environ.get("ATTUNE_E2E_EMBEDDING_PROVIDER", default_embedding_provider),
        "endpoint": embedding_endpoint,
        "api_key": os.environ.get("ATTUNE_E2E_EMBEDDING_API_KEY", ""),
    }
    model = os.environ.get("ATTUNE_E2E_EMBEDDING_MODEL", "").strip()
    if not model and scheduler:
        model = "embedding-int8"
    if model:
        embedding["model"] = model
    dims = os.environ.get("ATTUNE_E2E_EMBEDDING_DIMS", "").strip()
    if not dims and scheduler:
        dims = "512"
    if dims:
        embedding["dims"] = int(dims)
    task = os.environ.get("ATTUNE_E2E_EMBEDDING_TASK", "").strip()
    if not task and scheduler:
        task = "kb.query.embed"
    if task:
        embedding["task"] = task
    poll_timeout_ms = os.environ.get("ATTUNE_E2E_EMBEDDING_POLL_TIMEOUT_MS", "").strip()
    if not poll_timeout_ms and scheduler:
        poll_timeout_ms = "120000"
    if poll_timeout_ms:
        embedding["poll_timeout_ms"] = int(poll_timeout_ms)
    if call("PATCH", "/api/v1/settings", {"embedding": embedding}):
        has_embedding = 1
print(f"{has_llm}:{has_embedding}")
PYEOF
)
HAS_LLM="${SETUP_RESULT%%:*}"
HAS_EMBEDDING="${SETUP_RESULT##*:}"
if [ -n "${ATTUNE_E2E_LLM_ENDPOINT:-}" ] && [ "$HAS_LLM" = "1" ]; then
  echo "[4/5] 已按 ATTUNE_E2E_LLM_ENDPOINT 配置 LLM provider"
elif [ -n "$LOCAL_SCHEDULER" ] && [ "$HAS_LLM" = "1" ]; then
  echo "[4/5] 已按 edge scheduler 配置 LLM 路由"
else
  echo "[4/5] 未配置 cloud/scheduler LLM，跳过 legacy direct-Ollama chat E2E"
fi
if [ "$HAS_EMBEDDING" = "1" ]; then
  EMBEDDING_PROVIDER_LABEL="${ATTUNE_E2E_EMBEDDING_PROVIDER:-openai_compat}"
  if [ -n "$LOCAL_SCHEDULER" ] && [ -z "${ATTUNE_E2E_EMBEDDING_PROVIDER:-}" ]; then
    EMBEDDING_PROVIDER_LABEL="local_scheduler"
  fi
  echo "      已配置 embedding provider (${EMBEDDING_PROVIDER_LABEL})"
fi
if [ "$WITH_LONGTEXT" = "1" ]; then
  echo "      长文本 E2E 已启用，将使用当前 cloud/scheduler chat 配置"
  if [ -n "$LOCAL_SCHEDULER" ] && [ "$SCHEDULER_STRICT" != "0" ]; then
    export ATTUNE_LONGTEXT_REQUIRE_SCHEDULER_GENERATION="${ATTUNE_LONGTEXT_REQUIRE_SCHEDULER_GENERATION:-1}"
    export ATTUNE_LONGTEXT_REQUIRE_PROMPT_CACHE_METADATA="${ATTUNE_LONGTEXT_REQUIRE_PROMPT_CACHE_METADATA:-1}"
    export ATTUNE_LONGTEXT_SCHEDULER_GENERATION_P95_MS_MAX="${ATTUNE_LONGTEXT_SCHEDULER_GENERATION_P95_MS_MAX:-10000}"
    echo "      scheduler strict gate: generation=required prompt-cache=required p95<=${ATTUNE_LONGTEXT_SCHEDULER_GENERATION_P95_MS_MAX}ms"
  fi
fi
RUN_STANDARD_CHAT=0
if [ -n "$LOCAL_SCHEDULER" ] && [ "${ATTUNE_E2E_RUN_STANDARD_CHAT:-0}" != "1" ]; then
  RUN_STANDARD_CHAT=0
  echo "      edge scheduler 模式下跳过 Ollama 专用 memory_moat_chat_e2e.py"
elif [ "${ATTUNE_E2E_RUN_STANDARD_CHAT:-0}" = "1" ]; then
  RUN_STANDARD_CHAT=1
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
  memory_moat_v07routes_e2e.py
  memory_moat_search_quality_e2e.py
  memory_moat_stress_loop_e2e.py
)
[ "$RUN_STANDARD_CHAT" = "1" ] && SCRIPTS+=(memory_moat_chat_e2e.py)
[ "$WITH_LONGTEXT" = "1" ] && SCRIPTS+=(airplane_manual_longtext_e2e.py)

export ATTUNE_BASE_URL="http://localhost:$PORT"

TOTAL_FAIL=0
for s in "${SCRIPTS[@]}"; do
  echo "────── $s ──────"
  script_log="$DATA/${s%.py}.log"
  PYTHONUNBUFFERED=1 python3 "$REPO/tests/e2e/$s" > "$script_log" 2>&1
  rc=$?
  tail_lines="${ATTUNE_E2E_LOG_TAIL_LINES:-2}"
  if [ "$s" = "airplane_manual_longtext_e2e.py" ]; then
    tail_lines="${ATTUNE_E2E_LONGTEXT_LOG_TAIL_LINES:-40}"
  fi
  tail -n "$tail_lines" "$script_log"
  echo "log: $script_log"
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
