#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail=0

check() {
  local label="$1"
  local pattern="$2"
  shift 2
  local output
  if output="$(rg -n -S "$pattern" "$@" 2>/dev/null)"; then
    printf '\n[scheduler-boundary] %s\n%s\n' "$label" "$output" >&2
    fail=1
  fi
}

SERVER_PATHS=(rust/crates/attune-server/src rust/crates/attune-server/ui/src)
INSTALL_PATHS=(
  apps/attune-desktop/scripts
  scripts/deploy-linux.sh
  scripts/install-local.sh
)

check "direct local runtime symbols must not appear in server/ui" \
  'OllamaLlmProvider|OllamaProvider|OrtEmbeddingProvider|OrtRerankProvider|PpOcrProvider::ensure|detect_default_provider|detect_asr|transcribe_with_diarization|ensure_whisper|ensure_sensevoice|LMSTUDIO_ENDPOINT' \
  "${SERVER_PATHS[@]}"

check "direct local runtime endpoints must not appear in server/ui" \
  'localhost:11434|127\.0\.0\.1:11434|/api/tags|/api/ps|/ollama|/lmstudio' \
  "${SERVER_PATHS[@]}"

check "install/deploy scripts must not install or pull direct local runtimes" \
  'curl -fsSL https://ollama\.com/install\.sh|ollama pull|OllamaSetup\.exe|systemctl enable --now ollama|systemctl restart ollama|/api/tags' \
  "${INSTALL_PATHS[@]}"

check "server must use scheduler-aware ingest/parser entrypoints" \
  '\b(ingest_document|ingest_document_replacing|ingest_document_with_profile|parse_bytes_with_profile|parse_file_with_profile|parse_bytes|parse_file|scan_directory)\s*\(' \
  rust/crates/attune-server/src

if [[ "$fail" -ne 0 ]]; then
  printf '\nScheduler boundary audit failed. Local inference must go through scheduler/cloud routing only.\n' >&2
  exit 1
fi

printf 'Scheduler boundary audit passed.\n'
