#!/usr/bin/env bash
#
# Organize engine — multi-model tier matrix for the tier-3 LLM naming path
# (Task 12, per attune CLAUDE.md §「Agent 模型兜底原则」 D/E + ~/.claude/CLAUDE.md §4.5).
#
# WHAT THIS MEASURES
#   Only the *naming* step (OrganizationStrategy::label_cluster) uses an LLM.
#   Grouping (HDBSCAN) and role/kind assignment are deterministic and offline.
#   This harness feeds pre-clustered fixtures to the engine with each model tier
#   and records the produced cluster names so a human can judge naming quality /
#   JSON-parse robustness across tiers, then set the minimum tier in RELEASE.md.
#
#   Tiers (per §4.5 D — weak-local / weak-cloud / strong-cloud):
#     - weak local : qwen2.5:3b      (Ollama)
#     - weak cloud : gemini-1.5-flash | gpt-4o-mini   (BYOK)
#     - product    : deepseek-v4     (attune product default for text agents, §H)
#
# ⚠️  COMPUTE AUTHORIZATION REQUIRED (~/.claude/CLAUDE.md §1.3)
#   This script starts/uses local LLM inference (Ollama) and/or hits paid cloud
#   endpoints. Do NOT run it without explicit per-session approval covering the
#   model name + quant + expected VRAM + duration. It is therefore EXCLUDED from
#   CI: the corresponding Rust gate is #[ignore]d and never auto-runs.
#
# USAGE
#   bash scripts/run-organize-tier-matrix.sh            # all tiers it can reach
#   ORGANIZE_TIERS="qwen2.5:3b" bash scripts/run-organize-tier-matrix.sh
#
# ENV (OpenAI-compatible; defaults are placeholders — never hard-code secrets)
#   OLLAMA_HOST            default http://127.0.0.1:11434
#   ATTUNE_LLM_BASE_URL    cloud OpenAI-compat base (BYOK / gateway)
#   ATTUNE_LLM_API_KEY     read from env ONLY (e.g. source /tmp/secrets-*/key.env)
#
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OLLAMA_HOST="${OLLAMA_HOST:-http://127.0.0.1:11434}"
TIERS="${ORGANIZE_TIERS:-qwen2.5:3b gemini-1.5-flash deepseek-v4}"
OUT_DIR="${1:-$PROJECT_ROOT/reports/runs/organize-tier-matrix-$(date +%Y%m%dT%H%M%S)}"

mkdir -p "$OUT_DIR"
echo "[organize-matrix] output → $OUT_DIR"
echo "[organize-matrix] tiers   → $TIERS"

# Preflight: confirm Ollama only if a local model is requested (read-only check,
# does NOT start the daemon — starting it needs §1.3 approval).
if echo "$TIERS" | grep -Eq 'qwen|llama|phi'; then
  if ! curl -fsS "$OLLAMA_HOST/api/tags" >/dev/null 2>&1; then
    echo "::error:: local tier requested but Ollama not reachable at $OLLAMA_HOST."
    echo "          Start it ONLY after §1.3 approval, then re-run." >&2
    exit 3
  fi
fi

cd "$PROJECT_ROOT/rust"

# The Rust side runs the engine's tier-3 naming path against the golden fixtures
# under crates/attune-core/tests/golden/organize/, once per tier, via the
# #[ignore]d gate. The model name is passed through ATTUNE_MATRIX_MODEL so the
# test selects the right provider/endpoint.
for tier in $TIERS; do
  echo "==================================================================="
  echo "[organize-matrix] tier: $tier"
  log="$OUT_DIR/${tier//[:\/]/_}.log"
  if ATTUNE_MATRIX_MODEL="$tier" TMPDIR=/data \
       cargo test --release -p attune-core --test organize_golden_gate \
       -- --ignored organize_tier_matrix_naming 2>&1 | tee "$log"; then
    echo "[organize-matrix] tier $tier OK → $log"
  else
    echo "[organize-matrix] tier $tier FAILED (see $log) — record as 'tier unusable' in RELEASE.md"
  fi
done

echo "[organize-matrix] done. Review the per-tier logs and set the minimum"
echo "                  supported tier in rust/RELEASE.md per §4.5 E (fallback)."
