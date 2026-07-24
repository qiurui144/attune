#!/usr/bin/env bash
# Attune KB Demo — K3 one-command launcher (bulletproof)
# Starts: attune E2E (18906) → CORS proxy (8889) → Web demo (8888)
set -euo pipefail

DEMO_DIR="${DEMO_DIR:-/tmp/kb-web-demo}"
PASSWORD="${ATTUNE_E2E_PASSWORD:-e2e-pass-2026}"
KGREEN='\033[32m' KRED='\033[31m' KBLUE='\033[34m' KOFF='\033[0m'

log()  { printf "${KBLUE}[%s]${KOFF} %s\n" "$1" "$2"; }
ok()   { printf "${KGREEN}[%s]${KOFF} %s\n" "$1" "$2"; }
fail() { printf "${KRED}[%s]${KOFF} %s\n" "$1" "$2"; }

# ── Cleanup existing demo processes ──
log "clean" "Stopping any existing demo services..."
fuser -k 8888/tcp 2>/dev/null || true
fuser -k 8889/tcp 2>/dev/null || true
fuser -k 18906/tcp 2>/dev/null || true
sleep 2

# ── 1. Attune E2E instance ──
attune_pid="$(ps -eo pid,args | grep "/usr/bin/attune-server-headless --no-auth --port 18906" | grep -v grep | sed -n '1s/^ *\([0-9][0-9]*\).*/\1/p' || true)"
if [ -n "$attune_pid" ]; then
    ok "attune" "E2E instance already running on :18906"
else
    log "attune" "Starting E2E instance on :18906..."
    mkdir -p /tmp/attune-e2e-data /tmp/attune-e2e-config
    ATTUNE_CHAT_SCHEDULER_JOB_POLL_TIMEOUT_MS=45000 \
    XDG_DATA_HOME=/tmp/attune-e2e-data XDG_CONFIG_HOME=/tmp/attune-e2e-config \
        nohup /usr/bin/attune-server-headless --no-auth --port 18906 --host 0.0.0.0 \
        > /tmp/attune-e2e.log 2>&1 &
    sleep 3
    attune_pid="$(ps -eo pid,args | grep "/usr/bin/attune-server-headless --no-auth --port 18906" | grep -v grep | sed -n '1s/^ *\([0-9][0-9]*\).*/\1/p' || true)"
    if [ -n "$attune_pid" ]; then
        ok "attune" "Started"
    else
        fail "attune" "Failed to start"; exit 1
    fi
fi

# Unlock vault + config LLM
curl -sf -X POST http://127.0.0.1:18906/api/v1/vault/setup \
    -H "Content-Type: application/json" -d "{\"password\":\"${PASSWORD}\"}" > /dev/null 2>&1 || true
curl -sf -X POST http://127.0.0.1:18906/api/v1/vault/unlock \
    -H "Content-Type: application/json" -d "{\"password\":\"${PASSWORD}\"}" > /dev/null 2>&1 || true
curl -sf -X PATCH http://127.0.0.1:18906/api/v1/settings \
    -H "Content-Type: application/json" \
    -d '{"llm":{"provider":"local_scheduler","endpoint":"http://127.0.0.1:8090","model":"llm-chat"},"rerank":{"enabled":true,"task":"kb.query.rerank"}}' > /dev/null 2>&1 || true

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
IP=$(hostname -I 2>/dev/null | awk '{print $1}' || echo '192.168.100.233')
echo ""
echo "  🌐 Web Demo:  http://${IP}:8888/"
echo "  🔗 API Proxy: http://${IP}:8889/"
echo "  📡 Attune:    http://127.0.0.1:18906"
echo "  🚀 All services online."
