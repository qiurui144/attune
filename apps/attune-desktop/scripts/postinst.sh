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
# 兜底：新版 Ollama install.sh 在某些发行版（Ubuntu 25.10+/26.04）跳过 systemd 配置。
# 我们检测到 binary 但无 unit 时自己写一份最小化 unit，保证服务可启。
if command -v ollama >/dev/null 2>&1; then
  HAS_UNIT=0
  for cand in /etc/systemd/system/ollama.service /lib/systemd/system/ollama.service /usr/lib/systemd/system/ollama.service; do
    [ -e "$cand" ] && HAS_UNIT=1 && break
  done

  if [ "$HAS_UNIT" = "0" ]; then
    log "Ollama systemd unit missing — writing minimal unit (install.sh skipped systemd setup)"
    # 确保 ollama user/group 存在
    if ! getent group ollama >/dev/null; then groupadd -r ollama; fi
    if ! id ollama >/dev/null 2>&1; then
      useradd -r -s /bin/false -g ollama -d /usr/share/ollama -m ollama 2>/dev/null || true
    fi
    # render/video group 给 GPU 访问权（AMD/NVIDIA 都需要）
    usermod -aG render ollama 2>/dev/null || true
    usermod -aG video ollama 2>/dev/null || true

    cat > /etc/systemd/system/ollama.service <<UNIT
[Unit]
Description=Ollama Service
After=network-online.target

[Service]
ExecStart=/usr/local/bin/ollama serve
User=ollama
Group=ollama
Restart=always
RestartSec=3
Environment="PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

[Install]
WantedBy=default.target
UNIT
    log "wrote /etc/systemd/system/ollama.service"
    systemctl daemon-reload >/dev/null 2>&1 || true
  fi

  systemctl enable --now ollama >/dev/null 2>&1 || log "WARN: systemctl enable ollama failed"
  # 等 API 上线（最多 15s — 首次启动 + AMD ROCm 加载稍慢）
  i=0
  while [ "$i" -lt 15 ]; do
    if curl -sf http://localhost:11434/api/version >/dev/null 2>&1; then
      log "Ollama API ready @ localhost:11434"
      break
    fi
    sleep 1
    i=$((i+1))
  done
fi

# ─── 5. 模型拉取（按 RAM + GPU tier 自适应）─────────────────────────
# 用户拍板：必须 .deb 安装时一次性完成，不留"首次启动 wizard 再拉"的环节
RAM_GB=$(free -g 2>/dev/null | awk '/^Mem:/{print $2}' || echo 0)
HAS_NVIDIA=0
[ -e /dev/nvidia0 ] && HAS_NVIDIA=1
HAS_AMD=0
[ -n "${GFX:-}" ] && HAS_AMD=1

# 决策矩阵：与 scripts/deploy-linux.sh 的 tier 表保持一致
if [ "$RAM_GB" -ge 16 ] && [ "$HAS_NVIDIA" = "1" ]; then
  EMBED_MODEL="bge-m3"; CHAT_MODEL="qwen2.5:7b"; TIER="high"
elif [ "$RAM_GB" -ge 16 ] && [ "$HAS_AMD" = "1" ]; then
  EMBED_MODEL="bge-m3"; CHAT_MODEL="qwen2.5:3b"; TIER="mid"
elif [ "$RAM_GB" -ge 8 ]; then
  EMBED_MODEL="bge-small"; CHAT_MODEL="qwen2.5:1.5b"; TIER="low"
else
  EMBED_MODEL="bge-small"; CHAT_MODEL="qwen2.5:0.5b"; TIER="minimal"
fi

log "Hardware tier: RAM=${RAM_GB}GB NVIDIA=$HAS_NVIDIA AMD=$HAS_AMD → tier=$TIER"
log "Selected models: embed=$EMBED_MODEL chat=$CHAT_MODEL"

# 仅在 Ollama API 上线时拉模型；否则留下 hint 让用户后续 attune deploy 补
if curl -sf http://localhost:11434/api/version >/dev/null 2>&1; then
  for m in "$EMBED_MODEL" "$CHAT_MODEL"; do
    if ollama list 2>/dev/null | awk 'NR>1 {print $1}' | grep -qx "$m" \
       || ollama list 2>/dev/null | awk 'NR>1 {print $1}' | grep -qx "${m}:latest"; then
      log "  $m already pulled — skipping"
    else
      log "  pulling $m (~minutes; progress in syslog: 'journalctl -t attune-postinst -f')..."
      # ollama pull 在非 TTY 下输出 newline-separated progress；管到 logger 异步记
      if ollama pull "$m" 2>&1 | while IFS= read -r line; do
           # 只记关键转换（避免 1000 行 progress noise 灌爆 syslog）
           case "$line" in
             *success*|*verifying*|*"writing manifest"*|*error*|*Error*)
               logger -t "$LOG_TAG" -- "[pull $m] $line"
               ;;
           esac
         done; then
        log "  $m pulled OK"
      else
        log "  WARN: $m pull failed (network?). User can retry: ollama pull $m"
      fi
    fi
  done
else
  log "WARN: Ollama API not ready, skipping model pull. User can later: attune deploy"
fi

# ─── 6. 总结 ───────────────────────────────────────────────────────
log "post-install complete. Models: $EMBED_MODEL + $CHAT_MODEL"
log "next: launch 'Attune' from desktop menu — fully ready out of the box"

# 永不让 postinst 失败 — 阻塞 dpkg/rpm 比 Ollama 缺失更糟
exit 0
