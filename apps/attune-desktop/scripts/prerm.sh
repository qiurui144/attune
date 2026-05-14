#!/bin/sh
#
# attune Linux package pre-remove hook
# 触发：apt remove / dnf remove / apt purge 解除安装前。
# 任务：停止 attune 进程，但 **不动 Ollama**（用户可能还需要）。
#

set -e
LOG_TAG="attune-prerm"
log() { logger -t "$LOG_TAG" -- "$1"; printf '[attune-prerm] %s\n' "$1"; }

# 1. 杀任何在跑的 attune-server / attune-desktop 进程（不影响 Ollama）
if pgrep -f 'attune-server-headless|attune-desktop|attune ' >/dev/null 2>&1; then
  log "stopping attune processes..."
  pkill -TERM -f 'attune-server-headless|attune-desktop|attune ' || true
  sleep 2
  pkill -KILL -f 'attune-server-headless|attune-desktop|attune ' 2>/dev/null || true
fi

# 2. 移除 systemd HSA override（仅当是我们写的）
DROPIN=/etc/systemd/system/ollama.service.d/hsa-override.conf
if [ -f "$DROPIN" ] && grep -q 'attune-desktop postinst' "$DROPIN" 2>/dev/null; then
  log "removing $DROPIN (was set by attune postinst)"
  rm -f "$DROPIN"
  rmdir /etc/systemd/system/ollama.service.d 2>/dev/null || true
  systemctl daemon-reload >/dev/null 2>&1 || true
  systemctl restart ollama >/dev/null 2>&1 || true
fi

log "prerm complete (Ollama runtime + 用户数据 preserved)"
exit 0
