#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

VERSION=""
OUT_DIR="$ROOT/dist/release/riscv64-server-deb"
REPORTS_DIR="$ROOT/reports/release"
DRY_RUN=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
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
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      cat <<'HELP'
Build the Attune KB web-demo companion Debian package.

Usage:
  bash scripts/release/build-riscv64-web-demo-deb.sh [options]

Options:
  --version <value>     Override package version. Defaults to attune-server Cargo version.
  --out-dir <path>      Output directory for .deb, sha256, package metadata.
  --reports-dir <path>  Output directory for Markdown release report.
  --dry-run             Write the planned report without packaging.
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
  REPORT="$REPORTS_DIR/build-riscv64-web-demo-deb-dry-run.md"
else
  REPORT="$REPORTS_DIR/build-riscv64-web-demo-deb-$TS.md"
fi

STAGE="$OUT_DIR/pkgroot-attune-web-demo-$VERSION-all"
DEB="$OUT_DIR/attune-web-demo_${VERSION}_all.deb"
SHA="$DEB.sha256"
INFO="$OUT_DIR/attune-web-demo_${VERSION}_all.deb.info.txt"
CONTENTS="$OUT_DIR/attune-web-demo_${VERSION}_all.deb.contents.txt"

log() {
  printf '[web-demo-deb] %s\n' "$*"
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

{
  echo "# riscv64 Attune Web Demo Deb Build Report"
  echo
  echo "- Timestamp: $(date -Iseconds)"
  echo "- Commit: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "- Version: $VERSION"
  echo "- Architecture: all"
  echo "- Output dir: $OUT_DIR"
  echo "- Reports dir: $REPORTS_DIR"
  echo "- Dry run: $DRY_RUN"
  echo
  echo "## Package Boundary"
  echo
  echo "This companion package ships only the KB web-demo static UI, CORS proxy, and systemd units. It does not ship scheduler clients, inference runtimes, model weights, ORT, or Sherpa assets."
} > "$REPORT"

if [ "$DRY_RUN" = "1" ]; then
  {
    echo
    echo "## Planned Commands"
    echo
    echo "- stage kb-web-demo static files under /usr/share/attune/kb-web-demo"
    echo "- stage /etc/default/attune-web-demo"
    echo "- stage attune-web-demo.service and attune-web-demo-proxy.service"
    echo "- run dpkg-deb --build --root-owner-group"
  } >> "$REPORT"
  log "dry-run report: $REPORT"
  exit 0
fi

rm -rf "$STAGE"
mkdir -p \
  "$STAGE/DEBIAN" \
  "$STAGE/etc/default" \
  "$STAGE/lib/systemd/system" \
  "$STAGE/usr/share/attune/kb-web-demo" \
  "$STAGE/usr/share/doc/attune-web-demo"

install -m 0644 "$ROOT/kb-web-demo/index.html" "$STAGE/usr/share/attune/kb-web-demo/index.html"
install -m 0755 "$ROOT/kb-web-demo/cors-proxy.py" "$STAGE/usr/share/attune/kb-web-demo/cors-proxy.py"
install -m 0755 "$ROOT/kb-web-demo/start.sh" "$STAGE/usr/share/attune/kb-web-demo/start.sh"
install -m 0755 "$ROOT/kb-web-demo/start_k3.sh" "$STAGE/usr/share/attune/kb-web-demo/start_k3.sh"
install -m 0644 "$ROOT/LICENSE" "$STAGE/usr/share/doc/attune-web-demo/LICENSE"
install -m 0644 "$ROOT/NOTICE" "$STAGE/usr/share/doc/attune-web-demo/NOTICE"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: attune-web-demo
Version: $VERSION
Section: web
Priority: optional
Architecture: all
Maintainer: Attune <support@attune.ai>
Depends: python3, ca-certificates
Recommends: attune-server
Description: Attune KB web demo companion package
 Browser-based KB demo for Attune K3/NAS delivery. This package provides the
 static web demo and a local CORS proxy that forwards browser requests to
 Attune /api/v1. Scheduler access remains server-side through Attune.
EOF

cat > "$STAGE/DEBIAN/conffiles" <<'EOF'
/etc/default/attune-web-demo
EOF

cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
  systemctl enable attune-web-demo-proxy.service || true
  systemctl enable attune-web-demo.service || true
  systemctl restart attune-web-demo-proxy.service || true
  systemctl restart attune-web-demo.service || true
fi
EOF
chmod 0755 "$STAGE/DEBIAN/postinst"

cat > "$STAGE/DEBIAN/prerm" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl stop attune-web-demo.service || true
  systemctl stop attune-web-demo-proxy.service || true
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

cat > "$STAGE/etc/default/attune-web-demo" <<'EOF'
# Attune KB web-demo defaults.
# Browser traffic goes to ATTUNE_PROXY_PORT; proxy forwards to Attune /api/v1.
ATTUNE_WEB_DEMO_HOST=0.0.0.0
ATTUNE_WEB_DEMO_PORT=8968
ATTUNE_PROXY_HOST=0.0.0.0
ATTUNE_PROXY_PORT=8969
ATTUNE_TARGET_HOST=127.0.0.1
ATTUNE_TARGET_PORT=18900
ATTUNE_PROXY_RESPONSE_IDLE_TIMEOUT_SECONDS=600
EOF

cat > "$STAGE/lib/systemd/system/attune-web-demo.service" <<'EOF'
[Unit]
Description=Attune KB web demo
After=network-online.target attune-web-demo-proxy.service
Wants=network-online.target attune-web-demo-proxy.service

[Service]
Type=simple
EnvironmentFile=-/etc/default/attune-web-demo
WorkingDirectory=/usr/share/attune/kb-web-demo
ExecStart=/usr/bin/python3 -m http.server ${ATTUNE_WEB_DEMO_PORT} --bind ${ATTUNE_WEB_DEMO_HOST} --directory /usr/share/attune/kb-web-demo
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

cat > "$STAGE/lib/systemd/system/attune-web-demo-proxy.service" <<'EOF'
[Unit]
Description=Attune KB web demo API proxy
After=network-online.target attune-server.service
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=-/etc/default/attune-web-demo
WorkingDirectory=/usr/share/attune/kb-web-demo
ExecStart=/usr/bin/python3 /usr/share/attune/kb-web-demo/cors-proxy.py
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

bash "$ROOT/scripts/release/probe-attune-package-boundary.sh" "$STAGE"

{
  echo
  echo "## Package Staging"
  echo
  echo '```text'
  find "$STAGE" -maxdepth 5 -type f | sed "s#^$STAGE/##" | sort
  echo '```'
} >> "$REPORT"

run dpkg-deb --build --root-owner-group "$STAGE" "$DEB"
run_to_file "$SHA" sha256sum "$DEB"
run_to_file "$INFO" dpkg-deb --info "$DEB"
run_to_file "$CONTENTS" dpkg-deb --contents "$DEB"

{
  echo
  echo "## Result"
  echo
  echo "- Package: $DEB"
  echo "- SHA256: $SHA"
  echo "- Info: $INFO"
  echo "- Contents: $CONTENTS"
  echo "- Demo URL: http://<host>:8968/?api=http://<host>:8969"
} >> "$REPORT"

log "deb: $DEB"
log "sha256: $SHA"
log "report: $REPORT"
