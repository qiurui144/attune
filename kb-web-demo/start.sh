#!/usr/bin/env bash
# Attune KB Web Demo — one-command launcher
# Usage: bash kb-web-demo/start.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO_DIR="$ROOT/kb-web-demo"
PORT="${ATTUNE_DEMO_PORT:-8888}"
ATTUNE_PORT="${ATTUNE_DEMO_ATTUNE_PORT:-18906}"

echo "=== Attune KB Web Demo Launcher ==="
echo "  Demo:    http://$(hostname -I 2>/dev/null | awk '{print $1}' || echo 'localhost'):${PORT}/"
echo "  Attune:  http://127.0.0.1:${ATTUNE_PORT}"
echo ""

# Start web server
cd "$DEMO_DIR"
echo "[demo] Starting web server on :${PORT}..."
python3 -c "
from http.server import HTTPServer, SimpleHTTPRequestHandler
import os
os.chdir('$DEMO_DIR')
print('[demo] Serving on 0.0.0.0:${PORT}')
HTTPServer(('0.0.0.0', ${PORT}), SimpleHTTPRequestHandler).serve_forever()
"
