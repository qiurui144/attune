#!/bin/sh
#
# attune Linux package pre-remove hook
# 触发：apt remove / dnf remove / apt purge 解除安装前。
# 任务：停止 attune 进程；只清理由旧版 Attune 创建的 worker shim。
#

set -e
LOG_TAG="attune-prerm"
log() { logger -t "$LOG_TAG" -- "$1"; printf '[attune-prerm] %s\n' "$1"; }

attune_process_pids() {
  for proc in /proc/[0-9]*; do
    [ -d "$proc" ] || continue
    pid="${proc#/proc/}"
    [ "$pid" != "$$" ] || continue
    exe="$(readlink "$proc/exe" 2>/dev/null || true)"
    base="${exe##*/}"
    case "$base" in
      attune-server-headless|attune-desktop|attune)
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

# 1. 杀任何在跑的 attune-server / attune-desktop 进程
if [ -n "$(attune_process_pids)" ]; then
  log "stopping attune processes..."
  signal_attune_processes TERM
  sleep 2
  signal_attune_processes KILL
fi

# 2. 移除旧版 Attune 写入的 Ollama HSA override（仅当有明确 marker）。
# 不重启 Ollama：Attune 不再管理具体本地推理 worker。
DROPIN=/etc/systemd/system/ollama.service.d/hsa-override.conf
if [ -f "$DROPIN" ] && grep -q 'attune-desktop postinst' "$DROPIN" 2>/dev/null; then
  log "removing legacy worker drop-in $DROPIN (was set by old attune postinst)"
  rm -f "$DROPIN"
  rmdir /etc/systemd/system/ollama.service.d 2>/dev/null || true
  systemctl daemon-reload >/dev/null 2>&1 || true
fi

log "prerm complete (third-party runtimes + 用户数据 preserved)"
exit 0
