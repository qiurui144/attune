#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-safe}"

ensure_no_active_rust_builds() {
  local roots=("$ROOT/rust" /data/company/project/attune-pro)
  local targets=(
    "$ROOT/rust/target"
    /data/cargo-target/attune
    /data/cargo-target/attune-pro
    /data/attune-pro-target
    /data/tmp/attune-core-agent-target
    /data/tmp/attune-core-target
    /data/tmp/attune-pro-target
    /data/tmp/sv-poc-target
  )

  for proc in /proc/[0-9]*; do
    local exe base cwd cmdline path
    exe="$(readlink "$proc/exe" 2>/dev/null || true)"
    base="${exe##*/}"
    case "$base" in
      cargo|rustc|rustdoc) ;;
      *) continue ;;
    esac

    cwd="$(readlink "$proc/cwd" 2>/dev/null || true)"
    cmdline="$(tr '\0' ' ' < "$proc/cmdline" 2>/dev/null || true)"

    for path in "${roots[@]}" "${targets[@]}"; do
      [[ -n "$path" ]] || continue
      if [[ "$cwd" == "$path"* || "$cmdline" == *"$path"* ]]; then
        echo "active Rust build detected; skip cleanup: pid=${proc##*/} exe=$exe" >&2
        echo "cmdline: $cmdline" >&2
        exit 2
      fi
    done
  done
}

is_running_from() {
  local prefix="$1"
  for exe in /proc/[0-9]*/exe; do
    local target
    target="$(readlink "$exe" 2>/dev/null || true)"
    [[ "$target" == "$prefix"* ]] && return 0
  done
  return 1
}

remove_dir_if_idle() {
  local dir="$1"
  [[ -e "$dir" ]] || return 0
  if is_running_from "$dir"; then
    echo "skip running target: $dir" >&2
    return 0
  fi
  echo "rm -rf $dir"
  rm -rf "$dir"
}

echo "== before =="
"$ROOT/scripts/rust-cache-status.sh"

echo
echo "== clean target dirs =="
ensure_no_active_rust_builds
remove_dir_if_idle "$ROOT/rust/target"
remove_dir_if_idle /data/attune-pro-target
remove_dir_if_idle /data/tmp/attune-core-agent-target
remove_dir_if_idle /data/tmp/attune-core-target
remove_dir_if_idle /data/tmp/attune-pro-target
remove_dir_if_idle /data/tmp/sv-poc-target

if [[ "$MODE" == "all" ]]; then
  remove_dir_if_idle /data/cargo-target/attune
  remove_dir_if_idle /data/cargo-target/attune-pro
  echo "stop sccache server"
  SCCACHE_DIR="${SCCACHE_DIR:-/data/cache/sccache}" sccache --stop-server || true
  echo "rm -rf /data/cache/sccache"
  rm -rf /data/cache/sccache
else
  echo "safe mode keeps /data/cargo-target/* and bounded /data/cache/sccache"
fi

echo
echo "== after =="
"$ROOT/scripts/rust-cache-status.sh"
