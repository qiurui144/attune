#!/usr/bin/env bash
# Attune KB Demo — K3 one-command launcher (bulletproof)
# Starts: attune E2E (18906) → CORS proxy (8889) → Web demo (8888)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="${DEMO_DIR:-$SCRIPT_DIR}"
PASSWORD_FILE="${ATTUNE_E2E_PASSWORD_FILE:-/tmp/attune-e2e-vault-password}"
if [ -n "${ATTUNE_E2E_PASSWORD:-}" ]; then
    PASSWORD="$ATTUNE_E2E_PASSWORD"
elif [ -s "$PASSWORD_FILE" ]; then
    PASSWORD="$(cat "$PASSWORD_FILE")"
else
    if command -v openssl >/dev/null 2>&1; then
        PASSWORD="$(openssl rand -hex 16)"
    else
        PASSWORD="attune-$(date +%s)-$$"
    fi
    umask 077
    printf '%s' "$PASSWORD" > "$PASSWORD_FILE"
fi
SCHEDULER_URL="${ATTUNE_SCHEDULER_URL:-}"
SERVER_BIN="${ATTUNE_SERVER_BIN:-}"
KGREEN='\033[32m' KRED='\033[31m' KBLUE='\033[34m' KOFF='\033[0m'

log()  { printf "${KBLUE}[%s]${KOFF} %s\n" "$1" "$2"; }
ok()   { printf "${KGREEN}[%s]${KOFF} %s\n" "$1" "$2"; }
fail() { printf "${KRED}[%s]${KOFF} %s\n" "$1" "$2"; }

if [ -z "$SERVER_BIN" ]; then
    if command -v attune-server-headless >/dev/null 2>&1; then
        SERVER_BIN="$(command -v attune-server-headless)"
    elif [ -x /usr/bin/attune-server-headless ]; then
        SERVER_BIN="/usr/bin/attune-server-headless"
    elif [ -x "$HOME/.local/bin/attune-server-headless" ]; then
        SERVER_BIN="$HOME/.local/bin/attune-server-headless"
    else
        fail "attune" "attune-server-headless not found; set ATTUNE_SERVER_BIN"
        exit 1
    fi
fi

# ── Cleanup existing demo processes ──
log "clean" "Stopping any existing demo services..."
fuser -k 8888/tcp 2>/dev/null || true
fuser -k 8889/tcp 2>/dev/null || true
fuser -k 18906/tcp 2>/dev/null || true
sleep 2

# ── 1. Attune E2E instance ──
attune_pid="$(ps -eo pid,args | grep "attune-server-headless" | grep -- "--port 18906" | grep -v grep | sed -n '1s/^ *\([0-9][0-9]*\).*/\1/p' || true)"
if [ -n "$attune_pid" ]; then
    ok "attune" "E2E instance already running on :18906"
else
    log "attune" "Starting E2E instance on :18906..."
    mkdir -p /tmp/attune-e2e-data /tmp/attune-e2e-config
    ATTUNE_CHAT_SCHEDULER_JOB_POLL_TIMEOUT_MS=45000 \
    XDG_DATA_HOME=/tmp/attune-e2e-data XDG_CONFIG_HOME=/tmp/attune-e2e-config \
        nohup "$SERVER_BIN" --no-auth --port 18906 --host 0.0.0.0 \
        > /tmp/attune-e2e.log 2>&1 &
    sleep 3
    attune_pid="$(ps -eo pid,args | grep "attune-server-headless" | grep -- "--port 18906" | grep -v grep | sed -n '1s/^ *\([0-9][0-9]*\).*/\1/p' || true)"
    if [ -n "$attune_pid" ]; then
        ok "attune" "Started"
    else
        fail "attune" "Failed to start"; exit 1
    fi
fi

# Unlock vault + optional scheduler-backed AI config
curl -sf -X POST http://127.0.0.1:18906/api/v1/vault/setup \
    -H "Content-Type: application/json" -d "{\"password\":\"${PASSWORD}\"}" > /dev/null 2>&1 || true
curl -sf -X POST http://127.0.0.1:18906/api/v1/vault/unlock \
    -H "Content-Type: application/json" -d "{\"password\":\"${PASSWORD}\"}" > /dev/null 2>&1 || true
if [ -n "$SCHEDULER_URL" ]; then
    curl -sf -X PATCH http://127.0.0.1:18906/api/v1/settings \
        -H "Content-Type: application/json" \
        -d "{\"llm\":{\"provider\":\"local_scheduler\",\"endpoint\":\"${SCHEDULER_URL}\",\"model\":\"llm-chat\",\"api_key\":\"local-scheduler\"},\"embedding\":{\"provider\":\"local_scheduler\",\"endpoint\":\"${SCHEDULER_URL}\"},\"rerank\":{\"enabled\":true,\"provider\":\"local_scheduler\",\"endpoint\":\"${SCHEDULER_URL}\",\"task\":\"kb.query.rerank\"}}" > /dev/null 2>&1 || true
fi

# ── 2. CORS Proxy ──
log "proxy" "Starting CORS proxy on :8889 → :18906..."
ATTUNE_PROXY_PORT=8889 ATTUNE_TARGET_HOST=127.0.0.1 ATTUNE_TARGET_PORT=18906 ATTUNE_PROXY_RESPONSE_IDLE_TIMEOUT_SECONDS=600 \
    nohup python3 "$DEMO_DIR/cors-proxy.py" > /tmp/cors-proxy.log 2>&1 &
sleep 2
if curl -sf http://127.0.0.1:8889/api/v1/vault/status > /dev/null 2>&1; then
    ok "proxy" "CORS proxy ready on :8889"
else
    fail "proxy" "CORS proxy failed"; exit 1
fi

# ── 3. Web Demo ──
log "web" "Starting web demo on :8888..."
cd "$DEMO_DIR"
python3 -c "
from http.server import HTTPServer, SimpleHTTPRequestHandler
import os; os.chdir('$DEMO_DIR')
HTTPServer(('0.0.0.0', 8888), SimpleHTTPRequestHandler).serve_forever()
" > /tmp/web-demo.log 2>&1 &
sleep 2
if curl -sf http://127.0.0.1:8888/ > /dev/null 2>&1; then
    ok "web" "Web demo ready"
else
    fail "web" "Web demo failed"; exit 1
fi

# ── Done ──
IP=$(hostname -I 2>/dev/null | awk '{print $1}' || echo 'localhost')
echo ""
echo "  🌐 Web Demo:  http://${IP}:8888/?api=http://${IP}:8889"
echo "  🔗 API Proxy: http://${IP}:8889/"
echo "  📡 Attune:    http://127.0.0.1:18906"
echo "  🧠 Scheduler: ${SCHEDULER_URL:-not configured by launcher}"
echo "  🚀 All services online."
