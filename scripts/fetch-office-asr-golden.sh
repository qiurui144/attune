#!/usr/bin/env bash
# Fetch ASR golden audio samples (AISHELL-3 + LibriSpeech test-clean抽样).
#
# Usage:
#   ./scripts/fetch-office-asr-golden.sh         # 默认全量下载
#   ./scripts/fetch-office-asr-golden.sh --skip-zh   # 只下英文
#   ./scripts/fetch-office-asr-golden.sh --skip-en   # 只下中文
#
# 落点: rust/crates/attune-server/tests/golden/office/asr/{zh_aishell,en_libri}/
# 之后 cargo test office_asr_golden_gate 才能跑.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ASR_DIR="$REPO_ROOT/rust/crates/attune-server/tests/golden/office/asr"
TMP="${TMPDIR:-/tmp}/attune-asr-fetch"
mkdir -p "$TMP" "$ASR_DIR/zh_aishell" "$ASR_DIR/en_libri"

SKIP_ZH=0
SKIP_EN=0
for arg in "$@"; do
    case "$arg" in
        --skip-zh) SKIP_ZH=1 ;;
        --skip-en) SKIP_EN=1 ;;
        -h|--help)
            sed -n '2,11p' "$0"
            exit 0 ;;
    esac
done

# ─── LibriSpeech test-clean (英文, ~346 MB) ────────────────────────
if [[ $SKIP_EN -eq 0 ]]; then
    echo "[fetch] LibriSpeech test-clean (英文, 抽 20 段)"
    LIBRI_URL="https://www.openslr.org/resources/12/test-clean.tar.gz"
    LIBRI_TGZ="$TMP/libri-test-clean.tar.gz"
    if [[ ! -f "$LIBRI_TGZ" ]]; then
        echo "  → 下载 $LIBRI_URL"
        curl -fsSL --retry 3 "$LIBRI_URL" -o "$LIBRI_TGZ"
    fi
    echo "  → 解压 + 抽 20 段到 $ASR_DIR/en_libri/"
    mkdir -p "$TMP/libri"
    tar -xzf "$LIBRI_TGZ" -C "$TMP/libri" --skip-old-files
    # 抽 20 段最短的（控制 CI 时长）
    find "$TMP/libri" -name "*.flac" -size -1500k | sort -u | head -20 | while IFS= read -r flac; do
        id="libri-$(basename "$flac" .flac)"
        # FLAC → WAV (16 kHz mono PCM, whisper.cpp 要求)
        if command -v ffmpeg >/dev/null 2>&1; then
            ffmpeg -y -loglevel error -i "$flac" -ar 16000 -ac 1 "$ASR_DIR/en_libri/$id.wav"
        else
            cp "$flac" "$ASR_DIR/en_libri/$id.flac"
            echo "  ⚠ ffmpeg not found, kept FLAC: $id"
        fi
        # 找对应 transcript (LibriSpeech: 每章节一个 .trans.txt)
        chapter_dir=$(dirname "$flac")
        trans_file=$(ls "$chapter_dir"/*.trans.txt 2>/dev/null | head -1)
        if [[ -n "${trans_file:-}" ]]; then
            base=$(basename "$flac" .flac)
            text=$(grep -F "$base " "$trans_file" | head -1 | sed "s/^$base //")
            cat > "$ASR_DIR/en_libri/$id.expected.yaml" <<EOF
id: $id
audio_path: $id.wav
duration_sec: 0  # TODO fill from ffprobe
language: en
expected_transcript: "$text"
expected_speakers: 1
reviewer:
  name: LIBRISPEECH_TEST_CLEAN
  approved: true
EOF
        fi
    done
    echo "  ✓ LibriSpeech 20 段就绪"
fi

# ─── AISHELL-3 (中文, ~18 GB total — 我们只取抽样镜像) ────────────
if [[ $SKIP_ZH -eq 0 ]]; then
    echo "[fetch] AISHELL-3 sample (中文)"
    # AISHELL-3 完整集 18 GB, 不实用. 用 magicdata.com 提供的 demo wav,
    # 或者 OpenSLR 93 的 small split. 这里走最稳的: openslr 抽样.
    # 注: 真正 CI 部署时, 应用 SSCD-style hash + 私有镜像，本脚本仅 demo.
    echo "  ⚠ AISHELL-3 full set is 18 GB — skipping bulk download."
    echo "  ⚠ Use 'magicdata-asr-corpus' demo at https://www.magicdata.com/ or"
    echo "  ⚠ pull from your own private cache; place wav + expected.yaml"
    echo "  ⚠ under $ASR_DIR/zh_aishell/ manually before CI."

    # 占位: 写一个 README 提示用户怎么手工补
    cat > "$ASR_DIR/zh_aishell/PENDING.md" <<'EOF'
# AISHELL-3 ZH ASR Golden — Pending

CI 自动下载 AISHELL-3 18 GB 数据集不实际, 此目录留作 manual / private-mirror 填充.

补全方式:
1. 联系 AISHELL 维护方获取 test 集 (或购买商用 license)
2. 抽 20 段 1-3 分钟普通话音频 (单说话人，控制 CI 时长)
3. 转 16 kHz mono PCM WAV
4. 按 README.md 的 expected.yaml 格式写每段的 ground-truth transcript
5. 删除本 PENDING.md 文件

或者: 走自有合规来源 (员工自己录普通话朗读、有 release 的播客片段) 替代.
EOF
fi

echo
echo "[fetch] Done. Run golden gate with:"
echo "  cargo test -p attune-server --test office_asr_golden_gate --release"
