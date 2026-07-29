#!/usr/bin/env bash
# Generate the binary (non-UTF8) i18n fixtures.
#
# The UTF-8 text fixtures (japanese.md / korean.md / traditional_chinese.md /
# arabic_rtl.md / hebrew_rtl.txt / emoji_heavy.md) are committed directly — they
# are valid UTF-8 and diff cleanly.
#
# The non-UTF8 fixtures below are NOT committed (they would corrupt under
# editors / git text filters). They are regenerated deterministically here AND
# in-process by `i18n_ingest_search_test.rs::fixture_bytes` so the suite is
# self-contained on a clean checkout. This script is the human-readable SSOT for
# how those byte sequences are produced.
#
# Requires: python3 (stdlib codecs only).
set -euo pipefail
cd "$(dirname "$0")"

python3 - <<'PY'
# GBK-encoded Simplified Chinese (legacy Windows-CN encoding, NOT UTF-8).
# Contains an ASCII marker that MUST survive from_utf8_lossy decode.
gbk_text = "ASCII_GBK_MARKER 简体中文 GBK 编码测试 股东决议\n核心词 检索\n"
with open("gbk_simplified.txt", "wb") as f:
    f.write(gbk_text.encode("gbk"))

# Shift-JIS-encoded Japanese (legacy Windows-JP encoding, NOT UTF-8).
sjis_text = "ASCII_SJIS_MARKER 日本語 Shift_JIS エンコード テスト\n機械学習\n"
with open("shift_jis_japanese.txt", "wb") as f:
    f.write(sjis_text.encode("shift_jis"))

print("wrote gbk_simplified.txt + shift_jis_japanese.txt")
PY
