# spec: attune release 包管理接入

date: 2026-05-22
status: ratified (mini-spec; CI infra, 不是 product feature, 走 5 节)
target: v1.0.0 GA (2026-05-25) + 上架日 2026-05-26
scope: attune OSS 主仓 only — 不动 attune-pro / cloud

## 1. 目标与边界

**目标**：v1.0 GA 用户安装后获得 3 条升级路径，不再依赖手动下载新 release：

1. **Tauri in-app auto-updater**（点应用内按钮即升级，桌面用户主路径）
2. **Linux APT / RPM 软件源**（`apt install attune` / `dnf install attune`，命令行用户）
3. **Windows WinGet**（`winget install qiurui144.attune`，Win11 主路径）

**边界**：
- ✅ 仅改 `apps/attune-desktop`、`.github/workflows/`、`docs/`、`scripts/`、`README*.md`、`DEVELOP.md`
- ❌ 不调 Ollama / 任何本地 LLM / GPU 资源（全局 CLAUDE.md 红线）
- ❌ 不动 attune-pro / cloud 子项目
- ❌ 不做 Scoop / Chocolatey / Homebrew / AUR（推 v1.1，本 spec 仅文档化路径）
- ❌ 不做 macOS（per project CLAUDE.md 平台优先级 — macOS 暂不做）

## 2. 架构与数据流

### 2.1 Tauri auto-updater 数据流（无服务器，静态文件托管 on GitHub Releases）

```
开发者打 tag desktop-vX.Y.Z
   ↓
desktop-release.yml 触发
   ↓
build NSIS/MSI/DEB/RPM/AppImage
   ↓
tauri-cli signer 签每个 bundle (用 secrets.TAURI_SIGNING_PRIVATE_KEY)
   ↓ 产生 *.sig
   ↓
gen-latest-json.sh 生成 latest.json (含 platforms 每个 target 的 url + signature + version)
   ↓
softprops/action-gh-release 上传 bundle + *.sig + latest.json
   ↓
用户客户端（已装 desktop-vA.B.C）30s 后 / 用户点 "检查更新"
   ↓
tauri-plugin-updater GET https://github.com/qiurui144/attune/releases/latest/download/latest.json
   ↓ 解析 platforms.<target>.url + signature
   ↓
下载 bundle 验证签名 → 替换二进制 → 重启
```

**关键点**：`latest.json` 是 GitHub Releases 静态 asset，**用 `releases/latest/download/` URL 永远指向最新 release**，无需任何 server / DNS。这消除了 `updates.engi-stack.com` 域名依赖。

### 2.2 APT / RPM 源数据流

```
desktop-vX.Y.Z 触发 apt-rpm-repo.yml
   ↓
checkout gh-pages branch (orphan repo, 不含主代码)
   ↓
gh release download desktop-vX.Y.Z (下载 .deb / .rpm)
   ↓
APT: dpkg-scanpackages → Packages.gz; 用 gpg signing key 签 Release
RPM: createrepo_c → repodata/; 同 gpg key 签
   ↓
commit + push gh-pages
   ↓
public URLs:
  https://qiurui144.github.io/attune/apt/dists/stable/main/binary-amd64/Packages.gz
  https://qiurui144.github.io/attune/rpm/x86_64/repodata/repomd.xml
   ↓
用户 setup:
  curl -fsSL https://qiurui144.github.io/attune/apt-key.gpg | sudo apt-key add -
  echo "deb https://qiurui144.github.io/attune/apt stable main" | sudo tee /etc/apt/sources.list.d/attune.list
  sudo apt update && sudo apt install attune
```

### 2.3 WinGet 数据流

```
desktop-vX.Y.Z 触发 winget.yml
   ↓
vedantmgoyal2009/winget-releaser@v2 action
   ↓
从 release 抓 *.exe (NSIS) 计算 SHA256
   ↓
生成 manifest YAML (qiurui144.attune.{installer,locale.en-US,version}.yaml)
   ↓
fork microsoft/winget-pkgs → push branch → 自动开 PR
   ↓ (人工 / bot 审 PR)
合并后 winget 索引含 qiurui144.attune
   ↓
用户 `winget install qiurui144.attune`
```

## 3. 文件清单（要改 / 要新增）

| 文件 | 操作 | 内容要点 |
|------|------|---------|
| `apps/attune-desktop/tauri.conf.json` | edit | endpoint 从 `updates.engi-stack.com` → GitHub Releases raw URL；`dialog: true`（用户可见弹窗）|
| `apps/attune-desktop/capabilities/default.json` | edit | 加 `updater:default` permission |
| `apps/attune-desktop/src/main.rs` | edit | check + download + install 完整流程（替换当前只 log 的 stub）|
| `.github/workflows/desktop-release.yml` | edit | 加 signing env + latest.json 生成 + 上传 |
| `.github/workflows/apt-rpm-repo.yml` | new | tag push 触发 → 更新 gh-pages 上的 apt/rpm metadata |
| `.github/workflows/winget.yml` | new | tag push 触发 → 自动开 winget-pkgs PR |
| `scripts/gen-latest-json.sh` | new | 从 bundle 输出生成 Tauri v2 manifest |
| `scripts/generate-updater-key.sh` | new | 一键生成 ed25519 keypair（私钥提示用户加 Secret，公钥更新 conf）|
| `scripts/apt-repo-init.sh` | new | 首次 bootstrap gh-pages 上 apt/ 与 rpm/ 目录结构 |
| `docs/auto-updater-setup.md` | new | 维护者文档：keypair 管理 + secret 配置 + 验证流程 |
| `docs/install-package-managers.md` | new | 用户文档：apt/dnf/winget 三路径安装命令 |
| `README.md` + `README.zh.md` | edit | Download 节加 "Package managers" 子节 |
| `DEVELOP.md` | edit | Release Checklist 加签名 + 软件源步骤 |
| `RELEASE.md` | edit | v1.0.0 节加包管理接入条目 |

## 4. 风险登记 + 缓解

| 风险 | 缓解 |
|------|------|
| **私钥已存在但我不知道** — 当前 `tauri.conf.json` 已有 pubkey；若用户有匹配私钥则复用，否则签名验证失败客户端无法升级 | spec 要求维护者执行 `scripts/generate-updater-key.sh` **决定是否轮换**：若有 → 加 secret；若无 → 生成新对，更新 pubkey，提示 0.7.0 老客户端不能自动升级（首次手动安装）|
| **gh-pages 分支冲突** — 多次 release 并发可能 race | workflow 加 `concurrency: apt-rpm-repo` 阻止并发；用 retry-with-rebase |
| **WinGet PR 卡审** — microsoft/winget-pkgs review 可能 1-7 天 | 不阻塞发版；v1.0 GA 不要求 winget 当日上架；README 标注 "winget 通常发版后 1-3 天可用" |
| **APT/RPM gpg key 丢失** — 一旦丢失老用户 apt update 会签名错误 | secret 备份策略 + docs 文档化恢复流程；首次发布前生成保管 |
| **AppImage 不进 apt** — AppImage 不是 .deb，无法走 APT | 文档说明 AppImage 走 GitHub Releases 直下；APT 仅 .deb |
| **域名 `updates.engi-stack.com` 弃用** — 老 v0.7.0 客户端连不上更新会 log warn 但不 crash（main.rs 已 graceful）| v0.7.0 → v1.0.0 升级需手动；新 endpoint 在 v1.0.0 起永久走 GitHub Releases |
| **gh auth 不可用** — 我当前 session gh CLI token 失效，无法验证 secret/触发 workflow | 所有改动通过 push 触发；secret 配置由用户在 GitHub web UI 完成；docs 提供完整 step-by-step |

## 5. 验收清单

**P0 Tauri updater**：
- [ ] `tauri.conf.json` endpoint 改为 GitHub Releases URL
- [ ] `main.rs` 升级到 download + install（含错误处理）
- [ ] `desktop-release.yml` 含 signing env (`TAURI_SIGNING_PRIVATE_KEY` + password)
- [ ] workflow 产物含 bundle + `*.sig` + `latest.json`
- [ ] capabilities 允许 updater commands
- [ ] `scripts/generate-updater-key.sh` 可直接跑生成新对
- [ ] `docs/auto-updater-setup.md` 完整维护文档

**P1 APT/RPM**：
- [ ] `apt-rpm-repo.yml` workflow 写好（不强制 v1.0 当日 fire）
- [ ] `scripts/apt-repo-init.sh` bootstrap 脚本可用
- [ ] `docs/install-package-managers.md` 含 apt/dnf 完整命令

**P2 WinGet**：
- [ ] `winget.yml` workflow 写好
- [ ] docs 含 `winget install` 命令 + 1-3 天延迟说明

**文档**：
- [ ] README/README.zh.md Download 节扩展
- [ ] DEVELOP.md Release Checklist 同步
- [ ] RELEASE.md v1.0.0 节加 "包管理接入" 条目

**不改 / 不动**：
- [ ] attune-pro 仓任何文件
- [ ] cloud 仓任何文件
- [ ] 任何 Ollama / 本地 LLM 调用
- [ ] python/ 原型线

## 备注 — 与 v0.7.0 既有 stub 的关系

`main.rs:142-159` 已有 30s 启动后 `updater().check()` 的 log-only 实现，
本 spec 在此基础上**扩展为完整 check + download + install**，不删除现有逻辑。
`tauri.conf.json` 已 wired 的 updater 节本 spec 仅改 endpoint + dialog 字段，
保留 active+pubkey（pubkey 由维护者决定是否轮换）。
