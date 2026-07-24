#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT_FILE="$(mktemp -t attune-eval-fake-threshold-port-XXXXXX)"
OUT="$(mktemp -t attune-pr-rag-live-threshold-XXXXXX.json)"

python3 - "$PORT_FILE" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

port_file = sys.argv[1]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        return

    def send_json(self, status, payload):
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/api/v1/status":
            self.send_json(200, {"version": "fake-attune-threshold", "pending_embeddings": 0})
            return
        if parsed.path == "/api/v1/search":
            self.send_json(200, {
                "latency_ms": 1,
                "results": [
                    {"item_id": "tcpip_troubleshooting"},
                    {"item_id": "tcpip_support_workflow"}
                ],
            })
            return
        self.send_json(404, {"error": "not_found"})

    def do_POST(self):
        parsed = urlparse(self.path)
        length = int(self.headers.get("content-length") or "0")
        if length:
            self.rfile.read(length)
        if parsed.path == "/api/v1/upload":
            self.send_json(200, {"id": "uploaded", "status": "ready", "chunks_queued": 1})
            return
        if parsed.path == "/api/v1/chat":
            self.send_json(200, {
                "content": "TCP/IP 起源于 ARPANET 和 DARPA 研究；排障覆盖物理链路 IP 路由 DNS 抓包 拓扑 端口 日志",
                "answer_mode": "llm-chat",
                "knowledge_count": 2,
                "citations": [
                    {"title": "tcpip_origin", "item_id": "tcpip_origin"},
                    {"title": "tcpip_troubleshooting", "item_id": "tcpip_troubleshooting"},
                    {"title": "tcpip_support_workflow", "item_id": "tcpip_support_workflow"}
                ],
                "cost": {"provider": "local_scheduler", "model": "llm-chat", "latency_ms": 45000}
            })
            return
        self.send_json(404, {"error": "not_found"})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 50); do
  if [ -s "$PORT_FILE" ]; then
    break
  fi
  sleep 0.1
done

PORT="$(cat "$PORT_FILE")"
test -n "$PORT"

python3 "$ROOT/scripts/eval/run-suite.py" \
  --root "$ROOT" \
  --suite pr_rag_smoke \
  --base-url "http://127.0.0.1:$PORT" \
  --out "$OUT"

python3 - "$OUT" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
assert report["summary"]["pass"] is False
assert report["metrics"]["performance"]["chat_p95_ms"] == 45000
assert any(f["failure_layer"] == "threshold" for f in report["failures"])
assert any(f.get("metric") == "performance.chat_p95_ms" for f in report["failures"])
PY

echo "eval run-suite live threshold gate PASS"
