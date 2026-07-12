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
#   - cloud LLM or edge scheduler configured for embedding/chat where required
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

cat >&2 <<'EOF'
[J6] Real-corpus benchmark is not implemented yet.

Refusing to write a placeholder baseline. A valid implementation must:
  1. initialize and unlock the ephemeral vault;
  2. bind every corpus pinned by rust/tests/golden/queries.json;
  3. wait for indexing to complete;
  4. execute the queries against /api/v1/search;
  5. aggregate Hit@K / Recall@K / MRR;
  6. write the requested output JSON only from real measured results.
EOF
exit 2
