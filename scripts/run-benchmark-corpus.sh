#!/usr/bin/env bash
#
# W4 J6 — Real-corpus RAG benchmark harness (2026-04-27).
#
# Runs the deterministic-metrics benchmark against locked corpora.
# This is the v0.6.0 GA reproducibility script — output feeds
# docs/benchmarks/2026-Q2-baseline.json.
#
# Prereqs:
#   - cargo + rust toolchain
#   - corpora downloaded (see scripts/download-corpora.sh — TODO: add separate script)
#   - Ollama running with bge-m3 embedding model (or LLM provider configured)
#
# Usage:
#   bash scripts/run-benchmark-corpus.sh [output_json]
# Defaults to docs/benchmarks/2026-Q2-baseline.json.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_JSON="${1:-$PROJECT_ROOT/docs/benchmarks/2026-Q2-baseline.json}"

cd "$PROJECT_ROOT/rust"

echo "[J6] Building attune-server (release)..."
cargo build --release --bin attune-server

VAULT_DIR="$(mktemp -d -t attune-bench-vault-XXXX)"
echo "[J6] Using ephemeral vault at $VAULT_DIR"

cleanup() {
    echo "[J6] Cleaning up..."
    if [[ -n "${SERVER_PID:-}" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$VAULT_DIR"
}
trap cleanup EXIT

ATTUNE_VAULT_DIR="$VAULT_DIR" \
ATTUNE_BENCH_MODE=1 \
    ./target/release/attune-server &
SERVER_PID=$!

echo "[J6] Waiting for server to come up..."
for i in {1..30}; do
    if curl -sf "http://localhost:18900/api/v1/status" >/dev/null 2>&1; then
        echo "[J6] Server up."
        break
    fi
    sleep 1
done

# TODO (v0.6.0 GA PR): unlock vault, bind corpora, wait for indexing, run benchmark
# Skeleton for the GA reviewer:
#   1. POST /api/v1/vault/init with bench password
#   2. POST /api/v1/vault/unlock
#   3. POST /api/v1/index/bind for each corpus in queries.json _corpus_pins
#   4. Poll /api/v1/index/status until done (timeout 30 min)
#   5. cargo test --release -p attune-core --test rag_quality_benchmark -- \
#        --ignored --nocapture > /tmp/raw.txt
#   6. parse raw.txt → write $OUT_JSON

cat <<EOF >"$OUT_JSON"
{
  "_status": "placeholder — v0.6.0 GA benchmark PR will replace this",
  "_generated_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "_attune_version": "v0.6.0-rc.1",
  "_methodology_doc": "docs/benchmarks/2026-Q2.md",
  "scenarios": []
}
EOF

echo "[J6] Wrote placeholder $OUT_JSON"
echo "[J6] To complete: implement steps 1-6 in this script per the TODO above."
