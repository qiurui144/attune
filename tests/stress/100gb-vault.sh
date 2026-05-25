#!/usr/bin/env bash
# 100 GB synthetic vault stress — v1.0.5 framework(未真跑)
#
# 目标:验证 attune-server 在 100 GB 真实 vault 规模下:
#   - cold-start latency(open vault → first search ready)
#   - ingest throughput(items/sec,chunks/sec)
#   - search P99(FTS + vector + reranker)
#   - memory ceiling(RSS / VSZ,防 OOM)
#
# 真跑前提:
#   - 至少 200 GB free disk(synthetic 100 GB + 索引 ~60 GB + buffer)
#   - attune-server release binary(target/release/attune-server,不用 dev build)
#   - 6+ CPU core / 16+ GB RAM 物理机(笔电跑不动)
#
# 真跑(等 user 准备真服务器后):
#   ATTUNE_VAULT_DIR=/mnt/big/stress-vault \
#   ATTUNE_VAULT_PASSWORD=<test-pass-not-real> \
#   ./tests/stress/100gb-vault.sh
#
# 不真跑(本 sprint 仅准备 framework):
#   bash -n tests/stress/100gb-vault.sh   # 语法检查通过即可

set -euo pipefail

# ─── 配置(可通过环境变量 override) ────────────────────────────────────
VAULT_DIR="${ATTUNE_VAULT_DIR:-/tmp/attune-stress-vault}"
VAULT_PASSWORD="${ATTUNE_VAULT_PASSWORD:-test-pass-not-real}"   # 测试 stub,明示假
TARGET_BYTES="${TARGET_BYTES:-$((100 * 1024 * 1024 * 1024))}"   # 100 GB default
CHUNK_FILE_SIZE_KB="${CHUNK_FILE_SIZE_KB:-64}"                  # 每文件 64 KB
SERVER_BIN="${SERVER_BIN:-./target/release/attune-server}"
REPORT_DIR="${REPORT_DIR:-reports/stress/100gb-$(date +%Y%m%d-%H%M%S)}"

mkdir -p "$VAULT_DIR" "$REPORT_DIR"

echo "=== attune v1.0.5 100 GB vault stress ==="
echo "vault_dir:    $VAULT_DIR"
echo "target_bytes: $TARGET_BYTES ($((TARGET_BYTES / 1024 / 1024 / 1024)) GB)"
echo "chunk_size:   ${CHUNK_FILE_SIZE_KB} KB per file"
echo "report_dir:   $REPORT_DIR"

# ─── Phase 1: 预检 ───────────────────────────────────────────────────
echo "[1/5] preflight checks..."
if [[ ! -x "$SERVER_BIN" ]]; then
  echo "FATAL: $SERVER_BIN not found — run cargo build --release first"
  exit 1
fi
AVAIL_BYTES=$(df --output=avail -B1 "$VAULT_DIR" | tail -1)
if [[ "$AVAIL_BYTES" -lt $((TARGET_BYTES * 2)) ]]; then
  echo "FATAL: avail $AVAIL_BYTES < required $((TARGET_BYTES * 2)) (need 2x for index)"
  exit 1
fi

# ─── Phase 2: 生成 synthetic corpus ──────────────────────────────────
echo "[2/5] generating ~$((TARGET_BYTES / 1024 / 1024 / 1024)) GB synthetic corpus..."
CORPUS_DIR="$VAULT_DIR/corpus"
mkdir -p "$CORPUS_DIR"

# 生成器:每个 64 KB md 文件,内容为可压缩 + 可搜索的伪段落(避免纯随机字节让索引爆)
total_files=$((TARGET_BYTES / (CHUNK_FILE_SIZE_KB * 1024)))
echo "  → $total_files files target"

generate_corpus() {
  local i
  for ((i = 0; i < total_files; i++)); do
    local dir="$CORPUS_DIR/d$((i / 1000))"
    mkdir -p "$dir"
    # 用 base64(/dev/urandom) + 真实关键词混合,既可压缩又可被 FTS 命中
    {
      echo "# Doc $i — synthetic stress corpus"
      echo ""
      for k in knowledge graph rust async tantivy embedding hnsw vault chunk; do
        echo "## section $k"
        echo "This is paragraph about $k feature number $i with lorem ipsum dolor sit amet."
      done
      head -c $((CHUNK_FILE_SIZE_KB * 1024 - 500)) /dev/urandom | base64
    } > "$dir/doc-$i.md"
    if (( i % 10000 == 0 )); then
      echo "  ... generated $i / $total_files"
    fi
  done
}

# 跳过生成(若 corpus 已存在到目标大小)
existing_size=$(du -sb "$CORPUS_DIR" 2>/dev/null | awk '{print $1}' || echo 0)
if [[ "$existing_size" -lt "$TARGET_BYTES" ]]; then
  time generate_corpus 2>&1 | tee "$REPORT_DIR/01-generate.log"
else
  echo "  ✓ corpus already exists ($((existing_size / 1024 / 1024 / 1024)) GB), skipping generation"
fi

# ─── Phase 3: cold-start vault + 全量 ingest ────────────────────────
echo "[3/5] cold-start vault + bulk ingest..."

# 启 server 后台
ATTUNE_BIND=127.0.0.1:18901 \
ATTUNE_VAULT_DIR="$VAULT_DIR/vault" \
nohup "$SERVER_BIN" > "$REPORT_DIR/server.log" 2>&1 &
SERVER_PID=$!
echo "  → server pid=$SERVER_PID, waiting health..."
trap "kill $SERVER_PID 2>/dev/null || true" EXIT

# 等 :18901 ready(最多 30s)
for i in {1..30}; do
  if curl -fsS http://127.0.0.1:18901/api/v1/health > /dev/null 2>&1; then
    echo "  ✓ server ready in ${i}s"
    break
  fi
  sleep 1
done

# Init vault(测试 stub password,per CLAUDE.md § Secrets 严禁硬编码)
curl -fsS -X POST http://127.0.0.1:18901/api/v1/vault/init \
  -H "Content-Type: application/json" \
  -d "{\"password\":\"$VAULT_PASSWORD\"}" \
  | tee "$REPORT_DIR/02-vault-init.json"

# Bulk ingest(走 watch dir,real production path)
INGEST_START=$(date +%s)
curl -fsS -X POST http://127.0.0.1:18901/api/v1/index/watch \
  -H "Content-Type: application/json" \
  -d "{\"path\":\"$CORPUS_DIR\"}" \
  | tee "$REPORT_DIR/03-ingest-start.json"

# 轮询 ingest 完成(每 30s 检查 queue depth)
while true; do
  status=$(curl -fsS http://127.0.0.1:18901/api/v1/status 2>/dev/null || echo '{}')
  pending=$(echo "$status" | jq -r '.embed_queue_pending // 0')
  echo "$(date '+%H:%M:%S') pending=$pending"
  echo "$status" >> "$REPORT_DIR/04-ingest-progress.jsonl"
  [[ "$pending" -eq 0 ]] && break
  sleep 30
done
INGEST_END=$(date +%s)
echo "ingest_duration_sec=$((INGEST_END - INGEST_START))" | tee "$REPORT_DIR/05-ingest-duration.txt"

# ─── Phase 4: search latency P99 ──────────────────────────────────
echo "[4/5] search latency benchmark (1000 queries)..."
QUERIES=("knowledge" "graph rust" "tantivy embedding" "hnsw vault chunk" "async lorem")
echo "ts_ms,query,latency_ms,status" > "$REPORT_DIR/06-search-latency.csv"
for ((i = 0; i < 1000; i++)); do
  q="${QUERIES[$((i % ${#QUERIES[@]}))]}"
  t_start=$(date +%s%N)
  http_code=$(curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:18901/api/v1/search?q=$(printf %s "$q" | jq -sRr @uri)&limit=20")
  t_end=$(date +%s%N)
  latency_ms=$(( (t_end - t_start) / 1000000 ))
  echo "$(date +%s%3N),$q,$latency_ms,$http_code" >> "$REPORT_DIR/06-search-latency.csv"
done

# 计算 P50/P95/P99
awk -F, 'NR>1 {print $3}' "$REPORT_DIR/06-search-latency.csv" \
  | sort -n \
  | awk '
      { v[NR]=$1 }
      END {
        n=NR
        p50=v[int(n*0.5)]; p95=v[int(n*0.95)]; p99=v[int(n*0.99)]
        printf "p50=%d ms\np95=%d ms\np99=%d ms\n", p50, p95, p99
      }' | tee "$REPORT_DIR/07-search-percentiles.txt"

# ─── Phase 5: memory ceiling ──────────────────────────────────────
echo "[5/5] memory snapshot..."
if [[ -d "/proc/$SERVER_PID" ]]; then
  cat "/proc/$SERVER_PID/status" | grep -E '^(VmRSS|VmPeak|VmSize|Threads)' \
    | tee "$REPORT_DIR/08-memory.txt"
fi

# ─── 汇总 ────────────────────────────────────────────────────────
echo ""
echo "=== summary ==="
echo "report dir:    $REPORT_DIR"
echo "ingest_dur_s:  $((INGEST_END - INGEST_START))"
cat "$REPORT_DIR/07-search-percentiles.txt"
cat "$REPORT_DIR/08-memory.txt"

# 清理(可选,本 framework 默认保留 vault 便于二次 search)
# rm -rf "$VAULT_DIR"
