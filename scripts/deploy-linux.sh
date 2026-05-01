#!/usr/bin/env bash
#
# attune Linux 一键部署脚本（2026-05-01，R-deploy）
#
# 任务：在 *任何* Linux 机器（裸机或全新 Ubuntu/Debian/Fedora）上把 attune
# 跑起来 — 包含 Ollama 自动安装 + 硬件自适应 + 模型按 RAM/VRAM tier 拉取。
#
# 硬件适配矩阵：
#   - NVIDIA GPU       → Ollama 自动 CUDA 后端，CUDA_VISIBLE_DEVICES=0
#   - AMD APU/iGPU     → 调 enable-amd-rocm-ollama.sh 注入 HSA_OVERRIDE_GFX_VERSION
#   - AMD 独显         → 同上（gfx1100 等可能不需要 override）
#   - CPU only         → Ollama CPU 后端
#
# 模型按 RAM tier 选（**仅 Embedding，LLM 不本地预装**）：
#   ≥16GB RAM → bge-m3 (多语言 1024-dim)
#   <16GB RAM → bge-small (中英 384-dim)
# LLM 走远端 token 默认，K3 一体机镜像才单独预装 qwen2.5:1.5b/3b
#
# 用法：
#   ./scripts/deploy-linux.sh              # full auto
#   ./scripts/deploy-linux.sh --no-models  # 装 Ollama 但不拉模型（快速冒烟）
#   ./scripts/deploy-linux.sh --dry-run    # 只打印计划不执行
#
# 退出码：
#   0 = 成功
#   2 = 不支持的平台
#   3 = Ollama install 失败
#   4 = 模型拉取失败
#   5 = 验证 (embed call) 失败

set -euo pipefail

# ─── 参数解析 ───────────────────────────────────────────────────────
SKIP_MODELS=0
DRY_RUN=0
for arg in "$@"; do
  case "$arg" in
    --no-models) SKIP_MODELS=1 ;;
    --dry-run)   DRY_RUN=1 ;;
    -h|--help)
      sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "unknown arg: $arg (use --help)" >&2; exit 2 ;;
  esac
done

run_cmd() {
  if [ "$DRY_RUN" = "1" ]; then
    echo "[dry-run] $*"
  else
    "$@"
  fi
}

log() { printf "\033[1;36m[deploy]\033[0m %s\n" "$*"; }
warn() { printf "\033[1;33m[warn]\033[0m %s\n" "$*"; }
err()  { printf "\033[1;31m[err]\033[0m %s\n" "$*" >&2; }

# ─── 1. 平台检查 ────────────────────────────────────────────────────
log "step 1/6: platform check"
if [ "$(uname -s)" != "Linux" ]; then
  err "this script is Linux-only (got $(uname -s)). For Windows use deploy-windows.ps1 (TBD)."
  exit 2
fi
ARCH=$(uname -m)
if [ "$ARCH" != "x86_64" ] && [ "$ARCH" != "aarch64" ]; then
  err "unsupported arch: $ARCH"
  exit 2
fi
log "  Linux $ARCH ✓"

# ─── 2. 硬件检测 ────────────────────────────────────────────────────
log "step 2/6: hardware detect"
RAM_GB=$(free -g | awk '/^Mem:/{print $2}')
HW_TIER=""
GPU_KIND="cpu"
GFX_TARGET=""
NVIDIA=0
AMD=0

# NVIDIA via /dev/nvidia* or lspci
if [ -e /dev/nvidia0 ] || lspci 2>/dev/null | grep -qi "vga.*nvidia"; then
  NVIDIA=1
  GPU_KIND="nvidia"
fi

# AMD via /sys/class/kfd
if [ -d /sys/class/kfd/kfd/topology/nodes ]; then
  for props in /sys/class/kfd/kfd/topology/nodes/*/properties; do
    [ -r "$props" ] || continue
    NODE_V=$(awk '/^gfx_target_version / {print $2; exit}' "$props" 2>/dev/null || true)
    if [ -n "$NODE_V" ] && [ "$NODE_V" != "0" ]; then
      AMD=1
      GPU_KIND="amd"
      MAJOR=$((NODE_V / 10000))
      MINOR=$(((NODE_V / 100) % 100))
      STEP=$((NODE_V % 100))
      GFX_TARGET=$(printf "gfx%d%x%x" "$MAJOR" "$MINOR" "$STEP")
      break
    fi
  done
fi

# tier 决策
if [ "$RAM_GB" -ge 16 ] && [ "$NVIDIA" = "1" ]; then
  HW_TIER="high"          # 16GB+ + dGPU
elif [ "$RAM_GB" -ge 16 ] && [ "$AMD" = "1" ]; then
  HW_TIER="mid"           # 16GB+ + APU/iGPU
elif [ "$RAM_GB" -ge 8 ]; then
  HW_TIER="low"           # CPU only, 8-16GB
else
  HW_TIER="minimal"       # <8GB
fi

log "  RAM: ${RAM_GB} GB"
log "  GPU: $GPU_KIND${GFX_TARGET:+ ($GFX_TARGET)}"
log "  tier: $HW_TIER"

# ─── 3. Embedding 模型选择 ──────────────────────────────────────────
# 设计契约（CLAUDE.md "硬件感知的默认底座" + "成本感知与触发契约"）：
#   本地必装 4 底座 = Embedding + Reranker + ASR + OCR（不含 LLM）
#   LLM 走远端 token 默认 — 用户在 wizard 自配；K3 镜像才另装本地 LLM
case "$HW_TIER" in
  high|mid)     EMBED_MODEL="bge-m3" ;;     # 16GB+ → 多语言 1024-dim
  low|minimal)  EMBED_MODEL="bge-small" ;;  # <16GB → 中英 384-dim
esac
log "  embedding: $EMBED_MODEL"
log "  LLM: skipped — 默认远端 token；在 attune 首次启动 wizard 配 cloud API 或选 Ollama"

# ─── 4. Ollama 安装 ─────────────────────────────────────────────────
log "step 3/6: Ollama install check"
if command -v ollama &>/dev/null; then
  OLLAMA_VER=$(ollama --version 2>&1 | head -1)
  log "  already installed: $OLLAMA_VER"
else
  log "  not found, installing via official script..."
  if [ "$DRY_RUN" = "1" ]; then
    log "[dry-run] curl -fsSL https://ollama.com/install.sh | sh"
  else
    if ! curl -fsSL https://ollama.com/install.sh | sh; then
      err "ollama install script failed"
      exit 3
    fi
    log "  Ollama installed: $(ollama --version 2>&1 | head -1)"
  fi
fi

# ─── 5. AMD ROCm 启用（如适用）─────────────────────────────────────
log "step 4/6: GPU runtime config"
if [ "$AMD" = "1" ]; then
  log "  AMD detected — applying HSA override for $GFX_TARGET"
  if [ "$DRY_RUN" = "1" ]; then
    log "[dry-run] sudo bash $(dirname "$0")/enable-amd-rocm-ollama.sh"
  else
    if [ -x "$(dirname "$0")/enable-amd-rocm-ollama.sh" ]; then
      bash "$(dirname "$0")/enable-amd-rocm-ollama.sh" || warn "ROCm enable script failed (continuing with CPU fallback)"
    else
      warn "enable-amd-rocm-ollama.sh not found alongside this script — ROCm not configured"
    fi
  fi
elif [ "$NVIDIA" = "1" ]; then
  log "  NVIDIA detected — Ollama auto-uses CUDA backend (no extra config)"
else
  log "  CPU-only — no GPU runtime to configure"
fi

# ─── 6. 启动 Ollama 服务 ────────────────────────────────────────────
log "step 5/6: start Ollama service"
if [ "$DRY_RUN" = "0" ]; then
  if systemctl list-unit-files 2>/dev/null | grep -q '^ollama\.service'; then
    sudo systemctl enable --now ollama || warn "systemctl enable ollama failed (continuing)"
  fi
  # 等待 API 起来
  for i in $(seq 1 20); do
    if curl -sf http://localhost:11434/api/version &>/dev/null; then
      log "  Ollama API ready @ localhost:11434 (probe $i)"
      break
    fi
    sleep 1
    if [ "$i" = "20" ]; then
      err "Ollama API didn't respond on localhost:11434 after 20s"
      err "  systemd: $(systemctl is-active ollama 2>/dev/null || echo 'not-installed')"
      err "  journal: journalctl -u ollama --since '1 min ago'"
      exit 3
    fi
  done
fi

# ─── 7. 拉 Embedding 模型（不拉 LLM）──────────────────────────────
# LLM 不在本地必装清单 — 用户在 attune wizard 自配 cloud API 或 Ollama
if [ "$SKIP_MODELS" = "1" ]; then
  log "step 6/6: skipping embedding pull (--no-models)"
else
  log "step 6/6: pull embedding model ($EMBED_MODEL)"
  if [ "$DRY_RUN" = "1" ]; then
    log "[dry-run] ollama pull $EMBED_MODEL"
  else
    if ! ollama pull "$EMBED_MODEL"; then
      err "ollama pull $EMBED_MODEL failed"
      exit 4
    fi
  fi

  # 验证 embed call 真的工作
  log "  verify: embedding round-trip"
  if [ "$DRY_RUN" = "0" ]; then
    EMBED_RESP=$(curl -sf http://localhost:11434/api/embeddings \
      -d "{\"model\":\"$EMBED_MODEL\",\"prompt\":\"hello attune\"}" || true)
    if echo "$EMBED_RESP" | grep -q '"embedding"'; then
      DIM=$(echo "$EMBED_RESP" | grep -oE '\[[0-9e\.,\-]+\]' | head -1 | tr ',' '\n' | wc -l)
      log "  ✓ embed OK (dim ≈ $DIM)"
    else
      err "embed call failed — response did not contain 'embedding' key"
      err "$EMBED_RESP" | head -5
      exit 5
    fi
  fi
fi

# ─── 8. 总结 ────────────────────────────────────────────────────────
log "─── deployment summary ───"
log "  hardware:    $GPU_KIND${GFX_TARGET:+ ($GFX_TARGET)} | RAM ${RAM_GB} GB | tier=$HW_TIER"
log "  ollama:      $(ollama --version 2>&1 | head -1)"
log "  embedding:   $EMBED_MODEL (本地必装底座之一)"
log "  LLM:         走远端 token 默认；用户在 attune 首次启动 wizard 配置"
log "  endpoint:    http://localhost:11434"
log ""
log "其他底座（也由 .deb postinst 装好，本脚本仅辅助）："
log "  Reranker:  Xenova/bge-reranker-base (lazy load on first search)"
log "  ASR:       whisper-cli + ggml-small-q8 (走 attune .deb bundle 路径)"
log "  OCR:       PP-OCRv5 mobile (4 ONNX models in ~/.local/share/attune/models/ppocr)"
log ""
log "next: run attune-desktop or attune-server-headless"
log ""
log "deploy-linux.sh: done."
