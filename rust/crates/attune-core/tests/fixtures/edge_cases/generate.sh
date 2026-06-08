#!/usr/bin/env bash
# Deterministic generator for the large / binary edge-case fixtures.
#
# These are NOT committed (they are large or binary cruft); the test
# (`tests/ingest_edge_resource_test.rs`) regenerates them in-process when
# absent, so the suite is self-contained. Run this only when you want the
# files on disk for manual inspection.
#
# Usage:  bash generate.sh
set -euo pipefail
cd "$(dirname "$0")"

# 1. huge ~10MB UTF-8 text — repeated paragraph (bounded-memory probe).
python3 - <<'PY'
para = ("This is a repeating paragraph for the 10 MB ingest bound test. "
        "Keywords rust storage chunking performance scalability. "
        "此段落含中文用于多语言分块测试。\n\n")
target = 10 * 1024 * 1024  # 10 MB
buf = []
size = 0
i = 0
while size < target:
    if i % 400 == 0:
        h = f"## Section {i // 400}\n\n"
        buf.append(h); size += len(h.encode())
    buf.append(para); size += len(para.encode()); i += 1
with open("huge_10mb.txt", "w", encoding="utf-8") as f:
    f.write("# Huge Document\n\n")
    f.write("".join(buf))
PY

# 2. non-UTF8 byte file — invalid UTF-8 sequences mixed with ASCII.
#    Named .txt so the parser treats it as plain text (from_utf8_lossy path).
python3 - <<'PY'
# Lone continuation bytes, truncated multibyte, raw 0xFF/0xFE, embedded NUL.
data = (b"valid ascii prefix MARKER_ASCII\n"
        + bytes([0xFF, 0xFE, 0x80, 0x81, 0xC0, 0xC1])  # invalid UTF-8
        + b"\x00\x00middle\x00"                          # NUL bytes
        + bytes([0xED, 0xA0, 0x80])                       # UTF-16 surrogate (invalid in UTF-8)
        + b"more ascii MARKER_TAIL\n"
        + bytes(range(0x80, 0x100)))                      # full high-byte range
with open("non_utf8.txt", "wb") as f:
    f.write(data)
PY

# 3. 100k tiny lines (1 char each).
python3 - <<'PY'
with open("many_lines.txt", "w", encoding="utf-8") as f:
    for i in range(100_000):
        f.write("x\n")
PY

# 4. deeply-nested / oversized JSON structure.
python3 - <<'PY'
depth = 50_000
flat = 200_000
with open("deep_nested.json", "w", encoding="utf-8") as f:
    f.write("[" * depth)
    f.write("1")
    f.write("]" * depth)
    f.write("\n")
    # plus a huge flat array on a second line
    f.write("[" + ",".join("0" for _ in range(flat)) + "]\n")
PY

echo "generated:"
ls -la huge_10mb.txt non_utf8.txt many_lines.txt deep_nested.json
