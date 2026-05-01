#!/bin/sh
#
# attune Linux package post-install hook — Ollama 自动安装 + 硬件自适应
# (R-deploy / 2026-05-01)
#
# 触发时机：apt install attune-desktop_*.{deb,rpm} 解压完成后由 dpkg/rpm 调用。
# 失败时 dpkg/rpm 会回滚安装，所以这里要：
#   - 不交互（无 stdin/tty）
#   - 单一可恢复路径（每步带 || true 退化）
#   - 网络失败不阻断（首次启动 attune 时 wizard 再补）
#   - 操作幂等（重装/升级不破坏已有 Ollama 配置）
#
# 不做的事：
#   - 拉模型（耗时 + 占带宽 + 没 progress UI；交给 attune-desktop 首次启动 wizard）
#   - 创建 vault（用户数据，留给 setup 流程）

set -e

# dpkg/rpm postinst 使用 /bin/sh + 受限 PATH (/usr/sbin:/usr/bin:/sbin:/bin)，
# 不含 /usr/local/bin。Ollama install.sh 默认装到 /usr/local/bin/ollama —
# 必须显式扩展 PATH，否则 `command -v ollama` 永远找不到，重复触发 install.sh。
PATH="/usr/local/bin:/usr/local/sbin:$PATH"
export PATH

LOG_TAG="attune-postinst"
log() { logger -t "$LOG_TAG" -- "$1"; printf '[attune-postinst] %s\n' "$1"; }

# ─── 1. 平台 sanity ─────────────────────────────────────────────────
if [ "$(uname -s)" != "Linux" ]; then
  log "non-Linux platform; skipping post-install hooks."
  exit 0
fi

# ─── 2. Ollama 安装（缺失时）─────────────────────────────────────────
if command -v ollama >/dev/null 2>&1; then
  log "Ollama already present: $(ollama --version 2>&1 | head -1)"
else
  log "Ollama not found. Installing via official script (~600 MB download)..."
  if command -v curl >/dev/null 2>&1; then
    if curl -fsSL https://ollama.com/install.sh | sh >/dev/null 2>&1; then
      log "Ollama installed: $(ollama --version 2>&1 | head -1)"
    else
      log "WARN: ollama install failed (network?). User can re-run via 'attune deploy' later."
    fi
  else
    log "WARN: curl missing; cannot install Ollama. apt install curl + re-run dpkg/rpm-reconfigure attune-desktop"
  fi
fi

# ─── 3. AMD APU/iGPU HSA override ──────────────────────────────────
if [ -d /sys/class/kfd/kfd/topology/nodes ]; then
  GFX=""
  for props in /sys/class/kfd/kfd/topology/nodes/*/properties; do
    [ -r "$props" ] || continue
    NODE_V=$(awk '/^gfx_target_version / {print $2; exit}' "$props" 2>/dev/null || true)
    if [ -n "$NODE_V" ] && [ "$NODE_V" != "0" ]; then
      MAJOR=$((NODE_V / 10000))
      MINOR=$(((NODE_V / 100) % 100))
      STEP=$((NODE_V % 100))
      GFX=$(printf 'gfx%d%x%x' "$MAJOR" "$MINOR" "$STEP")
      break
    fi
  done

  if [ -n "$GFX" ]; then
    log "AMD GPU detected: $GFX"
    # 决定 override：APU/iGPU 通常需要
    OVERRIDE=""
    case "$GFX" in
      gfx1103|gfx1102|gfx1150|gfx1151) OVERRIDE="11.0.0" ;;  # Phoenix/Hawk Point/Strix
      gfx1036|gfx1035|gfx1034|gfx1033|gfx1032|gfx1031|gfx1030) OVERRIDE="10.3.0" ;;  # Rembrandt/Yellow Carp
      gfx900|gfx906|gfx908|gfx90a|gfx940|gfx942|gfx1100|gfx1101|gfx1200|gfx1201) OVERRIDE="" ;;  # 原生支持
      *) log "WARN: unmapped $GFX; skipping HSA override (you can set manually)" ;;
    esac

    if [ -n "$OVERRIDE" ]; then
      DROPIN=/etc/systemd/system/ollama.service.d/hsa-override.conf
      mkdir -p "$(dirname "$DROPIN")"
      # 不覆盖用户已有 drop-in（防破坏自定义配置）
      if [ ! -f "$DROPIN" ]; then
        cat > "$DROPIN" <<EOF
[Service]
Environment="HSA_OVERRIDE_GFX_VERSION=$OVERRIDE"
Environment="OLLAMA_NUM_PARALLEL=4"
Environment="OLLAMA_KEEP_ALIVE=24h"
# 由 attune-desktop postinst 生成 ($(date -Iseconds))
# 升级保留；卸载时由 prerm 移除
EOF
        log "wrote $DROPIN with HSA_OVERRIDE_GFX_VERSION=$OVERRIDE"
        systemctl daemon-reload >/dev/null 2>&1 || true
      else
        log "$DROPIN exists; not overwriting (user customization preserved)"
      fi
    fi
  fi
fi

# ─── 4. Ollama 服务启用 ─────────────────────────────────────────────
if [ -e /etc/systemd/system/ollama.service ] || [ -e /lib/systemd/system/ollama.service ] || [ -e /usr/lib/systemd/system/ollama.service ]; then
  systemctl enable --now ollama >/dev/null 2>&1 || log "WARN: systemctl enable ollama failed"
  # 等 API 上线（最多 10s）
  i=0
  while [ "$i" -lt 10 ]; do
    if curl -sf http://localhost:11434/api/version >/dev/null 2>&1; then
      log "Ollama API ready @ localhost:11434"
      break
    fi
    sleep 1
    i=$((i+1))
  done
fi

# ─── 5. 总结 ───────────────────────────────────────────────────────
log "post-install complete."
log "next: launch 'Attune' (model pull happens in first-run wizard with progress UI)"

# 永不让 postinst 失败 — 阻塞 dpkg/rpm 比 Ollama 缺失更糟
exit 0
