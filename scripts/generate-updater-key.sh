#!/usr/bin/env bash
# generate-updater-key.sh — 生成 / 轮换 Tauri auto-updater minisign keypair
#
# Tauri v2 用 minisign (ed25519) 签 update bundle.
# 这个脚本生成一对 keypair:
#   - 私钥 → 添加到 GitHub Actions Secret TAURI_SIGNING_PRIVATE_KEY (供 desktop-release.yml 用)
#   - 公钥 → 写入 apps/attune-desktop/tauri.conf.json plugins.updater.pubkey
#
# 用法:
#   scripts/generate-updater-key.sh [output-dir]
#     output-dir   私钥/公钥落盘目录,默认 ~/.attune-updater-keys/ (建议 chmod 700)
#
# 安全:
#   - 私钥**永远不要 commit 到 repo**
#   - 推荐流程:生成 → 复制到 GitHub Secret → 删本地私钥(secret 即唯一副本)
#   - 私钥丢失 = 老客户端无法接收新更新(签名验证失败),用户必须重装最新版

set -euo pipefail

OUT_DIR="${1:-$HOME/.attune-updater-keys}"
mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR"

PRIV_FILE="$OUT_DIR/attune-updater.key"
PUB_FILE="$OUT_DIR/attune-updater.pub"

if [ -f "$PRIV_FILE" ] || [ -f "$PUB_FILE" ]; then
  echo "错误:输出目录已存在 keypair: $OUT_DIR"
  echo "请先备份并删除现有 keypair 再轮换,或选用其他 output-dir."
  echo "  $PRIV_FILE"
  echo "  $PUB_FILE"
  exit 1
fi

# 检查 tauri CLI 可用
if ! command -v cargo >/dev/null 2>&1; then
  echo "错误:未找到 cargo.先安装 Rust toolchain."
  exit 2
fi

# Tauri 2 自带 signer 子命令:cargo tauri signer generate
if ! command -v tauri >/dev/null 2>&1; then
  # 没有全局 tauri,临时 cargo install
  echo "检测到无全局 tauri-cli,正在 cargo install..."
  cargo install --locked tauri-cli --version "^2.0"
fi

echo "生成 ed25519 keypair → $OUT_DIR"
# tauri signer generate 输出私钥和公钥到指定路径
tauri signer generate -w "$PRIV_FILE"

# tauri signer generate 会同时写 ${PRIV_FILE}.pub
if [ -f "${PRIV_FILE}.pub" ]; then
  mv "${PRIV_FILE}.pub" "$PUB_FILE"
fi

chmod 600 "$PRIV_FILE"
chmod 644 "$PUB_FILE"

echo ""
echo "==============================================="
echo "生成完成.下一步:"
echo "==============================================="
echo ""
echo "1. 复制公钥(base64 已编码) → 更新 apps/attune-desktop/tauri.conf.json"
echo "   plugins.updater.pubkey 字段:"
echo ""
cat "$PUB_FILE"
echo ""
echo "2. 复制私钥内容到 GitHub Actions Secret:"
echo "   - Repo Settings → Secrets and variables → Actions → New secret"
echo "   - Name: TAURI_SIGNING_PRIVATE_KEY"
echo "   - Value: (整个私钥文件内容,包含 'untrusted comment' 头)"
echo ""
echo "   私钥位置: $PRIV_FILE"
echo ""
echo "3. (可选)再加一个 TAURI_SIGNING_PRIVATE_KEY_PASSWORD secret"
echo "   如果你的私钥设置了密码(交互式 generate 会问)."
echo ""
echo "4. 验证 secret 配置 — push 一个 desktop-vX.Y.Z-test tag,看 workflow 是否产 *.sig 文件."
echo ""
echo "5. 完成后**删除本地私钥**: rm $PRIV_FILE"
echo "   GitHub Secret 即唯一备份."
echo ""
echo "==============================================="
