#!/bin/sh
#
# attune Linux package pre-install hook
# 触发：dpkg/rpm 解压 attune-desktop_*.{deb,rpm} 之前。
# 任务：保证升级时干净停 — 阻止"装了一半但旧版还在跑"竞态。
#

set -e
LOG_TAG="attune-preinst"
log() { logger -t "$LOG_TAG" -- "$1"; printf '[attune-preinst] %s\n' "$1"; }

attune_process_pids() {
  for proc in /proc/[0-9]*; do
    [ -d "$proc" ] || continue
    pid="${proc#/proc/}"
    [ "$pid" != "$$" ] || continue
    exe="$(readlink "$proc/exe" 2>/dev/null || true)"
    base="${exe##*/}"
    case "$base" in
      attune-server-headless|attune-desktop)
        printf '%s\n' "$pid"
        ;;
    esac
  done
}

signal_attune_processes() {
  signal="$1"
  pids="$(attune_process_pids)"
  [ -n "$pids" ] || return 0
  kill "-$signal" $pids 2>/dev/null || true
}

ACTION="${1:-install}"
log "action=$ACTION"

# 升级路径 (action=upgrade) 时，先优雅停旧版本进程
if [ "$ACTION" = "upgrade" ] || [ "$ACTION" = "install" ]; then
  if [ -n "$(attune_process_pids)" ]; then
    log "stopping running attune processes for clean upgrade..."
    signal_attune_processes TERM
    # 给 graceful shutdown 30s（与 R35 设计一致）
    i=0
    while [ "$i" -lt 30 ] && [ -n "$(attune_process_pids)" ]; do
      sleep 1
      i=$((i+1))
    done
    signal_attune_processes KILL
  fi
fi

exit 0
