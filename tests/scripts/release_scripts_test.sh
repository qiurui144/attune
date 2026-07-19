#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d -t attune-release-scripts-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

required=(
  "$ROOT/scripts/release/build-riscv64-server-deb.sh"
  "$ROOT/scripts/release/test-k3-nas-web-demo.sh"
  "$ROOT/scripts/maintenance/audit-scripts-and-outputs.sh"
  "$ROOT/scripts/maintenance/clean-workspace.sh"
)

for script in "${required[@]}"; do
  test -f "$script"
  bash -n "$script"
done

bash "$ROOT/scripts/release/build-riscv64-server-deb.sh" \
  --dry-run \
  --version 9.9.9-test \
  --out-dir "$TMP/dist" \
  --reports-dir "$TMP/reports"
test -f "$TMP/reports/build-riscv64-server-deb-dry-run.md"

FAKE_TARGET="riscv64-test-contract"
FAKE_BIN="$ROOT/rust/target/$FAKE_TARGET/release/attune-server-headless"
FAKE_PKG="$TMP/pkg"
mkdir -p "$(dirname "$FAKE_BIN")"
printf '#!/usr/bin/env sh\nexit 0\n' > "$FAKE_BIN"
chmod 0755 "$FAKE_BIN"
ATTUNE_RISCV64_TARGET="$FAKE_TARGET" bash "$ROOT/scripts/release/build-riscv64-server-deb.sh" \
  --skip-frontend \
  --skip-build \
  --skip-rvv-audit \
  --version 9.9.10-contract \
  --out-dir "$FAKE_PKG" \
  --reports-dir "$TMP/reports"
test -f "$FAKE_PKG/attune-server_9.9.10-contract_riscv64.deb"
test -f "$FAKE_PKG/attune-server_9.9.10-contract_riscv64.deb.sha256"
test "$(wc -l < "$FAKE_PKG/attune-server_9.9.10-contract_riscv64.deb.sha256")" -eq 1
sha256sum -c "$FAKE_PKG/attune-server_9.9.10-contract_riscv64.deb.sha256"
test "$(dpkg-deb --field "$FAKE_PKG/attune-server_9.9.10-contract_riscv64.deb" Architecture)" = "riscv64"

bash "$ROOT/scripts/release/test-k3-nas-web-demo.sh" \
  --dry-run \
  --skip-deb-check \
  --deb "$TMP/fake.deb" \
  --base-url http://127.0.0.1:18900 \
  --reports-dir "$TMP/reports"
test -f "$TMP/reports/k3-nas-web-demo-dry-run.md"

bash "$ROOT/scripts/maintenance/audit-scripts-and-outputs.sh" \
  --report-dir "$TMP/maintenance"
test -f "$TMP/maintenance/scripts-and-outputs-audit.md"
grep -q "Script Inventory" "$TMP/maintenance/scripts-and-outputs-audit.md"
grep -q "Output Roots" "$TMP/maintenance/scripts-and-outputs-audit.md"

bash "$ROOT/scripts/maintenance/clean-workspace.sh" \
  --dry-run \
  --report-dir "$TMP/maintenance"
test -f "$TMP/maintenance/clean-workspace-dry-run.md"
grep -q "Dry Run" "$TMP/maintenance/clean-workspace-dry-run.md"

bash "$ROOT/scripts/maintenance/clean-workspace.sh" \
  --dry-run \
  --report-dir "$TMP/maintenance" \
  --root tmp \
  --root tests/reports
grep -q '`tmp`' "$TMP/maintenance/clean-workspace-dry-run.md"
grep -q '`tests/reports`' "$TMP/maintenance/clean-workspace-dry-run.md"
if grep -q '`dist/release`' "$TMP/maintenance/clean-workspace-dry-run.md"; then
  echo "custom-root cleanup unexpectedly includes dist/release" >&2
  exit 1
fi

echo "release script contracts PASS"
