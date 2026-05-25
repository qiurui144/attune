# Install Attune via package managers

> 从 **v1.0.0** 开始,Attune 桌面应用支持通过系统包管理器一键安装 + 自动升级.
> 历史版本(v0.7.0 及更早)仅有 [GitHub Releases](https://github.com/qiurui144/attune/releases) 手动下载路径.

## 目录

- [Windows — WinGet](#windows--winget)
- [Linux — APT (Debian / Ubuntu)](#linux--apt-debian--ubuntu)
- [Linux — DNF / YUM (RHEL / Fedora / openSUSE)](#linux--dnf--yum-rhel--fedora--opensuse)
- [Linux — AppImage (通用)](#linux--appimage-通用)
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
| **Scoop** (Windows) | v1.1 | 需要单独 bucket 仓库,工作量大,推后 |
| **Chocolatey** (Windows) | v1.1 | 需要 community 审核,周期长 |
| **Homebrew** (macOS) | 不做 | 项目暂不支持 macOS(全局优先级) |
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
