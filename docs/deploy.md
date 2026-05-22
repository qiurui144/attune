# Deployment Guide

attune 支持 3 种部署形态. 选择基于 form factor (per [ADR 0002](adr/0002-formfactor-llm-split.md)).

## 1. Laptop / 桌面 (主流)

**目标用户**: 个人独占设备, 笔电/工作站.

### Linux (deb)

```bash
# 下载最新 GA .deb
wget https://github.com/qiurui144/attune/releases/download/desktop-v0.6.3/Attune_0.6.3_amd64.deb

# 装
sudo dpkg -i Attune_0.6.3_amd64.deb
# 自动装依赖: curl / poppler-utils / libwebkit2gtk-4.1-0 / libgtk-3-0 / libayatana-appindicator3-1

# 启 (桌面菜单 "Attune" 或命令行)
attune-desktop
```

post-install 自动准备 4 底座:
- Embedding: bge-m3 via Ollama (你需手装 Ollama)
- Reranker: lazy hf_hub (首搜下载 ~120 MB)
- ASR: whisper-cli + large-v3-turbo Q5 (中文 WER 5-7%)
- OCR: PP-OCRv5 mobile 21 MB

### Windows

下载 `Attune_0.6.3_x64-setup.exe` (NSIS) 或 `Attune_0.6.3_x64_en-US.msi` (企业).
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

通用 Linux (非 Debian 系):
```bash
chmod +x Attune_0.6.3_amd64.AppImage
./Attune_0.6.3_amd64.AppImage
```

## 2. Headless Server / NAS

**目标用户**: 多客户端访问同一知识库 (家庭 NAS / 工作组 / 自建云).

### 安装

```bash
# 下载 server tarball (4 平台)
wget https://github.com/qiurui144/attune/releases/download/v0.6.3/attune-linux-x86_64.tar.gz
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

## 3. K3 一体机 (RISC-V)

**目标用户**: 出厂预装, 零配置开机即用.

K3 镜像 build pipeline 在 `rv-spine-triton` + `rv-llama-cpp` 项目, 此处仅描述
attune 端集成. FormFactor 自动检测为 `K3Appliance`, LLM 默认走本地 Ollama (60 TOPS
INT4 via SpacemiT IME).

### 系统服务

K3 镜像出厂 systemd unit `attune-k3.service` 启动, 含:
- attune-server-headless on :18900
- ollama daemon (qwen2.5:3b 预装)
- 推理服务 :8080 (SpacemiT EP, IME GPU offload)

### 网络

K3 出厂 IP DHCP, 用户:
1. 局域网扫 mDNS `_attune._tcp.local`
2. 浏览器 `attune.local:18900` 即用
3. 第一次访问 wizard (无主密码), 设密码完成

### 升级

A/B 双分区 + signed firmware, OTA 拉新版 image:
```bash
attune-cli k3 upgrade  # 从 attune.ai/firmware/k3 拉最新
```

## 4. Docker / GitHub Container Registry (ghcr.io)

**目标用户**: 服务器/NAS 容器化部署、CI/CD 集成、自定义编排。

两个镜像由 `.github/workflows/docker-publish.yml` 在每次 `v*` tag push 时自动构建发布。

### 拉取镜像

```bash
# CLI（轻量，无 UI）
docker pull ghcr.io/qiurui144/attune-cli:v1.0.0

# Headless server（含嵌入式 Web UI，端口 18900）
docker pull ghcr.io/qiurui144/attune-server:v1.0.0

# 或用 latest（跟随最新 GA）
docker pull ghcr.io/qiurui144/attune-server:latest
```

### 启动 headless server

```bash
# 最简启动（vault 数据存容器内，重建会丢失）
docker run -d -p 18900:18900 ghcr.io/qiurui144/attune-server:v1.0.0

# 推荐：挂载数据卷持久化 vault
docker run -d \
  -p 18900:18900 \
  -v $HOME/.attune:/data \
  -e ATTUNE_DATA_DIR=/data \
  ghcr.io/qiurui144/attune-server:v1.0.0

# 带 TLS（Let's Encrypt 证书）
docker run -d \
  -p 18900:18900 \
  -v /etc/letsencrypt:/certs:ro \
  -v $HOME/.attune:/data \
  ghcr.io/qiurui144/attune-server:v1.0.0 \
  --tls-cert /certs/live/attune.example.com/fullchain.pem \
  --tls-key /certs/live/attune.example.com/privkey.pem
```

### 与 install pkg（.deb / .exe）的关系

| 形态 | 用途 | UI | Ollama | 推荐场景 |
|------|------|----|----|------|
| `.deb` / `.msi` / AppImage | 桌面应用（含系统托盘） | ✅ Tauri WebView | 本机自动检测 | 笔电 / 工作站个人使用 |
| Docker `attune-server` | Headless server（无桌面） | ✅ 嵌入 Web UI（浏览器访问） | 需宿主机 Ollama 或 K3 推理服务 | NAS / VPS / 团队共享 |
| Docker `attune-cli` | 命令行工具（无 UI） | ❌ | ❌ | 脚本自动化 / CI 管道 |

> Docker 镜像不含 Ollama、whisper.cpp 和 PP-OCR 底座模型。
> 启动后在 Web UI Settings → AI 大脑 配置外部 Ollama 地址或云端 token。

### 平台支持

镜像构建矩阵：`linux/amd64` + `linux/arm64`（aarch64，支持 K3 / 树莓派 / NAS）。

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
| Plugin marketplace | hub.attune.ai (公共) 或 自部署 pluginhub URL | — |
| 会员验证 | accounts.attune.ai 或 自部署 accounts URL | — |
| LLM Gateway | gateway.attune.ai (Pro Membership) 或 BYOK | — |

自部署用户 v0.6.3 起在 Settings → 会员 → "高级 · 自部署 cloud 后端" 配 3 URL.

## 故障排查

| 现象 | 检查 |
|------|------|
| `:18900` 启动失败 | 端口占用 / SSH tunnel 残留 (本次会话踩过, ss -tlpn 看) |
| Wizard "Ollama 没装" | `curl -fsSL https://ollama.com/install.sh \| sh`, 然后 `ollama pull bge-m3` |
| Chat "no LLM configured" | Settings → AI 大脑 → 配 cloud token 或选 Ollama |
| FTS 查询不命中新文件 | 后台 indexer 还在跑, 等几秒 (大 PDF 可能 OCR 慢) |
| Plugin 装后未显示 | `POST /api/v1/plugins/reload` 或重启 daemon |
