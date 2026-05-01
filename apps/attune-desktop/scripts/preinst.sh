#!/bin/sh
#
# attune Linux package pre-install hook
# 触发：dpkg/rpm 解压 attune-desktop_*.{deb,rpm} 之前。
# 任务：保证升级时干净停 — 阻止"装了一半但旧版还在跑"竞态。
#

set -e
LOG_TAG="attune-preinst"
log() { logger -t "$LOG_TAG" -- "$1"; printf '[attune-preinst] %s\n' "$1"; }

ACTION="${1:-install}"
log "action=$ACTION"

# 升级路径 (action=upgrade) 时，先优雅停旧版本进程
if [ "$ACTION" = "upgrade" ] || [ "$ACTION" = "install" ]; then
  if pgrep -f 'attune-server-headless|attune-desktop' >/dev/null 2>&1; then
    log "stopping running attune processes for clean upgrade..."
    pkill -TERM -f 'attune-server-headless|attune-desktop' || true
    # 给 graceful shutdown 30s（与 R35 设计一致）
    i=0
    while [ "$i" -lt 30 ] && pgrep -f 'attune-server-headless|attune-desktop' >/dev/null 2>&1; do
      sleep 1
      i=$((i+1))
    done
    pkill -KILL -f 'attune-server-headless|attune-desktop' 2>/dev/null || true
  fi
fi

exit 0
