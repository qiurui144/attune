#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REPORT_DIR="$ROOT/reports/maintenance"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-dir)
      REPORT_DIR="${2:-}"
      shift 2
      ;;
    -h|--help)
      cat <<'HELP'
Audit Attune script entry points and generated output locations.

Usage:
  bash scripts/maintenance/audit-scripts-and-outputs.sh [--report-dir path]

The script is read-only. It writes scripts-and-outputs-audit.md to the report dir.
HELP
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

mkdir -p "$REPORT_DIR"
REPORT="$REPORT_DIR/scripts-and-outputs-audit.md"

script_roots=(
  "scripts"
  "tests/e2e"
  ".github/scripts"
  "apps/attune-desktop/scripts"
  "rust/scripts"
)

output_roots=(
  "dist"
  "dist/release"
  "reports"
  "reports/runs"
  "reports/release"
  "reports/maintenance"
  "docs/reports"
  "docs/benchmarks"
  "tests/reports"
  "tmp"
  ".playwright-mcp"
  ".remember"
  "rust/target"
  "apps/attune-desktop/target"
  "extension/node_modules"
  "rust/crates/attune-server/ui/node_modules"
)

count_lines() {
  wc -l | awk '{print $1}'
}

tracked_count() {
  git -C "$ROOT" ls-files -- "$1" | count_lines
}

ignored_count() {
  git -C "$ROOT" status --ignored --porcelain -- "$1" 2>/dev/null | awk '$1 == "!!" {count++} END {print count+0}'
}

untracked_count() {
  git -C "$ROOT" status --porcelain -- "$1" 2>/dev/null | awk '$1 == "??" {count++} END {print count+0}'
}

dir_size() {
  if [ -e "$ROOT/$1" ]; then
    du -sh "$ROOT/$1" 2>/dev/null | awk '{print $1}'
  else
    printf '%s' "-"
  fi
}

file_count() {
  if [ -d "$ROOT/$1" ]; then
    find "$ROOT/$1" -type f 2>/dev/null | count_lines
  elif [ -f "$ROOT/$1" ]; then
    printf '1\n'
  else
    printf '0\n'
  fi
}

{
  echo "# Attune Script and Output Audit"
  echo
  echo "- Timestamp: $(date -Iseconds)"
  echo "- Branch: $(git -C "$ROOT" branch --show-current 2>/dev/null || echo unknown)"
  echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo
  echo "## Script Inventory"
  echo
  echo "| Root | Exists | Files | Tracked | Notes |"
  echo "| --- | ---: | ---: | ---: | --- |"
  for root in "${script_roots[@]}"; do
    exists="no"
    [ -e "$ROOT/$root" ] && exists="yes"
    files="$(file_count "$root")"
    tracked="$(tracked_count "$root")"
    notes=""
    case "$root" in
      scripts) notes="canonical top-level operational scripts; new release/maintenance subdirs live here" ;;
      tests/e2e) notes="canonical end-to-end test implementation" ;;
      .github/scripts) notes="CI helper scripts only" ;;
      apps/attune-desktop/scripts) notes="Tauri installer lifecycle hooks only" ;;
      rust/scripts) notes="Rust-workspace-specific fixture and quality helper scripts" ;;
    esac
    echo "| \`$root\` | $exists | $files | $tracked | $notes |"
  done
  echo
  echo "## Output Roots"
  echo
  echo "| Root | Exists | Size | Files | Tracked | Untracked | Ignored | Recommendation |"
  echo "| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |"
  for root in "${output_roots[@]}"; do
    exists="no"
    [ -e "$ROOT/$root" ] && exists="yes"
    size="$(dir_size "$root")"
    files="$(file_count "$root")"
    tracked="$(tracked_count "$root")"
    untracked="$(untracked_count "$root")"
    ignored="$(ignored_count "$root")"
    recommendation="inventory only"
    case "$root" in
      dist|dist/release) recommendation="canonical generated release artifact root; ignored" ;;
      reports/release) recommendation="canonical generated release validation reports; ignored" ;;
      reports/maintenance) recommendation="canonical generated maintenance reports; ignored" ;;
      reports/runs|tests/reports|tmp|.playwright-mcp) recommendation="generated local evidence; cleanup dry-run may target ignored files" ;;
      docs/reports|docs/benchmarks) recommendation="tracked historical evidence may live here; do not auto-clean" ;;
      .remember) recommendation="local assistant memory/cache; inventory only by default" ;;
      *target*|*node_modules*) recommendation="build dependency/cache output; cleanup only when user accepts rebuild cost" ;;
    esac
    echo "| \`$root\` | $exists | $size | $files | $tracked | $untracked | $ignored | $recommendation |"
  done
  echo
  echo "## Canonical New Output Locations"
  echo
  echo "- Build artifacts: \`dist/release/riscv64-server-deb/\`"
  echo "- Release validation reports: \`reports/release/\`"
  echo "- Maintenance reports: \`reports/maintenance/\`"
  echo
  echo "## Cleanup Policy"
  echo
  echo "Cleanup must default to dry-run. Apply mode may remove only git-ignored generated files with \`git clean -fdX -- <approved roots>\`."
} > "$REPORT"

echo "audit report: $REPORT"
