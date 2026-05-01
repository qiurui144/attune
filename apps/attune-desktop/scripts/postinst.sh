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

# ─── 5. Embedding 模型拉取（必要底座之一）+ K3 路径分支 ─────────────
# 设计原则（CLAUDE.md "硬件感知的默认底座"）：
#   本地必装的 4 底座 = Embedding + Reranker + ASR + OCR
#   LLM **不本地预装** — 笔电默认走远端 token；K3 一体机镜像例外
#
# Form factor 检测（与 attune-core::platform::detect_form_factor 同源）:
# - ATTUNE_FORM_FACTOR=k3 env var override（K3 镜像构建时 systemd-environment.d 写入）
# - /sys/class/dmi/id/product_name 含 k3 / jetson 关键字
FORM_FACTOR="laptop"
if [ "${ATTUNE_FORM_FACTOR:-}" = "k3" ] || [ "${ATTUNE_FORM_FACTOR:-}" = "k3appliance" ]; then
  FORM_FACTOR="k3"
elif [ -r /sys/class/dmi/id/product_name ]; then
  PROD=$(tr 'A-Z' 'a-z' < /sys/class/dmi/id/product_name 2>/dev/null)
  case "$PROD" in
    *k3*|*jetson*) FORM_FACTOR="k3" ;;
  esac
fi

# Embedding 按 RAM tier 选 bge-m3 (≥16GB) 或 bge-small (<16GB)
RAM_GB=$(free -g 2>/dev/null | awk '/^Mem:/{print $2}' || echo 0)
if [ "$RAM_GB" -ge 16 ]; then
  EMBED_MODEL="bge-m3"
  EMBED_TIER="full (bge-m3, 1024-dim, 多语言)"
else
  EMBED_MODEL="bge-small"
  EMBED_TIER="lite (bge-small, 384-dim, 中英)"
fi

log "Form factor: $FORM_FACTOR (set ATTUNE_FORM_FACTOR=k3 to force K3 path on non-DMI boxes)"
log "Embedding tier: RAM=${RAM_GB}GB → $EMBED_TIER"

# K3 路径：预装本地 LLM（笔电不走这条）
LOCAL_LLM=""
if [ "$FORM_FACTOR" = "k3" ]; then
  if [ "$RAM_GB" -ge 8 ]; then
    LOCAL_LLM="qwen2.5:3b"
  else
    LOCAL_LLM="qwen2.5:1.5b"
  fi
  log "K3 form factor → preinstall local LLM: $LOCAL_LLM (~2 GB)"
else
  log "Laptop form factor → LLM 走远端 token 默认；用户在 wizard 配置 cloud API 或 Ollama"
fi

# 拉模型（embedding + K3-only LLM）
pull_one_model() {
  local m="$1"
  if ollama list 2>/dev/null | awk 'NR>1 {print $1}' | grep -qx "$m" \
     || ollama list 2>/dev/null | awk 'NR>1 {print $1}' | grep -qx "${m}:latest"; then
    log "  $m already pulled — skipping"
    return 0
  fi
  log "  pulling $m (progress: journalctl -t attune-postinst -f)..."
  if ollama pull "$m" 2>&1 | while IFS= read -r line; do
       case "$line" in
         *success*|*verifying*|*"writing manifest"*|*error*|*Error*)
           logger -t "$LOG_TAG" -- "[pull $m] $line"
           ;;
       esac
     done; then
    log "  $m pulled OK"
    return 0
  fi
  log "  WARN: $m pull failed (network?). User can retry: ollama pull $m"
  return 1
}

# 仅在 Ollama API 上线时拉模型；否则留 hint
if curl -sf http://localhost:11434/api/version >/dev/null 2>&1; then
  pull_one_model "$EMBED_MODEL"
  if [ -n "$LOCAL_LLM" ]; then
    pull_one_model "$LOCAL_LLM"
  fi
else
  log "WARN: Ollama API not ready, skipping model pull. User can later: ollama pull $EMBED_MODEL${LOCAL_LLM:+ + ollama pull $LOCAL_LLM}"
fi

# ─── 6. 底座底层：ASR (whisper.cpp) + Reranker ──────────────────────
# 用户拍板：所有底座（embed+rerank+asr+ocr）必须 .deb 安装时全装好

# 6.1 whisper.cpp binary：bundled 在 /usr/lib/attune/bin/whisper-cli (Tauri resources)
#     建符号链到 /usr/local/bin 让 attune-server 的 PATH 找得到
# Tauri bundle 用 productName 大小写（"Attune" → /usr/lib/Attune/）
# 兼容大小写 + 旧 lowercase 路径
WHISPER_LINK="/usr/local/bin/whisper-cli"
WHISPER_BUNDLED=""
for cand in /usr/lib/Attune/bin/whisper-cli /usr/lib/attune/bin/whisper-cli; do
  [ -x "$cand" ] && WHISPER_BUNDLED="$cand" && break
done
if [ -n "$WHISPER_BUNDLED" ] && [ ! -e "$WHISPER_LINK" ]; then
  ln -sf "$WHISPER_BUNDLED" "$WHISPER_LINK"
  log "linked whisper-cli: $WHISPER_LINK -> $WHISPER_BUNDLED"
elif [ -n "$WHISPER_BUNDLED" ]; then
  log "whisper-cli already linked at $WHISPER_LINK"
else
  log "WARN: bundled whisper-cli missing under /usr/lib/Attune/bin/ (.deb 资源问题?)"
fi

# 6.2 ASR ggml 模型：~250 MB 走 HF 镜像
# whisper-small-q8_0 中文 WER 实测 < 20% (CLAUDE.md 验收标准)
ASR_DIR="/home/$SUDO_USER"
[ -z "$ASR_DIR" ] || [ "$SUDO_USER" = "" ] && ASR_DIR="$HOME"
ASR_DIR="$ASR_DIR/.local/share/attune/models/whisper"
ASR_MODEL_FILE="$ASR_DIR/ggml-small-q8_0.bin"
ASR_MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q8_0.bin"

if [ -f "$ASR_MODEL_FILE" ] && [ "$(stat -c%s "$ASR_MODEL_FILE" 2>/dev/null)" -gt 100000000 ]; then
  log "ASR model already present: $(du -h "$ASR_MODEL_FILE" | cut -f1)"
else
  log "downloading ASR model ggml-small-q8_0.bin (~250 MB)..."
  mkdir -p "$ASR_DIR"
  # 用 SUDO_USER 拥有，因为 dpkg 跑 root，但实际属于用户
  if curl -fsSL --connect-timeout 10 -o "${ASR_MODEL_FILE}.tmp" "$ASR_MODEL_URL" 2>/dev/null; then
    mv "${ASR_MODEL_FILE}.tmp" "$ASR_MODEL_FILE"
    [ -n "$SUDO_USER" ] && chown -R "$SUDO_USER:$SUDO_USER" "$ASR_DIR"
    log "  ASR model downloaded ($(du -h "$ASR_MODEL_FILE" | cut -f1))"
  else
    rm -f "${ASR_MODEL_FILE}.tmp"
    log "  WARN: ASR model download failed; user can later: attune deploy --with-asr"
  fi
fi

# 6.3 Reranker：bge-reranker-base ONNX (~120 MB) 通过 Rust hf_hub crate 自动下载
# 这里只提示用户首次查询会延迟，不主动 preload（避免 root 写到 user 缓存的权限问题）
log "Reranker (Xenova/bge-reranker-base ~120 MB): will be downloaded by attune-server on first search query (~5-10s one-time)"

# 6.4 PP-OCR (PaddleOCR) ONNX 模型 — 比 tesseract 中文准确率高 ~10-20%
# 4 个文件合计 ~16 MB，首次安装时下载到 ~/.local/share/attune/models/ppocr/
HOME_DIR="${SUDO_USER:+/home/$SUDO_USER}"
[ -z "$HOME_DIR" ] && HOME_DIR="$HOME"
PPOCR_DIR="$HOME_DIR/.local/share/attune/models/ppocr"
# 模型源决策（2026-05-01 修）:
# - PP-OCRv5 ONNX 字典在社区版 (bukuroo) 与 kreuzberg-paddle-ocr 期望格式不匹配
#   (v5 字典以　全角空格 + emoji 收尾，但 kreuzberg 期望 # 起始 + 空格收尾)
# - 降级到 RapidOCR/PP-OCRv4：paddle-ocr-rs 设计目标，标准 6623 字符字典格式
# - 字典格式与 kreuzberg crnn_net.rs init_keys 匹配（# CTC blank + 6623 chars + ' '）
RAPIDOCR_BASE="https://huggingface.co/SWHL/RapidOCR/resolve/main"
DICT_URL="https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/release/2.7/ppocr/utils/ppocr_keys_v1.txt"

PPOCR_OK=1
mkdir -p "$PPOCR_DIR"
download_with_dict_prep() {
  local src="$1" dest="$2" desc="$3"
  if [ -s "$PPOCR_DIR/$dest" ]; then
    log "  PP-OCR: $dest already present ($(du -h "$PPOCR_DIR/$dest" | cut -f1))"
    return 0
  fi
  log "  PP-OCR: downloading $dest ($desc)..."
  if curl -fsSL --connect-timeout 10 -o "$PPOCR_DIR/${dest}.tmp" "$src" 2>/dev/null; then
    mv "$PPOCR_DIR/${dest}.tmp" "$PPOCR_DIR/$dest"
    [ -n "$SUDO_USER" ] && chown "$SUDO_USER:$SUDO_USER" "$PPOCR_DIR/$dest"
    return 0
  else
    rm -f "$PPOCR_DIR/${dest}.tmp"
    log "  PP-OCR: WARN $dest download failed"
    PPOCR_OK=0
    return 1
  fi
}

download_with_dict_prep "$RAPIDOCR_BASE/PP-OCRv4/ch_PP-OCRv4_det_infer.onnx" \
  "ch_PP-OCRv5_det_mobile.onnx" "~5 MB det (PP-OCRv4)"
download_with_dict_prep "$RAPIDOCR_BASE/PP-OCRv1/ch_ppocr_mobile_v2.0_cls_infer.onnx" \
  "ch_ppocr_mobile_v2.0_cls.onnx" "~1 MB cls"
download_with_dict_prep "$RAPIDOCR_BASE/PP-OCRv4/ch_PP-OCRv4_rec_infer.onnx" \
  "ch_PP-OCRv5_rec_mobile.onnx" "~10 MB rec (PP-OCRv4)"

# 字典：PaddleOCR 官方 6623 字符 + kreuzberg 要求 prefix # / suffix ' '
# 处理：下载后用 awk 加 # 头 + 空格尾
DICT_PATH="$PPOCR_DIR/ppocr_keys_v1.txt"
if [ -s "$DICT_PATH" ] && head -c 1 "$DICT_PATH" | grep -q '#'; then
  log "  PP-OCR: ppocr_keys_v1.txt already prepared"
else
  log "  PP-OCR: downloading + preparing ppocr_keys_v1.txt (6623 chars + # / ' ')..."
  if curl -fsSL --connect-timeout 10 -o "$DICT_PATH.tmp" "$DICT_URL" 2>/dev/null; then
    # 在文件首尾加 # 和空格，匹配 kreuzberg-paddle-ocr CTC blank 格式
    {
      printf '#\n'
      cat "$DICT_PATH.tmp"
      printf ' \n'
    } > "$DICT_PATH"
    rm -f "$DICT_PATH.tmp"
    [ -n "$SUDO_USER" ] && chown "$SUDO_USER:$SUDO_USER" "$DICT_PATH"
  else
    rm -f "$DICT_PATH.tmp"
    log "  PP-OCR: WARN dict download failed"
    PPOCR_OK=0
  fi
fi
[ -n "$SUDO_USER" ] && chown -R "$SUDO_USER:$SUDO_USER" "$PPOCR_DIR" 2>/dev/null
if [ "$PPOCR_OK" = "1" ]; then
  log "  PP-OCR: 4 model files ready at $PPOCR_DIR"
fi

# ─── 7. 验证 4 必要底座完整性 ──────────────────────────────────────
# 设计契约（CLAUDE.md "硬件感知的默认底座" + "成本感知与触发契约"）：
#   本地必装 4 底座 = Embedding + Reranker + ASR + OCR
#   LLM 不在底座清单（远端 token 默认 / K3 镜像例外）
log "─── 4 foundation stack final check ──"
log "  Embedding: $(ollama list 2>/dev/null | grep -q bge && echo "OK ($EMBED_MODEL via Ollama)" || echo "MISSING")"
log "  Reranker:  lazy-load on first search (Xenova/bge-reranker-base ~120 MB via hf_hub)"
log "  ASR:       $(command -v whisper-cli >/dev/null && [ -f "$ASR_MODEL_FILE" ] && echo "OK (whisper-cli + ggml-small-q8)" || echo "PARTIAL (re-run apt or attune deploy)")"
log "  OCR:       $([ -f "$PPOCR_DIR/ch_PP-OCRv5_rec_mobile.onnx" ] && command -v pdftoppm >/dev/null && echo "OK (PP-OCRv5 mobile, 4 ONNX models + pdftoppm)" || echo "MISSING (re-run: apt install --reinstall attune)")"
if [ "$FORM_FACTOR" = "k3" ]; then
  log "─── LLM (K3 form factor — preinstalled) ──"
  log "  LLM: $(ollama list 2>/dev/null | grep -q qwen && echo "OK ($LOCAL_LLM via Ollama)" || echo "MISSING — user can: ollama pull $LOCAL_LLM")"
else
  log "─── LLM (Laptop form factor — user choice in wizard) ──"
  log "  LLM: NOT preinstalled by design — first-run wizard offers cloud API key or local Ollama"
fi

# ─── 8. 总结 ───────────────────────────────────────────────────────
log "post-install complete."
log "next: launch 'Attune' → first-run wizard configures LLM (cloud token recommended, Ollama optional)"

# 永不让 postinst 失败 — 阻塞 dpkg/rpm 比 Ollama 缺失更糟
exit 0
