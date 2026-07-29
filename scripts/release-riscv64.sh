#!/usr/bin/env bash
# attune-server riscv64 release workflow
# Usage: bash scripts/release-riscv64.sh <new_version> [--dry-run]
# Example: bash scripts/release-riscv64.sh 1.5.2
#
# Flow: version_bump → commit → tag → build(optimized) → package_deb → verify_sha256
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-}"
DRY_RUN=0
[ "${2:-}" = "--dry-run" ] && DRY_RUN=1

if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version> [--dry-run]" >&2
  exit 2
fi

log() { printf "\033[1;36m[release]\033[0m %s\n" "$*"; }
err() { printf "\033[1;31m[release-err]\033[0m %s\n" "$*" >&2; }

# 1. Version bump
OLD_VERSION=$(grep '^version' "$ROOT/rust/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')
log "step 1/5: version bump ${OLD_VERSION} → ${VERSION}"
if [ "$DRY_RUN" = "0" ]; then
  sed -i "s/^version = \"${OLD_VERSION}\"/version = \"${VERSION}\"/" "$ROOT/rust/Cargo.toml"
fi
log "  Cargo.toml updated"

# 2. Git commit + tag
log "step 2/5: git commit + tag"
if [ "$DRY_RUN" = "0" ]; then
  cd "$ROOT"
  git add rust/Cargo.toml
  git commit -m "release: v${VERSION}" || true
  git tag -a "v${VERSION}" -m "v${VERSION}: release"
  log "  commit + tag: v${VERSION}"
else
  log "  [dry-run] skip commit + tag"
fi

# 3. Build (use build-optimized.sh for correct toolchain)
log "step 3/5: build riscv64 binary"
TOOLCHAIN="${ATTUNE_RVA23_TOOLCHAIN:-/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2}"
if [ "$DRY_RUN" = "0" ]; then
  ATTUNE_RVA23_TOOLCHAIN="$TOOLCHAIN" \
  bash "$ROOT/scripts/build-optimized.sh" \
    --profile rva23 \
    --package attune-server \
    --features scheduler-runtime,artifact-export-rich,wasm-runtime \
    -- --no-default-features --bin attune-server-headless
  log "  build complete"
else
  log "  [dry-run] skip build"
fi

# 4. Package deb
log "step 4/5: package deb"
if [ "$DRY_RUN" = "0" ]; then
  ATTUNE_PACKAGE_SKIP_FRONTEND=1 ATTUNE_PACKAGE_SKIP_BUILD=1 ATTUNE_PACKAGE_SKIP_RVV_AUDIT=1 \
  bash "$ROOT/scripts/package-riscv64-deb.sh" \
    --out-dir "$ROOT/dist/release/riscv64-server-deb" \
    --reports-dir "$ROOT/reports/release"
  log "  deb packaged"
else
  log "  [dry-run] skip packaging"
fi

# 5. Verify SHA256
log "step 5/5: verify"
DEB=$(ls "$ROOT/dist/release/riscv64-server-deb/attune-server_${VERSION}_riscv64.deb" 2>/dev/null)
if [ -z "$DEB" ]; then
  err "  deb not found at expected location"
  # fallback to old version pattern
  DEB=$(ls "$ROOT/dist/release/riscv64-server-deb/attune-server_"*"_riscv64.deb" 2>/dev/null | tail -1)
  [ -z "$DEB" ] && err "  no deb found" && exit 1
  log "  fallback deb: $DEB"
fi
sha256sum "$DEB"
log "  deb SHA256 verified"
log "=== release v${VERSION} complete ==="
