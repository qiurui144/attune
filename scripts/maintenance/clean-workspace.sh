#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REPORT_DIR="$ROOT/reports/maintenance"
APPLY=0
INCLUDE_AGENT_CACHE=0
requested_roots=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      APPLY=0
      shift
      ;;
    --apply)
      APPLY=1
      shift
      ;;
    --report-dir)
      REPORT_DIR="${2:-}"
      shift 2
      ;;
    --include-agent-cache)
      INCLUDE_AGENT_CACHE=1
      shift
      ;;
    --root)
      requested_roots+=("${2:-}")
      shift 2
      ;;
    -h|--help)
      cat <<'HELP'
Clean ignored generated outputs from the Attune workspace.

Usage:
  bash scripts/maintenance/clean-workspace.sh --dry-run
  bash scripts/maintenance/clean-workspace.sh --apply
  bash scripts/maintenance/clean-workspace.sh --apply --root reports/runs --root tmp

Default mode is dry-run. Apply mode uses git clean -fdX only on approved generated roots.
It never calls git reset, git checkout, or removes tracked files.
Use --root to restrict cleanup to one or more approved generated roots.
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
if [ "$APPLY" = "1" ]; then
  REPORT="$REPORT_DIR/clean-workspace-apply.md"
  MODE_LABEL="Apply"
else
  REPORT="$REPORT_DIR/clean-workspace-dry-run.md"
  MODE_LABEL="Dry Run"
fi

roots=(
  "dist/release"
  "reports/release"
  "reports/maintenance"
  "reports/runs"
  "tests/reports"
  "tmp"
  ".playwright-mcp"
  "rust/target"
  "apps/attune-desktop/target"
  "extension/node_modules"
  "rust/crates/attune-server/ui/node_modules"
)

if [ "$INCLUDE_AGENT_CACHE" = "1" ]; then
  roots+=(".remember")
fi

if [ "${#requested_roots[@]}" -gt 0 ]; then
  roots=("${requested_roots[@]}")
fi

approved_roots=(
  "dist/release"
  "reports/release"
  "reports/maintenance"
  "reports/runs"
  "tests/reports"
  "tmp"
  ".playwright-mcp"
  "rust/target"
  "apps/attune-desktop/target"
  "extension/node_modules"
  "rust/crates/attune-server/ui/node_modules"
  ".remember"
)

is_approved_root() {
  local candidate="$1"
  local approved
  for approved in "${approved_roots[@]}"; do
    if [ "$candidate" = "$approved" ]; then
      return 0
    fi
  done
  return 1
}

for root in "${roots[@]}"; do
  if [ -z "$root" ] || [[ "$root" = /* ]] || [[ "$root" = *..* ]] || ! is_approved_root "$root"; then
    echo "refusing unapproved cleanup root: $root" >&2
    exit 2
  fi
done

clean_roots=("${roots[@]}")
if [ "$APPLY" = "1" ]; then
  clean_roots=()
  for root in "${roots[@]}"; do
    if [ "$root" = "reports/maintenance" ]; then
      continue
    fi
    clean_roots+=("$root")
  done
fi

{
  echo "# Attune Workspace Cleanup $MODE_LABEL"
  echo
  echo "- Timestamp: $(date -Iseconds)"
  echo "- Branch: $(git -C "$ROOT" branch --show-current 2>/dev/null || echo unknown)"
  echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "- Mode: $MODE_LABEL"
  echo "- Agent cache included: $INCLUDE_AGENT_CACHE"
  echo
  echo "## Approved Generated Roots"
  echo
  for root in "${clean_roots[@]}"; do
    echo "- \`$root\`"
  done
  if [ "$APPLY" = "1" ]; then
    echo "- \`reports/maintenance\` skipped in apply mode to preserve this report"
  fi
  echo
  echo "## Git Clean Output"
  echo
  echo '```text'
} > "$REPORT"

if [ "$APPLY" = "1" ]; then
  git -C "$ROOT" clean -fdX -- "${clean_roots[@]}" >> "$REPORT" 2>&1 || {
    rc=$?
    echo '```' >> "$REPORT"
    echo "cleanup failed with exit code $rc; report: $REPORT" >&2
    exit "$rc"
  }
else
  git -C "$ROOT" clean -ndX -- "${clean_roots[@]}" >> "$REPORT" 2>&1
fi

{
  echo '```'
  echo
  echo "## Safety"
  echo
  echo "- This script targets ignored files only via \`git clean -fdX\` in apply mode."
  echo "- Tracked files, untracked non-ignored files, docs, screenshots, and benchmark evidence are not removed by this command."
} >> "$REPORT"

echo "cleanup report: $REPORT"
