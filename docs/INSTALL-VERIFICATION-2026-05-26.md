# Install Package Managers — 5/26 Real-World Verification Plan

> **范围**：v1.0.0 GA tag (`desktop-v1.0.0` / `v1.0.0`) 已 push 后，在干净环境真实验证 user-facing 安装路径。
>
> **触发**：5/26 上架前必跑（per gap audit §11.3 — Release Engineering Gap 11.3 P1）。
>
> **执行人**：user 在物理 / 干净 VM 环境（**非 dev 机器**）操作，截图归档至 `docs/screenshots/v1.0-package-managers/`。
>
> **本文件不真跑 install**（per CLAUDE.md "不动 4090 / Ollama / key" + "spec ≠ 实测"）；这是 user 5/26 实测时的 checklist。

## 目录

- [0. 前置条件](#0-前置条件)
- [1. Windows 11 — winget](#1-windows-11--winget)
- [2. Ubuntu 24.04 — apt](#2-ubuntu-2404--apt)
- [3. Fedora 40 — dnf](#3-fedora-40--dnf)
- [4. AppImage 通用 Linux](#4-appimage-通用-linux)
- [5. Tauri 内置自更新（in-app updater）](#5-tauri-内置自更新in-app-updater)
- [6. 截图归档清单](#6-截图归档清单)
- [7. 失败应对策略](#7-失败应对策略)

---

## 0. 前置条件

### 0.1 GA tag 已 push

- [ ] `git tag -l "v1.0.0"` 返回 `v1.0.0`
- [ ] `git tag -l "desktop-v1.0.0"` 返回 `desktop-v1.0.0`
- [ ] GitHub Release 页面 `v1.0.0` + `desktop-v1.0.0` artifact 全 8 平台 binary 已上传

### 0.2 Workflow 已成功跑完

- [ ] `desktop-release.yml` 在 `desktop-v1.0.0` tag 上 ✅
- [ ] `winget.yml` 通过 `workflow_dispatch` 已手动触发（input: `desktop-v1.0.0`）→ 看到 PR 链接进 `microsoft/winget-pkgs`
- [ ] `apt-rpm-repo.yml` 通过 `workflow_dispatch` 已触发（input: `desktop-v1.0.0`）→ 看到 `gh-pages` 分支 apt/ + rpm/ 已更新
- [ ] `docker-publish.yml` （如适用）镜像 push 成功

### 0.3 上游审核状态

- [ ] microsoft/winget-pkgs PR 是否合并？（**1-7 工作日不确定** — 5/26 当日大概率仍 pending；本文预设 winget 路径**首次发版会 fail**，给应对策略）
- [ ] `https://qiurui144.github.io/attune/apt/dists/stable/Release` 可访问（返回 200 + 含 Signed-By）
- [ ] `https://qiurui144.github.io/attune/rpm/x86_64/repodata/repomd.xml` 可访问

### 0.4 干净环境清单

- [ ] **Windows 11**：刚装系统 / 全新 VM，未装过 Attune；`winget --version` ≥ 1.6
- [ ] **Ubuntu 24.04**：Docker container `ubuntu:24.04` 或 VM；首次 boot；无 `/var/lib/dpkg/info/attune.*` 残留
- [ ] **Fedora 40**：Docker container `fedora:40` 或 VM；首次 boot；无 `/var/lib/rpm/Attune*` 残留

---

## 1. Windows 11 — winget

### 1.1 命令

```powershell
# 检查 winget 可用
winget --version
# 预期: v1.6+ 或 v1.7+

# 搜索 attune
winget search Attune
# 预期成功: 看到 qiurui144.Attune 1.0.0 entry
# 预期 fail (大概率): "No package found matching input criteria"

# 若搜索命中,执行 install
winget install qiurui144.Attune
# 预期: 下载 NSIS .exe → 静默安装 → 完成

# 验证安装
winget list | findstr Attune
# 预期: qiurui144.Attune 1.0.0
```

### 1.2 预期 fail 信号

| 信号 | 含义 | 应对 |
|------|------|------|
| `No package found matching input criteria: Attune` | microsoft/winget-pkgs PR 未合并 | 手动下载 `attune_1.0.0_x64-setup.exe` 从 GitHub Releases；或等待 1-3 天 PR 合并后重试 |
| `winget search` 命中但 install 提示 `0x80D02002` | NSIS 签名验证失败 | 检查 Tauri signing key + winget manifest InstallerSha256 一致性 |
| `winget search` 命中但 fail 下载 | URL 404 / GitHub release artifact 缺 | 验证 desktop-release.yml 产物上传完整 |
| `Hash mismatch` | manifest 中 SHA256 与实际 .exe 不一致 | wingetcreate 算 hash 时机问题 — 重跑 winget.yml workflow |

### 1.3 启动验证

- [ ] 开始菜单可见「Attune」
- [ ] 双击启动，UI 加载 wizard step1 在 ≤5 秒内
- [ ] Vault unlock + login 流程 OK
- [ ] 截图：1 个 wizard 启动屏（`attune-v100-winget-wizard.png`）

### 1.4 卸载验证

```powershell
winget uninstall qiurui144.Attune
```
- [ ] 卸载干净（程序文件 + 注册表）
- [ ] Vault 数据**保留**（user data 不应被卸载删）

---

## 2. Ubuntu 24.04 — apt

### 2.1 命令

```bash
# 1. 导入 attune signing key
curl -fsSL https://qiurui144.github.io/attune/attune-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/attune-archive-keyring.gpg > /dev/null

# 2. 添加 repo
echo "deb [signed-by=/usr/share/keyrings/attune-archive-keyring.gpg] \
  https://qiurui144.github.io/attune/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/attune.list

# 3. 更新 + 安装
sudo apt update
sudo apt install -y attune

# 4. 验证
attune --version
# 预期: 1.0.0
dpkg -l | grep attune
# 预期: ii attune 1.0.0 amd64
```

### 2.2 预期 fail 信号

| 信号 | 含义 | 应对 |
|------|------|------|
| `curl: (22) The requested URL returned error: 404 Not Found` (keyring) | `gh-pages` 未上传 keyring 或路径错 | 检查 apt-rpm-repo.yml step 是否上传 `.gpg` 到 gh-pages 根 |
| `apt update` 报 `GPG error ... NO_PUBKEY` | keyring 未导入或路径错 | 重新导入 keyring |
| `apt update` 报 `Hash Sum mismatch` | gh-pages 上 `Packages` 索引与实际 .deb SHA 不一致 | 重跑 apt-rpm-repo.yml workflow |
| `Unable to locate package attune` | `apt update` 没成功跑或 `attune.list` 路径错 | `cat /etc/apt/sources.list.d/attune.list` 确认 + `apt-cache policy` 排查 |
| `attune depends on libwebkit2gtk-4.1-0` 缺依赖 | 干净 Ubuntu 缺 GTK4 / webkit | `sudo apt install -y libwebkit2gtk-4.1-0 libsoup-3.0-0` |

### 2.3 启动验证

- [ ] `attune` 在 PATH（`which attune` → `/usr/bin/attune`）
- [ ] GUI 启动（如 X / Wayland 可用）；headless VM 则 `attune --version` 替代
- [ ] 桌面 `.desktop` 文件存在：`ls /usr/share/applications/attune.desktop`
- [ ] 截图：apt install 终端输出 + 启动屏（`attune-v100-apt-install.png` + `attune-v100-apt-startup.png`）

### 2.4 升级验证

```bash
# 假定后续 v1.0.1 已发布
sudo apt update && sudo apt upgrade -y attune
# 预期: 升级到 1.0.1
```

### 2.5 卸载验证

```bash
sudo apt remove --purge -y attune
ls ~/.local/share/attune  # vault 数据应保留(per packaging policy)
```

---

## 3. Fedora 40 — dnf

### 3.1 命令

```bash
# 1. 添加 repo
sudo dnf config-manager --add-repo \
  https://qiurui144.github.io/attune/rpm/attune.repo
# OR 手动创建:
sudo tee /etc/yum.repos.d/attune.repo <<'EOF'
[attune]
name=Attune Repository
baseurl=https://qiurui144.github.io/attune/rpm/x86_64/
enabled=1
gpgcheck=1
gpgkey=https://qiurui144.github.io/attune/attune-archive-keyring.asc
EOF

# 2. 导入 GPG key
sudo rpm --import https://qiurui144.github.io/attune/attune-archive-keyring.asc

# 3. 安装
sudo dnf install -y attune

# 4. 验证
attune --version
rpm -qa | grep -i attune
```

### 3.2 预期 fail 信号

| 信号 | 含义 | 应对 |
|------|------|------|
| `Cannot download repomd.xml` 404 | gh-pages 上 rpm/x86_64/repodata 缺 | 重跑 apt-rpm-repo.yml |
| `GPG key not imported` | `.asc` 文件 404 | 检查 gh-pages 是否上传 ASCII-armored .asc（注意 .gpg 是 binary，.asc 是 armored）|
| `Public key for attune-1.0.0-1.x86_64.rpm is not installed` | GPG key ID 不匹配 .rpm 签名 | 重新签名 .rpm 或更新 keyring |
| `Conflicts: attune-1.0.0` (in case Fedora 40 有 namespace 冲突) | 暂未踩到，记录 | 改用 `attune-desktop` 重新打包 |

### 3.3 启动验证

- [ ] 与 §2.3 同
- [ ] 截图：`attune-v100-dnf-install.png`

### 3.4 升级

```bash
sudo dnf upgrade -y attune
```

### 3.5 卸载

```bash
sudo dnf remove -y attune
```

---

## 4. AppImage 通用 Linux

### 4.1 命令

```bash
# 1. 下载 AppImage（从 GitHub Releases）
wget https://github.com/qiurui144/attune/releases/download/desktop-v1.0.0/attune_1.0.0_amd64.AppImage

# 2. 加可执行权限
chmod +x attune_1.0.0_amd64.AppImage

# 3. 启动（无需 root，无需 install）
./attune_1.0.0_amd64.AppImage --version

# 4. （可选）集成到桌面
./attune_1.0.0_amd64.AppImage --appimage-extract
# 或用 AppImageLauncher
```

### 4.2 预期 fail 信号

| 信号 | 应对 |
|------|------|
| `bash: ./attune_1.0.0_amd64.AppImage: cannot execute binary file` | 架构不匹配（aarch64 系统下 amd64 binary）；下载 `aarch64.AppImage` |
| `AppImages require FUSE to run` | `sudo apt install -y fuse libfuse2` |
| GUI 启动 fail (Wayland) | 设置 `GDK_BACKEND=x11 ./attune.AppImage` |

### 4.3 截图

- [ ] `attune-v100-appimage-launch.png`

---

## 5. Tauri 内置自更新（in-app updater）

### 5.1 验证场景

假设 user 已装 v0.7.0，等待 v1.0.0 自动更新提示：

- [ ] 启动 v0.7.0 → 等待 30s 后看到「新版本可用」toast
- [ ] 点击「立即更新」→ 下载 .nsis-update / .AppImage / .msi → 重启
- [ ] 重启后 `attune --version` 显示 1.0.0
- [ ] Vault 数据保留（user data 不丢）

### 5.2 预期 fail 信号

| 信号 | 应对 |
|------|------|
| 不弹更新 toast | 检查 `tauri.conf.json` updater.endpoints 是否包含 `https://github.com/qiurui144/attune/releases/latest/download/latest.json` |
| 下载 fail (signature) | Tauri pubkey 嵌入 binary，新版本签名 key 应一致；若 rotate 了 key，老 client 卡住（需文档化 - per gap audit §11.4） |
| 重启后版本未变 | NSIS updater installer 路径错 / Tauri auto-restart 失败 — 手动重启 |

### 5.3 截图

- [ ] `attune-v100-updater-prompt.png`（更新提示 dialog）
- [ ] `attune-v100-updater-success.png`（更新完成后 about 页显示 v1.0.0）

---

## 6. 截图归档清单

5/26 执行后，所有截图归档：

```
docs/screenshots/v1.0-package-managers/
├── attune-v100-winget-search.png
├── attune-v100-winget-install.png    (or attune-v100-winget-not-found.png 若 fail)
├── attune-v100-winget-wizard.png
├── attune-v100-apt-install.png
├── attune-v100-apt-startup.png
├── attune-v100-dnf-install.png
├── attune-v100-dnf-startup.png
├── attune-v100-appimage-launch.png
├── attune-v100-updater-prompt.png
└── attune-v100-updater-success.png
```

提交 commit：`docs(screenshots): v1.0 GA install pkg manager verification 5/26`

---

## 7. 失败应对策略

### 7.1 winget PR 未合并（高概率 fail）

**症状**：`winget search Attune` 返回 `No package found` 或 `winget install` 失败。

**应对（5/26 当日）**：
1. **官网下载页主推「直接下载 .exe」**（per `docs/install-package-managers.md` 兜底路径）
2. footer 增加临时说明：「Windows users: winget package launching v1.0.1 (pending Microsoft review). For now, download .exe directly.」
3. 等待 microsoft/winget-pkgs PR 合并（1-7 工作日）；user 也可 GitHub 上对自己 PR 加 comment 加速
4. PR 合并后 `winget source update` 即可命中

### 7.2 apt/rpm repo 真发布 fail

**症状**：`curl https://qiurui144.github.io/attune/apt/dists/stable/Release` 404 或 keyring 404。

**应对**：
1. 检查 `apt-rpm-repo.yml` workflow 日志 — 是否 secrets 缺失（GPG_PRIVATE_KEY / GPG_PRIVATE_KEY_PASSWORD / GPG_KEY_ID）？
2. `git checkout gh-pages` 查看实际产物：
   ```bash
   git fetch origin gh-pages:gh-pages
   git ls-tree -r gh-pages | head
   ```
3. 若 gh-pages 分支空 → secrets 全缺 → user 配置 secrets 后重跑 workflow
4. 若 gh-pages 含 apt/ rpm/ 但 404 → GitHub Pages 启用问题 → repo Settings > Pages 启用 gh-pages 分支

### 7.3 v1.0.0 跑成 alpha/beta 状态（GA 未达 prerelease=false）

**症状**：GitHub Release 标 `Pre-release` 而非正式。

**应对**：
- 检查 desktop-release.yml `prerelease` 判定：tag `desktop-v1.0.0` 不含 `-rc/-alpha/-beta` → prerelease=false ✅
- 若仍标 Pre-release → Release 页手动 uncheck

### 7.4 5/26 当日全 fail 怎么办

**最低保障路径**（永远可工作的）：
1. GitHub Releases 页 https://github.com/qiurui144/attune/releases/tag/desktop-v1.0.0 公开可下载（无需任何 pkg manager）
2. README + 官网下载页主推「直接下载」+ 各 OS 安装包链接
3. winget / apt / rpm 可作为 v1.0.1 sprint（5/27-6/2）跟进 P1 项

### 7.5 文档已就绪

per gap audit §11.3 → **本文件即 P1 项**。5/26 user 实测后：
- PASS 路径：截图归档 + RELEASE.md 标「v1.0.0 winget/apt/rpm available」
- FAIL 路径：每条 fail 在 RELEASE.md「Known Limitations」节登记 + v1.0.1 修复 task 入 GitHub Issue

---

## 附录 A：5/26 user 执行简要 checklist

```
[ ] 0.1-0.4 prerequisites 通过
[ ] 1.1 winget search → 截图 (PASS or NOT FOUND)
[ ] 1.2-1.4 winget install + startup + uninstall（若 PASS）
[ ] 2.1-2.5 apt install + startup + upgrade + uninstall
[ ] 3.1-3.5 dnf install + startup + upgrade + uninstall
[ ] 4.1-4.3 AppImage launch
[ ] 5.1-5.3 Tauri updater (optional, 需提前装 v0.7.0)
[ ] 6 截图归档 + commit
[ ] 7 fail 项登记到 RELEASE.md Known Limitations
```

**Time box**：30 min（PASS 路径）OR 60 min（含 fail 排查 + 截图）。
