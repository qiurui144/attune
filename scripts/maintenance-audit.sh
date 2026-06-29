#!/usr/bin/env bash
# maintenance-audit.sh - repository maintenance invariants.
#
# This is a lightweight CI gate for hygiene regressions that are easy to miss in
# feature tests: tracked runtime artifacts, missing lockfiles, stale ports,
# desktop startup observability, UI empty links, and CI coverage hooks.

set -euo pipefail

REPO="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO"

PASS=0
FAIL=0

ok() {
  printf 'OK   %s\n' "$*"
  PASS=$((PASS + 1))
}

bad() {
  printf 'FAIL %s\n' "$*" >&2
  FAIL=$((FAIL + 1))
}

require_file() {
  local path="$1"
  local label="$2"
  if [ -f "$path" ]; then ok "$label"; else bad "$label missing: $path"; fi
}

require_dir() {
  local path="$1"
  local label="$2"
  if [ -d "$path" ]; then ok "$label"; else bad "$label missing: $path"; fi
}

require_tracked() {
  local path="$1"
  local label="$2"
  if git ls-files --error-unmatch "$path" >/dev/null 2>&1; then ok "$label"; else bad "$label not tracked: $path"; fi
}

require_grep() {
  local pattern="$1"
  local path="$2"
  local label="$3"
  if rg -q "$pattern" "$path"; then ok "$label"; else bad "$label not found in $path"; fi
}

require_no_grep() {
  local pattern="$1"
  local path="$2"
  local label="$3"
  local out
  out=$(rg -n "$pattern" "$path" 2>/dev/null || true)
  if [ -z "$out" ]; then
    ok "$label"
  else
    bad "$label"
    printf '%s\n' "$out" | sed -n '1,20p' >&2
  fi
}

require_no_grep_many() {
  local pattern="$1"
  local label="$2"
  shift 2
  local out
  out=$(rg -n --glob '!scripts/maintenance-audit.sh' "$pattern" "$@" 2>/dev/null || true)
  if [ -z "$out" ]; then
    ok "$label"
  else
    bad "$label"
    printf '%s\n' "$out" | sed -n '1,20p' >&2
  fi
}

printf 'Maintenance audit - %s\n' "$REPO"

# 1-4: required source surfaces.
require_dir "apps/attune-desktop" "desktop app exists"
require_dir "rust/crates/attune-server/ui/src" "embedded UI source exists"
require_dir "rust/crates/attune-core/tests" "core tests exist"
require_dir "tests/e2e" "repo e2e scripts exist"

# 5-7: reproducible dependency inputs.
require_tracked "rust/Cargo.lock" "Rust lockfile tracked"
require_tracked "rust/crates/attune-server/ui/package-lock.json" "embedded UI lockfile tracked"
require_tracked "python/tests/e2e/package-lock.json" "Python e2e Playwright lockfile tracked"

# 8-9: runtime artifacts do not become source.
tracked_logs=$(git ls-files '*.log' 'logs/**' '.playwright-mcp/**' '.remember/**' 'tmp/**' 'reports/runs/**' 2>/dev/null || true)
if [ -z "$tracked_logs" ]; then
  ok "no tracked runtime logs or run artifacts"
else
  bad "tracked runtime logs or run artifacts"
  printf '%s\n' "$tracked_logs" >&2
fi
require_grep '^reports/$' ".gitignore" "reports directory ignored by default"

# 10-12: desktop startup/diagnostics invariants.
require_grep 'ATTUNE_DESKTOP_PORT' "apps/attune-desktop/src/embedded_server.rs" "desktop port is configurable"
require_grep 'attune-desktop-startup\.log' "apps/attune-desktop/src/main.rs" "desktop startup log is explicit"
require_grep 'SERVER_ERROR' "apps/attune-desktop/src/embedded_server.rs" "desktop readiness exposes startup error"

# 13-14: port consistency for current embedded server contract.
require_grep '18900' "tests/e2e/kb_longloop_windows.ps1" "Windows KB loop targets desktop port 18900"
require_no_grep_many '28630' "no stale 28630 port in active code/scripts/workflows" \
  apps scripts tests/e2e rust/crates/attune-server/ui/src .github/workflows

# 15-16: UI interaction hygiene.
require_no_grep 'href=\\{?["'\''](#|javascript:|)["'\'']' "rust/crates/attune-server/ui/src" "embedded UI has no empty/javascript links"
require_no_grep 'onClick=\\{\\(\\) => \\{\\}\\}' "rust/crates/attune-server/ui/src" "embedded UI has no inert click handlers"

# 17-20: CI and release coverage hooks.
require_grep 'maintenance-audit\.sh' ".github/workflows/ci.yml" "CI runs maintenance audit"
require_grep 'working-directory: rust/crates/attune-server/ui' ".github/workflows/ci.yml" "CI builds embedded UI"
require_grep 'cargo audit' ".github/workflows/ci.yml" "CI runs cargo security audit"
require_grep 'openvino' ".github/workflows/desktop-release.yml" "desktop release includes OpenVINO bundle"

# 21-25: package-manager seed manifests follow the current release shape.
require_grep 'PackageVersion: 1\.5\.0' "packaging/winget/qiurui144.Attune.yaml" "WinGet version seed matches current release"
require_grep 'desktop-v1\.5\.0/Attune_1\.5\.0_x64-setup\.exe' "packaging/winget/qiurui144.Attune.installer.yaml" "WinGet installer URL matches desktop release"
require_grep '"version": "1\.5\.0"' "packaging/scoop/attune.json" "Scoop version seed matches current release"
require_grep 'attune-windows-x86_64\.zip' "packaging/scoop/attune.json" "Scoop uses Windows zip release asset"
require_grep 'version "1\.5\.0"' "packaging/homebrew/Formula/attune.rb" "Homebrew formula version seed matches current release"
require_grep 'python/tests/e2e && npm ci && npx playwright test' "scripts/test-pyramid.sh" "test pyramid E2E path matches current Playwright suite"
require_no_grep_many 'releases/download/v1\.0\.0|desktop-v1\.0\.0|attune-server:v1\.0\.0|attune-desktop-installers:1\.0\.0' \
  "active install docs do not point users to v1.0.0 artifacts" \
  docs/DEPLOY.md docs/INSTALL.md docs/wiki/index.md docs/wiki/quickstart.md packaging

printf '\nMaintenance audit summary: %s passed, %s failed\n' "$PASS" "$FAIL"
if [ "$FAIL" -ne 0 ]; then
  exit 1
fi
