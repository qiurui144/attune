#!/bin/bash
# ═══════════════════════════════════════════════════════════════
#  test-pyramid.sh — 一键跑 Attune 完整测试金字塔
# ═══════════════════════════════════════════════════════════════
#
# 6 层（per docs/TESTING.md 金字塔）：
#   1. Unit          — cargo test --lib  (~30s, 必跑)
#   2. Integration   — cargo test --test '*'  (~2min, 必跑)
#   3. Smoke         — scripts/smoke-test.sh  (~30s, 必跑)
#   4. Corpus        — scripts/run-benchmark-corpus.sh  (~5min, 可选 --with-corpus)
#   5. Quality       — cargo test --release rag_quality_benchmark  (~10s, 必跑)
#   6. E2E (browser) — Playwright via tests/e2e_rust/ (~5min, 可选 --with-e2e)
#
# 默认: 跑 1+2+3+5（必跑层），合计 ~3-4 min。
# 可选: --with-corpus 加第 4 层；--with-e2e 加第 6 层。
#
# 用法:
#   bash scripts/test-pyramid.sh                # 必跑 4 层
#   bash scripts/test-pyramid.sh --with-corpus  # + 真语料检索
#   bash scripts/test-pyramid.sh --with-e2e     # + 浏览器 e2e
#   bash scripts/test-pyramid.sh --all          # 全跑
#
# 输出:
#   tests/reports/test-pyramid-<timestamp>.md  生成 Markdown 报告
# ═══════════════════════════════════════════════════════════════

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_DIR"

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()  { echo -e "${RED}[FAIL]${NC}  $*" >&2; }
phase() { echo -e "\n${CYAN}━━━ $* ━━━${NC}"; }

# ── 解析参数 ──────────────────────────────────────────────────
WITH_CORPUS=false
WITH_E2E=false
for arg in "$@"; do
    case "$arg" in
        --with-corpus) WITH_CORPUS=true ;;
        --with-e2e)    WITH_E2E=true ;;
        --all)         WITH_CORPUS=true; WITH_E2E=true ;;
        -h|--help)
            sed -n '3,30p' "$0" | sed 's/^# *//; s/^#//'
            exit 0 ;;
    esac
done

# ── 报告目录 ──────────────────────────────────────────────────
REPORT_DIR="$PROJECT_DIR/tests/reports"
mkdir -p "$REPORT_DIR"
TS=$(date +%Y%m%d_%H%M%S)
REPORT="$REPORT_DIR/test-pyramid-$TS.md"

cat > "$REPORT" <<EOF
# Attune Test Pyramid Report

- Timestamp: $(date -Iseconds)
- Branch:    $(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
- Commit:    $(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
- With corpus: $WITH_CORPUS
- With e2e:    $WITH_E2E

## Layer Results

| Layer | Status | Tests | Duration | Notes |
|-------|--------|-------|----------|-------|
EOF

declare -A RESULTS
declare -A TIMINGS
declare -A COUNTS

run_layer() {
    local name="$1" desc="$2" cmd="$3"
    phase "$desc"
    local start=$(date +%s)
    local logfile="/tmp/test-pyramid-${name}.log"
    if (cd "$PROJECT_DIR" && eval "$cmd") > "$logfile" 2>&1; then
        local end=$(date +%s)
        RESULTS[$name]="✅ PASS"
        TIMINGS[$name]="$((end - start))s"
        # 提取测试数：兼容 cargo "test result: ok. N passed" 和 pytest "N passed"
        local cargo_count=$(grep -oE "test result: ok\. [0-9]+ passed" "$logfile" 2>/dev/null | \
            awk '{sum += $4} END {print sum+0}')
        local pytest_count=$(grep -oE "^[0-9]+ passed" "$logfile" 2>/dev/null | \
            awk '{sum += $1} END {print sum+0}')
        local pytest_count2=$(grep -oE "==+ [0-9]+ passed" "$logfile" 2>/dev/null | \
            awk '{sum += $2} END {print sum+0}')
        local smoke_count=$(grep -cE "^\[OK\] Test [0-9]+/[0-9]+:" "$logfile" 2>/dev/null)
        local count=$((${cargo_count:-0} + ${pytest_count:-0} + ${pytest_count2:-0} + ${smoke_count:-0}))
        COUNTS[$name]="${count:-N/A}"
        ok "$name: ${COUNTS[$name]} 测试通过, ${TIMINGS[$name]}"
    else
        local end=$(date +%s)
        RESULTS[$name]="❌ FAIL"
        TIMINGS[$name]="$((end - start))s"
        COUNTS[$name]="?"
        fail "$name: 失败 (日志: $logfile)"
        tail -10 "$logfile" >&2
    fi
}

# ── 1. Unit Tests (cargo test --lib) ──────────────────────────
run_layer "unit" "Layer 1: Unit Tests (cargo test --lib)" \
    "cd rust && cargo test --workspace --release --lib"

# ── 2. Integration Tests (cargo test --test '*') ──────────────
run_layer "integration" "Layer 2: Integration Tests (cargo test integration)" \
    "cd rust && cargo test --workspace --release --tests"

# ── 3. Smoke Test (server binary + key APIs) ──────────────────
run_layer "smoke" "Layer 3: Smoke Test (binary spawn + API ping)" \
    "bash $PROJECT_DIR/scripts/smoke-test.sh"

# ── 5. Quality Benchmark (mock corpus, deterministic) ─────────
# 顺序: 必跑 5 在前，4 是 optional
run_layer "quality" "Layer 5: Quality Benchmark (mock corpus, MRR)" \
    "cd rust && cargo test --release -p attune-core --test rag_quality_benchmark"

# ── 4. Corpus Integration (real GitHub corpus) ────────────────
if [ "$WITH_CORPUS" = "true" ]; then
    run_layer "corpus" "Layer 4: Corpus Integration (real rust-book + cs-notes)" \
        "cd rust && cargo test --release --test corpus_integration_test -- --ignored"
else
    RESULTS[corpus]="⏭️ SKIP"
    TIMINGS[corpus]="-"
    COUNTS[corpus]="-"
    warn "Layer 4: Corpus Integration (skipped, 加 --with-corpus 启用)"
fi

# ── 6. E2E Browser Tests (httpx + Playwright via Python) ──────
if [ "$WITH_E2E" = "true" ]; then
    run_layer "e2e" "Layer 6: E2E (server binary + httpx + browser)" \
        "cd $PROJECT_DIR && python3 -m pytest tests-e2e/ -v --tb=short"
else
    RESULTS[e2e]="⏭️ SKIP"
    TIMINGS[e2e]="-"
    COUNTS[e2e]="-"
    warn "Layer 6: E2E (skipped, 加 --with-e2e 启用)"
fi

# ── 写报告表 ──────────────────────────────────────────────────
{
    echo "| 1. Unit          | ${RESULTS[unit]} | ${COUNTS[unit]} | ${TIMINGS[unit]} | cargo test --lib |"
    echo "| 2. Integration   | ${RESULTS[integration]} | ${COUNTS[integration]} | ${TIMINGS[integration]} | cargo test --tests |"
    echo "| 3. Smoke         | ${RESULTS[smoke]} | 5 | ${TIMINGS[smoke]} | binary + API ping |"
    echo "| 4. Corpus        | ${RESULTS[corpus]} | ${COUNTS[corpus]} | ${TIMINGS[corpus]} | real GitHub corpus |"
    echo "| 5. Quality       | ${RESULTS[quality]} | ${COUNTS[quality]} | ${TIMINGS[quality]} | golden set MRR |"
    echo "| 6. E2E           | ${RESULTS[e2e]} | ${COUNTS[e2e]} | ${TIMINGS[e2e]} | Playwright Chrome |"
    echo ""
    echo "## Summary"
    echo ""
    if [[ "${RESULTS[unit]}" == *PASS* && "${RESULTS[integration]}" == *PASS* && \
          "${RESULTS[smoke]}" == *PASS* && "${RESULTS[quality]}" == *PASS* ]]; then
        echo "**✅ 必跑 4 层全部通过**"
    else
        echo "**❌ 必跑层失败**"
    fi
    echo ""
    echo "Reports for failed layers (if any):"
    for name in unit integration smoke quality corpus e2e; do
        if [[ "${RESULTS[$name]:-}" == *FAIL* ]]; then
            echo "- $name: \`/tmp/test-pyramid-${name}.log\`"
        fi
    done
} >> "$REPORT"

# ── 终端汇总 ──────────────────────────────────────────────────
echo ""
phase "Test Pyramid Report"
echo "  报告: $REPORT"
echo ""
cat "$REPORT" | sed -n '/## Layer/,/^$/p'

# Exit code
ANY_FAIL=false
for name in unit integration smoke quality; do
    if [[ "${RESULTS[$name]:-}" == *FAIL* ]]; then
        ANY_FAIL=true
    fi
done
[ "$ANY_FAIL" = "false" ] || exit 1
