#!/usr/bin/env bash
# Download test corpora at pinned versions.
# Runs idempotently — re-running is safe and fast.
#
# Usage: ./scripts/download-corpora.sh [corpus_name]
#   corpus_name: rust-book | cs-notes | openai-cookbook | all (default)

set -euo pipefail

CORPORA_DIR="$(cd "$(dirname "$0")/.." && pwd)/rust/tests/corpora"
mkdir -p "$CORPORA_DIR"

target="${1:-all}"

# Corpus definitions: name url pinned_ref subdir
declare -A CORPORA=(
  [rust-book]="https://github.com/rust-lang/book.git|trpl-v0.3.0|src"
  [cs-notes]="https://github.com/CyC2018/CS-Notes.git|c47a2a7|notes"
  [openai-cookbook]="https://github.com/openai/openai-cookbook.git|main|examples"
  # TIM168/technical_books 整仓 5.6 GB，master 分支（无 tag）；
  # 用 sparse-checkout 抽子目录 Python/ + Go/ + 人工智能&机器学习/ 已够场景 B 测试
  [technical-books]="https://github.com/TIM168/technical_books.git|master|."
)

fetch_corpus() {
  local name="$1"
  local spec="${CORPORA[$name]}"
  IFS='|' read -r url pin subdir <<< "$spec"

  local dest="$CORPORA_DIR/$name"
  if [ -d "$dest/.git" ]; then
    echo "[skip] $name already present at $dest"
    echo "       current ref: $(cd "$dest" && git describe --always)"
    return 0
  fi

  echo "[fetch] $name @ $pin"
  # 大仓（如 technical-books 5.6GB）用 sparse-checkout 只抽关键子目录
  if [ "$name" = "technical-books" ]; then
    git clone --depth=1 --filter=blob:none --sparse "$url" "$dest"
    (cd "$dest" && git sparse-checkout set \
        "Python" "Go" "人工智能&机器学习" "数据库" "算法")
  else
    git clone --depth=50 "$url" "$dest"
    (cd "$dest" && git checkout "$pin")
  fi
  echo "[ok]    $name content at $dest/$subdir"
}

if [ "$target" = "all" ]; then
  for name in "${!CORPORA[@]}"; do
    fetch_corpus "$name"
  done
else
  if [ -n "${CORPORA[$target]:-}" ]; then
    fetch_corpus "$target"
  else
    echo "Unknown corpus: $target"
    echo "Available: ${!CORPORA[*]}"
    exit 1
  fi
fi

echo
echo "Corpora ready under: $CORPORA_DIR"
du -sh "$CORPORA_DIR"/*/ 2>/dev/null | head
