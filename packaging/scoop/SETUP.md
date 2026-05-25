# Scoop bucket 设置指南(qiurui144/scoop-attune)

类似 Homebrew tap,Scoop 通过**独立 bucket 仓**分发,不必走 ScoopInstaller/Main(那里有 community 审核,周期长且未必接受所有项目).

## Step 1:创建独立 bucket 仓

GitHub web → New repository → 名称 **建议** `scoop-attune`(Scoop 约定但非强制前缀)
→ Public → 不勾任何模板 → Create.

仓库 URL:`https://github.com/qiurui144/scoop-attune`

## Step 2:本地克隆 + 推 manifest

```powershell
git clone https://github.com/qiurui144/scoop-attune.git
cd scoop-attune

# 复制 manifest(本仓库 → bucket 仓)
mkdir bucket -ErrorAction SilentlyContinue
Copy-Item C:\path\to\attune\packaging\scoop\attune.json bucket\attune.json

# 写 README
@"
# Attune Scoop Bucket

Scoop bucket for Attune CLI / server on Windows.

## Install

\`\`\`powershell
scoop bucket add attune https://github.com/qiurui144/scoop-attune
scoop install attune
\`\`\`

## Update

\`\`\`powershell
scoop update attune
\`\`\`

## Uninstall

\`\`\`powershell
scoop uninstall attune
scoop bucket rm attune
\`\`\`

## Source

https://github.com/qiurui144/attune
"@ | Out-File README.md -Encoding utf8

git add bucket/attune.json README.md
git commit -m "Initial bucket with attune 1.0.0"
git push origin main
```

Linux/macOS 同样可以,把 PowerShell 命令换 bash 即可(`mkdir bucket && cp ... && cat > README.md ...`).

## Step 3:计算 SHA256

`attune.json` 中 `hash` 字段是 placeholder.首次发版后:

```powershell
# 下载 release asset
Invoke-WebRequest -Uri "https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-windows-x86_64.tar.gz" -OutFile attune.tar.gz

# 计算
Get-FileHash -Algorithm SHA256 attune.tar.gz
```

或 bash:

```bash
curl -L -o /tmp/attune-windows.tar.gz \
  https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-windows-x86_64.tar.gz
sha256sum /tmp/attune-windows.tar.gz
```

把 hash 写入 `bucket/attune.json` 的 `architecture.64bit.hash` 后 commit + push.

## Step 4:本地验证

```powershell
scoop bucket add attune https://github.com/qiurui144/scoop-attune
scoop install attune
attune --version
# 期望: attune 1.0.0 (...)

# 或带 strict checker
scoop checkup
```

## Step 5:用户使用

```powershell
scoop bucket add attune https://github.com/qiurui144/scoop-attune
scoop install attune
```

## 自动更新(`scoop update` 友好)

`attune.json` 中 `checkver` + `autoupdate` 块让 Scoop 能自动 detect 新版:

```powershell
# Bucket maintainer 在 bucket 仓内跑(需要 ScoopBucketMaintainer / scoop-cli)
scoop-checkver attune
scoop-autoupdate attune
```

第三方 scoop-cli 跑 `checkver` 会从 GitHub release page 拉最新 tag,自动更新
manifest 中的 version + URL + hash,然后 maintainer commit + push.

未来可加 GitHub Action 自动跑这步,首期人工足够.

## bucket 仓维护规则

- 单 bucket 可放多个 manifest(bucket/*.json).未来加 attune-cli / attune-pro 等可同 bucket
- 不放业务代码,只放 manifest + README + LICENSE
- 每次 push 前 `scoop checkup` 验证 manifest 合法

## 与 WinGet 的关系

WinGet 适合**普通桌面用户**(GUI 安装 + 系统级 PATH).
Scoop 适合**开发者**(无管理员权限 + 隔离 / 多版本共存 + 命令行优先).

attune 同时支持两者,用户可任选.两条路径不冲突.

## 参考

- Scoop manifest schema: https://github.com/ScoopInstaller/Scoop/wiki/App-Manifests
- bucket 指南: https://github.com/ScoopInstaller/Scoop/wiki/Buckets
- checkver/autoupdate: https://github.com/ScoopInstaller/Scoop/wiki/App-Manifest-Autoupdate
