# riscv64 Server Deb K3 NAS Web Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and validate a riscv64 Attune headless server `.deb` for K3/NAS Web delivery while organizing release/test/maintenance scripts and generated outputs.

**Architecture:** Add release wrappers that package `attune-server-headless` with scheduler-owned inference runtime boundaries. Add K3/NAS validation wrappers that test the installed `.deb`, and maintenance scripts that inventory and clean ignored generated artifacts conservatively.

**Tech Stack:** Bash, Python 3 stdlib, `dpkg-deb`, systemd unit files, existing Rust/Cargo workspace, existing E2E Python/Playwright scripts.

## Global Constraints

- Attune `.deb` must not ship ORT, Sherpa, local model weights, or scheduler-owned inference runtimes.
- Scheduler `.deb` owns ORT, Sherpa, model weights, worker runtime, hardware acceleration, and model lifecycle.
- Default build toolchain path is `/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2`.
- Build feature set is `--no-default-features --features scheduler-runtime,artifact-export-rich,wasm-runtime`.
- Build artifacts go to `dist/release/riscv64-server-deb/`.
- Release validation reports go to `reports/release/`.
- Maintenance reports go to `reports/maintenance/`.
- Cleanup defaults to dry-run and may remove only git-ignored generated files in apply mode.
- Existing documented scripts remain callable from their current paths unless replaced by compatibility wrappers.

---

## File Structure

- Create `scripts/release/build-riscv64-server-deb.sh`: builds UI, cross-builds `attune-server-headless`, stages Debian package metadata, writes `.deb`, SHA256, and build report.
- Create `scripts/release/test-k3-nas-web-demo.sh`: validates a built `.deb` on K3/NAS over SSH and Web/API endpoints, with dry-run support.
- Create `scripts/maintenance/audit-scripts-and-outputs.sh`: inventories script locations and output roots, classifies tracked versus ignored outputs.
- Create `scripts/maintenance/clean-workspace.sh`: dry-run/apply wrapper around ignored generated output cleanup.
- Create `tests/scripts/release_scripts_test.sh`: script syntax and dry-run behavior test for the new wrappers.
- Modify `.gitignore`: explicitly keep new generated output roots ignored and document the canonical release/maintenance output roots.
- Modify `docs/DEPLOY.md`: document riscv64 headless server `.deb` for K3/NAS Web delivery and scheduler package boundary.
- Modify `docs/build-optimization.md`: document the complete scheduler-owned-inference build profile.

## Task 1: Tests for Release and Maintenance Script Contracts

**Files:**
- Create: `tests/scripts/release_scripts_test.sh`

**Interfaces:**
- Consumes: planned scripts at `scripts/release/*.sh` and `scripts/maintenance/*.sh`
- Produces: a single command `bash tests/scripts/release_scripts_test.sh` that verifies syntax and dry-run contracts

- [ ] **Step 1: Write the failing script contract test**

```bash
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

echo "release script contracts PASS"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: FAIL because the four new scripts do not exist yet.

- [ ] **Step 3: Commit test**

```bash
git add tests/scripts/release_scripts_test.sh
git commit -m "test(release): cover riscv64 delivery scripts"
```

## Task 2: riscv64 Server Deb Build Script

**Files:**
- Create: `scripts/release/build-riscv64-server-deb.sh`

**Interfaces:**
- Consumes: `scripts/build-optimized.sh`, `scripts/audit-rvv-vectorization.sh`, Rust workspace, UI package
- Produces: `attune-server_<version>_riscv64.deb`, `.sha256`, and report under configured output dirs

- [ ] **Step 1: Implement build script with dry-run first**

Script behavior:

```bash
bash scripts/release/build-riscv64-server-deb.sh --dry-run
bash scripts/release/build-riscv64-server-deb.sh
```

Required options:

```text
--version <value>
--toolchain <path>
--out-dir <path>
--reports-dir <path>
--skip-frontend
--skip-build
--skip-rvv-audit
--dry-run
```

Required package defaults:

```text
Package: attune-server
Architecture: riscv64
Depends: libc6, libgcc-s1, libstdc++6, curl, python3, poppler-utils, ca-certificates
Service: attune-server.service
Default host: 0.0.0.0
Default port: 18900
Default form factor: local_scheduler
Default data root: /var/lib/attune
```

Required cargo command:

```bash
ATTUNE_RVA23_TOOLCHAIN="$TOOLCHAIN" \
  bash scripts/build-optimized.sh --profile rva23 \
    --package attune-server \
    --features scheduler-runtime,artifact-export-rich,wasm-runtime \
    -- --no-default-features --bin attune-server-headless
```

- [ ] **Step 2: Run contract test**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: still FAIL until the remaining scripts are implemented, but the build script dry-run section passes.

- [ ] **Step 3: Run syntax check**

Run: `bash -n scripts/release/build-riscv64-server-deb.sh`

Expected: PASS.

- [ ] **Step 4: Commit build script**

```bash
git add scripts/release/build-riscv64-server-deb.sh
git commit -m "feat(release): build riscv64 server deb"
```

## Task 3: K3/NAS Web Demo Validation Script

**Files:**
- Create: `scripts/release/test-k3-nas-web-demo.sh`

**Interfaces:**
- Consumes: `.deb` from Task 2, optional SSH target, existing scheduler probe, existing E2E UI scripts
- Produces: K3/NAS validation report under `reports/release/`

- [ ] **Step 1: Implement K3/NAS validator**

Required options:

```text
--deb <path>
--host <ssh-host>
--ssh-user <user>
--base-url <url>
--scheduler-url <url>
--reports-dir <path>
--skip-deb-check
--skip-install
--skip-ui
--dry-run
```

Required environment compatibility:

```text
ATTUNE_K3_HOST
ATTUNE_K3_SSH_USER
ATTUNE_K3_BASE_URL
ATTUNE_K3_SCHEDULER_URL
ATTUNE_K3_E2E_PASSWORD
ATTUNE_K3_REMOTE_TMP
ATTUNE_K3_BACKGROUND_BIND_DIR
ATTUNE_K3_LONGTEXT_MANIFEST
```

Required checks:

```bash
dpkg-deb -f "$DEB" Architecture
sha256sum "$DEB"
ssh "$SSH_USER@$HOST" "uname -m && cat /etc/os-release | sed -n '1,6p'"
scp "$DEB" "$SSH_USER@$HOST:$REMOTE_TMP/"
ssh "$SSH_USER@$HOST" "dpkg -i '$REMOTE_TMP/$(basename "$DEB")' || apt-get -f install -y"
ssh "$SSH_USER@$HOST" "systemctl restart attune-server && systemctl is-active attune-server"
python3 scripts/probe-edge-scheduler-contract.py --base-url "$SCHEDULER_URL" --strict
curl -fsS "$BASE_URL/api/v1/status/health"
```

The script must not wipe K3 data by default.

- [ ] **Step 2: Run contract test**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: still FAIL until maintenance scripts are implemented, but K3 dry-run section passes.

- [ ] **Step 3: Commit K3 validator**

```bash
git add scripts/release/test-k3-nas-web-demo.sh
git commit -m "feat(release): validate k3 nas web package"
```

## Task 4: Maintenance Audit and Cleanup Scripts

**Files:**
- Create: `scripts/maintenance/audit-scripts-and-outputs.sh`
- Create: `scripts/maintenance/clean-workspace.sh`

**Interfaces:**
- Consumes: repository tree, `.gitignore`, git tracked/ignored state
- Produces: audit and cleanup dry-run reports under `reports/maintenance/`

- [ ] **Step 1: Implement script/output audit**

Audit must report:

```text
scripts/
tests/e2e/
.github/scripts/
apps/attune-desktop/scripts/
rust/scripts/
reports/
reports/runs/
docs/reports/
docs/benchmarks/
tests/reports/
tmp/
.playwright-mcp/
.remember/
dist/
target/bundle directories
```

- [ ] **Step 2: Implement conservative cleanup**

Cleanup defaults:

```bash
bash scripts/maintenance/clean-workspace.sh --dry-run
```

Apply mode:

```bash
bash scripts/maintenance/clean-workspace.sh --apply
```

Apply mode may only use ignored-file cleanup:

```bash
git clean -fdX -- <approved generated roots>
```

The script must not call `git reset`, `git checkout`, or remove tracked files.

- [ ] **Step 3: Run contract test**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: PASS.

- [ ] **Step 4: Commit maintenance scripts**

```bash
git add scripts/maintenance/audit-scripts-and-outputs.sh scripts/maintenance/clean-workspace.sh
git commit -m "feat(maintenance): audit and clean generated outputs"
```

## Task 5: Documentation and Ignore Rules

**Files:**
- Modify: `.gitignore`
- Modify: `docs/DEPLOY.md`
- Modify: `docs/build-optimization.md`

**Interfaces:**
- Consumes: scripts from Tasks 2-4
- Produces: documented canonical entry points and output roots

- [ ] **Step 1: Update `.gitignore` comments**

Add explicit canonical generated output roots:

```gitignore
# Canonical generated release/maintenance outputs
dist/release/
reports/release/
reports/maintenance/
```

- [ ] **Step 2: Update deployment docs**

Document:

```text
K3/NAS riscv64 headless server .deb:
- Attune package owns Web/API/control plane.
- Scheduler package owns ORT/Sherpa/models/runtimes.
- Install with dpkg.
- Open http://<nas-ip>:18900.
- Validate with scripts/release/test-k3-nas-web-demo.sh.
```

- [ ] **Step 3: Update build optimization docs**

Document the complete scheduler-owned-inference profile:

```bash
bash scripts/release/build-riscv64-server-deb.sh
```

and the underlying feature set:

```bash
--no-default-features --features scheduler-runtime,artifact-export-rich,wasm-runtime
```

- [ ] **Step 4: Run docs grep checks**

Run:

```bash
rg -n "build-riscv64-server-deb|test-k3-nas-web-demo|scheduler-owned|ORT|Sherpa" docs/DEPLOY.md docs/build-optimization.md .gitignore
```

Expected: all new entry points and package boundary text are present.

- [ ] **Step 5: Commit docs**

```bash
git add .gitignore docs/DEPLOY.md docs/build-optimization.md
git commit -m "docs(release): document riscv64 nas web delivery"
```

## Task 6: Final Verification

**Files:**
- No new files required

**Interfaces:**
- Consumes: all previous tasks
- Produces: verification summary

- [ ] **Step 1: Run script contract tests**

Run: `bash tests/scripts/release_scripts_test.sh`

Expected: PASS.

- [ ] **Step 2: Run shell syntax checks**

Run:

```bash
bash -n scripts/release/build-riscv64-server-deb.sh
bash -n scripts/release/test-k3-nas-web-demo.sh
bash -n scripts/maintenance/audit-scripts-and-outputs.sh
bash -n scripts/maintenance/clean-workspace.sh
```

Expected: PASS.

- [ ] **Step 3: Run maintenance audit**

Run:

```bash
bash scripts/maintenance/audit-scripts-and-outputs.sh
```

Expected: report written to `reports/maintenance/scripts-and-outputs-audit.md`.

- [ ] **Step 4: Run cleanup dry-run**

Run:

```bash
bash scripts/maintenance/clean-workspace.sh --dry-run
```

Expected: report written to `reports/maintenance/clean-workspace-dry-run.md`; no files deleted.

- [ ] **Step 5: Run package build dry-run**

Run:

```bash
bash scripts/release/build-riscv64-server-deb.sh --dry-run
```

Expected: report written to `reports/release/build-riscv64-server-deb-dry-run.md`.

- [ ] **Step 6: Attempt real package build when toolchain is available**

Run:

```bash
bash scripts/release/build-riscv64-server-deb.sh
```

Expected: `.deb`, SHA256, and build report under `dist/release/riscv64-server-deb/` and `reports/release/`. If the SpacemiT toolchain or cross-build dependency is unavailable, record the exact blocker.

- [ ] **Step 7: Report status**

Summarize:

```text
- scripts added
- docs updated
- test commands and results
- build artifact path or blocker
- cleanup report path
- K3 validation command to run with host-specific env
```
