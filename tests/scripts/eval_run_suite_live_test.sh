#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT_FILE="$(mktemp -t attune-eval-fake-port-XXXXXX)"
LOG_FILE="$(mktemp -t attune-eval-fake-log-XXXXXX.jsonl)"
OUT="$(mktemp -t attune-pr-rag-live-XXXXXX.json)"

python3 - "$PORT_FILE" "$LOG_FILE" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

port_file, log_file = sys.argv[1], sys.argv[2]


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

    def do_GET(self):
        parsed = urlparse(self.path)
        append_log({"method": "GET", "path": parsed.path, "query": parse_qs(parsed.query)})
        if parsed.path == "/api/v1/status":
            self.send_json(200, {
                "version": "fake-attune-live",
                "pending_embeddings": 0,
                "vector_index": True,
                "embedding_available": True
            })
            return
        if parsed.path == "/api/v1/search":
            query = parse_qs(parsed.query).get("q", [""])[0]
            self.send_json(200, {
                "latency_ms": 4,
                "results": [{
                    "item_id": "tcpip_troubleshooting",
                    "title": "tcpip_troubleshooting",
                    "content": "物理链路 IP 路由 DNS 抓包 拓扑 端口 日志 support workflow",
                    "score": 0.99,
                    "query": query
                }]
            })
            return
        self.send_json(404, {"error": "not_found"})

    def do_POST(self):
        parsed = urlparse(self.path)
        length = int(self.headers.get("content-length") or "0")
        body = self.rfile.read(length) if length else b""
        if parsed.path == "/api/v1/upload":
            append_log({"method": "POST", "path": parsed.path, "bytes": len(body)})
            self.send_json(200, {
                "id": f"uploaded-{len(body)}",
                "title": "uploaded tcpip fixture",
                "status": "ready",
                "chunks_queued": 1
            })
            return
        if parsed.path == "/api/v1/chat":
            payload = json.loads(body.decode("utf-8") or "{}")
            append_log({
                "method": "POST",
                "path": parsed.path,
                "body": payload,
            })
            self.send_json(200, {
                "content": "基于证据：TCP/IP 起源于 ARPANET 和 DARPA 研究；排障要检查物理链路、IP、路由、DNS、数据包捕获、拓扑、端口、日志，证据不足时继续索取材料。",
                "answer_mode": "llm-chat",
                "knowledge_count": 2,
                "citations": [
                    {"title": "tcpip_origin", "item_id": "tcpip_origin"},
                    {"title": "tcpip_troubleshooting", "item_id": "tcpip_troubleshooting"},
                    {"title": "tcpip_support_workflow", "item_id": "tcpip_support_workflow"}
                ],
                "cost": {
                    "provider": "local_scheduler",
                    "model": "llm-chat",
                    "latency_ms": 123
                }
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

python3 - "$OUT" "$LOG_FILE" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
logs = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8") if line.strip()]
paths = [row["path"] for row in logs]

assert report["schema_version"] == "attune.eval.report.v1"
assert report["suite_id"] == "pr_rag_smoke"
assert report["summary"]["pass"] is True
assert report["summary"]["cases"] == 3
assert report["summary"]["failures"] == 0
assert report["metrics"]["api"]["uploads"] == 3
assert report["metrics"]["api"]["searches"] == 3
assert report["metrics"]["api"]["chats"] == 3
assert report["metrics"]["answer"]["summary_turns"] == 1
assert report["metrics"]["answer"]["citation_hit_rate"] == 1.0
assert report["metrics"]["answer"]["required_term_rate"] == 1.0
assert report["metrics"]["performance"]["chat_p95_ms"] == 123
assert len(report["artifacts"]["turn_results"]) == 3
assert report["artifacts"]["turn_results"][0]["content_excerpt"]
assert report["artifacts"]["turn_results"][0]["citation_labels"]
assert report["artifacts"]["turn_results"][0]["timing"]["search_latency_ms"] == 4
assert report["artifacts"]["turn_results"][0]["timing"]["chat_latency_ms"] == 123
assert report["artifacts"]["turn_results"][0]["timing"]["scheduler_queue_wait_ms"] is None
assert report["artifacts"]["turn_results"][0]["observability"]["knowledge_count"] == 2
assert report["artifacts"]["turn_results"][0]["observability"]["answer_mode"] == "llm-chat"
assert "/api/v1/upload" in paths
assert "/api/v1/status" in paths
assert "/api/v1/search" in paths
assert "/api/v1/chat" in paths
chat_bodies = [row["body"] for row in logs if row["path"] == "/api/v1/chat"]
assert len(chat_bodies) == 3
assert chat_bodies[0]["session_id"].startswith("eval-pr_rag_smoke-networking_tcpip_troubleshooting-")
assert chat_bodies[0]["history"] == []
assert chat_bodies[1]["session_id"] == chat_bodies[0]["session_id"]
assert [h["role"] for h in chat_bodies[1]["history"]] == ["user", "assistant"]
assert "Cited sources:" in chat_bodies[1]["history"][1]["content"]
assert chat_bodies[2]["session_id"].startswith("eval-pr_rag_smoke-networking_tcpip_summary-")
assert chat_bodies[2]["session_id"] != chat_bodies[0]["session_id"]
assert chat_bodies[2]["history"] == []
PY

echo "eval run-suite live contract PASS"
