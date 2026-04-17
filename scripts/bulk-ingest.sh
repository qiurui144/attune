#!/usr/bin/env bash
# 将指定目录下的 Markdown 文件批量 POST 到 attune-server /api/v1/ingest。
#
# 用法：
#   ./scripts/bulk-ingest.sh <source_dir> [base_url]
#
# source_dir: 含 .md 文件的目录（递归）
# base_url:   attune-server 地址，默认 http://localhost:18900

set -euo pipefail

SOURCE_DIR="${1:?usage: bulk-ingest.sh <source_dir> [base_url]}"
BASE_URL="${2:-http://localhost:18900}"

if [ ! -d "$SOURCE_DIR" ]; then
  echo "error: $SOURCE_DIR not a directory" >&2
  exit 1
fi

total=0
success=0
failed=0

while IFS= read -r -d '' file; do
  total=$((total + 1))
  # 标题：去扩展名，去 rust-book 前缀
  title=$(basename "$file")
  title="${title%.*}"
  title="${title#ch*-*-}"

  # jq 从 stdin 读文件内容（避免 --arg 超过 ARG_MAX，对 >128KB 文件友好）
  body=$(jq -Rs --arg t "$title" \
    '{title: $t, content: ., source_type: "file"}' < "$file")

  # --data-binary 保留 JSON body 原始字节（--data 会规范化换行破坏 JSON）
  if curl -sSf -X POST -H 'Content-Type: application/json' \
       --data-binary "$body" \
       "$BASE_URL/api/v1/ingest" > /dev/null 2>&1; then
    success=$((success + 1))
  else
    failed=$((failed + 1))
    echo "[FAIL] $file" >&2
  fi

  # 每 10 条打印进度
  if [ $((total % 10)) -eq 0 ]; then
    echo "[progress] $total processed ($success ok, $failed fail)"
  fi
  # 递归扫描 .md / .txt；去重子目录里重名也 OK（后端按 id 存储）
done < <(find "$SOURCE_DIR" -type f \( -name "*.md" -o -name "*.txt" \) -print0)

echo
echo "=== bulk-ingest complete ==="
echo "Total:   $total"
echo "Success: $success"
echo "Failed:  $failed"
