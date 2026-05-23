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
