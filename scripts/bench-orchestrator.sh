#!/usr/bin/env bash
#
# Phase B Bench Orchestrator —— 端到端跑通双赛道 benchmark
#
# 流程：
#   1. 启动独立 attune-server-headless (端口 18901, ATTUNE_VAULT_DIR=/tmp/attune-bench-vault)
#   2. vault auto setup + unlock（统一 password）
#   3. bind 法律 corpus + 通用 corpus（在 ~/attune-bench 下）
#   4. 等 indexer + embedding 完成（poll /api/v1/index/status）
#   5. 跑 queries.json 的 scenarios
#   6. 报告数字
#
# 用法：
#   bash scripts/bench-orchestrator.sh [legal|general|all]
#
# 默认 all —— 法律 + 通用同跑。

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-all}"
PORT=18901
# attune_core::platform::data_dir() 通过 dirs::data_local_dir() 解析；
# 在 Linux 上等于 $XDG_DATA_HOME 或 ~/.local/share。我们用独立 XDG_DATA_HOME 隔离 bench vault。
XDG_DATA_HOME_BENCH="/tmp/attune-bench-xdg"
VAULT_PARENT="$XDG_DATA_HOME_BENCH/attune"
PASSWORD="bench-2026"
BENCH_HOME="$HOME/attune-bench"
BASE_URL="http://localhost:${PORT}"

# 颜色辅助
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'
log()  { echo -e "${GREEN}[bench]${NC} $*"; }
warn() { echo -e "${YELLOW}[bench]${NC} $*"; }
err()  { echo -e "${RED}[bench]${NC} $*" >&2; }

# ============================================================
# 1. 准备 vault dir + build server
# ============================================================
log "Bench XDG_DATA_HOME: $XDG_DATA_HOME_BENCH (vault: $VAULT_PARENT)"
rm -rf "$XDG_DATA_HOME_BENCH"
mkdir -p "$VAULT_PARENT"

cd "$PROJECT_ROOT/rust"
if [[ ! -x "./target/release/attune-server-headless" ]]; then
    log "Building attune-server-headless (release)..."
    cargo build --release -p attune-server --bin attune-server-headless 2>&1 | tail -3
else
    log "Reusing existing target/release/attune-server-headless"
fi

# ============================================================
# 2. 启动 server
# ============================================================
SERVER_PID=""
cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        log "Stopping server (pid=$SERVER_PID)..."
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

XDG_DATA_HOME="$XDG_DATA_HOME_BENCH" \
XDG_CONFIG_HOME="$XDG_DATA_HOME_BENCH/config" \
    ./target/release/attune-server-headless --port "$PORT" >/tmp/attune-bench-server.log 2>&1 &
SERVER_PID=$!
log "Server pid=$SERVER_PID, log=/tmp/attune-bench-server.log"

# 等 server up
for i in {1..30}; do
    if curl -sf "$BASE_URL/health" >/dev/null 2>&1; then
        log "Server up after ${i}s"
        break
    fi
    sleep 1
done

if ! curl -sf "$BASE_URL/health" >/dev/null; then
    err "Server failed to come up; tail log:"
    tail -20 /tmp/attune-bench-server.log
    exit 1
fi

# ============================================================
# 3. Vault setup + unlock
# ============================================================
log "Vault setup (password=$PASSWORD)..."
setup_resp=$(curl -sS -X POST "$BASE_URL/api/v1/vault/setup" \
    -H 'Content-Type: application/json' \
    -d "{\"password\":\"$PASSWORD\"}")
echo "  → setup: $setup_resp"

log "Vault unlock..."
TOKEN=$(curl -sSf -X POST "$BASE_URL/api/v1/vault/unlock" \
    -H 'Content-Type: application/json' \
    -d "{\"password\":\"$PASSWORD\"}" | python3 -c "import json,sys;print(json.load(sys.stdin).get('token',''))")
if [[ -z "$TOKEN" ]]; then
    err "Failed to unlock vault"
    exit 1
fi
log "Vault unlocked, token len=${#TOKEN}"

AUTH=(-H "Authorization: Bearer $TOKEN")

# ============================================================
# 4. Bind corpora (per target)
# ============================================================
bind_corpus() {
    local path="$1"
    local label="$2"
    local corpus_domain="${3:-general}"
    log "Binding $label (domain=$corpus_domain): $path"
    local resp
    resp=$(curl -sS -X POST "$BASE_URL/api/v1/index/bind" \
        "${AUTH[@]}" \
        -H 'Content-Type: application/json' \
        -d "{\"path\":\"$path\",\"recursive\":true,\"file_types\":[\"md\",\"txt\"],\"corpus_domain\":\"$corpus_domain\"}" \
        || echo '{"error":"curl failed"}')
    echo "  → $resp" | head -c 200
    echo
}

case "$TARGET" in
    legal|all)
        # F-Pro: 法律 corpus 标记 corpus_domain=legal 让跨域 penalty 生效
        bind_corpus "$BENCH_HOME/legal-sample/regulation" "legal_regulation" "legal"
        bind_corpus "$BENCH_HOME/legal-sample/case" "legal_case" "legal"
        ;;
esac

case "$TARGET" in
    general|all)
        # F-Pro: rust-book / cs-notes 标记 corpus_domain=tech
        if [[ -e "$BENCH_HOME/general/rust-book/src" ]]; then
            bind_corpus "$BENCH_HOME/general/rust-book/src" "rust-book-full" "tech"
        fi
        if [[ -e "$BENCH_HOME/general/cs-notes/notes" ]]; then
            bind_corpus "$BENCH_HOME/general/cs-notes/notes" "cs-notes-full" "tech"
        fi
        ;;
esac

# ============================================================
# 5. 等 index + embedding 完成
# ============================================================
log "Waiting for indexer + embedding queue to drain..."
for i in {1..600}; do
    status_resp=$(curl -s "${AUTH[@]}" "$BASE_URL/api/v1/index/status" 2>/dev/null || echo '{}')
    pending=$(echo "$status_resp" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    print(d.get('pending_embeddings', -1))
except Exception:
    print(-1)
" 2>/dev/null || echo -1)
    if [[ "$pending" == "0" ]]; then
        log "Embed queue drained after ${i}s"
        break
    fi
    if (( i % 30 == 0 )); then
        log "  ... still indexing, queue=$pending (${i}s)"
    fi
    sleep 1
done

# ============================================================
# 6. 跑 queries
# ============================================================
log "Running queries..."
QUERIES_JSON="$PROJECT_ROOT/rust/tests/golden/queries.json"
RESULTS_JSON="/tmp/bench-$(date +%s).json"

python3 - "$QUERIES_JSON" "$BASE_URL" "$TOKEN" "$RESULTS_JSON" <<'PYEOF'
import json, sys, urllib.parse, urllib.request

queries_path, base_url, token, out_path = sys.argv[1:5]
with open(queries_path) as f:
    spec = json.load(f)

results = {"scenarios": []}
for scen in spec["scenarios"]:
    sid = scen["id"]
    name = scen["name"]
    queries = scen.get("queries", [])
    print(f"\n=== Scenario {sid}: {name} ({len(queries)} queries) ===")
    scen_out = {"id": sid, "name": name, "results": []}
    for q in queries:
        qid = q["id"]
        query = q["query"]
        acceptable = set(q.get("acceptable_hits", []))
        url = f"{base_url}/api/v1/search?{urllib.parse.urlencode({'q': query, 'limit': 10})}"
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                resp = json.loads(r.read())
        except Exception as e:
            print(f"  [{qid}] ERROR: {e}")
            continue
        items = resp.get("results", resp.get("items", []))
        titles = [i.get("title", "") for i in items[:10]]
        # 简单判断：title 含任一 acceptable_hits 关键词 = hit
        hits = []
        for rank, t in enumerate(titles, 1):
            for a in acceptable:
                # 灵活匹配（中文标题 vs id 别名）
                if a in t or any(part in t for part in a.split("_")):
                    hits.append((rank, a, t))
                    break
        hit_at_10 = 1 if hits else 0
        first_rank = hits[0][0] if hits else 0
        rr = 1.0 / first_rank if first_rank else 0.0
        recall = len(set(h[1] for h in hits)) / max(len(acceptable), 1)
        print(f"  [{qid}] hit@10={hit_at_10} mrr={rr:.2f} recall={recall:.2f}")
        if not hits:
            print(f"    top-3 titles: {titles[:3]}")
        scen_out["results"].append({
            "id": qid, "query": query[:60],
            "hit_at_10": hit_at_10, "rr": rr, "recall": recall,
            "top_titles": titles[:5],
        })
    if scen_out["results"]:
        n = len(scen_out["results"])
        scen_out["aggregate"] = {
            "hit_at_10": sum(r["hit_at_10"] for r in scen_out["results"]) / n,
            "mrr": sum(r["rr"] for r in scen_out["results"]) / n,
            "recall_at_10": sum(r["recall"] for r in scen_out["results"]) / n,
        }
        print(f"  Aggregate: Hit@10={scen_out['aggregate']['hit_at_10']:.2f} "
              f"MRR={scen_out['aggregate']['mrr']:.2f} "
              f"Recall@10={scen_out['aggregate']['recall_at_10']:.2f}")
    results["scenarios"].append(scen_out)

with open(out_path, "w") as f:
    json.dump(results, f, ensure_ascii=False, indent=2)
print(f"\nWrote: {out_path}")
PYEOF

log "Done. Results: $RESULTS_JSON"
log "Server log: /tmp/attune-bench-server.log"

# 把 token 写文件，方便后续 law-pro/run_golden_qa 复用同一 server 实例
echo -n "$TOKEN" > /tmp/attune-bench-token
log "Token persisted: /tmp/attune-bench-token (for downstream golden_qa)"

# 默认在结尾保留 server（方便手动验 chat / citation）。
# BENCH_AUTOSHUTDOWN=1 bash bench-orchestrator.sh 会跑完后自动 kill server。
if [[ "${BENCH_AUTOSHUTDOWN:-0}" != "1" ]]; then
    log "BENCH_KEEP_SERVER=1 → 保留 server pid=$SERVER_PID, 端口 $PORT"
    log "  下一步: ATTUNE_URL=$BASE_URL ATTUNE_TOKEN=\$(cat /tmp/attune-bench-token) \\"
    log "         cargo run --release -p law-pro --bin run_golden_qa --manifest-path /data/company/project/attune-pro/Cargo.toml"
    # 取消 cleanup trap，由用户自己 kill
    trap - EXIT
    SERVER_PID=""
fi
