#!/usr/bin/env bash
# attune 本地一键安装脚本 — 桌面 / 个人服务器形态.
#
# 服务器端云端部署: bash /data/company/cloud/cloud.sh en (已就位, 不在此脚本范围)
#
# 此脚本流程:
# 1. 检查环境 (Rust toolchain / Ollama / poppler-utils / 网络)
# 2. cargo build attune-server-headless + attune-cli (release)
# 3. 装 systemd 用户服务 (可选: --systemd)
# 4. 引导 attune setup (vault 初始化)
# 5. 可选: --cloud-login <email> 自动登录 + sync pro 插件
# 6. 可选: --link-folder <path> 关联本地知识库目录
# 7. 启动 attune-server (后台或 systemd)

set -euo pipefail

CLOUD_URL="${ATTUNE_CLOUD_URL:-https://accounts.engi-stack.com}"
INSTALL_PREFIX="${ATTUNE_INSTALL_PREFIX:-$HOME/.local}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ATTUNE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# 默认行为 flags
USE_SYSTEMD=false
CLOUD_LOGIN_EMAIL=""
LINK_FOLDER=""
START_SERVER=true
SKIP_BUILD=false

usage() {
  cat <<EOF
Usage: $0 [OPTIONS]

OPTIONS:
  --systemd                       安装 systemd 用户服务 (开机自启)
  --cloud-login <email>           安装后登录云端 + 自动 sync pro 插件
  --link-folder <path>            关联本地目录到默认知识库
  --no-start                      不启动 attune-server (仅安装)
  --skip-build                    跳过 cargo build (使用已编译产物)
  -h, --help                      本帮助

ENV:
  ATTUNE_CLOUD_URL                云端 accounts URL (默认 https://accounts.engi-stack.com)
  ATTUNE_INSTALL_PREFIX           安装前缀 (默认 ~/.local)

EXAMPLES:
  $0 --systemd --cloud-login alice@example.com --link-folder ~/Documents/cases
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --systemd) USE_SYSTEMD=true; shift ;;
    --cloud-login) CLOUD_LOGIN_EMAIL="$2"; shift 2 ;;
    --link-folder) LINK_FOLDER="$2"; shift 2 ;;
    --no-start) START_SERVER=false; shift ;;
    --skip-build) SKIP_BUILD=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1"; usage; exit 1 ;;
  esac
done

step()  { echo -e "\n\033[36m━━━ $* ━━━\033[0m"; }
info()  { echo -e "\033[32m[INFO]\033[0m $*"; }
warn()  { echo -e "\033[33m[WARN]\033[0m $*"; }
error() { echo -e "\033[31m[ERR ]\033[0m $*"; exit 1; }

step "环境检查"
command -v cargo >/dev/null || error "Rust toolchain 缺失. 装: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
command -v pdftoppm >/dev/null || warn "poppler-utils 缺失 (OCR 不可用). Ubuntu: sudo apt install poppler-utils"
command -v ollama >/dev/null || warn "ollama 缺失 (本地 LLM 不可用). 装: curl -fsSL https://ollama.com/install.sh | sh"
info "Rust: $(rustc --version)"

step "构建二进制"
mkdir -p "$INSTALL_PREFIX/bin"
if [ "$SKIP_BUILD" = false ]; then
  cd "$ATTUNE_ROOT/rust"
  cargo build --release -p attune-cli -p attune-server
  info "✓ build complete"
fi
cp "$ATTUNE_ROOT/rust/target/release/attune" "$INSTALL_PREFIX/bin/attune"
cp "$ATTUNE_ROOT/rust/target/release/attune-server-headless" "$INSTALL_PREFIX/bin/attune-server-headless"
info "✓ binaries installed to $INSTALL_PREFIX/bin/"
info "  注意: 把 $INSTALL_PREFIX/bin 加入 PATH 如果没加"

step "Vault 初始化"
if "$INSTALL_PREFIX/bin/attune" status 2>&1 | grep -q '"state":\s*"Sealed"' \
   || ! "$INSTALL_PREFIX/bin/attune" status 2>&1 | grep -q '"state"'; then
  info "Vault 未 setup, 进入交互式 setup"
  "$INSTALL_PREFIX/bin/attune" setup || warn "setup 失败 (vault 可能已存在)"
else
  info "✓ vault 已初始化"
fi

if [ -n "$CLOUD_LOGIN_EMAIL" ]; then
  step "云端登录 + 自动同步 pro 插件"
  "$INSTALL_PREFIX/bin/attune" login "$CLOUD_LOGIN_EMAIL" --cloud-url "$CLOUD_URL"
  info "尝试 sync-plugins..."
  "$INSTALL_PREFIX/bin/attune" sync-plugins --cloud-url "$CLOUD_URL" || warn "sync 失败 (可能未付费或 plugin 未发布)"
fi

if [ -n "$LINK_FOLDER" ]; then
  step "关联本地知识库目录"
  "$INSTALL_PREFIX/bin/attune" link-folder "$LINK_FOLDER" --project default
fi

if [ "$USE_SYSTEMD" = true ]; then
  step "安装 systemd 用户服务"
  SYSTEMD_DIR="$HOME/.config/systemd/user"
  mkdir -p "$SYSTEMD_DIR"
  cat > "$SYSTEMD_DIR/attune-server.service" <<EOF
[Unit]
Description=Attune Server (private AI knowledge companion)
After=network-online.target

[Service]
ExecStart=$INSTALL_PREFIX/bin/attune-server-headless --host 127.0.0.1 --port 18900
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
EOF
  systemctl --user daemon-reload
  systemctl --user enable attune-server.service
  info "✓ systemd 服务已安装"
  if [ "$START_SERVER" = true ]; then
    systemctl --user start attune-server.service
    info "✓ attune-server 已启动 (systemd)"
    info "  状态: systemctl --user status attune-server"
    info "  日志: journalctl --user -u attune-server -f"
  fi
elif [ "$START_SERVER" = true ]; then
  step "启动 attune-server (后台)"
  if pgrep -f attune-server-headless >/dev/null; then
    warn "已有 attune-server 在跑, 跳过"
  else
    nohup "$INSTALL_PREFIX/bin/attune-server-headless" \
      --host 127.0.0.1 --port 18900 \
      > /tmp/attune-server.log 2>&1 &
    sleep 1.5
    info "✓ attune-server PID=$(pgrep -f attune-server-headless | head -1)"
    info "  日志: tail -f /tmp/attune-server.log"
    info "  停止: pkill -f attune-server-headless"
  fi
fi

step "完成"
info "Web UI:    http://127.0.0.1:18900/"
info "API 健康:  curl http://127.0.0.1:18900/health"
info "CLI 帮助:  $INSTALL_PREFIX/bin/attune --help"
echo
echo "📦 已装命令:"
echo "  attune login <email>            登录云端"
echo "  attune sync-plugins             拉云端 entitled pro 插件"
echo "  attune link-folder <path>       关联本地知识库目录"
echo "  attune plugin-list              看已装 plugin"
echo "  attune status                   vault 状态"
