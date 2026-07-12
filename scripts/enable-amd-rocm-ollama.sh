#!/usr/bin/env bash
# Legacy helper: 为自管 Ollama systemd 服务启用 AMD ROCm（iGPU / APU 适配）
#
# Attune 默认不再安装、启动或调优 Ollama。生产路径应通过 cloud/BYOK
# 或 edge scheduler 收口；只有维护旧的自管 Ollama 环境时才使用本脚本。
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
#   ATTUNE_ALLOW_LEGACY_DIRECT_OLLAMA=1 ./scripts/enable-amd-rocm-ollama.sh           # 自动检测
#   ATTUNE_ALLOW_LEGACY_DIRECT_OLLAMA=1 ./scripts/enable-amd-rocm-ollama.sh 11.0.0   # 指定覆盖版本
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

if [ "${ATTUNE_ALLOW_LEGACY_DIRECT_OLLAMA:-0}" != "1" ]; then
  echo "error: this is a legacy direct-Ollama helper." >&2
  echo "       Attune default paths use cloud/BYOK or an edge scheduler." >&2
  echo "       Set ATTUNE_ALLOW_LEGACY_DIRECT_OLLAMA=1 to manage a self-hosted Ollama runtime." >&2
  exit 2
fi

# 1. 获取 gfx target（KFD topology，跳过 CPU 节点即 gfx_target_version=0 的）
GFX=""
V=""
for props in /sys/class/kfd/kfd/topology/nodes/*/properties; do
  [ -r "$props" ] || continue
  NODE_V=$(awk '/^gfx_target_version / {print $2; exit}' "$props")
  if [ -n "$NODE_V" ] && [ "$NODE_V" != "0" ]; then
    V=$NODE_V
    MAJOR=$((V / 10000))
    MINOR=$(((V / 100) % 100))
    STEP=$((V % 100))
    GFX=$(printf "gfx%d%x%x" "$MAJOR" "$MINOR" "$STEP")
    echo "[detect] $props → $V → $GFX"
    break
  fi
done

if [ -z "$GFX" ]; then
  echo "error: no AMD GPU gfx_target_version found under /sys/class/kfd/kfd/topology/nodes/*/properties" >&2
  echo "       Is this an AMD system with AMDGPU + amdkfd kernel modules loaded?" >&2
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
# 提高并行度：attune queue worker batch=32，配合 NUM_PARALLEL=4
# 让 Ollama 能同时处理 4 个请求，充分利用 GPU（APU 共享内存够大）
Environment="OLLAMA_NUM_PARALLEL=4"
# 默认 5m 关会让 qwen2.5:3b 反复冷启动。长驻 VRAM 消除 60s 冷启动
Environment="OLLAMA_KEEP_ALIVE=24h"
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
