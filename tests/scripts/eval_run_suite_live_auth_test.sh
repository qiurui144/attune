#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT_FILE="$(mktemp -t attune-eval-fake-auth-port-XXXXXX)"
LOG_FILE="$(mktemp -t attune-eval-fake-auth-log-XXXXXX.jsonl)"
OUT="$(mktemp -t attune-pr-rag-live-auth-XXXXXX.json)"
TOKEN="eval-token-123"

python3 - "$PORT_FILE" "$LOG_FILE" "$TOKEN" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

port_file, log_file, token = sys.argv[1:4]


def append_log(row):
    with open(log_file, "a", encoding="utf-8") as f:
        f.write(json.dumps(row, ensure_ascii=False) + "\n")


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

    def require_auth(self):
        got = self.headers.get("authorization") or ""
        append_log({"method": self.command, "path": urlparse(self.path).path, "authorization": got})
        if got != f"Bearer {token}":
            self.send_json(401, {"error": "missing bearer token"})
            return False
        return True

    def do_GET(self):
        parsed = urlparse(self.path)
        if not self.require_auth():
            return
        if parsed.path == "/api/v1/status":
            self.send_json(200, {"version": "fake-attune-auth", "pending_embeddings": 0})
            return
        if parsed.path == "/api/v1/search":
            query = parse_qs(parsed.query).get("q", [""])[0]
            self.send_json(200, {
                "latency_ms": 3,
                "results": [{
                    "item_id": "tcpip_troubleshooting",
                    "title": "tcpip_troubleshooting",
                    "content": query,
                    "score": 0.99
                }]
            })
            return
        self.send_json(404, {"error": "not_found"})

    def do_POST(self):
        parsed = urlparse(self.path)
        length = int(self.headers.get("content-length") or "0")
        if length:
            self.rfile.read(length)
        if not self.require_auth():
            return
        if parsed.path == "/api/v1/upload":
            self.send_json(200, {"id": "uploaded", "status": "ready", "chunks_queued": 1})
            return
        if parsed.path == "/api/v1/chat":
            self.send_json(200, {
                "content": "基于证据：TCP/IP 起源于 ARPANET 和 DARPA 研究；排障要检查物理链路、IP、路由、DNS、抓包、拓扑、端口、日志，证据不足时继续索取材料。",
                "answer_mode": "llm-chat",
                "knowledge_count": 2,
                "citations": [
                    {"title": "tcpip_origin", "item_id": "tcpip_origin"},
                    {"title": "tcpip_troubleshooting", "item_id": "tcpip_troubleshooting"},
                    {"title": "tcpip_support_workflow", "item_id": "tcpip_support_workflow"}
                ],
                "cost": {"provider": "local_scheduler", "model": "llm-chat", "latency_ms": 123}
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
  --token "$TOKEN" \
  --out "$OUT"

python3 - "$OUT" "$LOG_FILE" "$TOKEN" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
logs = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8") if line.strip()]
expected = f"Bearer {sys.argv[3]}"

assert report["summary"]["pass"] is True
assert report["metrics"]["api"]["uploads"] == 3
assert report["metrics"]["api"]["searches"] == 3
assert report["metrics"]["api"]["chats"] == 3
assert logs
assert all(row["authorization"] == expected for row in logs), logs
PY

echo "eval run-suite live auth contract PASS"
