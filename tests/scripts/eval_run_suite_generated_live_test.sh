#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_ROOT="$(mktemp -d -t attune-eval-generated-live-XXXXXX)"
PORT_FILE="$(mktemp -t attune-eval-generated-port-XXXXXX)"
LOG_FILE="$(mktemp -t attune-eval-generated-log-XXXXXX.jsonl)"
OUT="$(mktemp -t attune-eval-generated-report-XXXXXX.json)"

mkdir -p "$TMP_ROOT/scripts/eval" "$TMP_ROOT/tests/eval/corpora/security" "$TMP_ROOT/tests/eval/scenarios/security" "$TMP_ROOT/tests/eval/suites"
cp "$ROOT/scripts/eval/generate-scale-corpus.py" "$TMP_ROOT/scripts/eval/generate-scale-corpus.py"
cp "$ROOT/scripts/eval/validate-manifests.py" "$TMP_ROOT/scripts/eval/validate-manifests.py"

cat > "$TMP_ROOT/tests/eval/corpora/security/generated_live_three.json" <<'JSON'
{
  "schema_version": "attune.eval.corpus.v1",
  "corpus_id": "generated_live_three",
  "domain": "security",
  "license": "generated-test-fixture",
  "source": {
    "type": "generated",
    "generator": "scripts/eval/generate-scale-corpus.py",
    "command": "python3 scripts/eval/generate-scale-corpus.py --documents 3 --domains security --out <workspace>"
  },
  "scale": {"tier": "T0", "documents": 3, "expected_chunks": 3},
  "profiles": {"generated_live_three": {"documents": ["generated:security:3"]}},
  "indexing": {"parser_modes": ["markdown"], "max_pending_seconds": 5}
}
JSON

cat > "$TMP_ROOT/tests/eval/scenarios/security/generated_live_security.json" <<'JSON'
{
  "schema_version": "attune.eval.scenario.v1",
  "scenario_id": "generated_live_security",
  "domain": "security",
  "scenario_type": "fact_lookup",
  "difficulty": "smoke",
  "corpus_id": "generated_live_three",
  "turns": [
    {
      "turn_id": "access_control",
      "message": "access control evidence?",
      "answer_mode": "fact_lookup",
      "requires_citations": true,
      "expected_sources": ["security::access-control"],
      "must_include": ["access control"],
      "must_not_include": ["networking"],
      "latency_budget_ms": 5000
    }
  ]
}
JSON

cat > "$TMP_ROOT/tests/eval/suites/generated_live_suite.json" <<'JSON'
{
  "schema_version": "attune.eval.suite.v1",
  "suite_id": "generated_live_suite",
  "purpose": "Generated corpus live upload contract.",
  "corpora": ["generated_live_three"],
  "scenarios": ["generated_live_security"],
  "gates": ["manifest", "generated_corpus_materialization", "ingest", "search", "chat"],
  "thresholds": {"retrieval_hit_at_5_min": 1.0, "citation_hit_rate_min": 1.0, "answer_accuracy_min": 1.0}
}
JSON

python3 - "$PORT_FILE" "$LOG_FILE" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

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
        append_log({"method": "GET", "path": parsed.path})
        if parsed.path == "/api/v1/status":
            self.send_json(200, {"version": "fake", "pending_embeddings": 0})
        elif parsed.path == "/api/v1/search":
            self.send_json(200, {"latency_ms": 1, "results": [{"title": "security::access-control"}]})
        else:
            self.send_json(404, {"error": "not_found"})

    def do_POST(self):
        parsed = urlparse(self.path)
        length = int(self.headers.get("content-length") or "0")
        body = self.rfile.read(length) if length else b""
        append_log({"method": "POST", "path": parsed.path, "bytes": len(body), "body_excerpt": body[:500].decode("utf-8", "replace")})
        if parsed.path == "/api/v1/upload":
            self.send_json(200, {"id": "uploaded", "status": "ready"})
        elif parsed.path == "/api/v1/chat":
            self.send_json(200, {
                "content": "access control evidence is cited.",
                "latency_ms": 1,
                "citations": [{"title": "security::access-control"}]
            })
        else:
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
python3 "$ROOT/scripts/eval/run-suite.py" --root "$TMP_ROOT" --suite generated_live_suite --base-url "http://127.0.0.1:$PORT" --out "$OUT"

python3 - "$OUT" "$LOG_FILE" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
logs = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8") if line.strip()]
uploads = [row for row in logs if row["path"] == "/api/v1/upload"]
assert report["summary"]["pass"] is True
assert report["metrics"]["api"]["uploads"] == 3
assert len(uploads) == 3
assert any("security::access-control" in row.get("body_excerpt", "") for row in uploads)
PY

echo "eval run-suite generated live contract PASS"
