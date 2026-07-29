#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEFAULT_TOOLCHAIN="/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2"

VERSION=""
TOOLCHAIN="${ATTUNE_RVA23_TOOLCHAIN:-$DEFAULT_TOOLCHAIN}"
OUT_DIR="$ROOT/dist/release/riscv64-server-deb"
REPORTS_DIR="$ROOT/reports/release"
TARGET="${ATTUNE_RISCV64_TARGET:-riscv64gc-unknown-linux-gnu}"
FEATURES="scheduler-runtime,artifact-export-rich,wasm-runtime"
SKIP_FRONTEND=0
SKIP_BUILD=0
SKIP_RVV_AUDIT=0
DRY_RUN=0

usage() {
  sed -n '2,90p' "$0" | sed 's/^# \{0,1\}//'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --toolchain)
      TOOLCHAIN="${2:-}"
      shift 2
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
      shift 2
      ;;
    --reports-dir)
      REPORTS_DIR="${2:-}"
      shift 2
      ;;
    --skip-frontend)
      SKIP_FRONTEND=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --skip-rvv-audit)
      SKIP_RVV_AUDIT=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      cat <<'HELP'
Build a riscv64 Attune headless server Debian package for K3/NAS Web delivery.

Usage:
  bash scripts/release/build-riscv64-server-deb.sh [options]

Options:
  --version <value>     Override package version. Defaults to attune-server Cargo version.
  --toolchain <path>    SpacemiT RVA23 toolchain root.
  --out-dir <path>      Output directory for .deb, sha256, package metadata.
  --reports-dir <path>  Output directory for Markdown release report.
  --skip-frontend       Do not rebuild rust/crates/attune-server/ui/dist.
  --skip-build          Reuse existing cross-built binary.
  --skip-rvv-audit      Do not run scripts/audit-rvv-vectorization.sh.
  --dry-run             Write the planned report without building or packaging.
HELP
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

detect_version() {
  python3 - "$ROOT/rust/crates/attune-server/Cargo.toml" <<'PY'
import sys
try:
    import tomllib
except ModuleNotFoundError:
    tomllib = None

path = sys.argv[1]
if tomllib is not None:
    with open(path, "rb") as fh:
        data = tomllib.load(fh)
    print(data["package"]["version"])
    raise SystemExit(0)

in_package = False
with open(path, "r", encoding="utf-8") as fh:
    for line in fh:
        stripped = line.strip()
        if stripped == "[package]":
            in_package = True
            continue
        if stripped.startswith("[") and in_package:
            break
        if in_package and stripped.startswith("version"):
            print(stripped.split("=", 1)[1].strip().strip('"'))
            raise SystemExit(0)
raise SystemExit("could not detect attune-server version")
PY
}

if [ -z "$VERSION" ]; then
  VERSION="$(detect_version)"
fi

mkdir -p "$OUT_DIR" "$REPORTS_DIR"
TS="$(date +%Y%m%d_%H%M%S)"
if [ "$DRY_RUN" = "1" ]; then
  REPORT="$REPORTS_DIR/build-riscv64-server-deb-dry-run.md"
else
  REPORT="$REPORTS_DIR/build-riscv64-server-deb-$TS.md"
fi

BIN="$ROOT/rust/target/$TARGET/release/attune-server-headless"
OSS_RAG_PACK_SRC="$ROOT/rust/crates/attune-core/assets/plugins/oss_rag_default"
STAGE="$OUT_DIR/pkgroot-attune-server-$VERSION-riscv64"
DEB="$OUT_DIR/attune-server_${VERSION}_riscv64.deb"
SHA="$DEB.sha256"
INFO="$OUT_DIR/attune-server_${VERSION}_riscv64.deb.info.txt"
CONTENTS="$OUT_DIR/attune-server_${VERSION}_riscv64.deb.contents.txt"
RVV_AUDIT="$REPORTS_DIR/rvv-audit-riscv64-server-$TS.txt"

log() {
  printf '[riscv64-deb] %s\n' "$*"
}

run() {
  log "+ $*"
  if [ "$DRY_RUN" != "1" ]; then
    "$@"
  fi
}

run_to_file() {
  local output="$1"
  shift
  log "+ $* > $output"
  if [ "$DRY_RUN" != "1" ]; then
    "$@" > "$output"
  fi
}

write_report_header() {
  {
    echo "# riscv64 Attune Server Deb Build Report"
    echo
    echo "- Timestamp: $(date -Iseconds)"
    echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "- Version: $VERSION"
    echo "- Target: $TARGET"
    echo "- Toolchain: $TOOLCHAIN"
    echo "- Features: $FEATURES"
    echo "- No default features: true"
    echo "- Output dir: $OUT_DIR"
    echo "- Reports dir: $REPORTS_DIR"
    echo "- Dry run: $DRY_RUN"
    echo
    echo "## Package Boundary"
    echo
    echo "Attune package owns NAS Web/API/control-plane delivery. Scheduler packages own ORT, Sherpa, model weights, worker runtimes, hardware acceleration, and model lifecycle."
    echo
  } > "$REPORT"
}

append_report() {
  {
    echo
    echo "$@"
  } >> "$REPORT"
}

write_report_header

if [ "$DRY_RUN" = "1" ]; then
  append_report "## Planned Commands"
  append_report '- frontend build unless `--skip-frontend` is set'
  append_report '- cross-build `attune-server-headless` with scheduler-owned inference feature set'
  append_report '- stage Debian package metadata, systemd unit, default environment, docs, and binary'
  append_report '- run `dpkg-deb --build --root-owner-group`'
  append_report '- write SHA256, package info, contents, and RVV audit output'
  log "dry-run report: $REPORT"
  exit 0
fi

if [ "$SKIP_FRONTEND" != "1" ]; then
  append_report "## Frontend Build"
  run bash -lc "cd '$ROOT/rust/crates/attune-server/ui' && npm ci && npm run build"
else
  append_report "## Frontend Build"
  append_report "Skipped by --skip-frontend."
fi

if [ "$SKIP_BUILD" != "1" ]; then
  append_report "## Rust Cross Build"
  run env ATTUNE_RVA23_TOOLCHAIN="$TOOLCHAIN" \
    bash "$ROOT/scripts/build-optimized.sh" \
      --profile rva23 \
      --package attune-server \
      --features "$FEATURES" \
      -- --no-default-features --bin attune-server-headless
else
  append_report "## Rust Cross Build"
  append_report "Skipped by --skip-build."
fi

if [ ! -x "$BIN" ]; then
  echo "cross-built binary not found or not executable: $BIN" >&2
  exit 1
fi

append_report "## Package Staging"
rm -rf "$STAGE"
mkdir -p \
  "$STAGE/DEBIAN" \
  "$STAGE/usr/bin" \
  "$STAGE/lib/systemd/system" \
  "$STAGE/etc/default" \
  "$STAGE/usr/share/attune/capability-packs/oss-rag-default" \
  "$STAGE/usr/share/doc/attune-server"

install -m 0755 "$BIN" "$STAGE/usr/bin/attune-server-headless"
install -m 0644 "$OSS_RAG_PACK_SRC/plugin.yaml" "$STAGE/usr/share/attune/capability-packs/oss-rag-default/plugin.yaml"
install -m 0644 "$OSS_RAG_PACK_SRC/prompt.md" "$STAGE/usr/share/attune/capability-packs/oss-rag-default/prompt.md"
install -m 0644 "$ROOT/LICENSE" "$STAGE/usr/share/doc/attune-server/LICENSE"
install -m 0644 "$ROOT/NOTICE" "$STAGE/usr/share/doc/attune-server/NOTICE"
install -m 0644 "$ROOT/README.md" "$STAGE/usr/share/doc/attune-server/README.md"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: attune-server
Version: $VERSION
Section: utils
Priority: optional
Architecture: riscv64
Maintainer: Attune <support@attune.ai>
Depends: libc6, libgcc-s1, libstdc++6, curl, python3, poppler-utils, ca-certificates
Description: Attune headless NAS Web server
 Attune headless server for K3/NAS Web delivery. This package ships the Attune
 Web/API control plane only. ORT, Sherpa, model weights, and inference worker
 runtimes are provided by scheduler packages.
EOF

cat > "$STAGE/DEBIAN/conffiles" <<'EOF'
/etc/default/attune-server
EOF

cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/usr/bin/env bash
set -e
install -d -m 0755 /var/lib/attune
install -d -m 0755 /var/lib/attune/data
install -d -m 0755 /var/lib/attune/config
install -d -m 0755 /var/lib/attune/data/attune/plugins/oss-rag-default
if [ -d /usr/share/attune/capability-packs/oss-rag-default ]; then
  cp -a /usr/share/attune/capability-packs/oss-rag-default/. /var/lib/attune/data/attune/plugins/oss-rag-default/
fi
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
  systemctl enable attune-server.service || true
  systemctl restart attune-server.service || true
fi
EOF
chmod 0755 "$STAGE/DEBIAN/postinst"

cat > "$STAGE/DEBIAN/prerm" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl stop attune-server.service || true
fi
EOF
chmod 0755 "$STAGE/DEBIAN/prerm"

cat > "$STAGE/DEBIAN/postrm" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
fi
EOF
chmod 0755 "$STAGE/DEBIAN/postrm"

cat > "$STAGE/etc/default/attune-server" <<'EOF'
# Attune NAS Web service defaults.
# Inference runtimes and model lifecycle are scheduler-owned.
ATTUNE_HOST=0.0.0.0
ATTUNE_PORT=18900
ATTUNE_EXTRA_ARGS=
ATTUNE_FORM_FACTOR=local_scheduler
ATTUNE_CHAT_SCHEDULER_JOB_POLL_TIMEOUT_MS=180000
XDG_DATA_HOME=/var/lib/attune/data
XDG_CONFIG_HOME=/var/lib/attune/config
EOF

cat > "$STAGE/lib/systemd/system/attune-server.service" <<'EOF'
[Unit]
Description=Attune headless NAS Web server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=-/etc/default/attune-server
WorkingDirectory=/var/lib/attune
ExecStart=/usr/bin/attune-server-headless --host ${ATTUNE_HOST} --port ${ATTUNE_PORT} $ATTUNE_EXTRA_ARGS
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

bash "$ROOT/scripts/release/probe-attune-package-boundary.sh" "$STAGE"

append_report '```text'
find "$STAGE" -maxdepth 5 -type f | sed "s#^$STAGE/##" | sort >> "$REPORT"
append_report '```'

append_report "## Debian Build"
run dpkg-deb --build --root-owner-group "$STAGE" "$DEB"
run_to_file "$SHA" sha256sum "$DEB"
run_to_file "$INFO" dpkg-deb --info "$DEB"
run_to_file "$CONTENTS" dpkg-deb --contents "$DEB"

append_report "Package: $DEB"
append_report "SHA256: $SHA"
append_report "Info: $INFO"
append_report "Contents: $CONTENTS"

if [ "$SKIP_RVV_AUDIT" != "1" ]; then
  append_report "## RVV Audit"
  if ATTUNE_RVV_AUDIT_STRICT="${ATTUNE_RVV_AUDIT_STRICT:-1}" \
      ATTUNE_RVV_AUDIT_MIN_MAIN_LINES="${ATTUNE_RVV_AUDIT_MIN_MAIN_LINES:-1}" \
      ATTUNE_RVV_AUDIT_MIN_CORE_LINES="${ATTUNE_RVV_AUDIT_MIN_CORE_LINES:-1}" \
        bash "$ROOT/scripts/audit-rvv-vectorization.sh" "$BIN" > "$RVV_AUDIT" 2>&1; then
    append_report "RVV audit: $RVV_AUDIT"
  else
    append_report "RVV audit failed: $RVV_AUDIT"
    cat "$RVV_AUDIT" >&2 || true
    exit 1
  fi
else
  append_report "## RVV Audit"
  append_report "Skipped by --skip-rvv-audit."
fi

append_report "## Result"
append_report "Build complete."
log "deb: $DEB"
log "sha256: $SHA"
log "report: $REPORT"
