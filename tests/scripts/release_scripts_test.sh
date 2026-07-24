#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d -t attune-release-scripts-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

required=(
  "$ROOT/scripts/package-riscv64-deb.sh"
  "$ROOT/scripts/audit-rvv-vectorization.sh"
  "$ROOT/scripts/release/build-riscv64-server-deb.sh"
  "$ROOT/scripts/release/probe-attune-package-boundary.sh"
  "$ROOT/scripts/release/test-k3-nas-web-demo.sh"
  "$ROOT/scripts/release/test-k3-rvv-runtime-gate.sh"
  "$ROOT/scripts/maintenance/audit-scripts-and-outputs.sh"
  "$ROOT/scripts/maintenance/clean-workspace.sh"
)

required_python=(
  "$ROOT/scripts/release/probe-nas-web-api-contract.py"
)

for script in "${required[@]}"; do
  test -f "$script"
  bash -n "$script"
done

for script in "${required_python[@]}"; do
  test -f "$script"
  python3 -m py_compile "$script"
done

desktop_hooks=(
  "$ROOT/apps/attune-desktop/scripts/preinst.sh"
  "$ROOT/apps/attune-desktop/scripts/prerm.sh"
  "$ROOT/apps/attune-desktop/scripts/postinst.sh"
  "$ROOT/apps/attune-desktop/scripts/postrm.sh"
)

for script in "${desktop_hooks[@]}"; do
  test -f "$script"
  bash -n "$script"
done

for script in "$ROOT/apps/attune-desktop/scripts/preinst.sh" "$ROOT/apps/attune-desktop/scripts/prerm.sh"; do
  if grep -Eq '\bp(kill|grep)\b[^\n]*-[A-Za-z]*f' "$script"; then
    echo "desktop maintainer hooks must not stop Attune with pgrep/pkill -f: $script" >&2
    exit 1
  fi
done

bash "$ROOT/scripts/release/build-riscv64-server-deb.sh" \
  --dry-run \
  --version 9.9.9-test \
  --out-dir "$TMP/dist" \
  --reports-dir "$TMP/reports"
test -f "$TMP/reports/build-riscv64-server-deb-dry-run.md"

bash "$ROOT/scripts/package-riscv64-deb.sh" \
  --dry-run \
  --version 9.9.9-test \
  --out-dir "$TMP/one-key-dist" \
  --reports-dir "$TMP/one-key-reports"
test -f "$TMP/one-key-reports/package-riscv64-deb-dry-run.md"
test -f "$TMP/one-key-reports/build-riscv64-server-deb-dry-run.md"
grep -q "One-key riscv64 Debian Package" "$TMP/one-key-reports/package-riscv64-deb-dry-run.md"
grep -q "scripts/release/build-riscv64-server-deb.sh" "$TMP/one-key-reports/package-riscv64-deb-dry-run.md"

BOUNDARY_OK="$TMP/boundary-ok"
BOUNDARY_BAD="$TMP/boundary-bad"
mkdir -p "$BOUNDARY_OK/usr/bin" "$BOUNDARY_BAD/usr/lib"
printf 'server\n' > "$BOUNDARY_OK/usr/bin/attune-server-headless"
printf 'model\n' > "$BOUNDARY_BAD/usr/lib/model.onnx"
bash "$ROOT/scripts/release/probe-attune-package-boundary.sh" "$BOUNDARY_OK" > "$TMP/boundary-ok.txt"
grep -q "attune package boundary PASS" "$TMP/boundary-ok.txt"
if bash "$ROOT/scripts/release/probe-attune-package-boundary.sh" "$BOUNDARY_BAD" > "$TMP/boundary-bad.txt" 2>&1; then
  echo "package boundary probe unexpectedly accepted model/runtime artifact" >&2
  exit 1
fi
grep -q "inference runtime/model-looking files" "$TMP/boundary-bad.txt"

FAKE_TOOLS="$TMP/fake-rvv-tools"
mkdir -p "$FAKE_TOOLS"
printf '%s\n' \
  '#!/usr/bin/env sh' \
  'if [ "$1" = "-d" ]; then' \
  '  printf " 0: 0c07f6d7           vsetvli a3,a5,e8,m1,ta,ma\n"' \
  '  printf " 4: 02070087           vle8.v v1,(a4)\n"' \
  'fi' > "$FAKE_TOOLS/objdump"
printf '%s\n' \
  '#!/usr/bin/env sh' \
  'if [ "$1" = "-A" ]; then' \
  '  printf "Attribute Section: riscv\n"' \
  '  printf "  Tag_RISCV_arch: \"rv64i2p1_v1p0_zve64d1p0\"\n"' \
  'fi' > "$FAKE_TOOLS/readelf"
chmod 0755 "$FAKE_TOOLS/objdump" "$FAKE_TOOLS/readelf"
FAKE_RVV_ARTIFACT="$TMP/fake-rvv-artifact"
printf 'fake artifact\n' > "$FAKE_RVV_ARTIFACT"
ATTUNE_RVV_OBJDUMP="$FAKE_TOOLS/objdump" \
ATTUNE_RVV_READELF="$FAKE_TOOLS/readelf" \
ATTUNE_RVV_AUDIT_STRICT=1 \
ATTUNE_RVV_AUDIT_MIN_MAIN_LINES=2 \
  bash "$ROOT/scripts/audit-rvv-vectorization.sh" "$FAKE_RVV_ARTIFACT" > "$TMP/rvv-audit-pass.txt"
grep -q "main_rvv_threshold_met: 1" "$TMP/rvv-audit-pass.txt"
if ATTUNE_RVV_OBJDUMP="$FAKE_TOOLS/objdump" \
    ATTUNE_RVV_READELF="$FAKE_TOOLS/readelf" \
    ATTUNE_RVV_AUDIT_STRICT=1 \
    ATTUNE_RVV_AUDIT_MIN_MAIN_LINES=3 \
      bash "$ROOT/scripts/audit-rvv-vectorization.sh" "$FAKE_RVV_ARTIFACT" > "$TMP/rvv-audit-fail.txt"; then
  echo "RVV audit threshold unexpectedly passed" >&2
  exit 1
fi
grep -q "main_rvv_threshold_met: 0" "$TMP/rvv-audit-fail.txt"

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
EXTRACTED_PKG="$TMP/extracted-contract"
dpkg-deb -x "$FAKE_PKG/attune-server_9.9.10-contract_riscv64.deb" "$EXTRACTED_PKG"
grep -q '\$ATTUNE_EXTRA_ARGS' "$EXTRACTED_PKG/lib/systemd/system/attune-server.service"
if grep -q '\${ATTUNE_EXTRA_ARGS}' "$EXTRACTED_PKG/lib/systemd/system/attune-server.service"; then
  echo "systemd unit must not pass an empty ATTUNE_EXTRA_ARGS argument" >&2
  exit 1
fi
test -f "$EXTRACTED_PKG/usr/share/attune/capability-packs/oss-rag-default/plugin.yaml"
test -f "$EXTRACTED_PKG/usr/share/attune/capability-packs/oss-rag-default/prompt.md"
grep -q "rag_profiles" "$EXTRACTED_PKG/usr/share/attune/capability-packs/oss-rag-default/plugin.yaml"
grep -q "30B" "$EXTRACTED_PKG/usr/share/attune/capability-packs/oss-rag-default/plugin.yaml"

bash "$ROOT/scripts/release/test-k3-nas-web-demo.sh" \
  --dry-run \
  --skip-deb-check \
  --deb "$TMP/fake.deb" \
  --base-url http://127.0.0.1:18900 \
  --reports-dir "$TMP/reports" \
  >/tmp/attune-k3-release-dry-run.log
ATTUNE_K3_EVAL_SUITE=k3_rag_release_smoke \
ATTUNE_K3_WEB_DEMO_BASE_URL=http://127.0.0.1:8890 \
ATTUNE_K3_WEB_DEMO_API_URL=http://127.0.0.1:8889 \
bash "$ROOT/scripts/release/test-k3-nas-web-demo.sh" \
  --dry-run \
  --skip-deb-check \
  --deb "$TMP/fake.deb" \
  --base-url http://127.0.0.1:18900 \
  --reports-dir "$TMP/reports"
test -f "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "K3 RVV Runtime Performance Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "worker_benchmark_gate.py" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_RVV_REQUIRE_PERF=1" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Configure Attune scheduler-native AI settings" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Require local scheduler chat metadata" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "NAS Web API Contract Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "embedding/vector queue drain" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Airplane GitHub Longtext Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Mechanical Design GitHub Longtext Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "https://github.com/shiroinekotfs/airplane-manual-collection.git" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "https://github.com/GEQfa/handbook-of-mechanical-design.git" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "edge_scheduler_comprehensive" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_LONGTEXT_CORPORA" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_LONGTEXT_REQUIRE_SCHEDULER_GENERATION=1" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Require long-text scheduler generation: 0" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Long-text PDF OCR guard" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_LONGTEXT_PDF_OCR=0" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_LONGTEXT_MANIFEST" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "Remote CI topology" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "RAG Eval Suite Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_EVAL_SUITE" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "k3_rag_release_smoke" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "scripts/eval/validate-manifests.py" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "scripts/eval/run-suite.py" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_EVAL_OUT" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "KB Web Demo Frontend Gate" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "ATTUNE_K3_WEB_DEMO_BASE_URL" "$TMP/reports/k3-nas-web-demo-dry-run.md"
grep -q "kb_web_demo_eval_frontend_e2e.py" "$TMP/reports/k3-nas-web-demo-dry-run.md"

python3 "$ROOT/scripts/release/probe-nas-web-api-contract.py" \
  --dry-run \
  --base-url http://127.0.0.1:18900 \
  --password e2e-pass-2026 \
  --bind-dir "$TMP/nas-web-bind" \
  --scheduler-url http://127.0.0.1:8090 \
  --server-scheduler-base http://127.0.0.1:8090 \
  --out "$TMP/reports/nas-web-api-contract-dry-run.json" > "$TMP/nas-web-api-contract-dry-run.txt"
test -f "$TMP/reports/nas-web-api-contract-dry-run.json"
grep -q "api_contract" "$TMP/nas-web-api-contract-dry-run.txt"
grep -q "vault" "$TMP/nas-web-api-contract-dry-run.txt"
grep -q "settings_scheduler" "$TMP/nas-web-api-contract-dry-run.txt"
grep -q "index_bind" "$TMP/nas-web-api-contract-dry-run.txt"
grep -q "vector_indexing" "$TMP/nas-web-api-contract-dry-run.txt"
grep -q "chat_scheduler" "$TMP/nas-web-api-contract-dry-run.txt"
grep -q "scheduler_observations" "$TMP/nas-web-api-contract-dry-run.txt"
grep -q "scheduler instability" "$TMP/nas-web-api-contract-dry-run.txt"

test -f "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "ATTUNE_K3_LONGTEXT_MANIFEST" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "ATTUNE_K3_LONGTEXT_E2E" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "ATTUNE_K3_LONGTEXT_REQUIRE_SCHEDULER_GENERATION" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "ATTUNE_K3_LONGTEXT_PDF_OCR" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "https://github.com/shiroinekotfs/airplane-manual-collection.git" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "https://github.com/GEQfa/handbook-of-mechanical-design.git" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "ATTUNE_K3_LONGTEXT_CORPORA" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "ATTUNE_K3_LONGTEXT_REPEAT_CHAT" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "ATTUNE_K3_SERVER_SCHEDULER_BASE" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "scheduler self-test" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "scheduler_observations" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"
grep -q "cross-host" "$ROOT/docs/testing/k3-nas-web-remote-ci.md"

test -f "$ROOT/docs/edge-scheduler-delivery.md"
grep -q "attune-edge-scheduler" "$ROOT/docs/edge-scheduler-delivery.md"
grep -q "cloud-only" "$ROOT/docs/edge-scheduler-delivery.md"
grep -q "systemd" "$ROOT/docs/edge-scheduler-delivery.md"
grep -q "Windows Service" "$ROOT/docs/edge-scheduler-delivery.md"
grep -q "30B" "$ROOT/docs/edge-scheduler-delivery.md"
grep -q "rag_profiles" "$ROOT/docs/plugin-protocol.md"
grep -q "attune-edge-scheduler" "$ROOT/docs/local-llm-setup.md"
grep -q "SchedulerBase" "$ROOT/tests/e2e/kb_longloop_windows.ps1"
grep -q "Windows Service" "$ROOT/tests/e2e/kb_longloop_windows.ps1"
grep -q "attune-edge-scheduler" "$ROOT/tests/e2e/kb_longloop_windows.ps1"

bash "$ROOT/scripts/release/test-k3-rvv-runtime-gate.sh" \
  --dry-run \
  --scheduler-url http://127.0.0.1:8090 \
  --reports-dir "$TMP/reports"
test -f "$TMP/reports/k3-rvv-runtime-gate-dry-run.md"
grep -q "K3 RVV Runtime Performance Gate" "$TMP/reports/k3-rvv-runtime-gate-dry-run.md"
grep -q "worker_benchmark_gate.py" "$TMP/reports/k3-rvv-runtime-gate-dry-run.md"
grep -q "cd .*SCHEDULER_ROOT" "$TMP/reports/k3-rvv-runtime-gate-dry-run.md"

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
