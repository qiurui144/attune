#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# Production RAG/chat/demo code must not contain answer-level fixtures for a
# specific customer corpus. Keep real corpus examples in tests/e2e only.
hits="$(python3 - <<'PY'
import pathlib
import re
import subprocess

roots = [
    pathlib.Path("rust/crates/attune-server/src"),
    pathlib.Path("rust/crates/attune-core/src"),
    pathlib.Path("kb-web-demo"),
]
pattern = re.compile(
    r"RTOS_DMAC|Linux_DMAC|hal_dma|dma_request_chan|sunxi_dma|"
    r"Allwinner|allwinner|tina-v821|V821|/mnt/hdd/allwinner"
)

tracked = subprocess.check_output(
    ["git", "ls-files", "--", *(str(root) for root in roots)], text=True
).splitlines()
untracked = subprocess.check_output(
    ["git", "ls-files", "--others", "--exclude-standard", "--", *(str(root) for root in roots)],
    text=True,
).splitlines()
files = sorted(set(tracked + untracked))

def production_lines(path: pathlib.Path):
    text = path.read_text(encoding="utf-8", errors="replace").splitlines()
    in_test_mod = False
    pending_cfg_test = False
    brace_depth = 0
    for line_no, line in enumerate(text, 1):
        stripped = line.strip()
        if not in_test_mod and stripped.startswith("#[cfg(test)]"):
            pending_cfg_test = True
            continue
        if pending_cfg_test and re.search(r"\bmod\s+tests\b", line):
            in_test_mod = True
            pending_cfg_test = False
            brace_depth = line.count("{") - line.count("}")
            if brace_depth <= 0:
                brace_depth = 1
            continue
        if pending_cfg_test and stripped and not stripped.startswith("#"):
            pending_cfg_test = False

        if in_test_mod:
            brace_depth += line.count("{") - line.count("}")
            if brace_depth <= 0:
                in_test_mod = False
            continue

        yield line_no, line

for file_name in files:
    path = pathlib.Path(file_name)
    if "/tests/" in file_name or path.name.endswith(("_test.rs", ".snap")):
        continue
    if not path.exists() or path.is_dir():
        continue
    for line_no, line in production_lines(path):
        if pattern.search(line):
            print(f"{file_name}:{line_no}:{line}")
PY
)"

if [ -n "$hits" ]; then
  echo "FAIL: production RAG/chat/demo code contains corpus-specific hardcoding:"
  echo "$hits"
  echo
  echo "Move these values to tests/e2e fixtures, user-provided config, plugin metadata, or derive them from retrieved evidence."
  exit 1
fi

echo "no-hardcoded-rag-domain-audit: PASS"
