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
Build the Attune OSS companion Debian package.

Usage:
  bash scripts/release/build-riscv64-oss-companion-deb.sh [options]

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
  REPORT="$REPORTS_DIR/build-riscv64-oss-companion-deb-dry-run.md"
else
  REPORT="$REPORTS_DIR/build-riscv64-oss-companion-deb-$TS.md"
fi

STAGE="$OUT_DIR/pkgroot-attune-oss-companion-$VERSION-all"
DEB="$OUT_DIR/attune-oss-companion_${VERSION}_all.deb"
SHA="$DEB.sha256"
INFO="$OUT_DIR/attune-oss-companion_${VERSION}_all.deb.info.txt"
CONTENTS="$OUT_DIR/attune-oss-companion_${VERSION}_all.deb.contents.txt"

log() {
  printf '[oss-companion-deb] %s\n' "$*"
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
  echo "# riscv64 Attune OSS Companion Deb Build Report"
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
  echo "This companion package ships OSS prompt/profile assets, generic skill YAML, and documentation. It does not ship scheduler clients, inference runtimes, model weights, ORT, Sherpa assets, customer corpora, or manual-specific answers."
  echo
  echo "## Runtime Materialization"
  echo
  echo "The package installs auditable assets under /usr/share/attune/oss-companion and materializes the OSS RAG profile plugin into ATTUNE_OSS_COMPANION_PLUGIN_DIR during postinst. The default target matches the packaged attune-server XDG data directory: /var/lib/attune/data/attune/plugins. It can be changed in /etc/default/attune-oss-companion."
} > "$REPORT"

if [ "$DRY_RUN" = "1" ]; then
  {
    echo
    echo "## Planned Commands"
    echo
    echo "- stage oss_rag_default plugin profile under /usr/share/attune/oss-companion/plugins"
    echo "- stage OSS skill-runtime YAML under /usr/share/attune/oss-companion/skills"
    echo "- stage /etc/default/attune-oss-companion"
    echo "- materialize plugin assets to ATTUNE_OSS_COMPANION_PLUGIN_DIR in postinst"
    echo "- run dpkg-deb --build --root-owner-group"
  } >> "$REPORT"
  log "dry-run report: $REPORT"
  exit 0
fi

rm -rf "$STAGE"
mkdir -p \
  "$STAGE/DEBIAN" \
  "$STAGE/etc/default" \
  "$STAGE/usr/share/attune/oss-companion/plugins/oss-rag-default" \
  "$STAGE/usr/share/attune/oss-companion/skills" \
  "$STAGE/usr/share/doc/attune-oss-companion"

install -m 0644 "$ROOT/rust/crates/attune-core/assets/plugins/oss_rag_default/plugin.yaml" \
  "$STAGE/usr/share/attune/oss-companion/plugins/oss-rag-default/plugin.yaml"
install -m 0644 "$ROOT/rust/crates/attune-core/assets/plugins/oss_rag_default/prompt.md" \
  "$STAGE/usr/share/attune/oss-companion/plugins/oss-rag-default/prompt.md"
install -m 0644 "$ROOT/rust/crates/attune-core/assets/skills/compare-to-table.yaml" \
  "$STAGE/usr/share/attune/oss-companion/skills/compare-to-table.yaml"
install -m 0644 "$ROOT/rust/crates/attune-core/assets/skills/reference-generate.yaml" \
  "$STAGE/usr/share/attune/oss-companion/skills/reference-generate.yaml"
install -m 0644 "$ROOT/rust/crates/attune-core/assets/skills/research-synthesis.yaml" \
  "$STAGE/usr/share/attune/oss-companion/skills/research-synthesis.yaml"
install -m 0644 "$ROOT/LICENSE" "$STAGE/usr/share/doc/attune-oss-companion/LICENSE"
install -m 0644 "$ROOT/NOTICE" "$STAGE/usr/share/doc/attune-oss-companion/NOTICE"

cat > "$STAGE/usr/share/doc/attune-oss-companion/README.md" <<'EOF'
# Attune OSS Companion

This package contains generic OSS companion assets for Attune deployments:

- `plugins/oss-rag-default/plugin.yaml`: declarative RAG profiles for chat,
  diagnostic/procedure questions, and summary workflows.
- `plugins/oss-rag-default/prompt.md`: generic grounding and answer contract.
- `skills/*.yaml`: generic OSS skill-runtime workflows for research synthesis,
  reference-based document generation, and comparison tables.

The assets are domain-neutral. They must not contain customer corpus paths,
manual-specific answers, model weights, scheduler clients, inference runtimes,
or private deployment secrets.

On Debian installation, the `oss-rag-default` plugin is materialized into the
configured Attune plugin directory so the server can scan it on restart. Override
`ATTUNE_OSS_COMPANION_PLUGIN_DIR` in `/etc/default/attune-oss-companion` when
Attune runs as a non-root service user.
EOF

cat > "$STAGE/DEBIAN/control" <<EOF
Package: attune-oss-companion
Version: $VERSION
Section: misc
Priority: optional
Architecture: all
Maintainer: Attune <support@attune.ai>
Recommends: attune-server
Description: Attune OSS companion profile and prompt package
 Generic OSS RAG profile, prompt, and skill workflow assets for Attune K3/NAS
 delivery. This package keeps prompt engineering and companion workflow assets
 auditable and separately installable from the server binary.
EOF

cat > "$STAGE/DEBIAN/conffiles" <<'EOF'
/etc/default/attune-oss-companion
EOF

cat > "$STAGE/etc/default/attune-oss-companion" <<'EOF'
# Attune OSS companion defaults.
# Override this when attune-server uses a different XDG_DATA_HOME/service user.
ATTUNE_OSS_COMPANION_PLUGIN_DIR=/var/lib/attune/data/attune/plugins
EOF

cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/usr/bin/env bash
set -e

if [ -f /etc/default/attune-oss-companion ]; then
  # shellcheck disable=SC1091
  . /etc/default/attune-oss-companion
fi

PLUGIN_DIR="${ATTUNE_OSS_COMPANION_PLUGIN_DIR:-/var/lib/attune/data/attune/plugins}"
TARGET="$PLUGIN_DIR/oss-rag-default"
SOURCE="/usr/share/attune/oss-companion/plugins/oss-rag-default"

if [ -d "$SOURCE" ]; then
  install -d -m 0755 "$TARGET"
  install -m 0644 "$SOURCE/plugin.yaml" "$TARGET/plugin.yaml"
  install -m 0644 "$SOURCE/prompt.md" "$TARGET/prompt.md"
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl try-restart attune-server.service >/dev/null 2>&1 || true
fi
EOF
chmod 0755 "$STAGE/DEBIAN/postinst"

cat > "$STAGE/DEBIAN/postrm" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
fi
EOF
chmod 0755 "$STAGE/DEBIAN/postrm"

bash "$ROOT/scripts/release/probe-attune-package-boundary.sh" "$STAGE"

{
  echo
  echo "## Package Staging"
  echo
  echo '```text'
  find "$STAGE" -maxdepth 6 -type f | sed "s#^$STAGE/##" | sort
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
} >> "$REPORT"

log "deb: $DEB"
log "sha256: $SHA"
log "report: $REPORT"
