#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROMPT="$ROOT/rust/crates/attune-core/assets/plugins/oss_rag_default/prompt.md"
PROFILE="$ROOT/rust/crates/attune-core/assets/plugins/oss_rag_default/plugin.yaml"

require_contains() {
  local file="$1"
  local needle="$2"
  if ! grep -Fq "$needle" "$file"; then
    echo "missing required OSS RAG prompt/profile contract in $file: $needle" >&2
    exit 1
  fi
}

require_absent() {
  local file="$1"
  local pattern="$2"
  if grep -Eiq "$pattern" "$file"; then
    echo "OSS RAG prompt/profile must not contain corpus-specific or answer-level steering: $pattern" >&2
    grep -Ein "$pattern" "$file" >&2 || true
    exit 1
  fi
}

require_contains "$PROMPT" "Reasoning contract for small local models"
require_contains "$PROMPT" "Classify the user request"
require_contains "$PROMPT" "Build an evidence map"
require_contains "$PROMPT" "Check whether the question is underspecified"
require_contains "$PROMPT" "Verify every claim"
require_contains "$PROMPT" "Do not copy an answer pattern from previous manuals"

require_contains "$PROFILE" "scope_terms: []"

for file in "$PROMPT" "$PROFILE"; do
  require_absent "$file" 'RTOS_DMAC|Linux_DMAC|hal_dma|dma_request_chan|sunxi_dma'
  require_absent "$file" '/mnt/hdd/allwinner|/mnt/hdd/rockchip|tina-v821|V821'
  require_absent "$file" 'scope_terms: \[[^]]*(rtos|linux|android|baremetal|u-boot)'
done

echo "oss-rag-prompt-contract: PASS"
