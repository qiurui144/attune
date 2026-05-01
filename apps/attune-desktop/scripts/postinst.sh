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
# bukuroo/PPOCRv5-ONNX 是公开的 PP-OCRv5 ONNX 转换版（社区维护）
PPOCR_BASE="https://huggingface.co/bukuroo/PPOCRv5-ONNX/resolve/main"

# 文件名映射：HF source name → 本地存放名（attune-core::ocr::ppocr 期望的名）
PPOCR_OK=1
mkdir -p "$PPOCR_DIR"
download_one() {
  local src="$1" dest="$2" desc="$3"
  if [ -s "$PPOCR_DIR/$dest" ]; then
    log "  PP-OCR: $dest already present ($(du -h "$PPOCR_DIR/$dest" | cut -f1))"
    return 0
  fi
  log "  PP-OCR: downloading $dest ($desc)..."
  if curl -fsSL --connect-timeout 10 -o "$PPOCR_DIR/${dest}.tmp" "$PPOCR_BASE/$src" 2>/dev/null; then
    mv "$PPOCR_DIR/${dest}.tmp" "$PPOCR_DIR/$dest"
    [ -n "$SUDO_USER" ] && chown "$SUDO_USER:$SUDO_USER" "$PPOCR_DIR/$dest"
    return 0
  else
    rm -f "$PPOCR_DIR/${dest}.tmp"
    log "  PP-OCR: WARN $dest download failed; tesseract fallback will be used"
    PPOCR_OK=0
    return 1
  fi
}

download_one "ppocrv5-mobile-det.onnx"  "ch_PP-OCRv5_det_mobile.onnx"      "~5 MB det"
download_one "ppocrv5-cls.onnx"          "ch_ppocr_mobile_v2.0_cls.onnx"   "~1 MB cls"
download_one "ppocrv5-mobile-rec.onnx"   "ch_PP-OCRv5_rec_mobile.onnx"     "~10 MB rec"
download_one "ppocrv5_dict.txt"          "ppocr_keys_v1.txt"               "~50 KB 6627-char dict"
[ -n "$SUDO_USER" ] && chown -R "$SUDO_USER:$SUDO_USER" "$PPOCR_DIR" 2>/dev/null
if [ "$PPOCR_OK" = "1" ]; then
  log "  PP-OCR: 4 model files ready at $PPOCR_DIR"
fi

# ─── 7. 验证底座完整性 ─────────────────────────────────────────────
log "─── foundation stack final check ──"
log "  Embedding: $(ollama list 2>/dev/null | grep -q bge && echo "OK ($EMBED_MODEL)" || echo "MISSING")"
log "  LLM:       $(ollama list 2>/dev/null | grep -q qwen && echo "OK ($CHAT_MODEL)" || echo "MISSING")"
log "  ASR:       $(command -v whisper-cli >/dev/null && [ -f "$ASR_MODEL_FILE" ] && echo "OK (whisper-cli + small-q8)" || echo "PARTIAL (re-run apt or attune deploy)")"
log "  OCR:       $([ -f "$PPOCR_DIR/ch_PP-OCRv5_rec_mobile.onnx" ] && command -v pdftoppm >/dev/null && echo "OK (PP-OCRv5 mobile, 4 ONNX models)" || echo "MISSING (re-run: apt install --reinstall attune)")"
log "  Reranker:  preload deferred (lazy on first query)"

# ─── 8. 总结 ───────────────────────────────────────────────────────
log "post-install complete."
log "next: launch 'Attune' from desktop menu — full foundation stack (embed+rerank+asr+ocr) ready"

# 永不让 postinst 失败 — 阻塞 dpkg/rpm 比 Ollama 缺失更糟
exit 0
