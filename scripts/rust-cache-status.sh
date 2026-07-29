#!/usr/bin/env bash
set -euo pipefail

export SCCACHE_DIR="${SCCACHE_DIR:-/data/cache/sccache}"
export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-20G}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "== disk =="
df -h / /data

echo
echo "== rust cache dirs =="
du -sh \
  "$ROOT/rust/target" \
  /data/cargo-target/attune \
  /data/cargo-target/attune-pro \
  /data/cache/sccache \
  /data/attune-pro-target \
  /data/tmp/attune-core-agent-target \
  /data/tmp/attune-core-target \
  /data/tmp/attune-pro-target \
  /data/tmp/sv-poc-target \
  2>/dev/null || true

echo
echo "== cargo config =="
for cfg in "$HOME/.cargo/config.toml" "$ROOT/.cargo/config.toml" "$ROOT/rust/.cargo/config.toml" /data/company/project/attune-pro/.cargo/config.toml; do
  if [[ -f "$cfg" ]]; then
    echo "-- $cfg"
    sed -n '1,120p' "$cfg"
  fi
done

echo
echo "== sccache =="
if command -v sccache >/dev/null 2>&1; then
  command -v sccache
  sccache --show-stats || true
else
  echo "sccache: not installed; cargo target-dir policy still applies"
fi
