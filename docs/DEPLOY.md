# Deployment Guide

attune 支持 3 种部署形态. 选择基于 form factor (per [ADR 0002](adr/0002-formfactor-llm-split.md)).

## 1. Laptop / 桌面 (主流)

**目标用户**: 个人独占设备, 笔电/工作站.

### Linux (deb)

> v1.0+ 起推荐走 APT 仓库（自动升级），见 [INSTALL.md](INSTALL.md) 与 [README](../README.md#-download) 的「系统包管理器」节。下方手动 .deb 流程仍受支持。

```bash
# 从 Releases 页取最新 desktop-vX.Y.Z 的 .deb（VERSION 替换为实际版本号，如 1.2.0）
VERSION=1.2.0
wget https://github.com/qiurui144/attune/releases/download/desktop-v${VERSION}/Attune_${VERSION}_amd64.deb

# 装
sudo dpkg -i Attune_${VERSION}_amd64.deb
# 自动装依赖: curl / poppler-utils / libwebkit2gtk-4.1-0 / libgtk-3-0 / libayatana-appindicator3-1

# 启 (桌面菜单 "Attune" 或命令行)
attune-desktop
```

post-install 只准备 Attune 本体与用户数据目录，不安装 AI worker 或模型权重。本地 embedding/rerank/OCR/ASR/LLM 统一通过 edge scheduler endpoint 接入；未配置 scheduler 时使用 cloud/BYOK 或基础全文检索降级。

### Windows

从 Releases 页取最新 `desktop-vX.Y.Z` 的 `Attune_<VERSION>_x64-setup.exe` (NSIS) 或 `Attune_<VERSION>_x64_en-US.msi` (企业); 或 `winget install qiurui144.Attune`.
双击安装, 任务栏图标启动.

### macOS

源码编译 (Apple Silicon):
```bash
git clone https://github.com/qiurui144/attune.git
cd attune/apps/attune-desktop
cargo tauri build --bundles dmg
```

(macOS .dmg 当前不在 release 矩阵, v0.7 候选).

### Linux AppImage

通用 Linux (非 Debian 系), 从 Releases 页取最新 `desktop-vX.Y.Z` 的 AppImage:
```bash
chmod +x Attune_<VERSION>_amd64.AppImage
./Attune_<VERSION>_amd64.AppImage
```

## 2. Headless Server / NAS

**目标用户**: 多客户端访问同一知识库 (家庭 NAS / 工作组 / 自建云).

### 安装

```bash
# 下载 server tarball (取最新 vX.Y.Z, VERSION 替换为实际版本号, 如 1.2.0)
VERSION=1.2.0
wget https://github.com/qiurui144/attune/releases/download/v${VERSION}/attune-linux-x86_64.tar.gz
tar xzf attune-linux-x86_64.tar.gz
sudo install -m 755 attune-server-headless /usr/local/bin/
sudo install -m 755 attune-cli /usr/local/bin/
```

### systemd

```ini
# /etc/systemd/system/attune.service
[Unit]
Description=Attune private knowledge server
After=network.target

[Service]
Type=simple
User=attune
ExecStart=/usr/local/bin/attune-server-headless
Restart=on-failure
Environment="ATTUNE_DATA_DIR=/var/lib/attune"
Environment="ATTUNE_BIND=0.0.0.0:18900"

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now attune
```

### TLS (NAS 多用户)

`attune-server-headless --tls-cert /etc/letsencrypt/live/attune.example.com/fullchain.pem \
  --tls-key /etc/letsencrypt/live/attune.example.com/privkey.pem`

或 reverse proxy 通过 caddy / nginx + Let's Encrypt.

### multi-user

attune 当前是 single-vault. NAS 多用户场景:
- 每用户独立 vault.db (用户 ID 进 path: ~/attune-{uid}/vault.db)
- 后端跑多 process port 隔离 (v0.7 候选: 单进程 多 vault 支持)

## 3. Edge Scheduler 设备 (RISC-V / x86 / Windows)

**目标用户**: 出厂预装, 零配置开机即用.

Scheduler 镜像 build pipeline 由 scheduler 项目维护, 此处仅描述 attune 端集成。FormFactor 自动检测为 `LocalSchedulerAppliance`，LLM/embedding/rerank/OCR/ASR 默认经 scheduler `:8090` 统一收口。

### K3 / NAS riscv64 headless server deb

K3/NAS Web 交付使用 headless server `.deb`，不是 Tauri 桌面包。Attune 包只交付
Web/API/control plane：vault、知识库导入、上传、搜索、chat 路由、设置、导出、
plugin/WASM runtime、隐私门禁和 systemd 服务。ORT、Sherpa、模型权重、RVV/IME
worker、其它推理 runtime 和模型生命周期全部由 scheduler `.deb` 管理。

构建 Attune 包：

```bash
bash scripts/release/build-riscv64-server-deb.sh
```

安装到 K3/NAS：

```bash
sudo dpkg -i dist/release/riscv64-server-deb/attune-server_*_riscv64.deb
sudo systemctl status attune-server --no-pager
```

默认监听 `0.0.0.0:18900`。安装后从局域网浏览器打开：

```text
http://<nas-ip>:18900
```

验收已安装包：

```bash
ATTUNE_K3_HOST=<nas-ip> \
ATTUNE_K3_BASE_URL=http://<nas-ip>:18900 \
ATTUNE_K3_SCHEDULER_URL=http://<nas-ip>:8090 \
  bash scripts/release/test-k3-nas-web-demo.sh \
    --deb dist/release/riscv64-server-deb/attune-server_*_riscv64.deb
```

K3/NAS gate 必须使用 K3/NAS 自己的文件系统路径做 `/api/v1/index/bind`，
前端主机只能作为浏览器或 Playwright driver，不能把前端主机路径传给服务端。

### 系统服务

Edge scheduler 镜像出厂 systemd unit 启动, 含:
- attune-server-headless on :18900
- scheduler service :8090 (模型生命周期、队列、硬件加速由 scheduler 管理)

### 网络

local scheduler 出厂 IP DHCP, 用户:
1. 局域网扫 mDNS `_attune._tcp.local`
2. 浏览器 `attune.local:18900` 即用
3. 第一次访问 wizard (无主密码), 设密码完成

### 升级

A/B 双分区 + signed firmware, OTA 拉新版 image:
```bash
attune-cli scheduler upgrade  # 从 engi-stack.com/firmware/edge-scheduler 拉最新（命令名以发行版为准）
```

## 4. Docker / GitHub Container Registry (ghcr.io)

**目标用户**: 服务器/NAS 容器化部署、CI/CD 集成、自定义编排。

两个镜像由 `.github/workflows/docker-publish.yml` 在每次 `v*` tag push 时自动构建发布。

### 拉取镜像

```bash
# CLI（轻量，无 UI）
docker pull ghcr.io/qiurui144/attune-cli:v1.5.0

# Headless server（含嵌入式 Web UI，端口 18900）
docker pull ghcr.io/qiurui144/attune-server:v1.5.0

# 或用 latest（跟随最新 GA）
docker pull ghcr.io/qiurui144/attune-server:latest
```

### 启动 headless server

```bash
# 最简启动（vault 数据存容器内，重建会丢失）
docker run -d -p 18900:18900 ghcr.io/qiurui144/attune-server:v1.5.0

# 推荐：挂载数据卷持久化 vault
docker run -d \
  -p 18900:18900 \
  -v $HOME/.attune:/data \
  -e ATTUNE_DATA_DIR=/data \
  ghcr.io/qiurui144/attune-server:v1.5.0

# 带 TLS（Let's Encrypt 证书）
docker run -d \
  -p 18900:18900 \
  -v /etc/letsencrypt:/certs:ro \
  -v $HOME/.attune:/data \
  ghcr.io/qiurui144/attune-server:v1.5.0 \
  --tls-cert /certs/live/attune.example.com/fullchain.pem \
  --tls-key /certs/live/attune.example.com/privkey.pem
```

### 与 install pkg（.deb / .exe）的关系

| 形态 | 用途 | UI | AI 执行路径 | 推荐场景 |
|------|------|----|------------|------|
| `.deb` / `.msi` / AppImage | 桌面应用（含系统托盘） | ✅ Tauri WebView | cloud/BYOK 或 edge scheduler | 笔电 / 工作站个人使用 |
| Docker `attune-server` | Headless server（无桌面） | ✅ 嵌入 Web UI（浏览器访问） | cloud/BYOK 或宿主/远端 scheduler | NAS / VPS / 团队共享 |
| Docker `attune-cli` | 命令行工具（无 UI） | ❌ | 按命令配置 | 脚本自动化 / CI 管道 |

> Docker 镜像不含第三方 AI runtime 或模型权重。
> 启动后在 Web UI Settings → AI 大脑配置 cloud/BYOK 或 edge scheduler 地址。

### 平台支持

镜像构建矩阵：`linux/amd64` + `linux/arm64`（aarch64，支持 local scheduler / 树莓派 / NAS）。

## 5. attune-desktop-installers（企业批量分发）

**目标用户**: 企业 IT 管理员、air-gap 环境、需要批量推送 installer 的 CI/CD 管道。

`attune-desktop-installers` OCI image 把所有平台 installer 打包进一个镜像，
通过 `.github/workflows/desktop-release.yml` 在每次 `desktop-v*` tag 时自动构建发布。
Packages tab 可以看到（`ghcr.io/qiurui144/attune-desktop-installers`）。

### 提取 installer

```bash
# 拉取指定版本
docker pull ghcr.io/qiurui144/attune-desktop-installers:1.5.0

# 查看镜像内所有 installer 文件
docker run --rm ghcr.io/qiurui144/attune-desktop-installers:1.5.0 ls /installers/

# 提取 Linux .deb 到当前目录
docker run --rm \
  -v "$PWD:/out" \
  ghcr.io/qiurui144/attune-desktop-installers:1.5.0 \
  cp /installers/Attune_1.5.0_amd64.deb /out/

# 提取 Windows NSIS installer
docker run --rm \
  -v "$PWD:/out" \
  ghcr.io/qiurui144/attune-desktop-installers:1.5.0 \
  cp /installers/Attune_1.5.0_x64-setup.exe /out/

# 提取全部 installer（bash glob 写法）
docker run --rm \
  -v "$PWD:/out" \
  --entrypoint sh \
  ghcr.io/qiurui144/attune-desktop-installers:1.5.0 \
  -c "cp /installers/* /out/"
```

### 镜像内容

| 文件名示例 | 平台 | 用途 |
|-----------|------|------|
| `Attune_X.Y.Z_amd64.deb` | Linux (Debian/Ubuntu) | dpkg 安装 |
| `Attune_X.Y.Z_x86_64.rpm` | Linux (Fedora/RHEL) | rpm 安装 |
| `Attune_X.Y.Z_amd64.AppImage` | Linux 通用 | 免安装直接运行 |
| `Attune_X.Y.Z_x64-setup.exe` | Windows (NSIS) | 双击安装，含 auto-updater |
| `Attune_X.Y.Z_x64_en-US.msi` | Windows (MSI) | 企业 GPO/SCCM 推送 |

> 注：installer image 基于 `scratch`（零系统层），仅含 `/installers/` 目录。
> 平台 `linux/amd64`，不需 QEMU——内容是文件，非可执行二进制。

## 切换 / 迁移

老设备 export vault profile, 新设备 wizard import:

```bash
# 老设备
attune-cli export --output my-vault-2026-05.profile
scp my-vault-2026-05.profile new-laptop:

# 新设备 wizard Step 5 选 "导入 .vault-profile"
```

`.vault-profile` 含 (per Phase A.5):
- encrypted item content (DEK 内部, 跨设备解需主密码或 device_secret)
- annotations / projects / chat history
- 不含: ML 模型 (重下) / temporary chunks

## 网络要求

| 场景 | 必需 | 可选 |
|------|------|------|
| 基础 chat (cloud LLM) | OpenAI / Anthropic / Gemini API 端 | — |
| 网络搜索 | 系统已装 Chrome (chromiumoxide CDP) | v0.7 fallback 自动下载 |
| Plugin marketplace | hub.engi-stack.com (公共) 或 自部署 pluginhub URL | — |
| 会员验证 | accounts.engi-stack.com 或 自部署 accounts URL | — |
| LLM Gateway | gateway.engi-stack.com (Pro Membership) 或 BYOK | — |

自部署用户在 Settings → 会员 → "高级 · 自部署 cloud 后端" 配 3 URL.

## 故障排查

| 现象 | 检查 |
|------|------|
| `:18900` 启动失败 | 端口占用 / SSH tunnel 残留 (本次会话踩过, ss -tlpn 看) |
| Edge scheduler 探测失败 | `python3 scripts/probe-edge-scheduler-contract.py --base-url http://127.0.0.1:8090 --strict` |
| Chat "no LLM configured" | Settings → AI 大脑 → 配 cloud/BYOK token 或 edge scheduler |
| FTS 查询不命中新文件 | 后台 indexer 还在跑, 等几秒 (大 PDF 可能 OCR 慢) |
| Plugin 装后未显示 | `POST /api/v1/plugins/reload` 或重启 daemon |
