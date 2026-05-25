# Homebrew tap 设置指南(qiurui144/homebrew-attune)

本目录的 formula 通过**独立 tap 仓**分发,不走 homebrew-core.

## 为什么不走 homebrew-core

- **notability 门槛**:homebrew-core 要求 GitHub star ≥ 75 + 持续维护 30 天 + 上游 release 节奏稳定;新项目几乎都被拒
- **审核延迟**:homebrew-core PR 排队 1-2 周
- **维护成本**:每次发新版必须开 PR + 自动 bumper 还要单独配置
- **自有 tap 的优势**:同步速度 = 你的 push 速度;格式自由(GUI cask + 多 binary 共存);命名空间隔离避免冲突

## Step 1:创建独立 tap 仓

GitHub web → New repository → 名称 **必须** 是 `homebrew-attune`(Homebrew 强制前缀)
→ Public → 不勾任何模板 → Create.

仓库 URL:`https://github.com/qiurui144/homebrew-attune`

## Step 2:本地克隆 + 推 Formula

```bash
git clone https://github.com/qiurui144/homebrew-attune.git
cd homebrew-attune

# 复制 formula(本仓库 → tap 仓)
mkdir -p Formula
cp /data/company/project/attune/packaging/homebrew/Formula/attune.rb Formula/attune.rb

# tap 仓的 README
cat > README.md <<'EOF'
# Attune Homebrew Tap

Attune CLI / server 的 Homebrew tap.

## Install

```bash
brew tap qiurui144/attune
brew install attune
```

## Upgrade

```bash
brew update && brew upgrade attune
```

## Uninstall

```bash
brew uninstall attune
brew untap qiurui144/attune
```

## 项目主仓

https://github.com/qiurui144/attune
EOF

git add Formula/attune.rb README.md
git commit -m "Initial tap with attune 1.0.0"
git push origin main
```

## Step 3:计算实际 SHA256

`attune.rb` 中 4 个 `sha256` 字段是 placeholder(`REPLACE_WITH_ACTUAL_SHA256_FROM_RELEASE_ASSET`).
首次发版后必须替换:

```bash
# macOS ARM(Apple Silicon)
curl -L -o /tmp/attune-macos-aarch64.tar.gz \
  https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-macos-aarch64.tar.gz
shasum -a 256 /tmp/attune-macos-aarch64.tar.gz

# Linux x86_64
curl -L -o /tmp/attune-linux-x86_64.tar.gz \
  https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-linux-x86_64.tar.gz
sha256sum /tmp/attune-linux-x86_64.tar.gz

# Linux aarch64
curl -L -o /tmp/attune-linux-aarch64.tar.gz \
  https://github.com/qiurui144/attune/releases/download/v1.0.0/attune-linux-aarch64.tar.gz
sha256sum /tmp/attune-linux-aarch64.tar.gz
```

把每个 sha256 hash 替换到 `Formula/attune.rb` 对应位置后,commit + push.

## Step 4:本地验证

```bash
brew tap qiurui144/attune
brew install attune
attune --version
# 期望: attune 1.0.0 (...)

# 或带 audit
brew audit --strict --online attune
```

## Step 5:用户使用

发版后宣传文案:

```bash
brew tap qiurui144/attune
brew install attune
```

## 后续发版自动化(v1.0.x+)

每次发新 GA tag `vX.Y.Z`:

1. 本仓库 push tag → `rust-release.yml` workflow 跑完
2. 抓 3 个 sha256(macos-aarch64 / linux-x86_64 / linux-aarch64)
3. 在 tap 仓更新 `Formula/attune.rb` 的 `version` + 3 个 `sha256` + 3 个 url 中的版本号
4. push tap 仓

未来可加 GitHub Action(`bump-homebrew-formula-action`)自动化 step 3-4,但首期人工足够.

## tap 仓维护规则

- 一个 tap 仓可放多个 formula(Formula/*.rb).未来 attune-cli / attune-pro-bridge 等可加进同 tap
- 不要在 tap 仓提交业务代码 — 只放 formula + README + LICENSE
- 每次 push 前 `brew audit --strict --online <formula>` 自检

## 参考

- Homebrew formula cookbook: https://docs.brew.sh/Formula-Cookbook
- tap 指南: https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap
- 自动化 bump: https://github.com/marketplace/actions/bump-homebrew-formula
