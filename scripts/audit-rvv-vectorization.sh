#!/usr/bin/env bash
#
# Audit whether a RISC-V Attune artifact contains RVV-capable attributes and
# vector instruction evidence. This is intentionally a post-build check: build
# flags alone do not prove the shipped binary kept vectorized kernels.

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_TOOLCHAIN="/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2"
ARTIFACT="${1:-$PROJECT_DIR/rust/target/riscv64gc-unknown-linux-gnu/release/attune-server-headless}"
TARGET_DIR="$(dirname "$ARTIFACT")"
DEPS_DIR="$TARGET_DIR/deps"
BUILD_DIR="$TARGET_DIR/build"
RVV_RE='(^|[[:space:]])(vsetvli|vsetivli|vsetvl|vle[0-9]+\.v|vse[0-9]+\.v|vl[0-9]+re[0-9]+\.v|vs[0-9]+r\.v|vfmacc|vfnmacc|vfadd|vfsub|vfmul|vfdiv|vfred|vadd\.v|vsub\.v|vmul\.v|vred|vrgather|vslide|vand\.v|vor\.v|vxor\.v)'
MIN_MAIN_LINES="${ATTUNE_RVV_AUDIT_MIN_MAIN_LINES:-0}"
MIN_CORE_LINES="${ATTUNE_RVV_AUDIT_MIN_CORE_LINES:-0}"
MIN_TOTAL_LINES="${ATTUNE_RVV_AUDIT_MIN_TOTAL_LINES:-0}"

require_nonnegative_int() {
  local name="$1"
  local value="$2"
  case "$value" in
    ''|*[!0-9]*)
      echo "$name must be a non-negative integer, got: $value" >&2
      exit 2
      ;;
  esac
}

find_prefixed_tool() {
  local suffix="$1"
  local env_name="$2"
  local env_value="${!env_name:-}"
  local candidate

  if [ -n "$env_value" ]; then
    printf '%s\n' "$env_value"
    return 0
  fi

  for candidate in \
    "$DEFAULT_TOOLCHAIN/bin/riscv64-unknown-linux-gnu-$suffix" \
    "/data/RV/rv-binutils/install-2.45/bin/riscv64-linux-gnu-$suffix" \
    "/data/RV/rv-binutils/install-2.44/bin/riscv64-linux-gnu-$suffix" \
    "$(command -v "riscv64-unknown-linux-gnu-$suffix" 2>/dev/null || true)" \
    "$(command -v "riscv64-linux-gnu-$suffix" 2>/dev/null || true)" \
    "$(command -v "$suffix" 2>/dev/null || true)"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

count_rvv_lines() {
  local path="$1"
  "$OBJDUMP" -d "$path" 2>/dev/null | grep -Eic "$RVV_RE" || true
}

print_rvv_sample() {
  local path="$1"
  "$OBJDUMP" -d "$path" 2>/dev/null | grep -Ei "$RVV_RE" | head -n "${ATTUNE_RVV_AUDIT_SAMPLE_LINES:-12}" || true
}

if [ ! -f "$ARTIFACT" ]; then
  echo "artifact not found: $ARTIFACT" >&2
  exit 2
fi

require_nonnegative_int ATTUNE_RVV_AUDIT_MIN_MAIN_LINES "$MIN_MAIN_LINES"
require_nonnegative_int ATTUNE_RVV_AUDIT_MIN_CORE_LINES "$MIN_CORE_LINES"
require_nonnegative_int ATTUNE_RVV_AUDIT_MIN_TOTAL_LINES "$MIN_TOTAL_LINES"

OBJDUMP="$(find_prefixed_tool objdump ATTUNE_RVV_OBJDUMP)"
READELF="$(find_prefixed_tool readelf ATTUNE_RVV_READELF)"

echo "== Attune RVV Vectorization Audit =="
echo "artifact: $ARTIFACT"
echo "objdump:  $OBJDUMP"
echo "readelf:  $READELF"
echo

if command -v file >/dev/null 2>&1; then
  echo "== File =="
  file "$ARTIFACT"
  echo
fi

echo "== RISC-V Attributes =="
attrs="$("$READELF" -A "$ARTIFACT" 2>/dev/null || true)"
if [ -n "$attrs" ]; then
  printf '%s\n' "$attrs" | sed -n '1,12p'
else
  echo "no attribute section reported"
fi
arch_attr="$(printf '%s\n' "$attrs" | sed -n 's/.*Tag_RISCV_arch: "\(.*\)"/\1/p' | head -n 1)"
attr_has_rvv=0
case "$arch_attr" in
  *"_v"*|*"zve"*|*"zv"*) attr_has_rvv=1 ;;
esac
echo "attribute_rvv_evidence: $attr_has_rvv"
echo

echo "== Main Binary RVV Instructions =="
main_rvv_count="$(count_rvv_lines "$ARTIFACT")"
echo "rvv_instruction_lines: $main_rvv_count"
if [ "${main_rvv_count:-0}" -gt 0 ]; then
  print_rvv_sample "$ARTIFACT"
fi
echo

echo "== Core Vector Libraries =="
lib_evidence=0
lib_seen=0
core_rvv_total=0
native_rvv_total=0
if [ "${ATTUNE_RVV_AUDIT_SCAN_NATIVE:-0}" = "1" ]; then
  find_args=(\( -name 'libnumkong*.rlib' -o -name 'libusearch*.rlib' -o -name '*.a' \))
else
  find_args=(\( -name 'libnumkong*.rlib' -o -name 'libusearch*.rlib' \))
fi
while IFS= read -r -d '' lib; do
  lib_seen=$((lib_seen + 1))
  lib_count="$(count_rvv_lines "$lib")"
  case "$(basename "$lib")" in
    libnumkong*|libusearch*) label="core" ;;
    *) label="native" ;;
  esac
  if [ "$label" = "core" ]; then
    core_rvv_total=$((core_rvv_total + lib_count))
  else
    native_rvv_total=$((native_rvv_total + lib_count))
  fi
  echo "$label $(realpath --relative-to "$PROJECT_DIR" "$lib" 2>/dev/null || printf '%s' "$lib"): rvv_instruction_lines=$lib_count"
  if [ "${lib_count:-0}" -gt 0 ]; then
    lib_evidence=1
    print_rvv_sample "$lib"
  fi
done < <(find "$DEPS_DIR" "$BUILD_DIR" -type f "${find_args[@]}" -print0 2>/dev/null | sort -z)
if [ "$lib_seen" -eq 0 ]; then
  echo "no core/native archive candidates found under $DEPS_DIR or $BUILD_DIR"
fi
if [ "${ATTUNE_RVV_AUDIT_SCAN_NATIVE:-0}" != "1" ]; then
  echo "native archive scan skipped; set ATTUNE_RVV_AUDIT_SCAN_NATIVE=1 to inspect all .a files"
fi
echo

rvv_evidence=0
if [ "$attr_has_rvv" -eq 1 ] || [ "${main_rvv_count:-0}" -gt 0 ] || [ "$lib_evidence" -eq 1 ]; then
  rvv_evidence=1
fi
total_rvv_count=$((main_rvv_count + core_rvv_total + native_rvv_total))
main_threshold_met=1
core_threshold_met=1
total_threshold_met=1
if [ "$main_rvv_count" -lt "$MIN_MAIN_LINES" ]; then
  main_threshold_met=0
fi
if [ "$core_rvv_total" -lt "$MIN_CORE_LINES" ]; then
  core_threshold_met=0
fi
if [ "$total_rvv_count" -lt "$MIN_TOTAL_LINES" ]; then
  total_threshold_met=0
fi

echo "== Summary =="
echo "rvv_evidence: $rvv_evidence"
echo "main_rvv_instruction_lines: $main_rvv_count"
echo "core_rvv_instruction_lines: $core_rvv_total"
echo "native_rvv_instruction_lines: $native_rvv_total"
echo "total_rvv_instruction_lines: $total_rvv_count"
echo "main_rvv_threshold_min: $MIN_MAIN_LINES"
echo "core_rvv_threshold_min: $MIN_CORE_LINES"
echo "total_rvv_threshold_min: $MIN_TOTAL_LINES"
echo "main_rvv_threshold_met: $main_threshold_met"
echo "core_rvv_threshold_met: $core_threshold_met"
echo "total_rvv_threshold_met: $total_threshold_met"
if [ "$rvv_evidence" -eq 0 ]; then
  echo "result: no RVV evidence found in artifact or scanned native libraries"
  if [ "${ATTUNE_RVV_AUDIT_STRICT:-0}" = "1" ]; then
    exit 3
  fi
elif [ "${ATTUNE_RVV_AUDIT_STRICT:-0}" = "1" ] \
    && { [ "$main_threshold_met" -eq 0 ] \
      || [ "$core_threshold_met" -eq 0 ] \
      || [ "$total_threshold_met" -eq 0 ]; }; then
  echo "result: RVV evidence found but strict thresholds were not met"
  exit 3
else
  echo "result: RVV evidence found"
fi
