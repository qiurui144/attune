#!/bin/sh
#
# attune .deb post-remove hook
# 触发：apt remove (action=remove) 或 apt purge (action=purge)
#
# remove → 仅清 binary，保留用户数据 + Ollama（默认 apt 行为）
# purge  → 用户主动清完整状态：删 ~/.local/share/attune（用户数据）
#          但 **依然不动 Ollama**（独立软件，可能其他应用在用）
#

set -e
LOG_TAG="attune-postrm"
log() { logger -t "$LOG_TAG" -- "$1"; printf '[attune-postrm] %s\n' "$1"; }

ACTION="${1:-remove}"
log "action=$ACTION"

case "$ACTION" in
  purge)
    # purge：清所有 attune 数据，但不动用户的 vault 备份（在用户家目录下，apt 不该越权）
    # 仅清系统级缓存
    log "purge requested. NOT removing ~/.local/share/attune (user data preserved by design)."
    log "to fully wipe: rm -rf ~/.local/share/attune ~/.config/npu-vault"
    log "to remove Ollama: 'sudo systemctl disable --now ollama' + 'sudo rm /usr/local/bin/ollama' (separate decision)"
    ;;
  remove)
    log "remove complete (data + Ollama preserved)"
    ;;
  *)
    # upgrade / failed-upgrade / disappear — 都是 dpkg 内部状态，不做实际清理
    log "no-op for action=$ACTION"
    ;;
esac

exit 0
