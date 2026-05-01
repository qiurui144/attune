# Attune 安装指南

> 跨平台安装路径速查。每条路径都包含 Ollama 自动安装 + 硬件自适应。

## 总览

| 平台 | 包格式 | 自动 Ollama 安装 | 自动硬件配置 | 备注 |
|------|--------|------------------|--------------|------|
| **Ubuntu / Debian** | `.deb` | ✅ postinst 自动 curl install.sh | ✅ AMD APU 自动注入 HSA_OVERRIDE | **推荐** |
| **Fedora / RHEL** | `.rpm` | ✅ postinst 共用 deb hook 脚本 | ✅ 同上 | 推荐 |
| **任何 Linux** | AppImage | ⚠️ 首次启动 wizard 引导 | ⚠️ 同上 | 便携 / 沙箱场景 |
| **Windows 10/11** | NSIS `.exe` | ✅ installer.nsh 下载 OllamaSetup.exe | N/A (CUDA / DirectML 自动) | **推荐** |
| **macOS** | — | — | — | 暂不支持 (per CLAUDE.md) |

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
- postinst：
  - 检查 Ollama，缺失则 `curl -fsSL https://ollama.com/install.sh | sh`
  - 检测 AMD GPU（`/sys/class/kfd/kfd/topology/nodes`），按 `gfx_target_version` 写 `HSA_OVERRIDE_GFX_VERSION` systemd drop-in：
    - gfx1103 / 1102 / 1150 / 1151（Phoenix / Hawk Point / Strix）→ `11.0.0`
    - gfx103x（Rembrandt / Yellow Carp）→ `10.3.0`
    - gfx900 / 906 / 908 / 90a / 940 / 942 / 1100 / 1101 / 1200 / 1201 → 原生支持，无 override
  - 启用 ollama systemd 服务 + API ready 探测（10s 超时）
  - **不**拉模型（留给首次启动 wizard，progress UI 友好）

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
