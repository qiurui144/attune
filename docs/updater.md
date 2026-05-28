# attune Tauri 桌面更新器(auto-updater + 部署)

> 合并自 `auto-updater-setup.md`(开发者 setup)+ `tauri-updater-deploy.md`(运维部署),
> 2026-05-28 收拢。

# Auto-updater 与软件源 — 维护者运维手册

> 面向 attune 维护者(release 操作员).用户 install 命令见 [`install-package-managers.md`](install-package-managers.md).

本文档说明:
1. **Tauri auto-updater** 私钥/公钥管理 + Secret 配置 + 验证流程
2. **APT / RPM 软件源** GPG key 管理 + 首次 bootstrap
3. **WinGet** PAT 配置 + 首次提交流程
4. 常见故障 + 回滚步骤

## 目录

- [1. Tauri auto-updater](#1-tauri-auto-updater)
- [2. APT / RPM 软件源](#2-apt--rpm-软件源)
- [3. WinGet](#3-winget)
- [4. 验收清单](#4-验收清单)
- [5. 故障与回滚](#5-故障与回滚)

---

## 1. Tauri auto-updater

### 1.1 架构(无服务器 / 静态文件)

```
客户端 → GET https://github.com/qiurui144/attune/releases/latest/download/latest.json
       → 解析 platforms.<target>.url + signature
       → 下载 *.AppImage / *_setup.exe + 同名 *.sig
       → 用 pubkey 验签
       → 替换二进制 + 重启
```

**关键点**:`latest.json` 是普通 GitHub Release asset.`releases/latest/download/` URL **永远指向最新非 prerelease release**.无需任何域名或自建服务.

### 1.2 Keypair 首次生成

```bash
# 生成 ed25519 keypair → 默认输出到 ~/.attune-updater-keys/
./scripts/generate-updater-key.sh

# 输出会提示三个动作:
#   (1) 把 .pub 内容复制进 apps/attune-desktop/tauri.conf.json plugins.updater.pubkey
#   (2) 把私钥(整个 .key 文件内容)加为 GitHub Actions Secret TAURI_SIGNING_PRIVATE_KEY
#   (3) 若 generate 时设置了密码,再加 TAURI_SIGNING_PRIVATE_KEY_PASSWORD secret
```

**安全建议**:
- 私钥设密码(交互式 generate 会问):防止 GitHub Actions 日志意外泄漏时无法直接使用
- 添加 Secret 后,**立即删除本地 .key 文件**(`rm ~/.attune-updater-keys/attune-updater.key`).GitHub Secret 即唯一备份
- 公钥同步进 `tauri.conf.json` 的同 commit 即声明轮换;**注意**老客户端会用旧公钥验签,需要手动升级一次

### 1.3 验证 Secret 配置

```bash
# 推一个 -test rc tag 触发完整 release workflow
git tag desktop-v1.0.0-rc.test
git push origin desktop-v1.0.0-rc.test

# 在 Actions 页面 desktop-release run 完成后,检查:
#   1. Release 页面有 *.sig 文件(.AppImage.sig + _setup.exe.sig)
#   2. Release 页面有 latest.json
#   3. cat 下载的 latest.json,确认 platforms.linux-x86_64 与 platforms.windows-x86_64 字段齐全
```

### 1.4 客户端验证

```bash
# Linux: 装最新 AppImage 或 deb,启动 30s 后查看日志
journalctl --user-unit attune-desktop -f
# 期望见:"update available: 1.0.0-rc.test -> 1.0.0-rc.test+N" 或 "no update available"

# 验证签名验证逻辑:故意把 latest.json signature 字段改坏,客户端应 log:
#   "update check failed: signature verification failed"
```

---

## 2. APT / RPM 软件源

### 2.1 架构(GH Pages 静态托管)

```
qiurui144.github.io/attune/
├── attune-archive-keyring.gpg     ← 用户 import 的公钥
├── apt/
│   ├── dists/stable/Release        ← 签名的 metadata
│   ├── dists/stable/InRelease      ← clearsign 形式
│   ├── dists/stable/Release.gpg
│   ├── dists/stable/main/binary-amd64/Packages{,.gz}
│   └── pool/main/*.deb
├── rpm/
│   ├── attune.repo                 ← 用户 cp 到 /etc/yum.repos.d/
│   └── x86_64/
│       ├── *.rpm
│       └── repodata/repomd.xml{,.asc}
└── index.html                      ← 首页给用户指南
```

### 2.2 GPG key 首次生成

```bash
# 在维护者本机
gpg --batch --gen-key <<EOF
%no-protection
Key-Type: RSA
Key-Length: 4096
Subkey-Type: RSA
Subkey-Length: 4096
Name-Real: Attune Archive Key
Name-Email: attune@your-domain.example
Expire-Date: 5y
%commit
EOF

# 列出新 key
gpg --list-secret-keys --keyid-format LONG

# 取 KEY_ID(40 字符 fingerprint 的最后 16 字符)
KEY_ID=<paste>

# 导出 ASCII-armored 私钥
gpg --armor --export-secret-keys "$KEY_ID" | base64 -w0 > attune-archive.gpg.b64

# 加到 GitHub Secrets:
#   GPG_PRIVATE_KEY        ← 整个 attune-archive.gpg.b64 文件内容
#   GPG_PRIVATE_KEY_PASSWORD ← 留空(上面 %no-protection)或私钥密码
#   GPG_KEY_ID             ← 上面取的 KEY_ID
```

### 2.3 首次发布

第一次发布时 `gh-pages` 分支不存在,workflow 的 "Bootstrap" step 会自动 init 并 push.之后所有 desktop-v* tag 都会 append 到这个 repo.

**手动触发**(GA 后):

```bash
# 通过 GitHub web UI: Actions → apt-rpm-repo → Run workflow → 输入 desktop-v1.0.0
# 或通过 gh CLI:
gh workflow run apt-rpm-repo.yml -f tag=desktop-v1.0.0
```

### 2.4 Enable GitHub Pages

在 repo Settings → Pages:
- Source: Deploy from a branch
- Branch: `gh-pages` / `(root)`
- Save

约 1 分钟后 `https://qiurui144.github.io/attune/` 可访问.

---

## 3. WinGet

### 3.1 GitHub PAT 生成

vedantmgoyal2009/winget-releaser 需要一个 classic PAT(GitHub fine-grained token 不行,因为要 fork 跨账户仓库):

```
GitHub → Settings → Developer settings → Personal access tokens → Tokens (classic)
→ Generate new token (classic)
  - Name: attune-winget-releaser
  - Expiration: 1 year
  - Scopes: public_repo
→ Generate

复制 token → Repo Settings → Secrets → New repository secret
  - Name: WINGET_TOKEN
  - Value: <paste>
```

### 3.2 首次发布

```bash
# 只能 GA(无 -rc/-alpha/-beta),手动 trigger:
gh workflow run winget.yml -f tag=desktop-v1.0.0
```

第一次会:
1. Fork microsoft/winget-pkgs 到 qiurui144 名下(自动)
2. Push 一个分支 `qiurui144.Attune-1.0.0`
3. 向 microsoft/winget-pkgs 开 PR

之后等审核(1-7 天).

### 3.3 验证

```bash
# Win11 上等 PR 合并后
winget search qiurui144.Attune     # 应见
winget install qiurui144.Attune     # 应成功安装
```

---

## 4. 验收清单

发新 GA(`desktop-v1.0.0`)的完整验收:

- [ ] `git tag desktop-v1.0.0 && git push origin desktop-v1.0.0`
- [ ] Actions → desktop-release ✅ build job(Linux + Windows)
- [ ] Actions → desktop-release ✅ latest-json job
- [ ] Release 页面含:
  - [ ] `Attune_1.0.0_amd64.AppImage` + `.sig`
  - [ ] `Attune_1.0.0_amd64.deb`
  - [ ] `Attune-1.0.0-1.x86_64.rpm`
  - [ ] `Attune_1.0.0_x64-setup.exe` + `.sig`
  - [ ] `Attune_1.0.0_x64_en-US.msi`
  - [ ] `latest.json`
- [ ] `gh workflow run apt-rpm-repo.yml -f tag=desktop-v1.0.0` ✅
- [ ] `gh-pages` 分支更新含新版 .deb / .rpm
- [ ] `gh workflow run winget.yml -f tag=desktop-v1.0.0` ✅
- [ ] microsoft/winget-pkgs PR opened
- [ ] 维护者本机装 0.9.x → 启动 → 30s 内见 "update available" toast
- [ ] 点 UI 更新按钮 → 下载 + 安装 + 重启 → 版本变 1.0.0
- [ ] `apt update && apt install attune` 在干净 Ubuntu 24.04 容器跑通
- [ ] `dnf install attune` 在干净 Fedora 容器跑通

---

## 5. 故障与回滚

### 5.1 latest.json 写错版本号 / signature

```bash
# 不要 force push 同一 tag,改在 release 页面手动 edit latest.json
gh release upload desktop-v1.0.0 latest.json --clobber

# 或重跑 latest-json job
gh workflow run desktop-release.yml --ref main
```

### 5.2 APT repo 签名失败 → 用户 apt update 报 NO_PUBKEY

```bash
# 检查 GPG_KEY_ID secret 是否对应到 attune-archive-keyring.gpg 中的 key
# 重新生成 gh-pages 上的公钥
gh workflow run apt-rpm-repo.yml -f tag=<最近一个 desktop-v*>
```

### 5.3 老 v0.7.0 客户端无法接收 v1.0.0 更新

v1.0.0 用了新 pubkey(若维护者轮换了 keypair).v0.7.0 客户端只能验证旧 pubkey 的签名,验签失败 → 静默拒绝更新.**解法**:

- README 顶部加 banner:"v0.7.x 用户请手动下载 v1.0.0 安装包,首次 GA 升级例外"
- 旧 v0.7.0 客户端 main.rs 已 graceful 处理(只 log warn 不 crash),用户体验上仅缺自动升级

### 5.4 回滚 desktop-v1.0.0

```bash
# 1. 把 GitHub Release 标记为 prerelease(避免被 "latest" URL 命中)
gh release edit desktop-v1.0.0 --prerelease

# 2. latest.json 改回 0.9.x
gh workflow run desktop-release.yml --ref desktop-v0.9.x

# 3. APT repo 删除新版 .deb
# (gh-pages 分支上手动 git revert 那次 publish commit)
```

---

## 关联文档

- **私钥部署 step-by-step（GA 前必读）**：[`tauri-updater-deploy.md`](tauri-updater-deploy.md)
- 用户安装:[`install-package-managers.md`](install-package-managers.md)
- Release 流程:[`../DEVELOP.md`](../DEVELOP.md) §Release Checklist
- Spec:[`superpowers/specs/2026-05-22-release-package-management.md`](superpowers/specs/2026-05-22-release-package-management.md)
# Tauri Updater 私钥部署 — User 操作指南

> **面向**：仓库 owner（qiurui144）在 5/25 GA 前完成私钥配置，确保 `desktop-v1.0.0` tag 触发的 `desktop-release.yml` 能成功签名产物。
>
> **前置已完成**：`apps/attune-desktop/tauri.conf.json` 的 `plugins.updater.pubkey` 字段已 land（commit `ed151e1`）。`scripts/generate-updater-key.sh` 已存在。

---

## 目录

- [1. 为什么需要私钥](#1-为什么需要私钥)
- [2. User 操作步骤（GA tag 前完成）](#2-user-操作步骤ga-tag-前完成)
  - [2.1 生成 keypair](#21-生成-keypair)
  - [2.2 更新 tauri.conf.json pubkey](#22-更新-tauriconfjson-pubkey)
  - [2.3 配置 GitHub Actions Secrets](#23-配置-github-actions-secrets)
  - [2.4 本地验证签名（可选）](#24-本地验证签名可选)
  - [2.5 安全收尾](#25-安全收尾)
- [3. 验证 Secret 已生效](#3-验证-secret-已生效)
- [4. 注意事项与常见错误](#4-注意事项与常见错误)

---

## 1. 为什么需要私钥

Tauri auto-updater 使用 **ed25519 minisign** 签名机制：

- `desktop-release.yml` build job 在每次打 `desktop-v*` tag 时，用 `TAURI_SIGNING_PRIVATE_KEY` 对产物（`.AppImage`、`.exe`）签名，生成同名 `.sig` 文件上传到 Release
- 客户端用 `tauri.conf.json` 里的 `pubkey` 验证 `.sig`，验证通过才执行自动更新
- **Secret 未配置 → workflow 报错 `TAURI_SIGNING_PRIVATE_KEY not set` → GA tag 构建失败**

---

## 2. User 操作步骤（GA tag 前完成）

### 2.1 生成 keypair

在 **本地开发机**（`/data/company/project/attune`）执行：

```bash
cd /data/company/project/attune
bash scripts/generate-updater-key.sh
```

> 脚本需要 `cargo tauri signer generate`。若本机尚未安装 tauri-cli，脚本会自动尝试 `cargo install tauri-cli`（需要 ~1-2 分钟，确保 `/tmp` 有足够空间）。

脚本执行完毕后，输出类似：

```
===============================================
生成完成。下一步:
===============================================

1. 复制公钥 → 更新 tauri.conf.json:

untrusted comment: minisign public key: XXXXXXXXXXXXXXXX
RWxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

2. 复制私钥内容到 GitHub Actions Secret:
   Name: TAURI_SIGNING_PRIVATE_KEY
   Value: (整个私钥文件内容，包含 'untrusted comment' 头)

   私钥位置: ~/.attune-updater-keys/attune-updater.key

3. (可选) 再加 TAURI_SIGNING_PRIVATE_KEY_PASSWORD secret
   如果 generate 时设置了密码。
```

**务必将私钥备份到 1Password 或等价密码管理器**，丢失私钥 = 老客户端无法接收自动更新。

---

### 2.2 更新 tauri.conf.json pubkey

将上一步输出的**公钥（base64 编码的两行内容）**编码为 base64，替换 `apps/attune-desktop/tauri.conf.json` 中的 `pubkey` 字段：

```bash
# 查看当前 pubkey（已 land 的占位值）
grep "pubkey" apps/attune-desktop/tauri.conf.json

# 将新生成的 .pub 文件转为 base64 单行
base64 -w0 ~/.attune-updater-keys/attune-updater.key.pub
```

将 `base64 -w0` 的输出替换到 `tauri.conf.json`：

```json
"plugins": {
  "updater": {
    "active": true,
    "endpoints": ["https://github.com/qiurui144/attune/releases/latest/download/latest.json"],
    "dialog": false,
    "pubkey": "<新生成的 base64 公钥>"
  }
}
```

然后 commit + push：

```bash
git add apps/attune-desktop/tauri.conf.json
git commit -m "chore(updater): update tauri updater pubkey to generated keypair"
git push origin develop
```

---

### 2.3 配置 GitHub Actions Secrets

打开：**https://github.com/qiurui144/attune/settings/secrets/actions**

点击 **"New repository secret"**，添加以下 2 个（必须）：

| Secret 名称 | 值 | 说明 |
|------------|---|-----|
| `TAURI_SIGNING_PRIVATE_KEY` | 整个 `~/.attune-updater-keys/attune-updater.key` 文件内容（含 `untrusted comment:` 头） | 用于签名产物 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 若 generate 时设了密码则填密码，否则填空字符串（留空 secret） | 私钥加密密码 |

> **复制私钥内容**：
> ```bash
> cat ~/.attune-updater-keys/attune-updater.key
> ```
> 全部选中粘贴到 Secret value 框（包括 `untrusted comment:` 那行）。

---

### 2.4 本地验证签名（可选）

如果本机已有 tauri-cli，可在配置 Secret 前本地验证签名流程：

```bash
cd apps/attune-desktop

# 导出私钥到环境变量
export TAURI_SIGNING_PRIVATE_KEY=$(cat ~/.attune-updater-keys/attune-updater.key)
# 若设了密码
# export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=<your-password>

# 仅构建 Linux deb（快，跳过 Windows）
cargo tauri build --bundles deb

# 验证 .sig 已生成
ls target/release/bundle/deb/*.sig
# 期望看到: Attune_x.y.z_amd64.deb.sig
```

---

### 2.5 安全收尾

Secret 配置完成后，**删除本地私钥**：

```bash
rm ~/.attune-updater-keys/attune-updater.key
# 公钥可保留
ls ~/.attune-updater-keys/
# 只剩 attune-updater.key.pub
```

> GitHub Secret 即唯一权威备份（已存 1Password）。本地私钥删除不影响任何构建。

---

## 3. 验证 Secret 已生效

### 方式 A：推测试 tag（推荐，GA 前必做）

```bash
git tag desktop-v1.0.0-rc.test
git push origin desktop-v1.0.0-rc.test
```

在 **Actions 页面** 观察 `desktop-release` workflow：

- ✅ build job 通过，日志含 `Signing bundle` 字样
- ✅ Release 页面出现 `*.AppImage.sig` + `*_setup.exe.sig`
- ✅ Release 页面出现 `latest.json`

测试完删除测试 tag 和 release：

```bash
git push origin --delete desktop-v1.0.0-rc.test
git tag -d desktop-v1.0.0-rc.test
gh release delete desktop-v1.0.0-rc.test --yes
```

### 方式 B：检查 workflow 日志

在已有的 `desktop-v*` workflow run 日志中，搜索 `TAURI_SIGNING_PRIVATE_KEY`，若看到 `Secret not found` 说明未配置。

---

## 4. 注意事项与常见错误

| 问题 | 原因 | 解法 |
|------|------|------|
| workflow 报 `TAURI_SIGNING_PRIVATE_KEY` not set | Secret 名称拼写错误或未添加 | 对照 §2.3 表格检查 secret 名称 |
| 签名验证失败（客户端拒绝更新） | `tauri.conf.json pubkey` 与生成的私钥不配对 | 重新执行 §2.2，确保 pubkey 是 `attune-updater.key.pub` 的 base64 |
| 老客户端（v0.7.0）无法自动更新 | 旧版客户端编译时 pubkey 不同 | 见 `auto-updater-setup.md` §5.3，首次 GA 升级需手动下载 |
| `cargo tauri signer generate` 失败（`No space left`） | `/tmp` 磁盘满，`cargo install tauri-cli` 失败 | 清理磁盘：`cargo clean`，或在 `/tmp` 空间充足的机器上运行 |
| `base64 -w0` 在 macOS 不可用 | macOS `base64` 无 `-w` 标志 | 改用 `base64 < file \| tr -d '\n'` |

---

## 关联文档

- 完整维护者运维手册（含 APT/RPM/WinGet 配置）：[`auto-updater-setup.md`](auto-updater-setup.md)
- 私钥生成脚本：[`../scripts/generate-updater-key.sh`](../scripts/generate-updater-key.sh)
- Tauri.conf.json：[`../apps/attune-desktop/tauri.conf.json`](../apps/attune-desktop/tauri.conf.json)
