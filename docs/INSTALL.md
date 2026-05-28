# Attune 安装指南

> 跨平台安装路径速查。每条路径都包含 Ollama 自动安装 + 硬件自适应 + 4 必要底座（Embedding / Reranker / ASR / OCR）。**LLM 不在本地必装清单**——笔电默认走远端 token，K3 一体机镜像例外。

## 总览

| 平台 | 包格式 | 自动 Ollama | 自动 GPU 配置 | 4 底座自动装 |
|------|--------|------------|---------------|------------|
| **Ubuntu / Debian** | `.deb` | ✅ postinst | ✅ AMD HSA_OVERRIDE | ✅ Embedding/Reranker/ASR/OCR 全装 |
| **Fedora / RHEL** | `.rpm` | ✅ 共用 hook | ✅ 同上 | ✅ 同上 |
| **任何 Linux** | AppImage | ⚠️ wizard 引导 | ⚠️ 手动 attune deploy | ⚠️ 用户运行后端动 |
| **Windows 10/11** | NSIS `.exe` | ✅ installer.nsh | N/A（CUDA/DirectML 自动） | ✅ 同上 |
| **macOS** | — | — | — | 暂不支持 |

## 4 必要底座（CLAUDE.md "硬件感知的默认底座"）

attune 装包后立刻就绪以下底座：

| 底座 | 模型 | 体积 | 来源 |
|------|------|------|------|
| **Embedding** | bge-m3 (≥16GB) / bge-small (<16GB) | 1.2 GB / 200 MB | postinst 调 `ollama pull` |
| **Reranker** | Xenova/bge-reranker-base ONNX | ~120 MB | 首次搜索 lazy 下载（5-10s 一次性延迟） |
| **ASR** | whisper-cli + ggml-small-q8 | 2.6 MB binary + 250 MB 模型 | binary 进 .deb bundle，模型 postinst 下载 |
| **OCR** | PP-OCRv5 mobile (det+cls+rec+dict) | 4 ONNX 文件 ~21 MB | postinst 从 HF `bukuroo/PPOCRv5-ONNX` 下载 |

**LLM**（**不**在底座清单 — 云端为主，本地为辅；本地 LLM 当前研发成本高，暂时不主推）：

笔电 wizard 推荐顺序：
1. **★ Attune Pro Membership**（登录即用）— 默认推荐
   - Endpoint: `https://gateway.engi-stack.com/v1`
   - 月费会员，token 配额由 attune 计费追踪
   - Gateway 路由到 OpenAI / Anthropic / Gemini（对用户透明）
2. **BYOK：用户已有的 API key**
   - OpenAI（ChatGPT Plus/Team 用户）
   - Anthropic（Claude Pro 用户）
   - Gemini（Gemini Advanced / Google AI Studio）
   - DeepSeek / Qwen / 其他 OpenAI 兼容
3. **本地 Ollama**（advanced）— 当前不主推，研发成本高
   - K3 一体机镜像构建时 `ATTUNE_FORM_FACTOR=k3` 让 postinst 预装 qwen2.5:1.5b/3b
   - 笔电用户选 Ollama 时手动 `ollama pull qwen2.5:3b`

## Linux

### Ubuntu / Debian (.deb)

```bash
# 1. 下载 .deb（GitHub Release 或自建）
wget https://github.com/qiurui144/attune/releases/download/desktop-v0.6.0/Attune_0.6.0_amd64.deb

# 2. 安装（自动解析依赖 + 触发 postinst）
sudo apt install ./Attune_0.6.0_amd64.deb

# 3. 验证
systemctl status ollama          # 应该是 active
attune-desktop                   # 启动 GUI
```

**安装时自动做的事**：
- preinst：停止任何在跑的旧版 attune 进程（30s 优雅 + 强杀）
- postinst（按顺序）：
  1. **Ollama**：缺失则 `curl -fsSL https://ollama.com/install.sh | sh`
  2. **AMD GPU**：检测 `/sys/class/kfd/kfd/topology/nodes` → `gfx_target_version` 写 `HSA_OVERRIDE_GFX_VERSION` systemd drop-in：
     - gfx1103 / 1102 / 1150 / 1151（Phoenix / Hawk Point / Strix）→ `11.0.0`
     - gfx103x（Rembrandt / Yellow Carp）→ `10.3.0`
     - gfx900 / 906 / 908 / 90a / 940 / 942 / 1100 / 1101 / 1200 / 1201 → 原生支持，无 override
  3. **systemd 服务**：启用 ollama systemd 单元 + API ready 探测（15s 超时）；如果 Ollama 安装跳过 systemd（新版 install.sh 在 Ubuntu 25.10+ 默认 user-mode），自己写最小化 unit + 创建 user/group
  4. **Embedding 底座**：`ollama pull bge-m3` 或 `bge-small`（按 RAM tier）
  5. **K3 路径** (form factor 检测命中)：再 `ollama pull qwen2.5:3b` 或 `1.5b`（**笔电不走这条**）
  6. **ASR 底座**：whisper-cli symlink 到 /usr/local/bin + 下载 ggml-small-q8.bin
  7. **OCR 底座**：下载 PP-OCRv5 mobile 4 个 ONNX 文件到 `~/.local/share/attune/models/ppocr/`
  8. **Reranker**：lazy（首次搜索查询时 hf_hub 自动下载）

**Form factor 检测**（决定是否 K3 路径）：
- `ATTUNE_FORM_FACTOR=k3` env var override（K3 镜像构建时 systemd-environment.d 写）
- `/sys/class/dmi/id/product_name` 含 `k3` 或 `jetson` 关键字
- 否则默认 `laptop`（不预装 LLM）

**卸载**：
```bash
sudo apt remove attune       # 仅清 binary，保留用户数据 + Ollama
sudo apt purge attune        # 同上（数据 / Ollama 仍保留 — 用户独立决定）
```

### Fedora / RHEL (.rpm)

```bash
sudo dnf install ./Attune-0.6.0-1.x86_64.rpm
```

行为与 .deb 完全一致（共用 4 个 hook 脚本）。

### AppImage（便携 / 任何发行版）

AppImage 设计上**没有 install hooks**，所以 Ollama 自动安装必须靠**首次启动 wizard**：

```bash
chmod +x Attune_0.6.0_amd64.AppImage
./Attune_0.6.0_amd64.AppImage
# → wizard Step3LLM 检测 Ollama 状态：
#   - ready: 直接选 → 完成
#   - missing: 显示 install 命令 + 复制按钮 + 重新扫描按钮
```

如果你在 AMD APU 上用 AppImage，需要**手动**配置 HSA override：

```bash
sudo bash scripts/enable-amd-rocm-ollama.sh
# 或者用 attune CLI：
attune deploy
```

## Windows

### NSIS `.exe`

下载 `Attune_0.6.0_x64-setup.exe`，双击安装。

**安装时自动做的事**（NSIS hooks）：
- PREINSTALL：杀旧版 attune-desktop.exe / attune-server-headless.exe
- POSTINSTALL：
  - 检查 Ollama 是否在 PATH（`where ollama`）
  - 缺失则 `inetc::get` 下载 OllamaSetup.exe → `OllamaSetup.exe /S`（静默安装）
  - Ollama Windows 服务自启（无需手动）

CUDA / DirectML 由 Ollama runtime 自动选择，**无需 HSA override**等 Linux 特有配置。

**卸载**：开始菜单 → Attune → Uninstall。**不卸 Ollama**（用户独立决定）。

## macOS

暂不支持。详见 [CLAUDE.md "平台优先级"](../CLAUDE.md)。

## 开发 / 源码部署

如果你从源码 build（不走 .deb / .exe），用 `attune deploy` CLI 或 `scripts/deploy-linux.sh` 拿同等的 Ollama + GPU 配置：

```bash
# 编译
cd rust && cargo build --release

# 一键部署 Ollama + GPU 配置 + 拉模型
./target/release/attune deploy

# 或仅装 Ollama 不拉模型（更快）
./target/release/attune deploy --no-models
```

## 模型选择矩阵（自适应）

由 `scripts/deploy-linux.sh` / 首次启动 wizard 根据 RAM + GPU 自动选：

| RAM | GPU | embed | chat |
|-----|-----|-------|------|
| ≥16 GB | NVIDIA dGPU | bge-m3 | qwen2.5:7b |
| ≥16 GB | AMD APU/iGPU | bge-m3 | qwen2.5:3b |
| 8-16 GB | CPU only | bge-small | qwen2.5:1.5b |
| <8 GB | — | bge-small | qwen2.5:0.5b（精度受限） |

用户可以在 wizard / Settings 里覆盖默认选择。

## 故障排查

### postinst 报 "Ollama not found" 但实际已装
原因：dpkg/rpm 默认 PATH 不含 `/usr/local/bin`。**已修复**（postinst 显式扩展 PATH）。如仍出现：

```bash
ls -la /usr/local/bin/ollama  # 验证安装位置
sudo PATH=/usr/local/bin:$PATH dpkg-reconfigure attune
```

### AMD GPU 检测到但 ROCm 未生效
检查 systemd 是否真加载了 drop-in：
```bash
systemctl show ollama -p Environment
# 应该看到 HSA_OVERRIDE_GFX_VERSION=11.0.0
```

如未生效：
```bash
sudo systemctl daemon-reload
sudo systemctl restart ollama
```

### 验证 Ollama 在用 GPU
```bash
# 跑一次 chat，看 tokens/s — CPU 通常 < 15 t/s, GPU > 25 t/s
time curl -sf http://localhost:11434/api/generate \
  -d '{"model":"qwen2.5:3b","prompt":"hi","stream":false,"options":{"num_predict":30}}' \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);print("t/s:",round(d["eval_count"]/(d["eval_duration"]/1e9),1))'
```

## 卸载完整清理（彻底删 attune + 数据）

```bash
# Linux .deb / .rpm
sudo apt remove attune  # 或 dnf remove

# 用户数据
rm -rf ~/.local/share/attune ~/.config/npu-vault

# Ollama runtime + 模型（独立决定）
sudo systemctl disable --now ollama
sudo rm /usr/local/bin/ollama
rm -rf ~/.ollama
```

---

## Package managers(APT / RPM / WinGet / homebrew)

# Install Attune via package managers

> 从 **v1.0.0** 开始,Attune 桌面应用支持通过系统包管理器一键安装 + 自动升级.
> 历史版本(v0.7.0 及更早)仅有 [GitHub Releases](https://github.com/qiurui144/attune/releases) 手动下载路径.

## 目录

- [Windows — WinGet](#windows--winget)
- [Windows — Scoop(开发者)](#windows--scoop开发者)
- [Linux — APT (Debian / Ubuntu)](#linux--apt-debian--ubuntu)
- [Linux — DNF / YUM (RHEL / Fedora / openSUSE)](#linux--dnf--yum-rhel--fedora--opensuse)
- [Linux — AppImage (通用)](#linux--appimage-通用)
- [Linux / macOS — Homebrew(CLI/server)](#linux--macos--homebrewcliserver)
- [Tauri in-app auto-updater](#tauri-in-app-auto-updater)
- [其他平台与路径(v1.1 规划)](#其他平台与路径v11-规划)
- [验证安装](#验证安装)

---

## Windows — WinGet

Windows 11 内置 `winget` CLI.Windows 10 用户可在 [Microsoft Store](https://apps.microsoft.com/detail/9NBLGGH4NNS1) 安装 "App Installer" 后用 `winget`.

```powershell
winget install qiurui144.Attune
```

升级:

```powershell
winget upgrade qiurui144.Attune
```

> **注**:首次发布的新版本会在 microsoft/winget-pkgs 审核 1-3 天后才能被 `winget search` 命中.
> 若 `winget` 提示 "No package found",可改走 [Tauri 内置自更新](#tauri-in-app-auto-updater) 或 [手动下载](https://github.com/qiurui144/attune/releases).

## Windows — Scoop(开发者)

Scoop 适合开发者:不需管理员权限、多版本共存、纯命令行.

### 首次安装

```powershell
# 如果还没装 Scoop:
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
Invoke-RestMethod -Uri https://get.scoop.sh | Invoke-Expression

# 添加 attune bucket + 安装
scoop bucket add attune https://github.com/qiurui144/scoop-attune
scoop install attune
```

### 升级

```powershell
scoop update attune
```

### 卸载

```powershell
scoop uninstall attune
scoop bucket rm attune
```

> **WinGet vs Scoop**:WinGet 装 Tauri 桌面 GUI(NSIS installer + 系统 PATH);Scoop 装 CLI/server 二进制(开发者隔离 + 易切版本).两者**可并存**,各装各的.

## Linux — APT (Debian / Ubuntu)

### 首次安装

```bash
# 1. 导入 attune signing key
curl -fsSL https://qiurui144.github.io/attune/attune-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/attune-archive-keyring.gpg > /dev/null

# 2. 添加软件源
echo "deb [signed-by=/usr/share/keyrings/attune-archive-keyring.gpg] \
  https://qiurui144.github.io/attune/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/attune.list

# 3. 更新索引 + 安装
sudo apt update
sudo apt install attune
```

### 后续升级

```bash
sudo apt update && sudo apt upgrade attune
```

`apt upgrade` 会与系统其他包统一升级 — 真正的"装上就忘".

### 卸载

```bash
sudo apt remove attune
# 或彻底清干净:
sudo apt purge attune
```

## Linux — DNF / YUM (RHEL / Fedora / openSUSE)

### 首次安装

```bash
# 1. 添加 .repo 配置
sudo curl -fsSL -o /etc/yum.repos.d/attune.repo \
  https://qiurui144.github.io/attune/rpm/attune.repo

# 2. 安装
sudo dnf install attune
# RHEL/CentOS 7 用 yum:
sudo yum install attune
```

### 后续升级

```bash
sudo dnf upgrade attune
```

### 卸载

```bash
sudo dnf remove attune
```

## Linux — AppImage (通用)

不绑定发行版的便携格式.适合不想加软件源、想直接试用的用户.

```bash
# 从 GitHub Releases 下载
curl -L -o Attune.AppImage \
  https://github.com/qiurui144/attune/releases/latest/download/Attune_amd64.AppImage
chmod +x Attune.AppImage
./Attune.AppImage
```

**AppImage 不走 APT 自动升级** — Tauri 内置 auto-updater 会处理(见下).

## Linux / macOS — Homebrew(CLI/server)

Homebrew tap 仅分发 **CLI/server 二进制**(不含 Tauri 桌面 GUI).适合 headless 部署、远程服务器、习惯 Homebrew 的 macOS 开发者.

### 首次安装

```bash
brew tap qiurui144/attune
brew install attune
```

### 升级

```bash
brew update && brew upgrade attune
```

### 卸载

```bash
brew uninstall attune
brew untap qiurui144/attune
```

### 启动

```bash
attune --help                  # CLI usage
attune-server-headless          # 启动 server 在 :18900
```

> **macOS 桌面 GUI**:Homebrew tap 仅装 CLI;桌面 .dmg 走 [GitHub Releases](https://github.com/qiurui144/attune/releases/latest)(v1.1 加 cask 自动化).

---

## Tauri in-app auto-updater

无论你通过哪种方式装的(WinGet / APT / DNF / AppImage / 手动 .exe),桌面应用启动 30 秒后会**静默检查更新**(不弹窗).如发现新版,顶栏出现一个 "有新版可用" 的提示.

**用户操作**:

1. 点提示中的 "立即更新" 按钮 → 应用开始下载新版(后台,带进度)
2. 下载完成 → 点 "重启应用" → 完成升级

**特点**:
- 完全离线工作 manifest(由 GitHub Releases 静态托管,不依赖 attune 自建服务)
- 用 ed25519 签名验证 — 中间人无法注入恶意更新
- 失败 graceful:网络不通时不弹窗、不 panic,仅 log warn

**首次手动更新例外**:从 v0.7.x → v1.0.0 这一跳,如果维护者轮换了签名 keypair,**老客户端无法自动接收 v1.0.0**.请手动下载新版(用上面任一种包管理器).从 v1.0.0 开始全程自动.

---

## 其他平台与路径(v1.1 规划)

| 平台 / 工具 | 状态 | 备注 |
|------------|------|------|
| **Scoop** (Windows) | ✅ v1.0.11 | 见 [Windows — Scoop](#windows--scoop开发者).独立 bucket `qiurui144/scoop-attune` |
| **Homebrew** (Linux/macOS) | ✅ v1.0.11 | 见 [Homebrew](#linux--macos--homebrewcliserver).独立 tap `qiurui144/homebrew-attune`,**CLI/server only** |
| **Chocolatey** (Windows) | v1.1 | 需要 community 审核,周期长 |
| **Homebrew Cask** (macOS GUI) | v1.1 | 桌面 GUI 走 cask,需先解决 macOS Tauri build |
| **AUR** (Arch) | v1.1 | 期待社区贡献 PKGBUILD |
| **Flatpak** (Linux) | 评估中 | 用户群少,优先级低 |
| **Snap** (Linux) | 不做 | Canonical 锁定,与 deb/AppImage 冲突 |

---

## 验证安装

```bash
attune --version
# 期望输出: attune 1.0.0 (...)
```

```bash
# 启动 desktop app
attune-desktop
# 或从开始菜单 / 应用程序坞点击 Attune 图标
```

启动后:
- Windows: 系统托盘出现 Attune 图标
- Linux: 系统托盘 / 通知区出现 Attune 图标
- Web UI: 浏览器自动打开 `http://127.0.0.1:18900`,显示 Wizard 引导首次配置

详细使用指南见 [README.md](../README.md) 的 "Quick Start" 节.

---

## 故障排查

### apt update 报 `NO_PUBKEY` 或 `EXPKEYSIG`

签名 key 可能更新.重新导入:

```bash
curl -fsSL https://qiurui144.github.io/attune/attune-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/attune-archive-keyring.gpg > /dev/null
sudo apt update
```

### dnf install 报 GPG check 失败

```bash
sudo rpm --import https://qiurui144.github.io/attune/attune-archive-keyring.gpg
sudo dnf clean all
sudo dnf install attune
```

### winget 找不到包

WinGet 索引 + manifest 审核约 1-3 天.可以:
- 改用 [GitHub Releases 直下](https://github.com/qiurui144/attune/releases/latest)
- 或等 1-3 天后重试 `winget search qiurui144.Attune`

### 内置自动升级一直显示 "检查中"

通常是网络访问 GitHub 受限.短期可改手动下载新版.

### 私有部署 / 离线场景

不想依赖 qiurui144.github.io?可以镜像 release artifact 到内网静态服务器,改 `tauri.conf.json` 中的 `plugins.updater.endpoints[0]` 指向内网 URL 后重新打包.详见 [auto-updater-setup.md](auto-updater-setup.md).
