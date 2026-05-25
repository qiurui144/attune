#!/usr/bin/env bash
# gen-latest-json.sh — 生成 Tauri v2 updater manifest (latest.json)
#
# 在 desktop-release.yml 中 build + sign 完成后调用,扫描 bundle 目录,
# 读各平台 *.sig 内容,组合成 static-format manifest 写到 stdout.
#
# 用法:
#   scripts/gen-latest-json.sh <version> <bundle-root> [release-notes-file]
#     <version>          产物版本号(不含前缀 v),如 1.0.0
#     <bundle-root>      apps/attune-desktop/target/release/bundle (含 nsis/ deb/ rpm/ appimage/)
#     [release-notes]    可选 — 释出说明文件路径,内容会嵌入 manifest
#
# 输出: stdout 即 manifest 内容.重定向 > latest.json.
#
# Tauri v2 static manifest schema (per tauri-plugin-updater 2.x):
# {
#   "version": "1.0.0",
#   "pub_date": "2026-05-22T12:34:56Z",
#   "notes": "...",
#   "platforms": {
#     "linux-x86_64": { "signature": "...", "url": "https://.../*.AppImage" },
#     "windows-x86_64": { "signature": "...", "url": "https://.../*.exe" },
#     ...
#   }
# }
#
# target 字符串映射 (per tauri_plugin_updater::target()):
#   linux-x86_64    = .AppImage (AppImage 是 Tauri Linux 自更新唯一支持的 bundle)
#   windows-x86_64  = NSIS .exe (MSI 不支持自更新)
#
# 注: macOS 在本 spec 不发版,所以不出现 darwin-x86_64 / darwin-aarch64.
#     deb / rpm 不进 manifest — 它们走 APT/RPM 仓库升级路径.

set -euo pipefail

VERSION="${1:?usage: gen-latest-json.sh <version> <bundle-root> [notes-file]}"
BUNDLE_ROOT="${2:?bundle root required}"
NOTES_FILE="${3:-}"

REPO_OWNER="${GITHUB_REPO_OWNER:-qiurui144}"
REPO_NAME="${GITHUB_REPO_NAME:-attune}"
TAG="${GITHUB_TAG:-desktop-v${VERSION}}"

base_url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/${TAG}"
pub_date="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Read notes if provided
notes=""
if [ -n "$NOTES_FILE" ] && [ -f "$NOTES_FILE" ]; then
  # strip trailing newline, escape for JSON via jq
  notes="$(cat "$NOTES_FILE")"
fi

# Locate platform artefacts + signatures
# Tauri produces *.sig sidecar next to each signed bundle.
linux_appimage=$(find "$BUNDLE_ROOT/appimage" -maxdepth 1 -name "*.AppImage" 2>/dev/null | head -1 || true)
linux_appimage_sig="${linux_appimage}.sig"

windows_nsis=$(find "$BUNDLE_ROOT/nsis" -maxdepth 1 -name "*.exe" 2>/dev/null | head -1 || true)
windows_nsis_sig="${windows_nsis}.sig"

# Helper to read a .sig file (Tauri stores it as one-line base64)
read_sig() {
  local f="$1"
  if [ -f "$f" ]; then
    tr -d '\n\r' < "$f"
  else
    echo ""
  fi
}

# Build platforms object via jq for safe JSON quoting
platforms_obj="{}"

if [ -n "$linux_appimage" ] && [ -f "$linux_appimage_sig" ]; then
  url="${base_url}/$(basename "$linux_appimage")"
  sig=$(read_sig "$linux_appimage_sig")
  platforms_obj=$(jq -n --arg url "$url" --arg sig "$sig" --argjson cur "$platforms_obj" \
    '$cur + {"linux-x86_64": {"signature": $sig, "url": $url}}')
fi

if [ -n "$windows_nsis" ] && [ -f "$windows_nsis_sig" ]; then
  url="${base_url}/$(basename "$windows_nsis")"
  sig=$(read_sig "$windows_nsis_sig")
  platforms_obj=$(jq -n --arg url "$url" --arg sig "$sig" --argjson cur "$platforms_obj" \
    '$cur + {"windows-x86_64": {"signature": $sig, "url": $url}}')
fi

# Compose final manifest
jq -n \
  --arg version "$VERSION" \
  --arg pub_date "$pub_date" \
  --arg notes "$notes" \
  --argjson platforms "$platforms_obj" \
  '{version: $version, pub_date: $pub_date, notes: $notes, platforms: $platforms}'
