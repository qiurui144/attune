#!/usr/bin/env bash
# 为 Ollama systemd 服务启用 AMD ROCm（iGPU / APU 适配）
#
# 背景：AMD Radeon 780M (Phoenix/Hawk Point/Strix APU) 的 gfx target 是 gfx1103，
# 不在 ROCm 官方白名单内，需要 HSA_OVERRIDE_GFX_VERSION 覆盖为一个被支持的值
# （通常是 11.0.0 = gfx1100）让 ROCm runtime 接受并下发计算。
#
# 本脚本会：
#   1. 检测 gfx target
#   2. 写 /etc/systemd/system/ollama.service.d/hsa-override.conf drop-in
#   3. daemon-reload + restart ollama
#
# 需 sudo；CLAUDE.md 记录的密码 123123 可在交互时输入。
#
# 用法：
#   ./scripts/enable-amd-rocm-ollama.sh           # 自动检测
#   ./scripts/enable-amd-rocm-ollama.sh 11.0.0   # 指定覆盖版本
#   ./scripts/enable-amd-rocm-ollama.sh --revert  # 移除配置

set -euo pipefail

DROPIN=/etc/systemd/system/ollama.service.d/hsa-override.conf

if [ "${1:-}" = "--revert" ]; then
  echo "[revert] removing $DROPIN"
  sudo rm -f "$DROPIN"
  sudo systemctl daemon-reload
  sudo systemctl restart ollama || true
  echo "[revert] done. Ollama restarted without HSA override."
  exit 0
fi

# 1. 获取 gfx target
GFX=""
for card in /sys/class/drm/card0/device/gfx_target_version \
            /sys/class/drm/card1/device/gfx_target_version; do
  if [ -r "$card" ]; then
    V=$(cat "$card")
    # 形如 110300 → 11.0.3（gfx1103），100301 → 10.3.1
    MAJOR=$((V / 10000))
    MINOR=$(((V / 100) % 100))
    STEP=$((V % 100))
    GFX=$(printf "gfx%d%x%x" "$MAJOR" "$MINOR" "$STEP")
    echo "[detect] $card → $V → $GFX"
    break
  fi
done

if [ -z "$GFX" ]; then
  echo "error: no AMD GPU gfx_target_version found. Is this an AMD system with AMDGPU driver?" >&2
  exit 1
fi

# 2. 决定 HSA override
OVERRIDE="${1:-}"
if [ -z "$OVERRIDE" ]; then
  case "$GFX" in
    gfx1103|gfx1102|gfx1150|gfx1151) OVERRIDE="11.0.0" ;;  # Phoenix / Hawk Point / Strix APU
    gfx1036|gfx1035|gfx1034|gfx1033|gfx1032|gfx1031|gfx1030) OVERRIDE="10.3.0" ;;  # Rembrandt / Yellow Carp etc.
    gfx900|gfx906|gfx908|gfx90a|gfx940|gfx942|gfx1100|gfx1101|gfx1200|gfx1201)
      echo "[detect] $GFX is natively supported, no override needed."
      exit 0
      ;;
    *)
      echo "error: $GFX not in known mapping. Pass override explicitly: $0 11.0.0" >&2
      exit 1
      ;;
  esac
fi

echo "[apply] $GFX → HSA_OVERRIDE_GFX_VERSION=$OVERRIDE"

# 3. 写 drop-in 并重启
sudo mkdir -p "$(dirname "$DROPIN")"
sudo tee "$DROPIN" > /dev/null <<EOF
[Service]
Environment="HSA_OVERRIDE_GFX_VERSION=$OVERRIDE"
# attune: 该 drop-in 由 scripts/enable-amd-rocm-ollama.sh 生成
# 撤销：./scripts/enable-amd-rocm-ollama.sh --revert
EOF

sudo systemctl daemon-reload
sudo systemctl restart ollama
sleep 2

# 4. 验证
if systemctl is-active --quiet ollama; then
  echo "[ok] ollama restarted with HSA_OVERRIDE_GFX_VERSION=$OVERRIDE"
  echo "[hint] run a model (e.g. 'ollama run qwen2.5:3b \"hi\"') then check 'nvidia-smi'-equivalent:"
  echo "       radeontop  OR  rocm-smi  OR  journalctl -u ollama --since '1 min ago' | grep -iE 'gpu|cuda|rocm'"
else
  echo "error: ollama failed to start. Check 'journalctl -u ollama'" >&2
  exit 1
fi
