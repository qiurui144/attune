# WinGet 首次提交指南(microsoft/winget-pkgs)

本目录 manifest 在首次发布(seed)、或 CI 自动化失败时,需要**人工**向上游
[`microsoft/winget-pkgs`](https://github.com/microsoft/winget-pkgs) 提交 PR.

后续 GA 版本由 `.github/workflows/winget.yml` 自动 wingetcreate + 开 PR,无需人工.

## 触发场景(本指南适用)

- ❶ **首次注册**:`qiurui144.Attune` 在 winget 仓库还没条目 → 必须人工 seed
- ❷ **CI 故障**:`winget.yml` 调用失败 或 WINGET_TOKEN secret 缺失/过期
- ❸ **schema 升级**:winget manifest schema 重大变更,wingetcreate 模板需人工修齐

CI 正常运行的常规 GA 不走本流程,见 §自动化路径.

## 自动化路径(参考,不用人工执行)

GA tag `desktop-vX.Y.Z` 推到上游后,人工 trigger:

```
GitHub → Actions → "winget" → Run workflow → tag = desktop-v1.0.0
```

Workflow 内部:
1. 校验 tag 格式
2. 调 `vedantmgoyal2009/winget-releaser@v2`
3. action 计算 SHA256、生成完整 manifest、fork microsoft/winget-pkgs、开 PR

后续人工只需在上游 PR 评论里回复 `@wingetcreate` 命令或等 reviewer.

## 人工提交流程(seed / 故障兜底)

### Step 1:fork microsoft/winget-pkgs

GitHub web → https://github.com/microsoft/winget-pkgs → Fork → 选 `qiurui144` namespace.

```bash
git clone https://github.com/qiurui144/winget-pkgs.git
cd winget-pkgs
git remote add upstream https://github.com/microsoft/winget-pkgs.git
git fetch upstream
git checkout -b add-qiurui144-attune-1.0.0 upstream/master
```

### Step 2:复制 manifest 到正确路径

WinGet 仓库目录结构:`manifests/<first-letter-lowercase>/<Publisher>/<PackageName>/<Version>/`

```bash
mkdir -p manifests/q/qiurui144/Attune/1.0.0
cp /data/company/project/attune/packaging/winget/qiurui144.Attune.yaml \
   /data/company/project/attune/packaging/winget/qiurui144.Attune.installer.yaml \
   /data/company/project/attune/packaging/winget/qiurui144.Attune.locale.en-US.yaml \
   manifests/q/qiurui144/Attune/1.0.0/
```

### Step 3:重算 SHA256

`installer.yaml` 的 `InstallerSha256` 字段是 placeholder,必须替换成实际值:

```bash
# 下载实际 release asset
curl -L -o /tmp/attune-setup.exe \
  https://github.com/qiurui144/attune/releases/download/desktop-v1.0.0/Attune_1.0.0_x64-setup.exe

# 计算 SHA256
sha256sum /tmp/attune-setup.exe
# 输出形如: abc123...def  /tmp/attune-setup.exe
# 复制 hash 替换到 installer.yaml 中 REPLACE_WITH_ACTUAL_SHA256_FROM_RELEASE_ASSET
```

或在 PowerShell:

```powershell
Get-FileHash -Algorithm SHA256 attune-setup.exe
```

### Step 4:本地验证 manifest

可选但强烈推荐 — 用 WinGet sandbox 验证 manifest 合法性:

```powershell
winget validate --manifest manifests\q\qiurui144\Attune\1.0.0
```

如果有 winget Sandbox 可以本地试装:

```powershell
.\Tools\SandboxTest.ps1 manifests\q\qiurui144\Attune\1.0.0
```

### Step 5:提交 PR

```bash
git add manifests/q/qiurui144/Attune/1.0.0/
git commit -m "Add qiurui144.Attune version 1.0.0"
git push origin add-qiurui144-attune-1.0.0
```

在 GitHub web 上开 PR:
- Title: `Add qiurui144.Attune version 1.0.0`
- Body: 简述软件 + 链接 release notes

### Step 6:等待审核

- WinGet community moderator 通常 1-7 天内审完
- 审核期间 bot 会自动跑 InstallTest + Validation
- 通过 → 合并 → 几小时后 `winget search Attune` 能命中

## 注意事项

- 一旦 `qiurui144.Attune` 进 winget 索引,后续 GA 版本可走自动化 workflow,不再走本流程
- `installer.yaml` 中 `InstallerUrl` 必须是 **真实可达**的 release asset(不能是私有/草稿 release)
- 仓库目录大小写敏感:`q`(小写)/ `qiurui144`(原样)/ `Attune`(原样)

## 撤回 / 删除版本

manifest 进上游后无法删除(那是用户已安装的依据).发现严重 bug 必须撤回时:
1. 发新 patch 版本(qiurui144.Attune 1.0.1)替代
2. 在 GitHub release 上标记旧版 deprecated
3. 极端情况:开 issue at microsoft/winget-pkgs 申请下架(罕见)

## 参考

- WinGet manifest schema: https://learn.microsoft.com/en-us/windows/package-manager/package/manifest
- 提交流程: https://github.com/microsoft/winget-pkgs/blob/master/CONTRIBUTING.md
- 字段 reference: https://github.com/microsoft/winget-pkgs/tree/master/doc/manifest/schema
