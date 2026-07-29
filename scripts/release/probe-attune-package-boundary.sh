#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <package-staging-dir>" >&2
  exit 2
fi

STAGE="$1"
if [ ! -d "$STAGE" ]; then
  echo "package staging directory not found: $STAGE" >&2
  exit 2
fi

matches="$(
  find "$STAGE" -type f | sed "s#^$STAGE/##" | grep -Ei '(^|/)(model[^/]*|.*\.onnx|.*onnxruntime.*|.*sherpa.*|.*ort.*|.*worker.*runtime.*)$' || true
)"

if [ -n "$matches" ]; then
  echo "package staging unexpectedly contains inference runtime/model-looking files" >&2
  printf '%s\n' "$matches" >&2
  exit 1
fi

echo "attune package boundary PASS: no inference runtime/model-looking files"
