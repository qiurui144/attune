#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT_FILE="$(mktemp -t attune-eval-fake-async-port-XXXXXX)"
LOG_FILE="$(mktemp -t attune-eval-fake-async-log-XXXXXX.jsonl)"
OUT="$(mktemp -t attune-pr-rag-live-async-XXXXXX.json)"

python3 - "$PORT_FILE" "$LOG_FILE" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

port_file, log_file = sys.argv[1], sys.argv[2]
polls = {}


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
        append_log({"method": "GET", "path": parsed.path})
        if parsed.path == "/api/v1/status":
            self.send_json(200, {"version": "fake-attune-async", "pending_embeddings": 0})
            return
        if parsed.path == "/api/v1/search":
            self.send_json(200, {
                "latency_ms": 2,
                "results": [{"item_id": "tcpip_troubleshooting", "title": "tcpip_troubleshooting"}]
            })
            return
        if parsed.path.startswith("/api/v1/chat/local-scheduler/jobs/"):
            job_id = parsed.path.rsplit("/", 1)[-1]
            polls[job_id] = polls.get(job_id, 0) + 1
            if polls[job_id] == 1:
                self.send_json(200, {
                    "job": {
                        "job_id": job_id,
                        "status": "running",
                        "task": "kb.query.ask",
                        "scheduled_as": "async",
                        "model": "llm-chat",
                        "queue_wait_ms": 7
                    }
                })
                return
            self.send_json(200, {
                "job": {
                    "job_id": job_id,
                    "status": "done",
                    "task": "kb.query.ask",
                    "scheduled_as": "async",
                    "model": "llm-chat",
                    "latency_ms": 321,
                    "queue_wait_ms": 7,
                    "cold_start_wait_ms": 0,
                    "outputs": {
                        "answer": "基于证据：TCP/IP 起源于 ARPANET 和 DARPA 研究；排障要检查物理链路、IP、路由、DNS、抓包、拓扑、端口、日志，证据不足时继续索取材料。",
                        "citations": [
                            {"title": "tcpip_origin", "item_id": "tcpip_origin"},
                            {"title": "tcpip_troubleshooting", "item_id": "tcpip_troubleshooting"},
                            {"title": "tcpip_support_workflow", "item_id": "tcpip_support_workflow"}
                        ]
                    }
                }
            })
            return
        self.send_json(404, {"error": "not_found"})

    def do_POST(self):
        parsed = urlparse(self.path)
        length = int(self.headers.get("content-length") or "0")
        if length:
            self.rfile.read(length)
        append_log({"method": "POST", "path": parsed.path})
        if parsed.path == "/api/v1/upload":
            self.send_json(200, {"id": "uploaded", "status": "ready", "chunks_queued": 1})
            return
        if parsed.path == "/api/v1/chat":
            self.send_json(200, {
                "content": "本地 scheduler 知识库回答任务已提交，job_id=job_async_eval_001。",
                "local_scheduler": {
                    "job_id": "job_async_eval_001",
                    "task": "kb.query.ask",
                    "scheduled_as": "async",
                    "status": "queued",
                    "model": "llm-chat"
                },
                "knowledge_count": 2,
                "citations": []
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

assert report["summary"]["pass"] is True
assert report["metrics"]["api"]["job_polls"] >= 2
assert report["metrics"]["scheduler"]["async_jobs"] == 3
assert report["metrics"]["scheduler"]["queue_wait_ms"]["p95"] == 7
assert report["metrics"]["scheduler"]["generation_latency_ms"]["p95"] == 321
assert any(path.startswith("/api/v1/chat/local-scheduler/jobs/job_async_eval_001") for path in paths)
PY

echo "eval run-suite live async job contract PASS"
