# Attune 安装指南

> 跨平台安装路径速查。安装包只负责 Attune 本体和用户数据目录准备，**不安装、不启动、不调优具体本地 AI worker**。AI 执行路径统一为云端/BYOK 或 edge scheduler；Ollama/llama.cpp/ORT 等只可作为 scheduler 内部实现或显式 legacy 自管环境。

## 总览

| 平台 | 包格式 | AI worker 管理 | Scheduler 配置 | 本体安装 |
|------|--------|---------------|----------------|----------|
| **Ubuntu / Debian** | `.deb` | 不管理；由 edge scheduler / cloud 承担 | wizard 或 `ATTUNE_EDGE_SCHEDULER_URL` | ✅ |
| **Fedora / RHEL** | `.rpm` | 不管理；共用 package hook | wizard 或 `ATTUNE_EDGE_SCHEDULER_URL` | ✅ |
| **任何 Linux** | AppImage | 不管理；首次启动引导 | wizard 手动填入 | ✅ |
| **Windows 10/11** | NSIS `.exe` | 不管理；不静默下载第三方 runtime | wizard 手动填入 | ✅ |
| **macOS** | — | — | — | 暂不支持 |

## AI 执行路径

Attune 的生产路径只有两类：

| 路径 | 用途 | 说明 |
|------|------|------|
| **Cloud / BYOK** | 通用 LLM 对话、复杂推理 | 默认推荐。用户在 wizard 或 Settings 配 OpenAI-compatible endpoint、模型和 key。 |
| **Edge scheduler** | 本地高性能知识库查询、embedding/rerank/OCR/ASR/LLM | 推荐给本机/边缘高性能平台。Attune 只连 scheduler 统一 API，不直连具体 worker。 |
| **Legacy self-managed runtime** | 研发或历史兼容 | 需要显式启用 legacy 脚本；不属于默认安装、部署、E2E 路径。 |

首启 wizard 推荐顺序：

1. **★ Attune Pro Membership**（登录即用）— 默认推荐
   - Endpoint: `https://gateway.engi-stack.com/v1`
   - 月费会员，token 配额由 attune 计费追踪
   - Gateway 路由到 OpenAI / Anthropic / Gemini（对用户透明）
2. **BYOK：用户已有的 API key**
   - OpenAI（ChatGPT Plus/Team 用户）
   - Anthropic（Claude Pro 用户）
   - Gemini（Gemini Advanced / Google AI Studio）
   - DeepSeek / Qwen / 其他 OpenAI 兼容
3. **Edge scheduler**（本机或局域网）
   - 示例：`http://127.0.0.1:8090`
   - Attune 使用 scheduler-native KB task 和 OpenAI-compatible chat 入口。

## Linux

### Ubuntu / Debian (.deb)

> v1.0+ 起推荐 APT 仓库一行安装 + 自动升级（见 [README](../README.md#-download) 的「系统包管理器」节）。下方手动 .deb 流程仍受支持。

```bash
# 1. 下载 .deb（取最新 desktop-vX.Y.Z, VERSION 替换为实际版本号, 如 1.2.0）
VERSION=1.2.0
wget https://github.com/qiurui144/attune/releases/download/desktop-v${VERSION}/Attune_${VERSION}_amd64.deb

# 2. 安装（自动解析依赖 + 触发 postinst）
sudo apt install ./Attune_${VERSION}_amd64.deb

# 3. 验证
attune-desktop                   # 启动 GUI
```

**安装时自动做的事**：
- preinst：停止任何在跑的旧版 attune 进程（30s 优雅 + 强杀）
- postinst（按顺序）：
  1. 创建 Attune 用户数据目录和日志目录。
  2. 清理旧版 Attune 写入的 worker shim（仅有明确 marker 时），不重启第三方 runtime。
  3. 如设置 `ATTUNE_EDGE_SCHEDULER_URL` 或 `ATTUNE_LOCAL_SCHEDULER_BASE`，做只读 scheduler 健康探测。
  4. 输出下一步配置提示：cloud/BYOK 或 edge scheduler。

**Form factor 检测**仅用于默认推荐：
- `ATTUNE_FORM_FACTOR=edge_scheduler` / `local_scheduler` env var override。
- `/sys/class/dmi/id/product_name` 含 `edge-scheduler` / `local-scheduler` / `attune-appliance` 关键字。
- 否则默认通用桌面路径：wizard 推荐 cloud/BYOK。

**卸载**：
```bash
sudo apt remove attune       # 仅清 binary，保留用户数据 + 第三方 runtime
sudo apt purge attune        # 仍保留用户数据和第三方 runtime；彻底清理见下方
```

### Fedora / RHEL (.rpm)

> v1.0+ 起推荐 DNF 仓库一行安装（见 [README](../README.md#-download)）。下方手动 .rpm 流程仍受支持（VERSION 替换为实际版本号, 如 1.2.0）。

```bash
VERSION=1.2.0
sudo dnf install ./Attune-${VERSION}-1.x86_64.rpm
```

行为与 .deb 完全一致（共用 4 个 hook 脚本）。

### AppImage（便携 / 任何发行版）

AppImage 设计上**没有 install hooks**，所以 AI 路径只通过**首次启动 wizard**配置：

```bash
# 取最新 desktop-vX.Y.Z 的 AppImage（VERSION 替换为实际版本号, 如 1.2.0）
VERSION=1.2.0
chmod +x Attune_${VERSION}_amd64.AppImage
./Attune_${VERSION}_amd64.AppImage
# → wizard Step3LLM 配置 cloud/BYOK 或 edge scheduler endpoint
```

如果你维护的是 legacy 自管 Ollama 环境，AMD APU HSA override 需要显式 legacy opt-in：

```bash
ATTUNE_ALLOW_LEGACY_DIRECT_OLLAMA=1 sudo -E bash scripts/enable-amd-rocm-ollama.sh
```

## Windows

### NSIS `.exe`

从 Releases 页取最新 `desktop-vX.Y.Z` 的 `Attune_<VERSION>_x64-setup.exe`，双击安装（或 `winget install qiurui144.Attune`，v1.0+ 推荐）。

**安装时自动做的事**（NSIS hooks）：
- PREINSTALL：杀旧版 attune-desktop.exe / attune-server-headless.exe
- POSTINSTALL：
  - 复制随包的必要桌面运行库。
  - 写入静默卸载注册表项，便于企业批量回滚。
  - 不下载或安装第三方 AI runtime。

Windows 高性能本地推理同样应通过 edge scheduler 统一收口；Attune 本体不直接选择 CUDA / DirectML worker。

**卸载**：开始菜单 → Attune → Uninstall。第三方 AI runtime 和模型文件不由 Attune 卸载。

## macOS

暂不支持。详见 [CLAUDE.md "平台优先级"](../CLAUDE.md)。

## 开发 / 源码部署

如果你从源码 build（不走 .deb / .exe），用 `scripts/deploy-linux.sh` 做主机预检和 scheduler contract 探测：

```bash
# 编译
cd rust && cargo build --release

# cloud-only：只检查本机依赖可见性，LLM 在 wizard/Settings 配置
../scripts/deploy-linux.sh --cloud-only

# edge mode：探测 scheduler contract
ATTUNE_EDGE_SCHEDULER_URL=http://127.0.0.1:8090 ../scripts/deploy-linux.sh
```

## Edge Scheduler 配置

Attune 只需要一个 scheduler endpoint。模型生命周期、RVV/AVX/ROCm/CUDA/DirectML 等硬件优化应由 scheduler 自己管理。

| 场景 | 配置 |
|------|------|
| 本机 scheduler | `http://127.0.0.1:8090` |
| 局域网 scheduler | `http://<host>:8090` |
| E2E 长文本 | `ATTUNE_E2E_LOCAL_SCHEDULER=http://127.0.0.1:8090` |
| 安装后只读探测 | `ATTUNE_EDGE_SCHEDULER_URL=http://127.0.0.1:8090` |

## 故障排查

### Edge scheduler 探测失败

```bash
ATTUNE_EDGE_SCHEDULER_URL=http://127.0.0.1:8090 scripts/deploy-linux.sh
python3 scripts/probe-edge-scheduler-contract.py --base-url http://127.0.0.1:8090 --strict
```

检查项：
- `/ready` 或 `/ready?hot=1` 返回 2xx。
- `/benchmark/contract` 暴露 `edge-scheduler-contract-v2`、prompt cache/refusal/schema metadata。
- `/models` 暴露模型 state/lifecycle。
- `/capacity` 暴露 memory 或 `dram_total_gb`。

### Legacy 自管 Ollama

默认安装/部署不会触碰 Ollama。历史自管环境需要自行安装和维护；Attune 只保留显式 legacy 辅助脚本，例如 AMD ROCm override：

```bash
ATTUNE_ALLOW_LEGACY_DIRECT_OLLAMA=1 sudo -E bash scripts/enable-amd-rocm-ollama.sh
```

## 卸载完整清理（彻底删 attune + 数据）

```bash
# Linux .deb / .rpm
sudo apt remove attune  # 或 dnf remove

# 用户数据
rm -rf ~/.local/share/attune ~/.config/npu-vault

# 第三方 AI runtime + 模型（独立决定，不由 Attune 自动删除）
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
# 期望输出: attune 1.5.0 (...)
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

不想依赖 qiurui144.github.io?可以镜像 release artifact 到内网静态服务器,改 `tauri.conf.json` 中的 `plugins.updater.endpoints[0]` 指向内网 URL 后重新打包.详见 [updater.md](updater.md).
