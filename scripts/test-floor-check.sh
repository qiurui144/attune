#!/usr/bin/env bash
# test-floor-check.sh — six-class test-floor guard (W4 #93).
#
# Mechanically enforces CLAUDE.md §6.1 "六类测试下限" + "Agent 验证铁律" as a CI
# gate, driven by the workspace SSOT `rust/agent_quality_manifest.yaml`:
#
#   golden     ≥ 10 real cases per agent       (universal floor; manifest fixture_min
#                                                shown for reference, enforced at runtime
#                                                by the gate test itself)
#   proptest   ≥ 3 property-test fns per agent  (fns inside proptest! blocks)
#   boundary   ≥ 5 #[test] (inline + boundary)
#   error      ≥ 3 error/expected_error cases
#   integration≥ 1 subprocess / E2E test
#   regression — every fixed bug adds a golden case (asserted by ratchet, not counted here)
#
# It ALSO enforces two structural guards:
#   (1) NEW-AGENT guard — any tests/golden/<agent>/ dir that is NOT referenced
#       by a manifest gate (test_name) FAILS: a new agent must ship a gate +
#       golden in the same PR (Agent 验证铁律 "同 PR 必须含测试").
#   (2) DOC-MATRIX presence — docs/TESTING.md must still carry the A–K
#       document-intelligence dimension matrix (release Gate 2/3/4 anchor).
#
# Modes:
#   (default / --hard)  deterministic structural checks are HARD (exit 1 on fail).
#   --warn              report only; always exit 0 (for opt-in rollout / local probe).
#
# Zero external deps (pure bash + grep/find/awk). Runs in CI and locally.
# No network, no LLM, no secrets.

set -uo pipefail

# ── locate repo root (script lives in <root>/scripts) ───────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RUST="$ROOT/rust"
MANIFEST="$RUST/agent_quality_manifest.yaml"
CORE_TESTS="$RUST/crates/attune-core/tests"
GOLDEN_DIR="$CORE_TESTS/golden"
TESTING_MD="$ROOT/docs/TESTING.md"

# ── floors (only-up; raising these is a deliberate ratchet) ─────────────────
GOLDEN_FLOOR=10
PROPTEST_FLOOR=3
BOUNDARY_FLOOR=5
ERROR_FLOOR=3

MODE="hard"
case "${1:-}" in
  --warn) MODE="warn" ;;
  --hard|"") MODE="hard" ;;
  *) echo "usage: $0 [--hard|--warn]"; exit 2 ;;
esac

fail_count=0
warn_count=0
note() { echo "  $*"; }
problem() {
  # $1 = severity (HARD|WARN), rest = message
  local sev="$1"; shift
  if [ "$sev" = "HARD" ] && [ "$MODE" = "hard" ]; then
    echo "  ✗ FAIL: $*"
    fail_count=$((fail_count + 1))
  else
    echo "  ⚠ WARN: $*"
    warn_count=$((warn_count + 1))
  fi
}

# safe match counter: prints an integer, never multi-line, never errors.
# (`grep -c` prints "0" AND exits 1 on no-match, so a naive `|| echo 0`
#  appends a second line — this wrapper avoids that classic bug.)
gcount() { grep -cE "$1" "$2" 2>/dev/null; true; }

if [ ! -f "$MANIFEST" ]; then
  echo "FATAL: manifest not found: $MANIFEST"
  exit 2
fi

echo "=================================================================="
echo "  test-floor-check.sh  (mode=$MODE)  — six-class floor + structure"
echo "  SSOT: rust/agent_quality_manifest.yaml"
echo "=================================================================="

# ── count golden cases in a dir (RECURSIVE — error cases live in an `error/`
#    subdir): single-doc files (^id:) count 1 each; multi-case list files
#    (^  - id: / ^- id:) count one per list item. ─────────────────────────────
count_golden_cases() {
  local dir="$1" total=0 f items
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    # list-style cases: lines like "  - id:" or "- id:" or "  - case_id:"
    items=$(gcount '^[[:space:]]*-[[:space:]]+(case_)?id:' "$f")
    items=${items:-0}
    if [ "$items" -gt 0 ]; then
      total=$((total + items))
    elif grep -qE '^(case_)?id:' "$f" 2>/dev/null; then
      # single-document file with a top-level `id:`/`case_id:` → 1 case
      total=$((total + 1))
    fi
  done < <(find "$dir" -type f \( -name '*.yaml' -o -name '*.yml' \) 2>/dev/null | sort)
  echo "$total"
}

# ── resolve the unique set of attune-core test files whose name contains the
#    agent slug OR its stem. Dedupes (slug-glob ⊂ stem-glob always overlaps),
#    so a file is never counted twice. Prints one path per line. ─────────────
agent_test_files() {
  local slug="$1" stem
  stem="${slug%_*}"
  { shopt -s nullglob
    for f in "$CORE_TESTS"/*"$slug"*.rs "$CORE_TESTS"/*"$stem"*.rs; do
      [ -e "$f" ] && echo "$f"
    done
    shopt -u nullglob
  } | sort -u
}

# ── property-test fns for an agent. proptest! blocks wrap MANY property fns;
#    §6.1 floor counts property tests, so count fns inside *proptest*.rs files
#    (all fns are property tests) PLUS inline property_*/prop_* invariant fns in
#    the agent's golden_gate file (e.g. linker uses #[test] fn property_…). ───
count_proptests_for() {
  local slug="$1" total=0 f n
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    case "$(basename "$f")" in
      *proptest*) n=$(gcount '^[[:space:]]*fn [a-z]' "$f") ;;        # (a) all fns
      *)          n=$(gcount '^[[:space:]]*fn (property_|prop_)' "$f") ;;  # (b) inline
    esac
    total=$((total + ${n:-0}))
  done < <(agent_test_files "$slug")
  echo "$total"
}

# ── boundary #[test] count in the agent's (deduped) test files ───────────────
count_unit_tests_for() {
  local slug="$1" total=0 f n
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    n=$(gcount '^[[:space:]]*#\[test\]' "$f")
    total=$((total + ${n:-0}))
  done < <(agent_test_files "$slug")
  echo "$total"
}

# ── error-case count: recurses (error cases live in an `error/` subdir AND as
#    *-error.yaml / eNN-*.yaml files), counts files flagged expected_error etc.
#    plus error-named yaml. ───────────────────────────────────────────────────
count_error_cases_for() {
  local dir="$1" n=0 a b
  if [ -d "$dir" ]; then
    # (a) content-flagged error cases (expected_error/error_kind/…) anywhere.
    a=$(grep -rlE 'expected_error|expect_error|error_kind|expect_fail' "$dir" 2>/dev/null | wc -l | tr -d ' ')
    # (b) the repo's error-case conventions: any yaml inside an `error/` subdir,
    #     OR error-named yaml (eNN-*.yaml / error-*.yaml / *-error.yaml).
    b=$(find "$dir" -type f -name '*.yaml' \( -path '*/error/*' \
            -o -iname 'e[0-9]*' -o -iname 'error-*.yaml' -o -iname '*-error.yaml' \) \
        2>/dev/null | sort -u | wc -l | tr -d ' ')
    n=$a; [ "$b" -gt "$n" ] && n=$b
  fi
  echo "$n"
}

# ── parse the manifest gates: emit "id<TAB>test_name<TAB>tier<TAB>fixture_min" ─
# awk over the YAML (gates are flat 2-space-indented blocks).
mapfile -t GATES < <(awk '
  /^gates:/ {ingates=1; next}
  /^[a-zA-Z_]+:/ && ingates && !/^  / {ingates=0}
  ingates && /^  - id:/ {
    if (id!="") print id"\t"tn"\t"tier"\t"fmin;
    id=$3; tn=""; tier=""; fmin=0
  }
  ingates && /^    test_name:/ {tn=$2}
  ingates && /^    tier:/ {tier=$2}
  ingates && /^    fixture_min:/ {fmin=$2}
  END { if (id!="") print id"\t"tn"\t"tier"\t"fmin }
' "$MANIFEST")

echo
echo "── 1. Per-agent six-class floor (manifest-driven) ──────────────────────"

# map a manifest gate id → its golden dir (best-effort by substring).
golden_dir_for() {
  local id="$1" d base
  shopt -s nullglob
  for d in "$GOLDEN_DIR"/*/; do
    base="$(basename "$d")"
    case "$id" in
      *"$base"*|"$base"*) echo "$d"; shopt -u nullglob; return ;;
    esac
    case "$base" in
      *"$id"*) echo "$d"; shopt -u nullglob; return ;;
    esac
  done
  shopt -u nullglob
  # explicit known aliases (manifest gate id ↔ golden/corpora dir mismatch)
  case "$id" in
    memory_consolidation) [ -d "$GOLDEN_DIR/memory_promotion" ] && echo "$GOLDEN_DIR/memory_promotion/" ;;
    self_evolving_skill)  [ -d "$GOLDEN_DIR/skill_evolution" ]  && echo "$GOLDEN_DIR/skill_evolution/" ;;
    linker)               [ -d "$CORE_TESTS/corpora/linker_golden" ] && echo "$CORE_TESTS/corpora/linker_golden/" ;;
    chat_reliability)     [ -d "$CORE_TESTS/corpora/chat_reliability_golden" ] && echo "$CORE_TESTS/corpora/chat_reliability_golden/" ;;
    *) echo "" ;;
  esac
}

for line in "${GATES[@]}"; do
  IFS=$'\t' read -r gid tname tier fmin <<< "$line"
  [ -n "$gid" ] || continue
  echo
  echo "  Gate: $gid  (tier=$tier, fixture_min=${fmin:-0})"

  # engine gates (OCR/ASR) have golden in attune-server; real-LLM gate has no
  # own golden dir (reuses deterministic corpora). Only enforce golden floor on
  # deterministic agent gates that own a core golden dir.
  gdir="$(golden_dir_for "$gid")"
  if [ "$tier" = "deterministic" ] && [ -n "$gdir" ] && [ -d "$gdir" ]; then
    gc=$(count_golden_cases "$gdir")
    # Universal §6.1 floor = 10. The gate's own runtime test enforces the
    # (stricter) manifest fixture_min; we show it for reference, not re-enforce
    # (avoids a count-method drift between this scanner and the gate harness).
    if [ "$gc" -ge "$GOLDEN_FLOOR" ]; then
      note "✓ golden: $gc cases (≥ $GOLDEN_FLOOR; manifest fixture_min=${fmin:-?})  [${gdir#"$ROOT"/}]"
    else
      problem HARD "$gid golden $gc < floor $GOLDEN_FLOOR  [${gdir#"$ROOT"/}]"
    fi

    ec=$(count_error_cases_for "$gdir")
    if [ "$ec" -ge "$ERROR_FLOOR" ]; then
      note "✓ error-case: $ec (≥ $ERROR_FLOOR)"
    else
      problem WARN "$gid error-cases $ec < floor $ERROR_FLOOR (expected_error/error_kind/eNN files)"
    fi
  elif [ "$tier" = "deterministic" ]; then
    problem WARN "$gid is deterministic but no core golden dir resolved — check manifest↔golden mapping"
  else
    note "· non-deterministic ($tier) — golden floor N/A (engine/LLM corpora)"
  fi

  # proptest + boundary apply to the deterministic agent gates.
  if [ "$tier" = "deterministic" ]; then
    pc=$(count_proptests_for "$gid")
    if [ "$pc" -ge "$PROPTEST_FLOOR" ]; then
      note "✓ proptest: $pc property fns (≥ $PROPTEST_FLOOR)"
    else
      problem WARN "$gid proptest $pc < floor $PROPTEST_FLOOR"
    fi
    bc=$(count_unit_tests_for "$gid")
    if [ "$bc" -ge "$BOUNDARY_FLOOR" ]; then
      note "✓ boundary #[test]: $bc (≥ $BOUNDARY_FLOOR)"
    else
      problem WARN "$gid boundary #[test] $bc < floor $BOUNDARY_FLOOR"
    fi
  fi
done

echo
echo "── 2. NEW-AGENT guard (golden dir must map to a manifest gate) ─────────"
# Every core golden dir must be claimed by some manifest gate (EXACT name match,
# not substring — a substring rule lets an unlucky/adversarial new dir name like
# 'doc' or 'skill' silently pass the very gate it must trip), OR be a known
# ungated corpus dir. A NEW unclaimed agent dir = a shipped agent with no gate →
# HARD fail (same-PR test rule). Build the allow-set of EXACT dir names first.
KNOWN_UNGATED="memory_continuity organize doc_compare_verdict"
# Allow-set = { each gid, each test_name with the _golden_gate suffix stripped,
#   the explicit golden↔gate aliases }. One name per line for exact-match lookup.
build_claimed_names() {
  local line gid tname tier fmin
  for line in "${GATES[@]}"; do
    IFS=$'\t' read -r gid tname tier fmin <<< "$line"
    [ -n "$gid" ] && echo "$gid"
    # test_name like 'document_classifier_agent_golden_gate' → also accept the
    # golden-dir stem 'document_classifier'.
    [ -n "$tname" ] && echo "${tname%%_agent_golden_gate}" && echo "${tname%%_golden_gate}"
  done
  # explicit golden/corpora dir ↔ gate aliases (dir name on the LEFT).
  printf '%s\n' memory_promotion skill_evolution linker_golden chat_reliability_golden \
                document_classifier chat_reliability
}
mapfile -t CLAIMED_NAMES < <(build_claimed_names | sort -u)

is_exact_claimed() {
  local b="$1" name
  for name in "${CLAIMED_NAMES[@]}"; do
    [ "$b" = "$name" ] && return 0
  done
  return 1
}

shopt -s nullglob
for d in "$GOLDEN_DIR"/*/; do
  base="$(basename "$d")"
  if is_exact_claimed "$base"; then
    note "✓ $base → claimed by a manifest gate"
  elif echo " $KNOWN_UNGATED " | grep -qw "$base"; then
    note "· $base → known ungated corpus (reused by E2E / real-LLM gate; allowed)"
  else
    problem HARD "golden dir '$base' has NO manifest gate — a new agent must ship a gate + golden in the SAME PR (Agent 验证铁律)"
  fi
done
shopt -u nullglob

echo
echo "── 3. DOC-MATRIX presence (A–K dimension matrix = release Gate anchor) ──"
if [ -f "$TESTING_MD" ]; then
  if grep -qE 'A-K|A–K' "$TESTING_MD" && grep -qE '维度覆盖矩阵|dimension.*matrix' "$TESTING_MD"; then
    # spot-check that at least 8 axis rows exist
    rows=$(gcount '^\| \*\*[A-K]\*\*' "$TESTING_MD"); rows=${rows:-0}
    if [ "$rows" -ge 8 ]; then
      note "✓ docs/TESTING.md carries the A–K dimension matrix ($rows axis rows; release Gate 2/3/4 anchor)"
    else
      problem HARD "docs/TESTING.md A–K matrix has only $rows axis rows (< 8) — release dimension gate degraded"
    fi
  else
    problem HARD "docs/TESTING.md missing the A–K document-intelligence dimension matrix (release Gate anchor)"
  fi
else
  problem HARD "docs/TESTING.md not found — release dimension matrix anchor missing"
fi

echo
echo "── 4. Workspace-wide floor sanity (proptest / integration presence) ────"
TOTAL_PROPTEST=$(grep -rlE '^[[:space:]]*proptest!' "$CORE_TESTS" 2>/dev/null | wc -l | tr -d ' ')
note "proptest files in attune-core/tests: $TOTAL_PROPTEST"
SUBPROC=$(find "$RUST/crates" -not -path '*/target/*' \( -path '*/tests/*subprocess*.rs' -o -path '*/tests/*_e2e*.rs' \) 2>/dev/null | grep -c '.')
if [ "$SUBPROC" -ge 1 ]; then
  note "✓ integration/E2E subprocess tests present: $SUBPROC"
else
  problem WARN "no *_subprocess.rs / *_e2e.rs integration test found (≥1 required)"
fi

echo
echo "=================================================================="
echo "  RESULT: hard-fails=$fail_count  warns=$warn_count  (mode=$MODE)"
echo "=================================================================="
if [ "$MODE" = "hard" ] && [ "$fail_count" -gt 0 ]; then
  echo "  ✗ six-class test floor NOT met — see FAIL lines above."
  echo "    Fix: add the missing golden/gate, or (deliberate) raise the manifest"
  echo "    fixture_min + ratchet_baseline together. Do NOT lower a floor."
  exit 1
fi
echo "  ✓ floor guards satisfied (hard checks)."
exit 0
