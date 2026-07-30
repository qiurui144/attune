#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

require_file_contains() {
  local file="$1"
  local needle="$2"
  if [[ ! -f "$file" ]]; then
    echo "missing required Rust cache policy file: $file" >&2
    exit 1
  fi
  if ! grep -Fq "$needle" "$file"; then
    echo "missing Rust cache policy in $file: $needle" >&2
    exit 1
  fi
}

require_file_contains "$ROOT/.cargo/config.toml" 'target-dir = "/data/cargo-target/attune"'
require_file_contains "$ROOT/rust/.cargo/config.toml" 'target-dir = "/data/cargo-target/attune"'
require_file_contains "$ROOT/scripts/rust-cache-status.sh" '/data/cargo-target/attune'
require_file_contains "$ROOT/scripts/rust-cache-clean.sh" '/data/cargo-target/attune'
require_file_contains "$ROOT/scripts/release/build-riscv64-server-deb.sh" 'cargo metadata'
require_file_contains "$ROOT/scripts/release/build-riscv64-server-deb.sh" 'target_directory'
require_file_contains "$ROOT/CLAUDE.md" 'workspace target 分仓隔离到 `/data/cargo-target/attune`'
require_file_contains "$ROOT/CLAUDE.md" '仓库根目录 `.cargo/config.toml` 与 `rust/.cargo/config.toml` 必须同时固定'
"$ROOT/scripts/rust-cache-status.sh" >/tmp/attune-rust-cache-status-test.txt

target_dir="$(
  cd "$ROOT"
  cargo metadata --manifest-path rust/Cargo.toml --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])'
)"
if [[ "$target_dir" != "/data/cargo-target/attune" ]]; then
  echo "cargo from repo root must use /data/cargo-target/attune, got: $target_dir" >&2
  exit 1
fi

if grep -R --include='*.sh' -n 'CARGO_TARGET_DIR=.*/tmp/attune' "$ROOT/scripts" >/tmp/attune-rust-cache-policy-grep.txt; then
  echo "repo scripts must not introduce ad-hoc Attune CARGO_TARGET_DIR paths:" >&2
  cat /tmp/attune-rust-cache-policy-grep.txt >&2
  exit 1
fi

if grep -R --include='*.sh' -n 'rust/target/[$]TARGET/release/attune-server-headless' "$ROOT/scripts" >/tmp/attune-rust-cache-policy-grep.txt; then
  echo "release scripts must not hardcode rust/target for cross-built binaries:" >&2
  cat /tmp/attune-rust-cache-policy-grep.txt >&2
  exit 1
fi

echo "rust-cache-policy: PASS"
