#!/usr/bin/env bash
#
# Build whisper-cli binary for inclusion in attune-desktop .deb / NSIS bundle.
#
# Usage: bash apps/attune-desktop/resources/bin/build-whisper-cli.sh [tag]
#   tag — optional whisper.cpp git tag/commit (default: master)
#
# Output: apps/attune-desktop/resources/bin/whisper-cli (replaces existing)
#
# Build flags chosen for broad CPU compat:
#   BUILD_SHARED_LIBS=OFF — static link libwhisper + libggml into binary
#   GGML_NATIVE=OFF       — no -march=native (else binary requires our build CPU's ISA)
#   GGML_AVX2=OFF         — no AVX2 (older AMD/Intel ICL- still work)

set -euo pipefail

TAG="${1:-master}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR=$(mktemp -d -t whisper-build-XXXX)
trap "rm -rf $BUILD_DIR" EXIT

echo "[build-whisper-cli] cloning whisper.cpp@$TAG into $BUILD_DIR"
git clone --depth 1 --branch "$TAG" https://github.com/ggml-org/whisper.cpp "$BUILD_DIR/whisper.cpp" 2>&1 | tail -3

cd "$BUILD_DIR/whisper.cpp"
echo "[build-whisper-cli] cmake configure"
cmake -B build \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_NATIVE=OFF \
  -DGGML_AVX2=OFF 2>&1 | tail -3

echo "[build-whisper-cli] cmake build (-j$(nproc))"
cmake --build build --config Release -j"$(nproc)" 2>&1 | tail -3

BIN="build/bin/whisper-cli"
[ -x "$BIN" ] || { echo "build failed: $BIN missing" >&2; exit 1; }

cp "$BIN" "$SCRIPT_DIR/whisper-cli"
chmod +x "$SCRIPT_DIR/whisper-cli"
echo "[build-whisper-cli] copied to $SCRIPT_DIR/whisper-cli ($(du -h "$SCRIPT_DIR/whisper-cli" | cut -f1))"
echo "[build-whisper-cli] dynamic deps:"
ldd "$SCRIPT_DIR/whisper-cli" | sed 's/^/  /'
